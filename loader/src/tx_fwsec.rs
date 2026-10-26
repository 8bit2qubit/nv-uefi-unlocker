//! FWSEC ignition phase (unlock_610.c 921-1026) + helpers (vbios_610.c,
//! prep_610.c), no_std ports. Part of the `tx` feature.
#![allow(dead_code)] // pre-wired ahead of the gate; behavior twin in sim/assets



use log::info;
use uefi::boot;

use crate::pci::PciIo;
use crate::tx::{hs_prevalidate, inaccessible, TxError, TxGpu};

/// Extracted FWSEC from the card's own PROM (V3 descriptor semantics).
pub struct FwsecInfo {
    pub image_off: u32,     // image bytes within the ROM (after desc+sig area)
    pub image_size: u32,    // stored size rounded up to 256 (DMA granularity)
    pub stored_size: u32,   // exact stored bytes from the descriptor
    pub imem_pa: u32,
    pub imem_va: u32,
    pub imem_size: u32,
    pub dmem_pa: u32,
    pub dmem_size: u32,
    pub data_off: u32,      // DMEM offset within the image (= IMEM size)
    pub pkc_off: u32,       // PKC signature slot offset in DMEM
    pub engine_mask: u32,
    pub ucode_id: u32,
    pub sig_versions: u32,  // slot bitmap
    pub sig_off: u32,       // signature area within the ROM (desc + 44)
    pub interface_off: u32, // FRTS interface block offset in DMEM
}

fn le16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn find_bit_header(rom: &[u8]) -> Option<usize> {
    let mut a = 0usize;
    while a + 12 <= rom.len() {
        if le16(rom, a) == 0xB8FF && &rom[a + 2..a + 6] == b"BIT\0" {
            let hs = rom[a + 8] as usize;
            if hs >= 12 && a + hs <= rom.len() {
                let sum: u32 = rom[a..a + hs].iter().map(|&b| b as u32).sum();
                if sum & 0xFF == 0 {
                    return Some(a);
                }
            }
        }
        a += 2;
    }
    None
}

/// Expansion-ROM relocate, kernel semantics (twin of the tested
/// assets::vbios::find_expansion_offset).
fn find_expansion_offset(rom: &[u8]) -> Option<usize> {
    let mut curr = 0usize;
    let mut base_size = None;
    for _ in 0..16 {
        let pcir = le16(rom, curr + 0x18) as usize;
        let p = curr + pcir;
        match rom.get(p..p + 4) {
            Some(b"PCIR") | Some(b"NPDS") | Some(b"RGIS") => {}
            _ => return None,
        }
        let slen = le16(rom, p + 0x0a) as usize;
        let imglen = le16(rom, p + 0x10) as usize;
        let code = rom[p + 0x14];
        let mut sub = imglen;
        let npde_at = p + slen + 0xf & !0xf;
        if rom.get(npde_at..npde_at + 4) == Some(b"NPDE".as_slice()) {
            let rev = le16(rom, npde_at + 4);
            if rev == 0x10 || rev == 0x11 {
                sub = le16(rom, npde_at + 8) as usize;
            }
        }
        if code == 0xE0 {
            return Some(curr - base_size?);
        }
        if code == 0x00 && base_size.is_none() {
            base_size = Some(sub * 512);
        }
        if sub == 0 || curr + sub * 512 + 0x20 > rom.len() {
            return None;
        }
        curr += sub * 512;
    }
    None
}

/// Compact BIT -> token 0x70 -> ucode table -> FWSEC walk (Ada V3 path).
pub fn prom_find_fwsec(rom: &[u8], debug: bool) -> Option<FwsecInfo> {
    prom_find_ucode(rom, if debug { 0x45 } else { 0x85 })
}

/// Scrubber ucode (app 0x49) - the kernel runs it on SEC2 BEFORE FWSEC when
/// BSI SCRUBBER_HANDOFF < DONE (kgspExecuteScrubberIfNeeded_AD102).
pub fn prom_find_scrubber(rom: &[u8]) -> Option<FwsecInfo> {
    prom_find_ucode(rom, 0x49)
}

fn prom_find_ucode(rom: &[u8], want_app: u8) -> Option<FwsecInfo> {
    let bit = find_bit_header(rom)?;
    let expansion = find_expansion_offset(rom)?;
    let hdr_size = rom[bit + 8] as usize;
    let stride = rom[bit + 9] as usize;
    let tokens = rom[bit + 10] as usize;
    if !(6..=8).contains(&stride) {
        return None;
    }
    for t in 0..tokens {
        let tok = bit + hdr_size + t * stride;
        if rom.get(tok..tok + 2) != Some(&[0x70, 0x02][..]) {
            continue;
        }
        let data_ptr = if stride >= 8 { le32(rom, tok + 4) as usize } else { le16(rom, tok + 4) as usize };
        let table = expansion + le32(rom, data_ptr) as usize;
        if rom.get(table) != Some(&1) {
            continue;
        }
        let esz = rom[table + 2] as usize;
        let count = rom[table + 3] as usize;
        for e in 0..count {
            let at = table + 6 + e * esz;
            if *rom.get(at)? != want_app {
                continue;
            }
            let desc_off = expansion + le32(rom, at + 2) as usize;
            let vdesc = le32(rom, desc_off);
            let ver = (vdesc >> 8) & 0xFF;
            let size = vdesc >> 16;
            if want_app != 0x85 && ver != 3 {
                continue; // non-FWSEC entries only parsed in V3 (scrubber 0x49)
            }
            if ver == 2 {
                // Turing V2 (60B, BOOT_WITH_LOADER). NVOC-measured TU106:
                // IMEM load 0x9600 = sec_base 0x300 + sec_size 0x9300,
                // stored 0x9a10 = IMEM 0x9600 + DMEM 0x410.
                if size < 60 || desc_off + 60 > rom.len() {
                    return None;
                }
                let stored = le32(rom, desc_off + 4);
                let interface = le32(rom, desc_off + 12);
                let imem_load = le32(rom, desc_off + 20);
                let dmem_load = le32(rom, desc_off + 32);
                return Some(FwsecInfo {
                    image_off: desc_off as u32 + size,
                    image_size: (stored + 255) & !255,
                    stored_size: stored,
                    imem_pa: 0,
                    imem_size: imem_load,
                    imem_va: le32(rom, desc_off + 24),
                    dmem_pa: 0,
                    dmem_size: dmem_load,
                    data_off: imem_load,
                    pkc_off: le32(rom, desc_off + 8),
                    engine_mask: 0x400,
                    ucode_id: 9,
                    sig_versions: 0,
                    sig_off: desc_off as u32 + 44,
                    interface_off: interface,
                });
            }
            if ver != 3 || size < 44 || desc_off + 44 > rom.len() {
                return None; // other versions unsupported
            }
            // Field semantics aligned 1:1 with the TESTED assets::vbios V3
            // mapping (real-machine log caught a semantic mixup: imem/dmem
            // phys bases read as sizes -> imem=0x0/0xf500).
            let imem_phys = le32(rom, desc_off + 16);
            let imem_load = le32(rom, desc_off + 20);
            let dmem_phys = le32(rom, desc_off + 28);
            let dmem_load = le32(rom, desc_off + 32);
            // Reference semantics (vbios_610.c): the DMA image size is the
            // STORED size rounded UP to 256 (Falcon DMA granularity); the
            // tail is zero-filled by us.
            let stored = le32(rom, desc_off + 4);
            return Some(FwsecInfo {
                image_off: desc_off as u32 + size,
                image_size: (stored + 255) & !255,
                stored_size: stored,
                imem_pa: imem_phys,
                imem_size: imem_load,
                imem_va: le32(rom, desc_off + 24),
                dmem_pa: dmem_phys,
                dmem_size: dmem_load,
                // image layout is [IMEM | DMEM]: DMEM begins after IMEM load
                data_off: imem_load,
                pkc_off: le32(rom, desc_off + 8),
                engine_mask: le16(rom, desc_off + 36) as u32,
                ucode_id: rom[desc_off + 38] as u32,
                sig_versions: le16(rom, desc_off + 40) as u32,
                sig_off: desc_off as u32 + 44,
                interface_off: le32(rom, desc_off + 12),
            });
        }
    }
    None
}

/// gpu_unlock_fuse_version: bit position + 1 of the low-16 value.
pub fn fuse_version(raw: u32) -> u32 {
    let mut value = raw & 0xffff;
    if value == 0 {
        return 0;
    }
    let mut index = 0;
    while value > 1 {
        value >>= 1;
        index += 1;
    }
    index + 1
}

/// Signature slot offset = popcount of version bits BELOW the fuse version.
pub fn fwsec_sig_offset(versions: u32, fuse_ver: u32, sig_size: u32) -> Result<u32, TxError> {
    if fuse_ver >= 16 || versions > 0xffff {
        return Err(TxError::EInval);
    }
    if versions & (1 << fuse_ver) == 0 {
        return Err(TxError::ERange);
    }
    Ok((versions & ((1 << fuse_ver) - 1)).count_ones() * sig_size)
}

/// gpu_unlock_fwsec_patch_frts: locate the audited V3 mapper in FWSEC DMEM
/// and inject the FRTS command carrying our layout address.
pub fn fwsec_patch_frts(image: &mut [u8], fw: &FwsecInfo, frts_offset: u64) -> Result<(), TxError> {
    if frts_offset == 0 || frts_offset & 0xfff != 0 || (frts_offset >> 12) > u32::MAX as u64 {
        return Err(TxError::EInval);
    }
    let d0 = fw.data_off as usize;
    let io = fw.interface_off as usize;
    if d0 + fw.dmem_size as usize > image.len() || io + 4 > fw.dmem_size as usize {
        return Err(TxError::EInval);
    }
    let dmem = &mut image[d0..d0 + fw.dmem_size as usize];
    if dmem[io] != 1 || dmem[io + 1] != 4 || dmem[io + 2] != 8 {
        return Err(TxError::EIO); // unsupported interface
    }
    let entries = dmem[io + 3] as usize;
    let mut off = io + 4;
    if entries < 2 || off + entries * 8 > dmem.len() {
        return Err(TxError::EInval);
    }
    let mut mapper = None;
    for _ in 0..entries {
        let dest = le32(dmem, off + 4) as usize;
        if le32(dmem, off) == 4 {
            if mapper.is_some() || dest + 64 > dmem.len() {
                return Err(TxError::EInval);
            }
            mapper = Some(dest);
        }
        off += 8;
    }
    let mapper = mapper.ok_or(TxError::EIO)?;
    if le16(dmem, mapper + 4) != 3 || le16(dmem, mapper + 6) < 64 {
        return Err(TxError::EIO); // only the audited V3 mapper layout
    }
    let cmd_off = le32(dmem, mapper + 8) as usize;
    let cmd_size = le32(dmem, mapper + 12) as usize;
    if cmd_size < 44 || cmd_off + cmd_size > dmem.len() {
        return Err(TxError::EInval);
    }
    let mut cmd = [0u8; 44];
    cmd[0..4].copy_from_slice(&1u32.to_le_bytes());
    cmd[4..8].copy_from_slice(&24u32.to_le_bytes());
    cmd[20..24].copy_from_slice(&2u32.to_le_bytes());
    cmd[24..28].copy_from_slice(&1u32.to_le_bytes());
    cmd[28..32].copy_from_slice(&20u32.to_le_bytes());
    cmd[32..36].copy_from_slice(&((frts_offset >> 12) as u32).to_le_bytes());
    cmd[36..40].copy_from_slice(&0x100u32.to_le_bytes());
    cmd[40..44].copy_from_slice(&2u32.to_le_bytes());
    dmem[cmd_off..cmd_off + 44].copy_from_slice(&cmd);
    dmem[mapper + 44..mapper + 48].copy_from_slice(&0x15u32.to_le_bytes());
    info!("FWSEC mapper patched: frts={frts_offset:#x} cmd@{cmd_off:#x}");
    Ok(())
}

/// Layout register adapter (twin of the tested sim::layout).
pub struct LayoutOut {
    pub wpr_end: u64,
    pub frts_offset: u64,
    pub boot_offset: u64,
    pub gsp_offset: u64,
    pub heap_offset: u64,
    pub wpr_start: u64,
    pub nonwpr_start: u64,
}

pub fn layout_from_registers(
    gpu: &TxGpu,
    image_size: u64,
    boot_size: u64,
) -> Result<LayoutOut, TxError> {
    let fb_raw = gpu.read32_raw(0x0011_83a4);
    let vga_raw = gpu.read32_raw(0x0062_5f04);
    let plm_raw = gpu.read32_raw(0x001f_a7c8);
    let lo_raw = gpu.read32_raw(0x001f_a82c);
    let hi_raw = gpu.read32_raw(0x001f_a830);
    // Log the cold board inputs even when the layout is rejected - the state
    // cannot be reconstructed once the NVIDIA driver has bound.
    info!(
        "layout inputs: fb_mib={fb_raw:#010x} vga={vga_raw:#010x} mmu_plm={plm_raw:#010x} mmu_lo={lo_raw:#010x} mmu_hi={hi_raw:#010x} image={image_size:#x} boot={boot_size:#x}"
    );
    if inaccessible(fb_raw) || inaccessible(plm_raw) || inaccessible(lo_raw) || inaccessible(hi_raw)
    {
        return Err(TxError::EIO);
    }
    let fb_mib = fb_raw;
    // The VGA workspace pointer is the one layout input a poison value is
    // tolerable for: only bit3 decides, and bit3=0 (also the value on a
    // healthy card) means "no workspace pointer, reserve the FB top 1 MiB".
    // Measured 2026-10-26: a hybrid-shutdown boot with the dGPU as console
    // reads this register fine and takes the bit3=0 path (vga = FB top); a
    // boot where the iGPU owned the console left it gated (0xbadf1301) on the
    // same machine. Both land on the same fallback value.
    let vga = if inaccessible(vga_raw) {
        info!(
            "layout: VGA workspace gated ({vga_raw:#010x}) - dGPU was not the console owner; using the FB-top fallback (bit3=0 path, same value as a healthy card)"
        );
        0
    } else {
        vga_raw
    };
    let plm = plm_raw;
    let lo = lo_raw;
    let hi = hi_raw;
    if fb_mib == 0 || fb_mib == !0u32 || plm & 1 == 0 {
        return Err(TxError::ENodev);
    }
    let usable_fb = (fb_mib as u64) << 20;
    if usable_fb < 256 << 20 {
        info!("layout rejected: usable FB below 256 MiB");
        return Err(TxError::ERange);
    }
    let mut vga_base = usable_fb - (1 << 20);
    if vga & (1 << 3) != 0 {
        vga_base = ((vga >> 8) as u64) << 16;
        if vga_base < usable_fb - (1 << 20) {
            vga_base = usable_fb - 0x20000;
        }
    }
    if vga_base >= usable_fb {
        info!("layout rejected: VGA base at or above usable FB");
        return Err(TxError::ERange);
    }
    let mmu_lock_low = ((lo >> 4) as u64) << 12;
    let mmu_hi = ((hi >> 4) as u64) << 12;
    let mmu_top = (mmu_hi.saturating_add(0xffff)) & !0xffffu64;
    let mmu_lock_valid = mmu_hi >= mmu_lock_low && (mmu_top >> 20) == (usable_fb >> 20);
    if !mmu_lock_valid {
        info!("layout mmu: lock ignored (high address is not the FB top)");
    }
    let reserved = if mmu_lock_valid && mmu_lock_low < vga_base { mmu_lock_low } else { vga_base };

    let wpr_end = reserved & !(128 * 1024 - 1);
    let frts = wpr_end - (1 << 20);
    let boot = (frts - boot_size) & !4095u64;
    let gsp = (boot - image_size) & !(64 * 1024 - 1);
    let heap_off = (gsp - (88 << 20)) & !(1024 * 1024 - 1);
    let wpr_start = heap_off - (1 << 20);
    let nonwpr_start = wpr_start - (1 << 20);
    if nonwpr_start >= usable_fb
        || wpr_end > usable_fb
        || wpr_start >= wpr_end
        || gsp >= boot || boot >= frts || frts >= wpr_end
        || nonwpr_start < usable_fb - (256 << 20)
    {
        info!("layout rejected: reservation calculation");
        return Err(TxError::ERange);
    }
    info!("layout: fb={usable_fb:#x} vga={vga_base:#x} frts={frts:#x} wpr=[{wpr_start:#x},{wpr_end:#x}]");
    Ok(LayoutOut { wpr_end, frts_offset: frts, boot_offset: boot, gsp_offset: gsp, heap_offset: heap_off, wpr_start, nonwpr_start })
}

/// FWSEC ignition phase (unlock_610.c 921-1026, 1:1): read PROM, parse,
/// patch FRTS for our layout, install the fuse-version signature slot,
/// fire on the GSP, verify scratch + WPR2 against the layout.
pub fn fwsec_ignite(gpu: &TxGpu, layout: &LayoutOut) -> Result<(), TxError> {
    const GSP_BASE: u32 = 0x0011_0000;
    const GSP_FBIF: u32 = 0x0011_0600;
    const GSP_FALCON2: u32 = 0x0011_1000;
    info!("FWSEC: reading PROM at {:#x}", 0x300000);

    let mapped = 1024 * 1024usize;
    let mut rom_words = alloc::vec![0u32; mapped / 4];
    let st = gpu.pci.bar_read_u32_buf(gpu.bar, 0x300000, rom_words.as_mut());
    if st != uefi::Status::SUCCESS {
        info!("FWSEC-TRACE: PROM bulk read failed {:?}", st);
        return Err(TxError::ENodev);
    }
    let rom: &[u8] = unsafe { core::slice::from_raw_parts(rom_words.as_ptr() as *const u8, mapped) };
    info!("FWSEC: PROM bytes={mapped} signature={:#x}", rom[0] as u16 | 0);

    let fw = match prom_find_fwsec(&rom, false) {
        Some(f) => f,
        None => { info!("FWSEC-TRACE: PROM parse found no FWSEC_PROD V3"); return Err(TxError::ENodev); }
    };
    if fw.engine_mask != 0x400 {
        info!("FWSEC-TRACE: unsupported engine mask={:#x}", fw.engine_mask);
        return Err(TxError::ENodev);
    }
    info!(
        "FWSEC: imem={:#x}/{:#x} dmem={:#x}/{:#x} pkc={:#x} sigs={:#x} ucode={}",
        fw.imem_pa, fw.imem_size, fw.dmem_pa, fw.dmem_size, fw.pkc_off, fw.sig_versions, fw.ucode_id
    );
    info!(
        "FWSEC: stored={:#x} dma_image={:#x} (256-rounded, tail zeroed) data_off={:#x}",
        fw.stored_size, fw.image_size, fw.data_off
    );

    info!("FWSEC-TRACE: alloc {} bytes for FWSEC image", fw.image_size);
    let mut dma = match gpu.pci.dma_alloc(fw.image_size as usize) {
        Ok(d) => d,
        Err(st) => { info!("FWSEC-TRACE: dma_alloc FAILED {:?}", st); return Err(TxError::ENodev); }
    };
    unsafe {
        // Zero the whole block first (rounded tail must be 0), then copy
        // the exact stored-size payload.
        core::ptr::write_bytes(dma.host, 0, fw.image_size as usize);
        core::ptr::copy_nonoverlapping(
            rom.as_ptr().add(fw.image_off as usize),
            dma.host,
            fw.stored_size as usize,
        );
    }
    {
        let image = unsafe { core::slice::from_raw_parts_mut(dma.host, fw.image_size as usize) };
        fwsec_patch_frts(image, &fw, layout.frts_offset)?;
    }

    let fuse_raw = gpu.read32(0x0082_41c0 + (fw.ucode_id - 1) * 4)?;
    let version = fuse_version(fuse_raw);
    info!("FWSEC fuse: raw={fuse_raw:#x} version={version}");
    let sig_at = fwsec_sig_offset(fw.sig_versions, version, 384)?;
    let d0 = fw.data_off as usize;
    unsafe {
        core::ptr::copy_nonoverlapping(
            rom.as_ptr().add(fw.sig_off as usize + sig_at as usize),
            dma.host.add(d0 + fw.pkc_off as usize),
            384,
        );
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    // Portal placement instead of falcon_hs DMA: the GSP falcon DMA engine
    // accepts 64 commands (2x32 slots) and never consumes them even with
    // FBIF unblocked + DMACTL=0 (measured 2026-10-26). The register portal
    // is proven on both engines; everything after placement (BROM PKC
    // params, BOOTVEC, start, HALT) matches the kernel HS sequence.
    const GSP_SEC: u32 = 1 << 28;
    crate::tx::falcon_reset(gpu, GSP_BASE, GSP_FBIF, GSP_FALCON2)?;
    crate::tx::falcon_dma_unblock(gpu, GSP_BASE, GSP_FBIF)?;
    crate::tx::falcon_authority_probe(gpu, GSP_BASE, "GSP");
    let image = unsafe { core::slice::from_raw_parts(dma.host, fw.image_size as usize) };
    let imem = &image[..fw.imem_size as usize];
    {
        // Kernel FWSEC DMA commands carry SEC=1 for the whole IMEM.
        let blocks = imem.len().div_ceil(256);
        let mut tag = fw.imem_va >> 8;
        for blk in 0..blocks {
            let off = blk * 256;
            gpu.write32(GSP_BASE + 0x180, ((fw.imem_pa + off as u32) & 0x00ff_fffc) | GSP_SEC | 1 << 24);
            gpu.write32(GSP_BASE + 0x188, tag & 0xffff);
            tag += 1;
            for w in 0..64 {
                let b = off + w * 4;
                let v = if b + 4 <= imem.len() {
                    u32::from_le_bytes([imem[b], imem[b + 1], imem[b + 2], imem[b + 3]])
                } else {
                    let mut tmp = [0u8; 4];
                    for (k, t) in tmp.iter_mut().enumerate() {
                        if b + k < imem.len() {
                            *t = imem[b + k];
                        }
                    }
                    u32::from_le_bytes(tmp)
                };
                gpu.write32(GSP_BASE + 0x184, v);
            }
        }
        info!("FWSEC portal: IMEM {} blocks ({} bytes, secure)", blocks, imem.len());
    }
    let dmem = &image[fw.data_off as usize..(fw.data_off + fw.dmem_size) as usize];
    crate::tx_portal::portal_load_dmem(gpu, GSP_BASE, fw.dmem_pa, dmem);
    // BROM PKC params (kernel _kgspSetupPkcParams) + BOOTVEC + start.
    gpu.write32(GSP_FALCON2 + 0x210, fw.pkc_off);
    gpu.write32(GSP_FALCON2 + 0x19c, fw.engine_mask);
    gpu.write32(GSP_FALCON2 + 0x198, fw.ucode_id & 0xff);
    gpu.write32(GSP_FALCON2 + 0x180, 1); // MOD_SEL = ALGO_RSA3K
    gpu.write32(GSP_BASE + 0x104, fw.imem_va);
    // kflcnStartCpu semantics: ALIAS_EN set -> start via the alias register.
    let cpuctl_pre = gpu.pci.bar_read_u32(gpu.bar, (GSP_BASE + 0x100) as u64);
    if cpuctl_pre & (1 << 6) != 0 {
        gpu.write32(GSP_BASE + 0x130, 1 << 1);
    } else {
        gpu.write32(GSP_BASE + 0x100, 1 << 1);
    }
    let r = gpu.poll_us(GSP_BASE + 0x100, 1 << 4, 1 << 4, 5_000_000, true);
    let cpuctl = gpu.pci.bar_read_u32(gpu.bar, (GSP_BASE + 0x100) as u64);
    let exci = gpu.pci.bar_read_u32(gpu.bar, (GSP_BASE + 0x0d0) as u64);
    let mbox0 = gpu.pci.bar_read_u32(gpu.bar, (GSP_BASE + 0x40) as u64);
    info!("FWSEC portal forensic: CPUCTL={cpuctl:#010x} MAILBOX0={mbox0:#010x} EXCI={exci:#010x}");

    let scratch = gpu.read32(0x1400 + 0x0e * 4)?;
    info!("FWSEC result: scratch={scratch:#x} expected_hi16=0");
    let wpr_lo = gpu.pci.bar_read_u32(gpu.bar, 0x001f_a824);
    let wpr_hi = gpu.pci.bar_read_u32(gpu.bar, 0x001f_a828);
    info!(
        "FWSEC WPR2: low={wpr_lo:#x} high={wpr_hi:#x} expected_page={:#x} end={:#x}",
        layout.frts_offset >> 12, layout.wpr_end
    );
    r?;
    if scratch >> 16 != 0 {
        return Err(TxError::EIO);
    }
    if (wpr_hi >> 4) == 0
        || (wpr_hi >> 4) < (wpr_lo >> 4)
        || ((wpr_hi >> 4) as u64) << 12 >= layout.wpr_end
        || (wpr_lo >> 4) as u64 != layout.frts_offset >> 12
    {
        return Err(TxError::EIO);
    }
    let _ = dma.release(gpu.pci);
    info!("FWSEC: WPR2 verified");
    Ok(())
}

/// allow unused-import warnings for pre-wired items
const _: () = {
    let _ = hs_prevalidate as fn(u64, u32, u32, u32, u32, u32, u32, u32) -> Result<u64, TxError>;
    let _: Option<&PciIo> = None;
};

/// Kernel kgspExecuteScrubberIfNeeded_AD102 port: when the BSI scrubber
/// handoff is not DONE, the scrubber ucode must run on SEC2 first - until
/// then the GSP/SEC2 domains stay in their post-GFW gated state (SEC2_CPUCTL
/// badf5620; DMA fetches die behind REQUIRE_CTX).
///
/// The ucode is the KERNEL-EXACT artifact (ksec2GetBinArchiveSecurescrubUcode
/// BINDATA → BTL1, `\\scrubber_ad10x.bin`): image 7424B, ucode 0xd, 2×RSA-384.
/// The VBIOS PROM app 0x49 we tried first is a DIFFERENT ucode (22528B) that
/// is not what advances the handoff (measured 2026-10-26).
pub fn scrubber_if_needed(gpu: &TxGpu) -> Result<(), TxError> {
    const SEC2_BASE: u32 = 0x0084_0000;
    const SEC2_FBIF: u32 = 0x0084_0600;
    const SEC2_FALCON2: u32 = 0x0084_1000;

    let bsi = gpu.pci.bar_read_u32(gpu.bar, 0x0011_80fc);
    let handoff = bsi >> 29 & 7;
    info!("SCRUB: BSI_SCRATCH15={bsi:#010x} handoff={handoff} (DONE=3)");
    if handoff >= 3 {
        info!("SCRUB: handoff already DONE - skipping");
        return Ok(());
    }

    let bundle = crate::tx::read_boot_file("\\scrubber_ad10x.bin")
        .ok_or(TxError::ENodev)
        .inspect_err(|_| info!("SCRUB: \\scrubber_ad10x.bin missing on boot volume"))?;
    let (hdr, image, sigs) = crate::tx_booter::load_booter(&bundle, "scrub")?;
    info!(
        "SCRUB: bundle ok - code {:#x}+{:#x}@va{:#x} data {:#x}+{:#x} patch {:#x} ucode {:#x}",
        hdr.app_code_off, hdr.app_code_size, hdr.app_code_off, hdr.data_off, hdr.data_size, hdr.patch_loc, hdr.ucode_id
    );

    // Signature slot: SEC2 per-ucode fuse (kernel ksec2ReadUcodeFuseVersion:
    // FUSE_OPT_FPF_SEC2_UCODE1_VERSION + 4*(ucodeId-1)), then stock patch.
    let fuse_raw = gpu.read32(0x0082_4140 + (hdr.ucode_id - 1) * 4)?;
    let version = fuse_version(fuse_raw);
    info!("SCRUB: fuse_raw={fuse_raw:#010x} version={version} sigs={}", hdr.num_sigs);
    let mut image = alloc::vec::Vec::from(image);
    crate::tx_booter::booter_patch_stock(&mut image, &hdr, sigs, version)?;

    // HS geometry (kernel BootFromHs mapping): IMEM lands at physical 0
    // with virtual tags from imemVa; DMEM at 0; BROM sig lives at
    // patch_loc - dataOffset inside DMEM (0 here).
    let sig_dmem = hdr.patch_loc - hdr.data_off;

    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    // Reset + open the engine's DMA path (the scrubber's own FB scrub
    // translates through this FBIF), then portal-place the image - content
    // placement is equivalent to the kernel's DMA, the BROM validates from
    // IMEM/DMEM + the armed BROM registers.
    crate::tx::falcon_reset(gpu, SEC2_BASE, SEC2_FBIF, SEC2_FALCON2)?;
    crate::tx::falcon_dma_unblock(gpu, SEC2_BASE, SEC2_FBIF)?;
    // Kernel BootFromHs placement: secure code only, physical dest 0,
    // virtual tags from imemVa (the NS part of the image is not loaded).
    crate::tx_portal::portal_load_imem(
        gpu, SEC2_BASE, 0, hdr.app_code_off,
        &image[hdr.app_code_off as usize..(hdr.app_code_off + hdr.app_code_size) as usize], true,
    );
    crate::tx_portal::portal_load_dmem(
        gpu, SEC2_BASE, 0,
        &image[hdr.data_off as usize..(hdr.data_off + hdr.data_size) as usize],
    );

    // BROM params (kernel _kgspSetupPkcParams via unlock_610.c offsets).
    gpu.write32(SEC2_FALCON2 + 0x210, sig_dmem);
    gpu.write32(SEC2_FALCON2 + 0x19c, hdr.engine_id);
    gpu.write32(SEC2_FALCON2 + 0x198, hdr.ucode_id & 0xff);
    gpu.write32(SEC2_FALCON2 + 0x180, 1); // MOD_SEL = ALGO_RSA3K
    gpu.write32(SEC2_BASE + 0x104, hdr.app_code_off); // BOOTVEC = imemVa
    let cpuctl = gpu.read32(SEC2_BASE + 0x100)?;
    info!("SCRUB start: cpuctl={cpuctl:#x}");
    if cpuctl & (1 << 6) != 0 {
        gpu.write32(SEC2_BASE + 0x130, 1 << 1);
    } else {
        gpu.write32(SEC2_BASE + 0x100, 1 << 1);
    }
    let r = gpu.poll_us(SEC2_BASE + 0x100, 1 << 4, 1 << 4, 5_000_000, true);

    // Post-start forensics: HALT alone is not proof (a failed launch also
    // leaves the core halted) - read-back is the verdict.
    let cpuctl_now = gpu.pci.bar_read_u32(gpu.bar, (SEC2_BASE + 0x100) as u64);
    let exci = gpu.pci.bar_read_u32(gpu.bar, (SEC2_BASE + 0x0d0) as u64);
    let tracepc = gpu.pci.bar_read_u32(gpu.bar, (SEC2_BASE + 0x14c) as u64);
    let imemc = gpu.pci.bar_read_u32(gpu.bar, (SEC2_BASE + 0x180) as u64);
    let dmectl2 = gpu.pci.bar_read_u32(gpu.bar, (SEC2_BASE + 0x10c) as u64);
    let mbox0 = gpu.pci.bar_read_u32(gpu.bar, (SEC2_BASE + 0x40) as u64);
    info!(
        "SCRUB forensic: CPUCTL={cpuctl_now:#010x} MAILBOX0={mbox0:#010x} EXCI={exci:#010x} TRACEPC={tracepc:#010x} IMEMC0={imemc:#010x} DMACTL={dmectl2:#010x}"
    );
    // SEC2 DMEM dump via portal read (interface block + DMEM top window).
    let io = sig_dmem;
    let mut dump = alloc::vec![0u32; 32];
    crate::tx_portal::portal_read_dmem(gpu, SEC2_BASE, io, &mut dump);
    let nonzero: usize = dump.iter().filter(|&&w| w != 0).count();
    info!("SCRUB forensic: SEC2 DMEM@{io:#x} nonzero words = {nonzero}/32; first 8:");
    for i in 0..8 {
        info!("  DMEM[{i}] = {:#010x}", dump[i]);
    }
    let top = hdr.data_size & !0x7f;
    let mut stack = alloc::vec![0u32; 16];
    crate::tx_portal::portal_read_dmem(gpu, SEC2_BASE, top.saturating_sub(64), &mut stack);
    info!("SCRUB forensic: SEC2 DMEM top window @{:#x}: nonzero {}/16", top.saturating_sub(64), stack.iter().filter(|&&w| w != 0).count());
    for (i, w) in stack.iter().enumerate() {
        if *w != 0 {
            info!("  TOP[{i}] = {w:#010x}");
        }
    }

    // Completion: BSI handoff advances to DONE (poll generously).
    let mut handoff_now = 0;
    for _ in 0..50 {
        let b = gpu.pci.bar_read_u32(gpu.bar, 0x0011_80fc);
        handoff_now = b >> 29 & 7;
        if handoff_now >= 3 {
            break;
        }
        boot::stall(core::time::Duration::from_millis(100));
    }
    r?;
    if handoff_now < 3 {
        info!("SCRUB: SEC2 fired but handoff stuck at {handoff_now}");
        return Err(TxError::ETimeout { tolerated: false });
    }
    info!("SCRUB: handoff DONE - SEC2/GSP domains un-gated");
    Ok(())
}
