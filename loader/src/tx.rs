//! M3-class transaction machinery — behavioral port of unlock_610.c
//! (falcon_reset / poll_us / falcon_dma / falcon_hs prevalidation) to the
//! uefi-rs PciIo BAR0 window. Compiled ONLY with `--features tx`;
#![allow(dead_code)] // pre-wired ahead of the M3 gate; test surface lives in sim/
//! the default shipped binary carries zero GPU-write paths (constitution 1.7).

use core::time::Duration;
use uefi::boot;
use log::info;
use uefi::boot::stall;

use crate::pci::PciIo;

/// Falcon engine bases (BAR0 offsets).
pub mod eng {
    pub const GSP: u32 = 0x0011_0000;
    pub const SEC2: u32 = 0x0084_0000;
    /// SEC2 fbif / falcon2 sub-bases (unlock_610.c falcon_eng for SEC2).
    pub const SEC2_FBIF: u32 = 0x0084_0600;
    pub const SEC2_FALCON2: u32 = 0x0084_1000;
}

pub mod regs {
    pub const GFW_PLM: u32 = 0x0011_8128;
    pub const GFW_BOOT: u32 = 0x0011_8234;
    pub const GFW_CPUCTL: u32 = 0x0011_0100;
    pub const FB_SIZE_MIB: u32 = 0x0011_83a4;
    pub const MMU_PLM: u32 = 0x001f_a7c8;
    pub const WPR2_LO: u32 = 0x001f_a824;
    pub const WPR2_HI: u32 = 0x001f_a828;
    pub const MMU_LOCK_LO: u32 = 0x001f_a82c;
    pub const MMU_LOCK_HI: u32 = 0x001f_a830;
    pub const FEAT_PLM: u32 = 0x0082_3804;
    pub const FEAT_SS0: u32 = 0x0082_381c;
    pub const FEAT_SS1: u32 = 0x0082_3820;
    // Ada four-write chain
    pub const WPR_PLM: u32 = 0x001f_a7c4;
    pub const WPRCFG_PLM: u32 = 0x001f_a7cc;
    pub const FBPA_PLM: u32 = 0x009a_0148;

    pub const GFW_BOOT_COMPLETED: u32 = 0xff;
    pub const CPUCTL_HALTED: u32 = 0x10;
    /// Falcon DMA command for an IMEM block (unlock_610 dma_cmd(true)).
    pub const DMA_CMD_IMEM: u32 = 6 << 8 | 0x14;
    /// DMEM variant (dma_cmd(false)).
    pub const DMA_CMD_DMEM: u32 = 6 << 8; // gpu_unlock_dma_cmd(false): plain 0x600
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TxError {
    /// Register read came back inaccessible (0xffffffff / 0xbadf****).
    EIO,
    ENodev,
    /// Poll budget exhausted; `tolerated` marks the HW-bug-3419321 path.
    ETimeout { tolerated: bool },
    EInval,
    ERange,
    EBusy,
    EAlready,
}

pub fn inaccessible(value: u32) -> bool {
    value == !0u32 || (value & 0xffff_0000) == 0xbadf_0000
}

/// BAR0-backed engine view.
pub struct TxGpu<'a> {
    pub pci: &'a PciIo,
    pub bar: u8,
}

impl TxGpu<'_> {
    pub fn read32(&self, off: u32) -> Result<u32, TxError> {
        let v = self.pci.bar_read_u32(self.bar, off as u64);
        if inaccessible(v) {
            return Err(TxError::EIO);
        }
        Ok(v)
    }

    /// Raw BAR0 read: the poison word is returned, not turned into an error.
    /// For registers where a BADF value is tolerable and the caller has to
    /// classify it (layout's VGA workspace: only bit3 matters, upstream only
    /// rejects ~0 there).
    pub fn read32_raw(&self, off: u32) -> u32 {
        self.pci.bar_read_u32(self.bar, off as u64)
    }

    /// THE write path of the whole project — exists only in `tx` builds.
    pub fn write32(&self, off: u32, v: u32) {
        self.pci.bar_write_u32(self.bar, off as u64, v);
    }

    /// poll_us port: mask/expect/budget semantics identical to the reference,
    /// including the tolerated-timeout distinction (log wording kept apart).
    pub fn poll_us(
        &self,
        off: u32,
        mask: u32,
        expect: u32,
        budget_us: u32,
        timeout_is_failure: bool,
    ) -> Result<(), TxError> {
        let step_us = 75u32; // usleep_range(50, 100) midpoint
        let max_iters = budget_us / step_us + 1;
        for _ in 0..max_iters {
            let value = self.pci.bar_read_u32(self.bar, off as u64);
            if inaccessible(value) {
                info!("poll inaccessible: reg={off:#x} last={value:#x} mask={mask:#x} expected={expect:#x} budget_us={budget_us}");
                return Err(TxError::EIO);
            }
            if value & mask == expect {
                return Ok(());
            }
            stall(Duration::from_micros(step_us as u64));
        }
        info!(
            "poll timeout{}: reg={off:#x} mask={mask:#x} expected={expect:#x} budget_us={budget_us}",
            if timeout_is_failure { "" } else { " tolerated" }
        );
        Err(TxError::ETimeout { tolerated: !timeout_is_failure })
    }
}

/// Falcon engine reset (unlock_610.c falcon_reset, 1:1):
/// pre-reset HWCFG2 wait (timeout tolerated, HW bug 3419321) -> reset
/// pulse with 10 propagation reads each way -> scrub wait -> optional
/// RISC-V<->Falcon BCR switch -> chip id into the boot vector (+0x84).
pub fn falcon_reset(gpu: &TxGpu, base: u32, _fbif: u32, falcon2: u32) -> Result<(), TxError> {    info!("Falcon reset begin: base={base:#x}");
    // Pre-reset wait; ETIMEDOUT tolerated.
    if let Err(e) = gpu.poll_us(base + 0xf4, 1 << 31, 1 << 31, 1000, false) {
        match e {
            TxError::ETimeout { tolerated: true } => {
                info!("Falcon pre-reset timeout tolerated per HW bug 3419321");
            }
            other => return Err(other),
        }
    }
    let engine = gpu.read32(base + 0x3c0)?;
    gpu.write32(base + 0x3c0, engine | 1);
    for _ in 0..10 {
        let _ = gpu.pci.bar_read_u32(gpu.bar, (base + 0x3c0) as u64);
    }
    gpu.write32(base + 0x3c0, engine & !1u32);
    for _ in 0..10 {
        let _ = gpu.pci.bar_read_u32(gpu.bar, (base + 0x3c0) as u64);
    }
    // Scrub wait (must complete).
    gpu.poll_us(base + 0xf4, 1 << 12, 0, 1_000_000, true)?;
    let hwcfg = gpu.read32(base + 0xf4)?;
    if hwcfg & (1 << 10) != 0 {
        // RISC-V-capable GSP: force the Falcon core explicitly (kernel
        // kflcnResetIntoFalcon_GA102 semantics - BCR_CTRL bit4 CORE_SELECT
        // 0=Falcon/1=RISCV, bit0 VALID reports the switch completed). The
        // GFW leaves the RISC-V side after boot; a bare "bit4 reads 0"
        // does NOT prove a completed Falcon selection, and a stale RISC-V
        // selection stalls the Falcon-compat DMA queue (observed: 32-slot
        // XFER queue never drains).
        let bcr = gpu.read32(falcon2 + 0x668)?;
        info!("Falcon reset: BCR before = {bcr:#010x} (valid={} core_select={})",
            bcr & 1, bcr >> 4 & 1);
        let falcon_confirmed = (bcr & 1) != 0 && (bcr >> 4 & 1) == 0;
        if !falcon_confirmed {
            gpu.write32(falcon2 + 0x668, 0);
            gpu.poll_us(falcon2 + 0x668, 1, 1, 1_000_000, true)?;
            let bcr2 = gpu.read32(falcon2 + 0x668)?;
            info!("Falcon reset: BCR after switch = {bcr2:#010x}");
            // NOTE: a second engine reset cycle here was tried (2026-10-26)
            // and REVERTED: the reset wiped the BCR switch back to 0x110 AND
            // hung the HWCFG bit12 scrub wait on both engines (EIO). The
            // switch-after-reset order matches unlock_610.c exactly.
        }
    }
    let chip = gpu.read32(0)?;
    gpu.write32(base + 0x84, chip);
    info!("Falcon reset done: base={base:#x} hwcfg={hwcfg:#x} chip={chip:#x}");
    Ok(())
}

/// Open the engine's FBIF/DMA path for physical, ctx-less access
/// (unlock_610.c falcon_dma_load trio + kernel kflcnDisableCtxReq):
/// FBIF_CTL.ALLOW_PHYS_NO_CTX, DMACTL.REQUIRE_CTX=0, TRANSCFG(0)=
/// COHERENT_SYSMEM|PHYSICAL. The scrubber/ucode's own runtime DMA dies
/// behind REQUIRE_CTX=1 (requests queue but are never serviced - the
/// "2x32-slot stall" signature). Every portaled image still needs this
/// because the ucode itself DMA-translates through the same FBIF.
pub fn falcon_dma_unblock(gpu: &TxGpu, base: u32, fbif: u32) -> Result<(), TxError> {
    let mut ctl = gpu.read32(fbif + 0x24)?;
    ctl |= 1 << 7;
    gpu.write32(fbif + 0x24, ctl);
    gpu.write32(base + 0x10c, 0);
    let mut trans = gpu.read32(fbif)?;
    trans = (trans & !7) | 5;
    gpu.write32(fbif, trans);
    // Read-back (constitution 1.1): a gated FBIF domain drops these
    // silently and every later DMA fetch dies at translation.
    let (ctl_rb, trans_rb, dmactl_rb) =
        (gpu.read32(fbif + 0x24)?, gpu.read32(fbif)?, gpu.read32(base + 0x10c)?);
    info!(
        "FALCON-UNBLOCK: FBIF_CTL={ctl_rb:#010x} TRANSCFG0={trans_rb:#010x} DMACTL={dmactl_rb:#010x} (want ctl bit7=1, trans&7=5, dmactl=0)"
    );
    if ctl_rb & (1 << 7) == 0 || trans_rb & 7 != 5 || dmactl_rb != 0 {
        return Err(TxError::EIO);
    }
    Ok(())
}

/// Differential authority scan (candidate C): read the same falcon PRI
/// offsets on BOTH engines and log them side by side with poison bucketing.
/// Motivation (probe-proven 2026-10-26): SEC2 CPUCTL START writes are
/// dropped while GSP accepts the identical write, and SEC2 CPUCTL reads
/// badf5620 while GSP reads 0x10 - mapping the read-gate pattern across the
/// block localizes the authority gate. +0x3c4 is TOTORONG's reset-PLM
/// candidate. Data ports (IMEMD/DMEMD) are skipped to avoid side effects.
pub fn authority_scan(gpu: &TxGpu) {
    const GSP: u32 = 0x0011_0000;
    const SEC2: u32 = 0x0084_0000;
    const GSP2: u32 = 0x0011_1000;
    const SEC22: u32 = 0x0084_1000;
    const OFFS: &[(&str, u32)] = &[
        ("ENG", 0x000),
        ("MBOX0", 0x040),
        ("MBOX1", 0x044),
        ("EXCI", 0x0d0),
        ("HWCFG", 0x0f4),
        ("CPUCTL", 0x100),
        ("BOOTVEC", 0x104),
        ("CAPS", 0x108),
        ("DMACTL", 0x10c),
        ("DMATRFBASE", 0x110),
        ("DMATRFCMD", 0x118),
        ("DMATRFFBOFFS", 0x11c),
        ("DMATRFBASE1", 0x120),
        ("CPUCTL_ALIAS", 0x130),
        ("TRACEPC", 0x14c),
        ("IMEMC", 0x180),
        ("IMEMT", 0x188),
        ("DMEMC", 0x1c0),
        ("UNK300", 0x300),
        ("ENG_RESET", 0x3c0),
        ("RESET_PLM?", 0x3c4),
        ("UNK440", 0x440),
        ("UNK4E4", 0x4e4),
        ("UNK4F4", 0x4f4),
        ("UNK520", 0x520),
        ("UNK524", 0x524),
    ];
    const OFFS2: &[(&str, u32)] = &[
        ("MOD_SEL", 0x180),
        ("UCODE_ID", 0x198),
        ("ENGIDMASK", 0x19c),
        ("PARAADDR", 0x210),
        ("RISCV388", 0x388),
        ("BCR", 0x668),
    ];
    let bucket = |v: u32| match v {
        0xffff_ffff => "FFFF",
        v if v & 0xbadf_0000 == 0xbadf_0000 => "BADF",
        _ => "ok",
    };
    info!("AUTH-SCAN begin (differential GSP vs SEC2)");
    for (name, off) in OFFS {
        let g = gpu.pci.bar_read_u32(gpu.bar, (GSP + off) as u64);
        let s = gpu.pci.bar_read_u32(gpu.bar, (SEC2 + off) as u64);
        info!("AUTH-SCAN falcon +{off:#05x} {name:13} gsp={g:#010x}[{}] sec2={s:#010x}[{}]", bucket(g), bucket(s));
    }
    for (name, off) in OFFS2 {
        let g = gpu.pci.bar_read_u32(gpu.bar, (GSP2 + off) as u64);
        let s = gpu.pci.bar_read_u32(gpu.bar, (SEC22 + off) as u64);
        info!("AUTH-SCAN falcon2 +{off:#05x} {name:13} gsp={g:#010x}[{}] sec2={s:#010x}[{}]", bucket(g), bucket(s));
    }
    info!("AUTH-SCAN end");
}

/// Post-reset authority probe for one falcon: mini-scan the registers that
/// matter for start, then a SAFE write-stickiness test on CPUCTL - write
/// ALIAS_EN (bit6) WITHOUT START (bit0) and read back. If bit6 sticks,
/// CPUCTL writes work and the START bit itself is gated/broken (retry via
/// alias); if it does not stick, the register is write-protected entirely.
/// Returns the post-probe CPUCTL value.
pub fn falcon_authority_probe(gpu: &TxGpu, base: u32, name: &str) -> u32 {
    const OFFS: &[(&str, u32)] = &[
        ("CPUCTL", 0x100),
        ("CPUCTL_ALIAS", 0x130),
        ("DMACTL", 0x10c),
        ("BOOTVEC", 0x104),
        ("EXCI", 0x0d0),
        ("HWCFG", 0x0f4),
        ("ENG_RESET", 0x3c0),
        ("RESET_PLM?", 0x3c4),
    ];
    for (n, off) in OFFS {
        let v = gpu.pci.bar_read_u32(gpu.bar, (base + off) as u64);
        info!("AUTH-POST {name} +{off:#05x} {n:13} = {v:#010x}{}", {
            if v & 0xbadf_0000 == 0xbadf_0000 { "[BADF]" } else if v == 0xffff_ffff { "[FFFF]" } else { "" }
        });
    }
    let pre = gpu.pci.bar_read_u32(gpu.bar, (base + 0x100) as u64);
    if pre & 0xbadf_0000 == 0xbadf_0000 {
        info!("AUTH-POST {name}: CPUCTL unreadable, write probe skipped");
        return pre;
    }
    // SAFE probe: ALIAS_EN without START.
    gpu.write32(base + 0x100, pre | 0x40);
    let stuck = gpu.pci.bar_read_u32(gpu.bar, (base + 0x100) as u64);
    info!("AUTH-POST {name}: CPUCTL write-probe {pre:#010x} -> {stuck:#010x} ({})", 
        if stuck & 0x40 != 0 { "bit6 STUCK - writes work" } else { "bit6 dropped - write-protected" });
    if stuck & 0x40 == 0 {
        // restore; leave the register as found
        gpu.write32(base + 0x100, pre);
    }
    stuck
}

/// DMATRFMOFFS is 24 bits, FBOFFS is 32 bits, and BASE is 49 bits.
/// Reject truncation/wrap before submitting any part of a transfer.
pub fn falcon_dma_valid(dest: u32, mem_off: u32, src: u64, size: u32) -> bool {
    size != 0
        && (size as u64 | dest as u64 | mem_off as u64 | src) & 255 == 0
        && dest as u64 + size as u64 <= 1 << 24
        && mem_off as u64 + size as u64 <= 1 << 32
        && src < 1 << 49
        && mem_off as u64 + size as u64 <= (1u64 << 49) - src
}

/// Falcon DMA transfer (unlock_610.c falcon_dma, 1:1): idle poll ->
/// program source address -> per-256B chunks -> done poll.
pub fn falcon_dma(
    gpu: &TxGpu,
    base: u32,
    mut dest: u32,
    mut mem_off: u32,
    src: u64,
    size: u32,
    cmd: u32,
) -> Result<(), TxError> {
    info!("Falcon DMA: base={base:#x} dest={dest:#x} offset={mem_off:#x} src={src:#x} size={size:#x} cmd={cmd:#x}");
    if !falcon_dma_valid(dest, mem_off, src, size) {
        return Err(TxError::EInval);
    }
    gpu.poll_us(base + 0x118, 1, 0, 1_000_000, true)?;
    gpu.write32(base + 0x110, (src >> 8) as u32);
    gpu.write32(base + 0x128, ((src >> 8) >> 32) as u32 & 0x1ff);
    let mut done = 0u32;
    while done < size {
        // Wait for FIFO space; on stall, dump state and try a kick round:
        // re-write XFER_EXT_BASE (same value) - some Falcon generations
        // require the base register touch to re-arm the transfer engine
        // after the 32-slot queue fills.
        if let Err(e) = gpu.poll_us(base + 0x118, 1, 0, 1_000_000, true) {
            info!("Falcon DMA progress: done={done} total={size}");
            dma_stall_dump(gpu, base);
            let mut recovered = false;
            for round in 0..3 {
                boot::stall(core::time::Duration::from_millis(1000));
                // Kick 1: re-arm the external base register.
                gpu.write32(base + 0x110, (src >> 8) as u32);
                gpu.write32(base + 0x128, ((src >> 8) >> 32) as u32 & 0x1ff);
                // Kick 2: nudge the block-on-fifo register (0 then re-check).
                gpu.write32(base + 0x10c, 0);
                if let Ok(()) = gpu.poll_us(base + 0x118, 1, 0, 1_000_000, true) {
                    info!("Falcon DMA: recovered after kick round {round} - resuming at done={done}");
                    recovered = true;
                    break;
                }
                dma_stall_dump(gpu, base);
            }
            if !recovered {
                return Err(e);
            }
        }
        // PORT experiment: blocks 32+ issue on PORT 1 (cmd bits12:14) - if
        // the stall is port-0-local queue state, the transfer completes.
        let cmd_port = if done >= 32 * 256 { cmd | (1 << 12) } else { cmd };
        gpu.write32(base + 0x114, dest);
        gpu.write32(base + 0x11c, mem_off);
        gpu.write32(base + 0x118, cmd_port);
        done += 256;
        dest += 256;
        mem_off += 256;
        // Golden-sample trace: full register picture at block boundaries
        // (blocks 0-2 right after issue, and the 30-34 window where the
        // queue stalls) - pins the semantics of 0x118 bit0 and 0x120
        // empirically instead of trusting cross-generation docs.
        let blk = done / 256;
        if blk <= 3 || (30..=34).contains(&blk) {
            let c118 = gpu.pci.bar_read_u32(gpu.bar, (base + 0x118) as u64);
            let c120 = gpu.pci.bar_read_u32(gpu.bar, (base + 0x120) as u64);
            let c11c = gpu.pci.bar_read_u32(gpu.bar, (base + 0x11c) as u64);
            info!("DMA-BLK {blk}: 0x118={c118:#010x} 0x120={c120:#010x} 0x11c={c11c:#010x}");
        }
    }
    gpu.poll_us(base + 0x118, 2, 2, 1_000_000, true)
}

/// Read-only state dump at a DMA stall (EXCI has the exception reason).
fn dma_stall_dump(gpu: &TxGpu, base: u32) {
    let r = |off: u32| gpu.pci.bar_read_u32(gpu.bar, (base + off) as u64);
    let cpuctl = r(0x100);
    let dmactl = r(0x10c);
    let cmd_last = r(0x118);
    let exci = r(0x0d0);
    let rm = r(0x084);
    let bootvec = r(0x104);
    let hwcfg = r(0x0f4);
    let xfer_status = r(0x120);
    info!(
        "DMA-STALL dump: CPUCTL={cpuctl:#010x} (halt={} stop={} iready={}) DMACTL={dmactl:#010x} CMD={cmd_last:#010x} EXCI={exci:#010x} RM={rm:#010x} BOOTVEC={bootvec:#010x} HWCFG={hwcfg:#010x}",
        cpuctl >> 4 & 1,
        cpuctl >> 5 & 1,
        cpuctl >> 3 & 1,
    );
    info!(
        "DMA-STALL: XFER_STATUS={xfer_status:#010x} (loads_pending={} stores_pending={} pending={})",
        xfer_status >> 24 & 7,
        xfer_status >> 16 & 7,
        xfer_status >> 1 & 1,
    );
    // EXCI field decode (falcon v4): bits8-16 exception, 17:16 substatus (best effort)
    if exci != 0 {
        info!("DMA-STALL: EXCI code={:#05x} sub={:#03x}", exci >> 8 & 0xffff, exci >> 24 & 0xff);
    }
}

/// falcon_hs geometry prevalidation (before any reset or transfer):
/// Range = image-offset arithmetic; Inval = DMA geometry rejected.
pub fn hs_prevalidate(
    image: u64,
    code_off: u32,
    code_size: u32,
    code_va: u32,
    code_pa: u32,
    data_off: u32,
    data_size: u32,
    data_pa: u32,
) -> Result<u64, TxError> {
    let (code_off, code_va, data_off) = (code_off as u64, code_va as u64, data_off as u64);
    if image > u64::MAX - code_off
        || image + code_off < code_va
        || image > u64::MAX - data_off
    {
        return Err(TxError::ERange);
    }
    let code_src = image + code_off - code_va;
    if !falcon_dma_valid(code_pa, code_va as u32, code_src, code_size)
        || (data_size != 0 && !falcon_dma_valid(data_pa, 0, image + data_off, data_size))
    {
        return Err(TxError::EInval);
    }
    Ok(code_src)
}

/// Cold-state guards (task brief 9.6 semantics).
/// `post_flr`: we just FLR'd the function ourselves - the GPU's own re-boot
/// re-provisions WPR2 (measured 0xe00), which is NOT the "already used"
/// dirty state the guard exists to catch. PLM check stays.
pub fn claim_guards(gpu: &TxGpu, post_flr: bool) -> Result<(), TxError> {
    // FEAT_PLM is PL0-LOCKED on Turing (measured: badf5040) - the EALREADY
    // check is only possible where the register reads. An inaccessible read
    // (EIO) is treated as "cannot evaluate", not as a failure.
    match gpu.read32(regs::FEAT_PLM) {
        Err(TxError::EIO) => info!("claim: FEAT PLM unreadable (Turing-locked) - EALREADY check skipped"),
        Err(e) => return Err(e),
        Ok(plm) => {
            if plm == 0xffff_ffff {
                info!("claim: EALREADY (FEAT PLM open)");
                return Err(TxError::EAlready);
            }
        }
    }
    let hi = gpu.read32(regs::WPR2_HI)?;
    if hi != 0 && !post_flr {
        info!("claim: EBUSY (WPR2 hi={hi:#x})");
        return Err(TxError::EBusy);
    }
    if hi != 0 {
        info!("claim: post-FLR WPR2 hi={hi:#x} (self-induced, accepted)");
    }
    Ok(())
}

/// GFW completion wait (bounded poll; 2.05s per gfw_eval.h).
pub fn gfw_wait(gpu: &TxGpu) -> Result<(), TxError> {
    gpu.poll_us(regs::GFW_BOOT, 0xff, regs::GFW_BOOT_COMPLETED, 2_050_000, true)
}

/// falcon_hs full port (unlock_610.c 347-432): prevalidate -> reset ->
/// FBIF physical mode -> code/data DMAs -> signature/selection regs ->
/// boot vector + mailboxes -> start (CPUCTL alias path aware) -> wait.
/// `image` is the DMA (device) address of the firmware image.
pub fn falcon_hs(
    gpu: &TxGpu,
    base: u32,
    fbif: u32,
    falcon2: u32,
    code_off: u32,
    code_size: u32,
    code_va: u32,
    code_pa: u32,
    data_off: u32,
    data_size: u32,
    data_pa: u32,
    sig_dmem: u32,
    engine_mask: u32,
    ucode_id: u32,
    image: u64,
    mailbox0: u32,
    mailbox1: u32,
    wait_plm: bool,
) -> Result<(), TxError> {
    // Transfer sizes are rounded up to the 256B Falcon DMA granularity
    // (the image tail is zero-filled by the caller, so the extra bytes
    // transfer zeros - same semantics as the reference's rounded image).
    let code_size = (code_size + 255) & !255;
    let data_size = (data_size + 255) & !255;
    info!("Falcon HS: image={image:#x} code={code_size:#x}@va={code_va:#x} data={data_size:#x}@off={data_off:#x} sig_dmem={sig_dmem:#x} eng={engine_mask:#x} ucode={ucode_id}");
    let code_src = hs_prevalidate(image, code_off, code_size, code_va, code_pa, data_off, data_size, data_pa)?;
    info!("Falcon HS: prevalidate ok (code_src={code_src:#x})");
    falcon_reset(gpu, base, fbif, falcon2)?;

    // FBIF: physical addressing mode.
    let mut trans = gpu.read32(fbif + 0x24)?;
    trans |= 1 << 7;
    gpu.write32(fbif + 0x24, trans);
    gpu.write32(base + 0x10c, 0);
    // Read-back (constitution 1.1): UC_BLOCK_ON_FIFO must actually clear.
    let block_on_fifo = gpu.read32(base + 0x10c).unwrap_or(0);
    let caps = gpu.pci.bar_read_u32(gpu.bar, (base + 0x108) as u64);
    info!(
        "Falcon HS: BLOCK_ON_FIFO after write0 = {block_on_fifo:#010x}; CAPS={caps:#010x} (code={}K data={}K fifo={}K slots={})",
        (caps >> 0 & 0x1ff) * 256 / 1024,
        (caps >> 9 & 0x1ff) * 256 / 1024,
        (caps >> 18 & 0xff) * 256 / 1024,
        caps >> 26 & 0x3f
    );
    let mut trans = gpu.read32(fbif)?;
    trans = (trans & !7u32) | 5;
    gpu.write32(fbif, trans);
    // FBIF write-back verification (constitution 1.1): if the FBIF domain
    // is gated (BSI handoff != DONE), these writes may be silently dropped
    // and every DMA fetch would be rejected at translation = the queue
    // fills but nothing is consumed.
    let ctl_rb = gpu.read32(fbif + 0x24);
    let trans_rb = gpu.read32(fbif)?;
    let dmactl_rb = gpu.read32(base + 0x10c).unwrap_or(0);
    info!(
        "Falcon HS: FBIF verify - CTL(+24)={:010x} TRANSCFG(0)={:010x} DMACTL={:010x}",
        ctl_rb.unwrap_or(0),
        trans_rb,
        dmactl_rb
    );
    // Post-reset IMEM state probes (read-only): page-tag/scrub hypothesis
    // for the 8KB DMA stall. IMEMC0 carries SECRET(bit28)/BLK(bits23:8);
    // a 250ms settle lets any asynchronous scrub finish before DMA.
    boot::stall(core::time::Duration::from_millis(250));
    let imemc0 = gpu.pci.bar_read_u32(gpu.bar, (base + 0x180) as u64);
    let hw = gpu.read32(base + 0xf4)?;
    let bsi = gpu.pci.bar_read_u32(gpu.bar, 0x0011_80fc);
    info!(
        "Falcon HS: post-reset IMEMC0={imemc0:#010x} (secret={} scrub_active={} blk={}) HWCFG={hw:#010x} BSI_SCRATCH15={bsi:#010x} (scrubber_handoff={})",
        imemc0 >> 28 & 1,
        imemc0 >> 31 & 1,
        imemc0 >> 8 & 0xffff,
        bsi >> 29 & 7,
    );

    falcon_dma(gpu, base, code_pa, code_va, code_src, code_size, regs::DMA_CMD_IMEM)?;
    if data_size != 0 {
        falcon_dma(gpu, base, data_pa, 0, image + data_off as u64, data_size, regs::DMA_CMD_DMEM)?;
    }

    // Signature + engine selection, then boot vector and mailboxes.
    gpu.write32(falcon2 + 0x210, sig_dmem);
    gpu.write32(falcon2 + 0x19c, engine_mask);
    gpu.write32(falcon2 + 0x198, ucode_id & 0xff);
    gpu.write32(falcon2 + 0x180, 1);
    gpu.write32(base + 0x104, code_va);
    gpu.write32(base + 0x40, mailbox0);
    gpu.write32(base + 0x44, mailbox1);

    let cpuctl = gpu.read32(base + 0x100)?;
    info!("Falcon start: base={base:#x} cpuctl={cpuctl:#x} code_va={code_va:#x} sig_dmem={sig_dmem:#x} engine_mask={engine_mask:#x} ucode={ucode_id} mailbox={mailbox1:#x}:{mailbox0:#x}");
    if cpuctl & (1 << 6) != 0 {
        gpu.write32(base + 0x130, 1 << 1); // CPUCTL alias start path
    } else {
        gpu.write32(base + 0x100, 1 << 1);
    }

    if wait_plm {
        // ~5s budget at the poll step rate; the PLM cold->open transition
        // is the payload-execution proof.
        let max_iters = 5_000_000 / 75 + 1;
        for _ in 0..max_iters {
            let engine = gpu.pci.bar_read_u32(gpu.bar, (base + 0x100) as u64);
            if engine == !0u32 || engine & 0xffff_0000 == 0xbadf_0000 {
                return Err(TxError::ENodev);
            }
            let plm = gpu.pci.bar_read_u32(gpu.bar, regs::FEAT_PLM as u64);
            if plm == !0u32 {
                return Ok(()); // cold entry was closed: this transition proves execution
            }
            if engine & (1 << 4) != 0 {
                info!("early PLM failed: cpuctl={engine:#x} plm={plm:#x}");
                return Err(TxError::EIO);
            }
            stall(Duration::from_micros(75));
        }
        return Err(TxError::ETimeout { tolerated: false });
    }
    // Non-PLM wait: HALT bit.
    gpu.poll_us(base + 0x100, 1 << 4, 1 << 4, 5_000_000, true)
}

/// List every entry in the boot-volume root: names + sizes as the FIRMWARE
/// sees them (kills hidden-extension / rename / placement hypotheses in
/// one log).
pub fn list_root_entries() {

    // Directory = FileHandle; read entries RAW (RegularFile::read) and parse
    // the EFI_FILE_INFO layout ourselves: size/file_size/physical_size
    // (3x u64 = 24), 3x Time (16B each = 48), attribute u64 = 72, name
    // UCS-2 from offset 72. This bypasses uefi's entry API, whose error
    // path instantiates CStr16 helpers that reference wcslen under LTO.
    let Ok(mut fs) = boot::get_image_file_system(boot::image_handle()) else {
        info!("TX-ASSET-DIAG: get_image_file_system FAILED");
        return;
    };
    let Ok(mut dir) = fs.open_volume() else {
        info!("TX-ASSET-DIAG: open_volume FAILED");
        return;
    };
    // Directory is a newtype over RegularFile; transmute the reference to
    // reach RegularFile::read directly (same layout, repr(transparent)),
    // bypassing the entry API whose error path references wcslen under LTO.
    // Safety: Directory(RegularFile) is repr(transparent) over the same
    // FileProtocolV1 handle; read is valid on directory handles per UEFI.
    #[allow(unused_imports)]
    use uefi::proto::media::file::RegularFile;
    let raw: &mut RegularFile = unsafe { core::mem::transmute(&mut dir) };
    loop {
        let mut buf = [0u8; 512];
        let n = match raw.read(&mut buf) {
            Ok(n) => n,
            Err(e) => { info!("TX-ROOT-DIR: raw read err {:?}", e.status()); break; }
        };
        if n == 0 {
            break; // end of directory
        }
        if n < 76 {
            info!("TX-ROOT-DIR: short entry {n}");
            continue;
        }
        let file_size = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let mut name = [0u8; 96];
        let mut m = 0usize;
        let mut chars = 0usize;
        while chars + 1 < n - 72 && m + 1 < name.len() {
            let u = u16::from_le_bytes([buf[72 + chars], buf[73 + chars]]);
            if u == 0 { break; }
            name[m] = if (0x20..0x7f).contains(&u) { u as u8 } else { b'?' };
            m += 1;
            chars += 2;
        }
        let name = core::str::from_utf8(&name[..m]).unwrap_or("?");
        info!("TX-ROOT-DIR: {name} size={file_size}");
    }
}

/// Shared PciIo access helper for the pre-transaction FLR experiment.
fn pre_flr_experiment(handle: uefi::Handle) {
    // Opt-in via a flag file on the boot volume. The kernel reference flow
    // (and unlock_610.c) run the scrubber/FWSEC chain WITHOUT any pre-FLR -
    // and our measured post-FLR state (GFW_BOOT stuck 0x101, both chips)
    // is an environment the kernel never sees. Default OFF so the next
    // round tests the kernel-matching sequence.
    if read_boot_file("\\pre-flr-on.txt").is_none() {
        info!("PRE-FLR: disabled (no \\pre-flr-on.txt) - kernel-matching flow");
        return;
    }
    info!("PRE-FLR experiment: resetting function to clean state");
    use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams};
    // Safety: GetProtocol during boot, single-threaded; the FLR runs while
    // the scoped protocol handle is alive, then the reference is dropped.
    let mut scoped = match unsafe {
        boot::open_protocol::<crate::pci::PciIo>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    } {
        Ok(p) => p,
        Err(_) => { info!("PRE-FLR: cannot open PciIo"); return; }
    };
    flr_once(&mut scoped);

    // The FLR triggers the GPU's own re-boot: GFW restarts and re-provisions
    // WPR2 (measured: gfw_boot=0x101 in-progress, wpr2_hi=0xe00). WAIT for
    // that boot to settle before anything else (reference wait_post_flr_gfw).
    let mut settled = false;
    for i in 0..100 {
        let gfw = scoped.bar_read_u32(0, 0x0011_8234);
        if gfw == 0xff {
            info!("PRE-FLR: GFW re-boot completed after {} polls (100ms each)", i + 1);
            settled = true;
            break;
        }
        boot::stall(core::time::Duration::from_millis(100));
    }
    if !settled {
        info!("PRE-FLR: GFW did not re-complete within 10s - continuing anyway");
    }
    let bsi = scoped.bar_read_u32(0, 0x0011_80fc);
    info!("PRE-FLR: post-settle BSI_SCRATCH15={bsi:#010x} (handoff={})", bsi >> 29 & 7);
}

/// Formatted variant: substitutes `{}` with `family` then reads.
pub fn read_boot_file_fmt(pattern: &str, family: &str) -> Option<alloc::vec::Vec<u8>> {
    let path = alloc::format!("{}", pattern.replacen("{}", family, 1));
    read_boot_file(&path)
}

/// Read one file from the boot volume with per-step diagnostics.
pub fn read_boot_file(path: &str) -> Option<alloc::vec::Vec<u8>> {
    // Runtime path: CString16::try_from (cstr16! only takes literals).
    let path_c = match uefi::CString16::try_from(path) {
        Ok(p) => p,
        Err(_) => { info!("TX-ASSET-DIAG: {path} not UCS-2 convertible"); return None; }
    };
    let path: &uefi::CStr16 = &path_c;
    use alloc::vec::Vec;
    use uefi::proto::media::file::{File, FileInfo, FileMode, FileAttribute};

    let mut fs = match boot::get_image_file_system(boot::image_handle()) {
        Ok(fs) => fs,
        Err(e) => { info!("TX-ASSET-DIAG: {path} fs open failed {:?}", e.status()); return None; }
    };
    let mut dir = match fs.open_volume() {
        Ok(d) => d,
        Err(e) => { info!("TX-ASSET-DIAG: {path} open_volume failed {:?}", e.status()); return None; }
    };
    let file = match dir.open(path, FileMode::Read, FileAttribute::empty()) {
        Ok(f) => f,
        Err(e) => { info!("TX-ASSET-DIAG: {path} OPEN failed {:?}", e.status()); return None; }
    };
    let mut file = match file.into_regular_file() {
        Some(f) => f,
        None => { info!("TX-ASSET-DIAG: {path} not a regular file"); return None; }
    };
    // Exact size first (preallocate once - no 84MB realloc churn).
    let size = match file.get_boxed_info::<FileInfo>() {
        Ok(info) => info.file_size() as usize,
        Err(e) => { info!("TX-ASSET-DIAG: {path} get_info failed {:?}", e.status()); return None; }
    };
    let mut out = Vec::with_capacity(size);
    unsafe { out.set_len(0); }
    let mut chunk = [0u8; 65536];
    let mut got = 0usize;
    while got < size {
        let want = (size - got).min(chunk.len());
        match file.read(&mut chunk[..want]) {
            Ok(0) => { info!("TX-ASSET-DIAG: {path} EOF at {got}/{size}"); break; }
            Ok(n) => { out.extend_from_slice(&chunk[..n]); got += n; }
            Err(e) => { info!("TX-ASSET-DIAG: {path} read err at {got}/{size}: {:?}", e.status()); return None; }
        }
    }
    info!("TX-ASSET-DIAG: {path} loaded {}/{} bytes", out.len(), size);
    Some(out)
}

/// Assets for the transaction, loaded from the boot volume (tx builds
/// carry their bundles next to the app; assets pipeline produces them).
pub struct TxAssets {
    /// PCI device-id family key ('ad102' | 'ga102' | 'tu10x').
    pub family: &'static str,
    /// Booter bundle (BTL1), e.g. booter_load_ad102.bin.
    pub booter: alloc::vec::Vec<u8>,
    /// GSP boot descriptor (gsp_boot_desc_ad102.bin, 84 bytes).
    pub boot_desc: Option<alloc::vec::Vec<u8>>,
    /// GSP bootloader image (gsp_boot_ad102.bin) — the boot image.
    pub gsp_boot: alloc::vec::Vec<u8>,
    /// GSP-RM firmware image (.fwimage of gsp_ga10x.bin) — the radix payload.
    pub gsp_image: alloc::vec::Vec<u8>,
    /// Stock signature section (.fwsignature_ad10x, 4096 bytes).
    pub stock_sig: alloc::vec::Vec<u8>,
    /// Optional V67 payload (present = four-write unlock; absent = chain replay).
    pub v67: Option<alloc::vec::Vec<u8>>,
}

/// Transaction, phases mirror sim::transaction and the C reference:
/// guards -> gfw -> layout -> FWSEC ignition (WPR2) -> [booter fire ->
/// triad -> selectors -> signature restore -> FLR + post-FLR].
/// The bracketed tail lands with the booter-publication tranche.
pub fn run_transaction(gpu: &TxGpu, assets: &TxAssets, pci_handle: uefi::Handle) -> Result<(), TxError> {
    info!("TX: BEGIN (phases: detach claim pre-flr scrub gfw layout fwsec booter verify selectors restore flr)");

    // TOTORONG craft: all console + vendor drivers off the device, PciIo
    // exclusively claimed, IO|MEM|BME enabled, config baseline captured.
    // (Black screen starts here by design; the log file keeps recording.)
    if let Err(e) = crate::tx_detach::detach_and_claim(pci_handle) {
        info!("TX: detach+claim FAILED: {e}");
        return Err(TxError::ENodev);
    }
    info!("TX: detach+claim ok");

    // PRE-FLR (reference never does this, but it also never runs in a
    // firmware window with BSI handoff=1): force the card through a full
    // function reset. The FLR triggers the GPU's own re-boot (GFW restarts,
    // WPR2 re-provisioned) - pre_flr_experiment waits for that boot to
    // settle and re-reads the BSI handoff.
    pre_flr_experiment(pci_handle);

    claim_guards(gpu, true)?;

    // Differential authority map while both engines sit in the as-booted
    // state: localizes the SEC2 start-authority gate before any reset.
    authority_scan(gpu);

    // The kernel AD102 driver runs the SEC2 scrubber before FWSEC, but the
    // community unlock chains (unlock_610.c "GA102 has no scrubber" + every
    // gpu-unlock-ada card through 28e1) boot FWSEC/booter WITHOUT one - and
    // their entry state equals ours (GFW done, BSI handoff=1). Attempt it,
    // log the verdict, proceed either way: whether the chain crosses the
    // gated state decides the next fix round.
    match crate::tx_fwsec::scrubber_if_needed(gpu) {
        Ok(()) => info!("TX: scrubber ok (domains un-gated)"),
        Err(e) => info!(
            "TX: scrubber did not complete ({e:?}) - proceeding without it (community chain shape)"
        ),
    }

    let boot_size = assets.gsp_boot.len() as u64;
    // layout_610.c in.image_size = the GSP ELF payload (the radix image,
    // 84 MB on AD10x), NOT the boot loader. Passing the boot size here
    // sized the gsp region for 36 KiB and wpr_meta_pack then (correctly)
    // refused: 84 MB > boot_offset - gsp_offset (EInval, 2026-10-26 log).
    let gsp_image_size = assets.gsp_image.len() as u64;
    let layout = crate::tx_fwsec::layout_from_registers(gpu, gsp_image_size, boot_size)?;
    info!("TX: layout ok (frts={:#x})", layout.frts_offset);

    crate::tx_fwsec::fwsec_ignite(gpu, &layout)?;
    info!("TX: fwsec ok (WPR2 verified)");

    use crate::tx_booter as tb;

    // Boot descriptor + booter bundle validation (per-family pinned
    // geometry; family comes from the PCI device-id mapping in tx_phase).
    let boot_image_len = assets.gsp_boot.len() as u32;
    let desc = assets.boot_desc.as_ref().ok_or(TxError::EInval)?;
    let boot = tb::boot_desc_parse(desc, boot_image_len)?;
    let (hdr, booter_image, booter_sigs) = tb::load_booter(&assets.booter, assets.family)?;
    info!("TX: booter bundle ok (image {} sigs {})", booter_image.len(), booter_sigs.len());

    // DMA allocations: booter image, v67-or-stock signature, meta, stock, boot image.
    let payload_mode = assets.v67.is_some();
    let sig_size = if payload_mode { tb::V67_SIZE } else { assets.stock_sig.len() };
    let booter_len = booter_image.len();
    let mut booter_dma = gpu.pci.dma_alloc(booter_len).map_err(|_| TxError::ENodev)?;
    let mut sig_dma = gpu.pci.dma_alloc(sig_size.max(4096)).map_err(|_| TxError::ENodev)?;
    let mut meta_dma = gpu.pci.dma_alloc(tb::WPR_META_SIZE).map_err(|_| TxError::ENodev)?;
    let mut stock_dma = gpu.pci.dma_alloc(4096).map_err(|_| TxError::ENodev)?;
    let mut boot_dma = gpu.pci.dma_alloc(assets.gsp_boot.len()).map_err(|_| TxError::ENodev)?;

    unsafe {
        // The IMAGE always carries the STOCK signature: the BROM validates
        // the booter before it runs. Payload mode aims the WprMeta
        // signature DMA at the V67 blob instead - the booter's own
        // (unbounded) signature copy overflows its DMEM stack: that IS the
        // exploit (patching the image with V67 is impossible anyway -
        // 0xFA00 > any booter image, EInval).
        core::ptr::copy_nonoverlapping(booter_image.as_ptr(), booter_dma.host, booter_len);
        {
            let fuse_raw = gpu.read32(0x0082_4140 + (hdr.ucode_id - 1) * 4)?;
            let fuse_ver = crate::tx_fwsec::fuse_version(fuse_raw);
            info!("TX: booter sig fuse_ver={fuse_ver} (payload_mode={payload_mode})");
            let image = core::slice::from_raw_parts_mut(booter_dma.host, booter_len);
            tb::booter_patch_stock(image, &hdr, booter_sigs, fuse_ver)?;
        }
        match &assets.v67 {
            Some(v67) => core::ptr::copy_nonoverlapping(v67.as_ptr(), sig_dma.host, tb::V67_SIZE),
            None => core::ptr::copy_nonoverlapping(assets.stock_sig.as_ptr(), sig_dma.host, assets.stock_sig.len()),
        }
        core::ptr::copy_nonoverlapping(assets.stock_sig.as_ptr(), stock_dma.host, assets.stock_sig.len());
        core::ptr::copy_nonoverlapping(assets.gsp_boot.as_ptr(), boot_dma.host, assets.gsp_boot.len());
    }

    // Radix over the GSP-RM image (84 MiB) — one big contiguous DMA block.
    let radix = tb::radix_build(gpu, &assets.gsp_image)?;

    // WprMeta publication (first signature call) then fire.
    let fb_size = (gpu.read32(0x0011_83a4)? as u64) << 20;
    let vga = gpu.read32(0x0062_5f04)?;
    let vga_offset = if vga & (1 << 3) != 0 {
        let base = ((vga >> 8) as u64) << 16;
        if base < fb_size - (1 << 20) { fb_size - 0x20000 } else { base }
    } else {
        fb_size - (1 << 20)
    };
    {
        let meta = unsafe { core::slice::from_raw_parts_mut(meta_dma.host, tb::WPR_META_SIZE) };
        tb::wpr_meta_pack(meta, &layout, fb_size, vga_offset, 1 << 20,
                          sig_dma.dev, sig_size as u64, radix.level0,
                          assets.gsp_image.len() as u64, boot_dma.dev, &boot)?;
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    info!("TX: WprMeta published (payload_mode={payload_mode})");

    // Portal placement needs the HOST pointer to the patched image.
    let booter_view = unsafe { core::slice::from_raw_parts(booter_dma.host, booter_len) };
    tb::booter_fire(gpu, &hdr, booter_view, meta_dma.dev)?;
    info!("TX: booter fired");

    tb::verify_execution(gpu, payload_mode)?;
    info!("TX: execution verified");

    // Selectors (payload mode: PLM open -> host writes stick).
    if payload_mode {
        tb::write_selector(gpu, "SS0", 0x0082_381c, tb::SS0_UNLOCK)?;
        tb::write_selector(gpu, "SS1", 0x0082_3820, tb::SS1_UNLOCK)?;
        info!("SELECTOR RESULT: SS0={:#x} SS1={:#x}", tb::SS0_UNLOCK, tb::SS1_UNLOCK);
        // Stock signature restore BEFORE FLR.
        let meta = unsafe { core::slice::from_raw_parts_mut(meta_dma.host, tb::WPR_META_SIZE) };
        tb::signature_restore(meta, stock_dma.dev, assets.stock_sig.len() as u64);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        info!("TX: stock signature restored");
    }

    tb::flr_phase(gpu, payload_mode)?;
    info!("POST-FLR: verified (selectors survive AON)");

    // Restore the display path before handing off (TOTORONG craft): the
    // FLR-rebooted function re-binds its vendor GOP/driver stack. Non-fatal
    // - bootmgfw self-establishes a console regardless.
    crate::tx_detach::reconnect_all(pci_handle);

    // Unwind: release every DMA buffer.
    let _ = booter_dma.release(gpu.pci);
    let _ = sig_dma.release(gpu.pci);
    let _ = meta_dma.release(gpu.pci);
    let _ = stock_dma.release(gpu.pci);
    let _ = boot_dma.release(gpu.pci);
    let mut radix = radix;
    let _ = radix.dma.release(gpu.pci);
    info!("FINAL: transaction complete, all DMA released");
    Ok(())
}

/// One Function-Level Reset with spec timing and command restore.
/// Per reference post-FLR expectations: PCI identity intact, GFW completion
/// persists, WPR2 cleared, engines halted - the "cold-boot-equivalent" state.
pub fn flr_once(pci: &crate::pci::PciIo) {
    // pci_save_state analog: command + BAR0-5 config dwords (0x04..0x28).
    // PCIe FLR resets these to defaults - WITHOUT restore, the device no
    // longer decodes its BARs and every memory read master-aborts to 0xff
    // (measured: gfw_boot=0xffffffff after FLR). This is the concrete
    // content of the Linux dev's "PCI re-discovery" point.
    let mut saved = [0u32; 10]; // 0x04 command, 0x08..0x0c, 0x10..0x24 BARs
    for (i, slot) in saved.iter_mut().enumerate() {
        *slot = pci.config_read_u32(0x04 + i as u32 * 4);
    }
    // TOTORONG craft (port.c GuFlr): save Device Control (our old code wrote
    // 0x0000 over it, wiping MPS/MRR), wait Transactions Pending clear,
    // set the FLR bit preserving the rest of the register, then 100 ms of
    // NO access at all (PCI spec: the function need not respond).
    let devctl = pci.config_read_u16(0x78 + 0x08);
    for _ in 0..1000 {
        let devsta = pci.config_read_u16(0x78 + 0x0a);
        if devsta == 0xffff || devsta & 0x20 == 0 {
            break;
        }
        boot::stall(core::time::Duration::from_millis(1));
    }
    pci.cfg_write_u16(0x04, 0x0003); // decode on for the restore, BME off
    pci.cfg_write_u16(0x78 + 0x08, devctl | 0x8000);
    boot::stall(core::time::Duration::from_millis(100)); // spec FLR time, silence
    pci.cfg_write_u16(0x78 + 0x08, devctl); // restore MPS/MRR, clear FLR bit
    // Wait for the function to answer config again before touching BARs.
    for _ in 0..1000 {
        let id = pci.config_read_u32(0x00);
        if id != 0xffff_ffff && id != 0 && id & 0xffff == 0x10de {
            break;
        }
        boot::stall(core::time::Duration::from_millis(1));
    }
    // pci_restore_state analog, TOTORONG order: decode OFF while BARs move,
    // high BAR dwords BEFORE low dwords, command (never BME) last + verify.
    pci.cfg_write_u16(0x04, 0x0000);
    for i in (2..10).rev() {
        pci.cfg_write_u32(0x04 + i as u32 * 4, saved[i]);
    }
    pci.cfg_write_u16(0x04, (saved[0] as u16) & !0x0004);
    let cmd = pci.config_read_u16(0x04);
    let sta = pci.config_read_u16(0x78 + 0x0a);
    let gfw = pci.bar_read_u32(0, 0x0011_8234);
    let wpr_hi = pci.bar_read_u32(0, 0x001f_a828);
    info!(
        "FLR: cmd={cmd:#06x} devsta={sta:#06x} (pending={}) gfw_boot={gfw:#x} wpr2_hi={wpr_hi:#x}",
        sta & 0x200 != 0,
    );
}

/// Emergency FLR on any transaction failure: wipes every half-state we may
/// have left (engine resets, BCR switches, full DMA queues, SEC bit state)
/// so the Windows driver receives a cold-boot-equivalent card. TOTORONG's
/// unconditional end-of-transaction FLR serves the same purpose.
pub fn emergency_flr(pci_handle: uefi::Handle) {
    use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams};
    // Safety: GetProtocol; the exclusive claim has been dropped by now.
    let pci = match unsafe {
        boot::open_protocol::<crate::pci::PciIo>(
            OpenProtocolParams {
                handle: pci_handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    } {
        Ok(p) => p,
        Err(_) => {
            info!("EMERGENCY-FLR: cannot open PciIo - powering off is the only recovery");
            return;
        }
    };
    flr_once(&pci);
}
