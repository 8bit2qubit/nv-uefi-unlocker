//! Booter publication + execution + selectors + FLR — final transaction
//! segment, behavioral port of unlock_610.c (434-489, 749-800, 878-920,
//! 1113-1432) and prep_610.c (boot_desc parse, booter patch, wpr_meta_pack).
#![allow(dead_code)]


use log::info;

use crate::tx::{TxError, TxGpu};
use crate::tx_fwsec::LayoutOut;

pub const WPR_META_MAGIC: u64 = 0xdc3a_ae21_371a_60b3;
pub const WPR_META_SIZE: usize = 256;
pub const V67_SIZE: usize = 0xfa00;
pub const SS0_UNLOCK: u32 = 0x8888_8888;
pub const SS1_UNLOCK: u32 = 0x8;
pub const BTL1_MAGIC: u32 = 0x314C_5442;

fn le16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn wr32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn wr64(d: &mut [u8], o: usize, v: u64) {
    d[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
fn rd64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}

/// GSP boot descriptor (prep_610.c gpu_unlock_boot_desc_parse, extended
/// with the Turing RM_RISCV_UCODE_DESC **v4** variant, 76 bytes).
pub struct BootDesc {
    pub code_offset: u32,
    pub data_offset: u32,
    pub manifest_offset: u32,
    pub image_size: u32,
    /// desc d7 (appVersion) — Turing writes it to GSP FALCON_OS after the
    /// booter load (kgspBootstrap_TU102).
    pub app_version: u32,
}

pub fn boot_desc_parse(desc: &[u8], image_size: u32) -> Result<BootDesc, TxError> {
    let f = |i: usize| le32(desc, i * 4);
    if desc.len() == 76 {
        // v4 "eb riscv boot": bootloader d1/d2 + param d3/d4 are the only
        // populated ranges on TU102; monitor/data/code (d8/d10/d12) are
        // zero and flow into WprMeta verbatim (kgspPopulateWprMeta_TU102).
        if f(0) != 4 {
            return Err(TxError::EInval);
        }
        for &i in &[1usize, 3] {
            let (off, len) = (f(i), f(i + 1));
            if len == 0 || off > image_size || len > image_size - off {
                return Err(TxError::ERange);
            }
        }
        return Ok(BootDesc {
            manifest_offset: f(8),
            data_offset: f(10),
            code_offset: f(12),
            image_size,
            app_version: f(7),
        });
    }
    if desc.len() != 84
        || f(0) != 5
        || f(14) != 1
        || f(19) != image_size
        || f(20) != 0
    {
        return Err(TxError::EInval);
    }
    for &i in &[1usize, 3, 8, 10, 12] {
        let (off, len) = (f(i), f(i + 1));
        if len == 0 || off > image_size || len > image_size - off {
            return Err(TxError::ERange);
        }
    }
    for i in 5..=7 {
        if f(i) != 0 {
            return Err(TxError::EIO); // secure-partition boot not supported
        }
    }
    for i in 15..=18 {
        if f(i) != 0 {
            return Err(TxError::EIO);
        }
    }
    Ok(BootDesc {
        manifest_offset: f(8),
        data_offset: f(10),
        code_offset: f(12),
        image_size,
        app_version: f(7),
    })
}

/// Booter bundle header (BTL1), field names follow the kernel's
/// s_allocateUcodeFromBinArchive header struct (kernel_gsp_booter.c).
pub struct BooterHdr {
    /// fields[0]/[1]: non-secure IMEM offset/size inside the image.
    pub imem_ns_off: u32,
    pub imem_ns_size: u32,
    /// fields[2]/[3]: DMEM offset/size inside the image.
    pub data_off: u32,
    pub data_size: u32,
    /// fields[5]/[6]: secure IMEM offset/size inside the image.
    pub app_code_off: u32,
    pub app_code_size: u32,
    pub patch_loc: u32,
    pub engine_id: u32,
    pub ucode_id: u32,
    pub num_sigs: u32,
    /// Per-signature size (sig_total / num_sigs): 384 (RSA) on GA10x/Ada,
    /// 16 on TU102 BOOT_DIRECT.
    pub sig_bytes: u32,
}

/// Load and validate a BTL1 booter bundle. Geometry comes from the bundle
/// header itself and is cross-checked against the per-family pins measured
/// from pinned 610.43.03 (constitution: identity pinned, no silent drift).
pub fn load_booter<'a>(bundle: &'a [u8], family: &str) -> Result<(BooterHdr, &'a [u8], &'a [u8]), TxError> {
    if bundle.len() < 72 || le32(bundle, 0) != BTL1_MAGIC || le32(bundle, 4) != 1 {
        return Err(TxError::EInval);
    }
    let h = |i: usize| le32(bundle, 8 + i * 4);
    let image_size = le32(bundle, 8 + 14 * 4);
    let sig_total = le32(bundle, 8 + 15 * 4);
    let hdr = BooterHdr {
        imem_ns_off: h(0),
        imem_ns_size: h(1),
        data_off: h(2),
        data_size: h(3),
        app_code_off: h(5),
        app_code_size: h(6),
        patch_loc: h(9),
        ucode_id: h(10),
        engine_id: h(11),
        num_sigs: h(12),
        sig_bytes: h(13),
    };
    // Kernel checks: numSigs > 0, total % numSigs == 0, patch inside image.
    if hdr.num_sigs == 0
        || sig_total == 0
        || sig_total % hdr.num_sigs != 0
        || hdr.patch_loc as u64 + (sig_total / hdr.num_sigs) as u64 > image_size as u64
    {
        info!("Booter: signature table inconsistent");
        return Err(TxError::EInval);
    }
    // IMEM/DMEM transfer ranges inside the image, 256B Falcon blocks.
    let ranges = [
        (hdr.imem_ns_off, hdr.imem_ns_size),
        (hdr.data_off, hdr.data_size),
        (hdr.app_code_off, hdr.app_code_size),
    ];
    for (off, size) in ranges {
        if off as u64 + size as u64 > image_size as u64 || off % 256 != 0 || size % 256 != 0 {
            info!("Booter: transfer range outside image ({off:#x}+{size:#x})");
            return Err(TxError::EInval);
        }
    }
    // Per-family pins (measured 2026-10-25 from pinned 610.43.03 bundles):
    // image size, patch_loc, ucode_id, engine_id, num_sigs, per-sig bytes.
    // "scrub" = the kernel-exact SEC2 scrubber BINDATA (boot-type HS).
    let (img, patch, ucode, engine, nsigs, per_sig) = match family {
        "ga102" => (60416, 35344, 3, 1, 2, 384),
        "ad102" => (56832, 32272, 3, 1, 2, 384),
        "tu10x" => (59136, 34560, 0xd, 1, 1, 16),
        "scrub" => (7424, 0x1a00, 0xd, 1, 2, 384),
        _ => {
            info!("Booter: unknown family {family}");
            return Err(TxError::EInval);
        }
    };
    if image_size != img
        || hdr.patch_loc != patch
        || hdr.ucode_id != ucode
        || hdr.engine_id != engine
        || hdr.num_sigs != nsigs
        || sig_total != per_sig * nsigs
        || bundle.len() != 72 + image_size as usize + sig_total as usize
    {
        info!(
            "Booter: geometry mismatch for {family} (image {image_size} patch {:#x} ucode {:#x} sigs {} total {sig_total})",
            hdr.patch_loc, hdr.ucode_id, hdr.num_sigs
        );
        return Err(TxError::EInval);
    }
    Ok((
        hdr,
        &bundle[72..72 + image_size as usize],
        &bundle[72 + image_size as usize..],
    ))
}

/// prep_610.c gpu_unlock_booter_patch: install the fuse-selected stock
/// signature into the patch slot (num_sigs-1-fuse_ver indexing).
pub fn booter_patch_stock(image: &mut [u8], hdr: &BooterHdr, signatures: &[u8], fuse_ver: u32) -> Result<(), TxError> {
    if hdr.num_sigs < 2 || fuse_ver >= hdr.num_sigs {
        return Err(TxError::ERange);
    }
    let sig_size = hdr.sig_bytes as usize;
    let index = (hdr.num_sigs - 1 - fuse_ver) as usize;
    let at = hdr.patch_loc as usize;
    if at + sig_size > image.len() || (index + 1) * sig_size > signatures.len() {
        return Err(TxError::EInval);
    }
    image[at..at + sig_size].copy_from_slice(&signatures[index * sig_size..(index + 1) * sig_size]);
    Ok(())
}

/// Replace the patch slot with the V67 payload image (the exploit vehicle).
pub fn booter_patch_v67(image: &mut [u8], hdr: &BooterHdr, v67: &[u8]) -> Result<(), TxError> {
    if v67.len() != V67_SIZE || hdr.patch_loc as usize + V67_SIZE > image.len() {
        return Err(TxError::EInval);
    }
    image[hdr.patch_loc as usize..hdr.patch_loc as usize + V67_SIZE].copy_from_slice(v67);
    Ok(())
}

/// Radix: three-level page tables over the GSP image, carved from ONE
/// contiguous DMA block (EFI-friendly variant of the per-page C allocator;
/// the tree only needs valid per-page device addresses).
pub struct RadixBlock {
    pub dma: crate::pci::DmaBuffer,
    pub level0: u64, // device address of L0
    pub pages: usize,
}

pub fn radix_build(gpu: &TxGpu, image: &[u8]) -> Result<RadixBlock, TxError> {
    const PAGE: usize = 4096;
    let leaves = image.len().div_ceil(PAGE);
    let l2 = leaves.div_ceil(512);
    let l1 = l2.div_ceil(512).max(1);
    let tables = 1 + l1 + l2;
    let total = tables + leaves;
    let dma = gpu.pci.dma_alloc(total * PAGE).map_err(|_| TxError::ENodev)?;
    // Safety: dma.host is valid for total*PAGE bytes.
    let mem = unsafe { core::slice::from_raw_parts_mut(dma.host, total * PAGE) };
    for b in mem.iter_mut() {
        *b = 0;
    }
    let dev = dma.dev;
    let page_addr = |i: usize| dev + (i as u64) * PAGE as u64;
    // leaves
    for i in 0..leaves {
        let off = i * PAGE;
        let n = (image.len() - off).min(PAGE);
        mem[tables * PAGE + off..tables * PAGE + off + n].copy_from_slice(&image[off..off + n]);
    }
    // tables
    let mut first = 0usize;
    for level in 0..3 {
        let counts = [1usize, l1, l2, leaves];
        let child = first + counts[level];
        for i in 0..counts[level] {
            let at = i * (PAGE / 8);
            let n = (counts[level + 1] - at).min(PAGE / 8);
            for e in 0..n {
                let addr = page_addr(child + at + e);
                let dst = (first + i) * PAGE + e * 8;
                wr64(mem, dst, addr);
            }
        }
        first = child;
    }
    info!("radix: leaves={leaves} tables={tables} level0={dev:#x}");
    Ok(RadixBlock { dma, level0: dev, pages: total })
}

/// prep_610.c gpu_unlock_wpr_meta_pack, 1:1.
#[allow(clippy::too_many_arguments)]
pub fn wpr_meta_pack(
    meta: &mut [u8],
    layout: &LayoutOut,
    frts_size: u64,
    signature_dma: u64,
    signature_size: u64,
    radix_dma: u64,
    image_size: u64,
    boot_dma: u64,
    boot: &BootDesc,
) -> Result<(), TxError> {
    // FB size and VGA reservation come from the layout that produced the
    // offsets - they must not be re-read from the device here (the VGA
    // register is gated on a non-console dGPU and a strict re-read EIO'd).
    let fb_size = layout.fb_size;
    let vga_offset = layout.vga_base;
    if signature_size == 0 || image_size == 0 || boot.image_size == 0
        || vga_offset >= fb_size
        || radix_dma & 4095 != 0 || boot_dma & 4095 != 0
        || layout.frts_offset >= layout.wpr_end
        || frts_size != layout.wpr_end - layout.frts_offset
        || layout.wpr_end > vga_offset
        || image_size > layout.boot_offset - layout.gsp_offset
        || boot.image_size as u64 > layout.frts_offset - layout.boot_offset
    {
        return Err(TxError::EInval);
    }
    for b in meta.iter_mut() {
        *b = 0;
    }
    wr64(meta, 0, WPR_META_MAGIC);
    wr64(meta, 8, 1);
    wr64(meta, 16, radix_dma);
    wr64(meta, 24, image_size);
    wr64(meta, 32, boot_dma);
    wr64(meta, 40, boot.image_size as u64);
    wr64(meta, 48, boot.code_offset as u64);
    wr64(meta, 56, boot.data_offset as u64);
    wr64(meta, 64, boot.manifest_offset as u64);
    wr64(meta, 72, signature_dma);
    wr64(meta, 80, signature_size);
    wr64(meta, 88, layout.nonwpr_start);
    wr64(meta, 96, layout.nonwpr_start);
    wr64(meta, 104, layout.wpr_start - layout.nonwpr_start);
    wr64(meta, 112, layout.wpr_start);
    wr64(meta, 120, layout.heap_offset);
    wr64(meta, 128, layout.gsp_offset - layout.heap_offset);
    wr64(meta, 136, layout.gsp_offset);
    wr64(meta, 144, layout.boot_offset);
    wr64(meta, 152, layout.frts_offset);
    wr64(meta, 160, frts_size);
    wr64(meta, 168, layout.wpr_end);
    wr64(meta, 176, fb_size);
    wr64(meta, 184, vga_offset);
    wr64(meta, 192, fb_size - vga_offset);
    Ok(())
}

/// Stock-signature restore: meta signature slot back to the stock DMA
/// (constitution 9.5 - the payload must never remain published).
pub fn signature_restore(meta: &mut [u8], stock_dma: u64, stock_size: u64) {
    wr64(meta, 72, stock_dma);
    wr64(meta, 80, stock_size);
}

/// Selector write with read-back (unlock_610.c write_selector, 1:1).
pub fn write_selector(gpu: &TxGpu, name: &str, offset: u32, unlock: u32) -> Result<(), TxError> {
    let value = gpu.read32(offset)?;
    info!("selector read: {name}={value:#x} expected_unlock={unlock:#x}");
    if value == unlock {
        return Ok(());
    }
    info!("selector write: {name}={unlock:#x}");
    gpu.write32(offset, unlock);
    let back = gpu.read32(offset)?;
    if back == unlock {
        Ok(())
    } else {
        info!("selector FAILED readback: {name}={back:#x}");
        Err(TxError::EIO)
    }
}

/// SEC2 booter fire, GA10x/Ada HS variant. Portal placement (the falcon
/// DMA engine accepts 64 commands and never consumes them in this window,
/// measured on both engines), then the kernel BootFromHs tail verbatim:
/// IMEM at physical 0 with virtual tags from app_code_off (dest=imemPa=0,
/// memOff=imemVa), DMEM at 0 (the patched signature rides inside the DMEM
/// section at patch_loc - data_off), BROM PKC params, BOOTVEC=imemVa,
/// WprMeta address in the mailboxes, start, HALT.
pub fn booter_fire(gpu: &TxGpu, hdr: &BooterHdr, image: &[u8], meta_dma: u64) -> Result<(), TxError> {
    const SEC2: u32 = crate::tx::eng::SEC2;
    let sig_dmem = hdr.patch_loc - hdr.data_off;
    // Kernel rule: every HS execute is preceded by an engine reset. After a
    // successful scrubber the BSI machine re-locks SEC2's PRI window
    // (post-halt reads go badf5620) - the reset re-opens it.
    crate::tx::falcon_reset(gpu, SEC2, crate::tx::eng::SEC2_FBIF, crate::tx::eng::SEC2_FALCON2)?;
    crate::tx::falcon_dma_unblock(gpu, SEC2, crate::tx::eng::SEC2_FBIF)?;
    crate::tx::falcon_authority_probe(gpu, SEC2, "SEC2");
    // Kernel BootFromHs loads ONLY the secure code segment, and lands it at
    // physical IMEM 0 (dest=imemPa=0) with virtual tags from imemVa - the
    // NS/os-code part of the image is NOT loaded at all. Our old load (NS at
    // 0 + sec at 0x100) double-tagged va 0x100: the core fetched the NS junk
    // and insta-faulted (EXCI 0x0100, TRACEPC stuck at 0x100).
    let sec = &image[hdr.app_code_off as usize..(hdr.app_code_off + hdr.app_code_size) as usize];
    crate::tx_portal::portal_load_imem(gpu, SEC2, 0, hdr.app_code_off, sec, true);
    let dmem = &image[hdr.data_off as usize..(hdr.data_off + hdr.data_size) as usize];
    crate::tx_portal::portal_load_dmem(gpu, SEC2, 0, dmem);
    gpu.write32(crate::tx::eng::SEC2_FALCON2 + 0x210, sig_dmem);
    gpu.write32(crate::tx::eng::SEC2_FALCON2 + 0x19c, hdr.engine_id);
    gpu.write32(crate::tx::eng::SEC2_FALCON2 + 0x198, hdr.ucode_id & 0xff);
    gpu.write32(crate::tx::eng::SEC2_FALCON2 + 0x180, 1); // MOD_SEL = RSA3K
    // BROM param read-back: the START drop proved SEC2 has per-register
    // write gates - verify the BROM block actually latched.
    let rom_rb = (
        gpu.read32(crate::tx::eng::SEC2_FALCON2 + 0x210)?,
        gpu.read32(crate::tx::eng::SEC2_FALCON2 + 0x19c)?,
        gpu.read32(crate::tx::eng::SEC2_FALCON2 + 0x198)?,
        gpu.read32(crate::tx::eng::SEC2_FALCON2 + 0x180)?,
    );
    info!(
        "BOOTER brom readback: PARAADDR={:#x} ENGIDMASK={:#x} UCODE_ID={:#x} MOD_SEL={:#x} (want {sig_dmem:#x}/{:#x}/{:#x}/1)",
        rom_rb.0, rom_rb.1, rom_rb.2, rom_rb.3, hdr.engine_id, hdr.ucode_id & 0xff
    );
    gpu.write32(SEC2 + 0x104, hdr.app_code_off); // BOOTVEC = imemVa
    gpu.write32(SEC2 + 0x040, meta_dma as u32);
    gpu.write32(SEC2 + 0x044, (meta_dma >> 32) as u32);
    let cpuctl = gpu.read32(SEC2 + 0x100)?;
    // EXCI clear attempt: SEC2 latches EXCI=0x01000100 across engine resets
    // (GSP clears to 0). If that latch blocks start, clearing it unlocks.
    let exci_pre = gpu.pci.bar_read_u32(gpu.bar, (SEC2 + 0x0d0) as u64);
    gpu.write32(SEC2 + 0x0d0, 0);
    let exci_post = gpu.pci.bar_read_u32(gpu.bar, (SEC2 + 0x0d0) as u64);
    let eng = gpu.pci.bar_read_u32(gpu.bar, (SEC2 + 0x000) as u64);
    info!(
        "BOOTER exci probe: pre={exci_pre:#010x} post-clear={exci_post:#010x}; ENG={eng:#010x} (cpuctl={cpuctl:#010x})"
    );
    info!(
        "BOOTER fire: sec-imem {:#x}+{:#x}@dest0 dmem {:#x}+{:#x} sig@{sig_dmem:#x} meta={meta_dma:#x} cpuctl={cpuctl:#x}",
        hdr.app_code_off, hdr.app_code_size, hdr.data_off, hdr.data_size
    );
    if cpuctl & (1 << 6) != 0 {
        gpu.write32(SEC2 + 0x130, 1 << 1);
    } else {
        gpu.write32(SEC2 + 0x100, 1 << 1);
    }
    // START-drop probe: on the GSP the same write starts the core (FWSEC
    // ran); on SEC2 the core has never shown STARTED. Read back IMMEDIATELY
    // (before any halt wait): if CPUCTL is byte-identical to the pre-start
    // value, the write was silently dropped by a PLM-class gate - the
    // single fact that redirects the whole SEC2 investigation.
    let imm0 = gpu.pci.bar_read_u32(gpu.bar, (SEC2 + 0x100) as u64);
    uefi::boot::stall(core::time::Duration::from_millis(5));
    let imm1 = gpu.pci.bar_read_u32(gpu.bar, (SEC2 + 0x100) as u64);
    info!("BOOTER start-probe: pre={cpuctl:#010x} imm={imm0:#010x} +5ms={imm1:#010x}");
    if imm0 == cpuctl && cpuctl & (1 << 6) == 0 {
        // Write appears dropped: retry once through the alias path.
        gpu.write32(SEC2 + 0x130, 1 << 1);
        let imm2 = gpu.pci.bar_read_u32(gpu.bar, (SEC2 + 0x100) as u64);
        info!("BOOTER start-probe: alias retry -> {imm2:#010x}");
    }
    let r = gpu.poll_us(SEC2 + 0x100, 1 << 4, 1 << 4, 5_000_000, true);
    let cpuctl_now = gpu.pci.bar_read_u32(gpu.bar, (SEC2 + 0x100) as u64);
    let mbox0 = gpu.pci.bar_read_u32(gpu.bar, (SEC2 + 0x40) as u64);
    let exci = gpu.pci.bar_read_u32(gpu.bar, (SEC2 + 0x0d0) as u64);
    info!("BOOTER forensic: CPUCTL={cpuctl_now:#010x} MAILBOX0={mbox0:#010x} EXCI={exci:#010x}");
    r?;
    Ok(())
}

/// SEC2 booter fire, Turing BOOT_DIRECT variant (kernel_gsp_falcon_tu102.c
/// s_prepareHsFalconDirect + kgspExecuteHsFalcon_TU102 tail). No HS
/// bootstrap, no DMA: the image is portal-copied into SEC2 IMEM/DMEM
/// (non-secure part at 0, secure part at ALIGN_UP(ns_size,256), DMEM at 0),
/// BOOTVEC=0, mailboxes carry the WprMeta address; success = HALT with
/// mailbox0 == 0. The caller resets SEC2 first.
pub fn booter_fire_direct(gpu: &TxGpu, hdr: &BooterHdr, image: &[u8], meta_dma: u64) -> Result<(), TxError> {
    const FALCON_CPUCTL: u32 = 0x100;
    const FALCON_BOOTVEC: u32 = 0x104;
    const FALCON_MAILBOX0: u32 = 0x040;
    const FALCON_MAILBOX1: u32 = 0x044;
    const CPUCTL_START: u32 = 0x1;
    let base = crate::tx::eng::SEC2;

    let ns = &image[hdr.imem_ns_off as usize..(hdr.imem_ns_off + hdr.imem_ns_size) as usize];
    crate::tx_portal::portal_load_imem(gpu, base, 0, hdr.imem_ns_off, ns, false);
    let sec_dest = hdr.imem_ns_size.div_ceil(256) * 256;
    let sec = &image[hdr.app_code_off as usize..(hdr.app_code_off + hdr.app_code_size) as usize];
    crate::tx_portal::portal_load_imem(gpu, base, sec_dest, hdr.app_code_off, sec, true);
    let dmem = &image[hdr.data_off as usize..(hdr.data_off + hdr.data_size) as usize];
    crate::tx_portal::portal_load_dmem(gpu, base, 0, dmem);

    // BOOTVEC = start of non-secure code; mailboxes = WprMeta phys addr.
    gpu.write32(base + FALCON_BOOTVEC, 0);
    gpu.write32(base + FALCON_MAILBOX0, meta_dma as u32);
    gpu.write32(base + FALCON_MAILBOX1, (meta_dma >> 32) as u32);
    gpu.write32(base + FALCON_CPUCTL, CPUCTL_START);
    // Wait for HALT (CPUCTL bit4); read-back of mailbox0 is the verdict.
    gpu.poll_us(base + FALCON_CPUCTL, 0x10, 0x10, 5_000_000, false)
        .or(Err(TxError::ETimeout { tolerated: false }))?;
    let mbox0 = gpu.read32(base + FALCON_MAILBOX0)?;
    info!("BOOTER-DIRECT: halted, mailbox0={mbox0:#x} (0 = success)");
    if mbox0 != 0 {
        return Err(TxError::EIO);
    }
    Ok(())
}

/// Execution verdict. Payload mode (V67): the Ada four-PLM read-back plus
/// the mailbox marker. Chain-replay mode (stock booter): the booter's own
/// status (mailbox0 == 0, kernel s_executeBooterUcode semantics) plus WPR2
/// still up - the stock booter never touches the PLM selectors, so
/// demanding unlock values here would fail a correct run.
pub fn verify_execution(gpu: &TxGpu, payload_mode: bool) -> Result<(), TxError> {
    let plm = gpu.pci.bar_read_u32(gpu.bar, crate::tx::regs::FEAT_PLM as u64);
    let ss0 = gpu.pci.bar_read_u32(gpu.bar, crate::tx::regs::FEAT_SS0 as u64);
    let ss1 = gpu.pci.bar_read_u32(gpu.bar, crate::tx::regs::FEAT_SS1 as u64);
    info!("POST-BOOT: FEAT_PLM={plm:#x} SS0={ss0:#x} SS1={ss1:#x}");
    let mbox0 = gpu.pci.bar_read_u32(gpu.bar, (crate::tx::eng::SEC2 + 0x40) as u64);
    let wpr_hi = gpu.pci.bar_read_u32(gpu.bar, 0x001f_a828);
    info!("POST-BOOT: SEC2 MAILBOX0={mbox0:#010x} WPR2_HI={wpr_hi:#x}");
    if !payload_mode {
        if mbox0 != 0 {
            info!("POST-BOOT: booter status {mbox0:#x} (nonzero = failed)");
            return Err(TxError::EIO);
        }
        if wpr_hi >> 4 == 0 {
            info!("POST-BOOT: WPR2 down after stock boot");
            return Err(TxError::EIO);
        }
        return Ok(());
    }
    if plm != 0xffff_ffff {
        return Err(TxError::EIO);
    }
    let wprcfg = gpu.pci.bar_read_u32(gpu.bar, crate::tx::regs::WPRCFG_PLM as u64);
    let fbpa = gpu.pci.bar_read_u32(gpu.bar, crate::tx::regs::FBPA_PLM as u64);
    let wpr = gpu.pci.bar_read_u32(gpu.bar, crate::tx::regs::WPR_PLM as u64);
    info!("POST-BOOT: WPRCFG={wprcfg:#x} FBPA={fbpa:#x} WPR={wpr:#x}");
    if wprcfg != 0xffff_f0ff || wpr != 0xffff_ffff {
        return Err(TxError::EIO);
    }
    // FBPA is observational: on every AD10x card tested (2705/2805/
    // 28e1 community data + our 28e0 cold value) the write sticks at
    // 0xffffff0f - a flip is welcome, being stuck is NOT a failure.
    if fbpa != 0xffff_ff0f && fbpa != 0xffff_ffff {
        return Err(TxError::EIO);
    }
    if mbox0 != 0x67b0_0001 {
        info!("POST-BOOT: mailbox0 marker={mbox0:#x}");
        return Err(TxError::EIO);
    }
    Ok(())
}

/// FLR phase (unlock_610.c unlock_transaction_reset, EFI adaptation):
/// pre-FLR selector verify -> BME clear -> config-space FLR trigger ->
/// post-FLR GFW/WPR2/selector survival. PCI config via PciIo.
pub fn flr_phase(gpu: &TxGpu, payload_mode: bool) -> Result<(), TxError> {
    let ss0 = gpu.read32(crate::tx::regs::FEAT_SS0)?;
    let ss1 = gpu.read32(crate::tx::regs::FEAT_SS1)?;
    info!("pre-FLR selectors: ss0={ss0:#x} ss1={ss1:#x}");
    if payload_mode && (ss0 != SS0_UNLOCK || ss1 != SS1_UNLOCK) {
        return Err(TxError::EIO);
    }
    // pci_save_state analog: FLR resets the BARs to 0 - without restoring
    // them the post-FLR BAR polls read 0xffffffff (first flr_phase entry
    // ever failed exactly there, 2026-10-26).
    let mut saved = [0u32; 10]; // 0x04 command, 0x08..0x0c, 0x10..0x24 BARs
    for (i, slot) in saved.iter_mut().enumerate() {
        *slot = gpu.pci.config_read_u32(0x04 + i as u32 * 4);
    }
    // BME clear (PCI command bit 2), then FLR (Device Control bit 15) -
    // TOTORONG craft: preserve the rest of Device Control (our old write
    // of 0x8000 clobbered MPS/MRR), wait Transactions Pending first, and
    // keep 100 ms of total silence after the FLR write (PCI spec).
    gpu.pci.cfg_write_u16(4, 0x0007 & !0x0004); // MMIO+port IO on, BME off
    let cmd = gpu.pci.config_read_u16(0x04);
    if cmd & 0x0004 != 0 {
        info!("FLR: BME stuck");
        return Err(TxError::EIO);
    }
    let devctl = gpu.pci.config_read_u16(0x78 + 0x08);
    for _ in 0..1000 {
        let devsta = gpu.pci.config_read_u16(0x78 + 0x0a);
        if devsta == 0xffff || devsta & 0x20 == 0 {
            break;
        }
        uefi::boot::stall(core::time::Duration::from_millis(1));
    }
    // PCIe cap at 0x78: Device Control at +0x08; FLR bit 15.
    gpu.pci.cfg_write_u16(0x78 + 0x08, devctl | 0x8000);
    info!("PCIe FLR triggered");
    uefi::boot::stall(core::time::Duration::from_millis(100)); // no access of any kind to the function
    gpu.pci.cfg_write_u16(0x78 + 0x08, devctl);
    // Wait for config to answer before any BAR access (TOTORONG GuFlr tail).
    for _ in 0..1000 {
        let id = gpu.pci.config_read_u32(0x00);
        if id != 0xffff_ffff && id != 0 && id & 0xffff == 0x10de {
            break;
        }
        uefi::boot::stall(core::time::Duration::from_millis(1));
    }
    // pci_restore_state analog: decode off, high BAR dwords first, command
    // (never BME) last - then the post-FLR BAR polls work.
    gpu.pci.cfg_write_u16(4, 0x0000);
    for i in (2..10).rev() {
        gpu.pci.cfg_write_u32(0x04 + i as u32 * 4, saved[i]);
    }
    gpu.pci.cfg_write_u16(4, (saved[0] as u16) & !0x0004);
    let cmd_rb = gpu.pci.config_read_u16(0x04);
    info!("FLR: pci state restored (cmd={cmd_rb:#06x})");
    // Post-FLR: the chip reboots itself; GFW_BOOT cycles through states and
    // the cores settle into HALT afterwards (observed: CPUCTL 0x0 while
    // rebooting - the immediate HALT check fired too early). Poll HALT
    // generously; a timeout is logged but NOT fatal - the FLR itself is the
    // hardware guarantee that our states are wiped.
    let _ = gpu.poll_us(crate::tx::regs::GFW_BOOT, 0xff, 0xff, 5_000_000, false);
    match gpu.poll_us(crate::tx::regs::GFW_CPUCTL, 0x10, 0x10, 10_000_000, false) {
        Ok(()) => info!("POST-FLR: GFW halted the cores"),
        Err(_) => {
            let cpuctl = gpu.read32(crate::tx::regs::GFW_CPUCTL).unwrap_or(0);
            info!("POST-FLR: GSP HALT not observed within 10s (cpuctl={cpuctl:#x}) - continuing (FLR is the state guarantee)");
        }
    }
    let lo = gpu.pci.bar_read_u32(gpu.bar, 0x001f_a824 as u64);
    let hi = gpu.pci.bar_read_u32(gpu.bar, 0x001f_a828 as u64);
    info!("POST-FLR: WPR2={lo:#x}/{hi:#x}");
    if hi >> 4 != 0 {
        return Err(TxError::EIO);
    }
    let ss0 = gpu.read32(crate::tx::regs::FEAT_SS0)?;
    let ss1 = gpu.read32(crate::tx::regs::FEAT_SS1)?;
    info!("POST-FLR selectors: ss0={ss0:#x} ss1={ss1:#x}");
    if payload_mode && (ss0 != SS0_UNLOCK || ss1 != SS1_UNLOCK) {
        return Err(TxError::EIO); // AON survival violated
    }
    Ok(())
}
