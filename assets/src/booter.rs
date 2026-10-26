//! Booter bundle assembly from BINDATA labels, ported from
//! extract_boot_assets.py `build_bundle` + `extract_assets`.
//!
//! Bundle layout: MAGIC "BTL1"(0x314C5442) + version 1 + 14-field header +
//! image/signature lengths + IMAGE_PROD + SIG_PROD. Every structural check
//! from the reference is preserved verbatim; any mismatch hard-fails.

use crate::bindata::{array_bytes, BindataError};

pub const MAGIC: u32 = 0x314C_5442; // "BTL1"

#[derive(Debug)]
pub enum BooterError {
    Bindata(BindataError),
    UnexpectedRecordSize,
    UnsupportedSignatureTable,
    SignaturePatchOutsideDmem,
    InvalidTransferRange(usize),
    UnsupportedBootloaderDescriptor,
    DescriptorRangeOutsideImage(usize),
}

impl From<BindataError> for BooterError {
    fn from(e: BindataError) -> Self {
        Self::Bindata(e)
    }
}

impl std::fmt::Display for BooterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bindata(e) => write!(f, "bindata: {e}"),
            Self::UnexpectedRecordSize => write!(f, "unexpected BooterLoad record size"),
            Self::UnsupportedSignatureTable => write!(f, "unsupported production Booter signature/application table"),
            Self::SignaturePatchOutsideDmem => write!(f, "signature patch is outside DMEM image"),
            Self::InvalidTransferRange(i) => write!(f, "invalid Falcon transfer range #{i}"),
            Self::UnsupportedBootloaderDescriptor => write!(f, "unsupported bootloader descriptor"),
            Self::DescriptorRangeOutsideImage(i) => write!(f, "bootloader descriptor range #{i} outside image"),
        }
    }
}

pub fn build_bundle(source_text: &str) -> Result<Vec<u8>, BooterError> {
    let image = array_bytes(source_text, "IMAGE_PROD")?.data;
    let header = array_bytes(source_text, "HEADER_PROD")?.data;
    let signatures = array_bytes(source_text, "SIG_PROD")?.data;
    let patch_loc = array_bytes(source_text, "PATCH_LOC")?.data;
    let patch_meta = array_bytes(source_text, "PATCH_META")?.data;
    let num_sigs = array_bytes(source_text, "NUM_SIGS")?.data;

    if header.len() != 36 || patch_loc.len() != 4 || patch_meta.len() != 12 || num_sigs.len() != 4 {
        return Err(BooterError::UnexpectedRecordSize);
    }
    let fields: Vec<u32> = header.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let loc = u32::from_le_bytes(patch_loc.try_into().unwrap());
    let meta: Vec<u32> = patch_meta.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let (_fuse, engine, ucode) = (meta[0], meta[1], meta[2]);
    let count = u32::from_le_bytes(num_sigs.try_into().unwrap());

    if count == 0 || signatures.len() != count as usize * 384 || fields[4] != 1 || ucode == 0 {
        return Err(BooterError::UnsupportedSignatureTable);
    }
    if loc < fields[2]
        || loc as u64 + 384 > fields[2] as u64 + fields[3] as u64
        || fields[2] as u64 + fields[3] as u64 > image.len() as u64
    {
        return Err(BooterError::SignaturePatchOutsideDmem);
    }
    let ranges: [(u32, u32); 3] = [(fields[0], fields[1]), (fields[2], fields[3]), (fields[5], fields[6])];
    for (i, &(offset, size)) in ranges.iter().enumerate()
    {
        if offset as u64 + size as u64 > image.len() as u64 || offset % 256 != 0 || size % 256 != 0 {
            return Err(BooterError::InvalidTransferRange(i));
        }
    }

    let mut out = Vec::with_capacity(72 + image.len() + signatures.len());
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    let mut hdr14: [u32; 14] = [0; 14];
    hdr14[..9].copy_from_slice(&fields[..9]);
    hdr14[9] = loc;
    hdr14[10] = ucode;
    hdr14[11] = engine;
    hdr14[12] = count;
    hdr14[13] = signatures.len() as u32;
    for w in hdr14 {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(&(image.len() as u32).to_le_bytes());
    out.extend_from_slice(&(signatures.len() as u32).to_le_bytes());
    out.extend_from_slice(&image);
    out.extend_from_slice(&signatures);
    Ok(out)
}

/// GspRmBoot bootloader descriptor checks (extract_assets tail):
/// returns (image, desc) after validating the 84-byte descriptor.
pub fn build_gsp_boot(source_text: &str) -> Result<(Vec<u8>, Vec<u8>), BooterError> {
    let image = array_bytes(source_text, "UCODE_IMAGE_PROD")?.data;
    let desc = array_bytes(source_text, "UCODE_DESC_PROD")?.data;
    if desc.len() != 84 {
        return Err(BooterError::UnsupportedBootloaderDescriptor);
    }
    let fields: Vec<u32> = desc.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    if fields[0] != 5 || fields[14] != 1 || fields[19] != image.len() as u32 || fields[20] != 0 {
        return Err(BooterError::UnsupportedBootloaderDescriptor);
    }
    for (i, &index) in [1usize, 3, 8, 10, 12].iter().enumerate() {
        if fields[index + 1] == 0 || fields[index] as u64 + fields[index + 1] as u64 > image.len() as u64 {
            return Err(BooterError::DescriptorRangeOutsideImage(i));
        }
    }
    Ok((image, desc))
}

/// Turing (TU102) BooterLoad: KGSP_FLCN_UCODE_BOOT_DIRECT variant of
/// s_allocateUcodeFromBinArchive (kernel_gsp_booter.c). Same BINDATA labels,
/// but the signature table is NOT RSA-384 (TU102 ships one 16-byte patch
/// signature) and PATCH_SIG must be zero. The bundle layout is identical to
/// build_bundle's BTL1 (all 9 header fields + appended control words).
pub fn build_bundle_direct(source_text: &str) -> Result<Vec<u8>, BooterError> {
    let image = array_bytes(source_text, "IMAGE_PROD")?.data;
    let header = array_bytes(source_text, "HEADER_PROD")?.data;
    let signatures = array_bytes(source_text, "SIG_PROD")?.data;
    let patch_loc = array_bytes(source_text, "PATCH_LOC")?.data;
    let patch_meta = array_bytes(source_text, "PATCH_META")?.data;
    let patch_sig = array_bytes(source_text, "PATCH_SIG")?.data;
    let num_sigs = array_bytes(source_text, "NUM_SIGS")?.data;

    if header.len() != 36 || patch_loc.len() != 4 || patch_meta.len() != 12 || num_sigs.len() != 4 || patch_sig.len() != 4 {
        return Err(BooterError::UnexpectedRecordSize);
    }
    let fields: Vec<u32> = header.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let loc = u32::from_le_bytes(patch_loc.try_into().unwrap());
    let meta: Vec<u32> = patch_meta.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let (_fuse, engine, ucode) = (meta[0], meta[1], meta[2]);
    let count = u32::from_le_bytes(num_sigs.try_into().unwrap());
    if u32::from_le_bytes(patch_sig.try_into().unwrap()) != 0 {
        return Err(BooterError::UnsupportedSignatureTable);
    }
    // Kernel checks: numApps == 1, numSigs > 0, totalSize % numSigs == 0.
    if fields[4] != 1 || count == 0 || count as usize > signatures.len() || signatures.len() % count as usize != 0 {
        return Err(BooterError::UnsupportedSignatureTable);
    }
    let sig_size = signatures.len() / count as usize;
    if ucode == 0 || loc as u64 + sig_size as u64 > image.len() as u64 {
        return Err(BooterError::SignaturePatchOutsideDmem);
    }
    // IMEM parts (os fields[0:2], app fields[5:7]) and DMEM (fields[2:4])
    // must land inside the image on 256-byte Falcon block boundaries.
    let ranges: [(u32, u32); 3] = [(fields[0], fields[1]), (fields[2], fields[3]), (fields[5], fields[6])];
    for (i, &(offset, size)) in ranges.iter().enumerate() {
        if offset as u64 + size as u64 > image.len() as u64 || offset % 256 != 0 || size % 256 != 0 {
            return Err(BooterError::InvalidTransferRange(i));
        }
    }

    let mut out = Vec::with_capacity(72 + image.len() + signatures.len());
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    let mut hdr14: [u32; 14] = [0; 14];
    hdr14[..9].copy_from_slice(&fields[..9]);
    hdr14[9] = loc;
    hdr14[10] = ucode;
    hdr14[11] = engine;
    hdr14[12] = count;
    hdr14[13] = signatures.len() as u32;
    for w in hdr14 {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(&(image.len() as u32).to_le_bytes());
    out.extend_from_slice(&(signatures.len() as u32).to_le_bytes());
    out.extend_from_slice(&image);
    out.extend_from_slice(&signatures);
    Ok(out)
}

/// Turing GspRmBoot: RM_RISCV_UCODE_DESC **version 4** (19 dwords, 76 bytes,
/// "eb riscv boot") — bootloader/param ranges are the only populated fields
/// on TU102; monitor/data/code triple stays zero and the WprMeta carries
/// those zeros verbatim (kgspPopulateWprMeta_TU102).
pub fn build_gsp_boot_v4(source_text: &str) -> Result<(Vec<u8>, Vec<u8>), BooterError> {
    let image = array_bytes(source_text, "UCODE_IMAGE")?.data;
    let desc = array_bytes(source_text, "UCODE_DESC")?.data;
    if desc.len() != 76 {
        return Err(BooterError::UnsupportedBootloaderDescriptor);
    }
    let fields: Vec<u32> = desc.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    if fields[0] != 4 {
        return Err(BooterError::UnsupportedBootloaderDescriptor);
    }
    // d1/d2 bootloader, d3/d4 bootloaderParam — must sit inside the image.
    for (i, &index) in [1usize, 3].iter().enumerate() {
        if fields[index + 1] == 0 || fields[index] as u64 + fields[index + 1] as u64 > image.len() as u64 {
            return Err(BooterError::DescriptorRangeOutsideImage(i));
        }
    }
    Ok((image, desc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindata::ArrayBytes;

    fn label_src(label: &str, data: &[u8]) -> String {
        let hex: Vec<String> = data.iter().map(|b| format!("0x{b:02x}")).collect();
        format!(
            "/* COMPRESSION: NO */\n/* DATA SIZE (bytes): {} */\n\
             static const NvU8 BINDATA_LABEL_{label}_data[] = {{\n  {}\n}};\n",
            data.len(),
            hex.join(", "),
        )
    }

    #[test]
    fn bundle_assembles_and_roundtrips_fields() {
        let mut image = vec![0u8; 2048];
        // DMEM region at fields[2]..+512 holds the signature patch slot.
        for (i, b) in 0x100u32.to_le_bytes().iter().enumerate() {
            image[0x300 + i] = *b;
        }
        let fields: [u32; 9] = [0, 256, 0x300, 768, 1, 0x500, 256, 0, 0];
        let mut header = Vec::new();
        for f in fields {
            header.extend_from_slice(&f.to_le_bytes());
        }
        let sigs: Vec<u8> = (0..384).map(|i| i as u8).collect();
        let src = format!(
            "{}{}{}{}{}{}",
            label_src("IMAGE_PROD", &image),
            label_src("HEADER_PROD", &header),
            label_src("SIG_PROD", &sigs),
            label_src("PATCH_LOC", &0x400u32.to_le_bytes()),
            // PATCH_META = fuse(0), engine(1), ucode(2), little-endian u32s
            label_src("PATCH_META", &[0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]),
            label_src("NUM_SIGS", &1u32.to_le_bytes()),
        );
        let bundle = build_bundle(&src).expect("bundle builds");
        assert_eq!(&bundle[0..4], &MAGIC.to_le_bytes());
        assert_eq!(bundle.len(), 72 + 2048 + 384);
        // header word 9 (loc) lands at offset 8 + 9*4
        let loc = u32::from_le_bytes(bundle[8 + 36..8 + 40].try_into().unwrap());
        assert_eq!(loc, 0x400);
        let _ = ArrayBytes { data: vec![], compressed: false, declared_size: 0 }; // silencer
    }

    #[test]
    fn misaligned_transfer_range_rejected() {
        let image = vec![0u8; 2048];
        let fields: [u32; 9] = [3, 256, 0x300, 768, 1, 0x500, 256, 0, 0]; // offset 3 not 256-aligned
        let mut header = Vec::new();
        for f in fields {
            header.extend_from_slice(&f.to_le_bytes());
        }
        let src = format!(
            "{}{}{}{}{}{}",
            label_src("IMAGE_PROD", &image),
            label_src("HEADER_PROD", &header),
            label_src("SIG_PROD", &vec![0u8; 384]),
            label_src("PATCH_LOC", &0x400u32.to_le_bytes()),
            label_src("PATCH_META", &[0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]),
            label_src("NUM_SIGS", &1u32.to_le_bytes()),
        );
        assert!(matches!(build_bundle(&src), Err(BooterError::InvalidTransferRange(0))));
    }

    fn direct_src(fields: [u32; 9], sigs: &[u8], patch_loc: u32) -> String {
        let mut header = Vec::new();
        for f in fields {
            header.extend_from_slice(&f.to_le_bytes());
        }
        format!(
            "{}{}{}{}{}{}{}",
            label_src("IMAGE_PROD", &vec![0u8; 0xE700]),
            label_src("HEADER_PROD", &header),
            label_src("SIG_PROD", sigs),
            label_src("PATCH_SIG", &0u32.to_le_bytes()),
            label_src("PATCH_LOC", &patch_loc.to_le_bytes()),
            label_src("PATCH_META", &[0, 0, 0, 0, 1, 0, 0, 0, 0xd, 0, 0, 0]),
            label_src("NUM_SIGS", &1u32.to_le_bytes()),
        )
    }

    #[test]
    fn direct_bundle_assembles_tu102_shape() {
        // TU102 reality: image 59136=0xE700, os data @0x8500+0x6200, sec IMEM
        // @0x100+0x8400, patch @0x8700, one 16-byte signature.
        let fields: [u32; 9] = [0, 256, 0x8500, 0x6200, 1, 0x100, 0x8400, 0x100, 0];
        let src = direct_src(fields, &[0x5Au8; 16], 0x8700);
        let bundle = build_bundle_direct(&src).expect("direct bundle builds");
        assert_eq!(bundle.len(), 72 + 0xE700 + 16);
        let word = |i: usize| u32::from_le_bytes(bundle[8 + i * 4..12 + i * 4].try_into().unwrap());
        assert_eq!(word(10), 0xd); // ucode id
        assert_eq!(word(12), 1); // num sigs
        assert_eq!(word(13), 16); // total sig bytes
    }

    #[test]
    fn direct_bundle_rejects_nonzero_patch_sig() {
        let fields: [u32; 9] = [0, 256, 0x8500, 0x6200, 1, 0x100, 0x8400, 0x100, 0];
        let src = direct_src(fields, &[0u8; 16], 0x8700).replace(
            "BINDATA_LABEL_PATCH_SIG_data[] = {\n  0x00, 0x00, 0x00, 0x00",
            "BINDATA_LABEL_PATCH_SIG_data[] = {\n  0x01, 0x00, 0x00, 0x00",
        );
        assert!(matches!(build_bundle_direct(&src), Err(BooterError::UnsupportedSignatureTable)));
    }

    #[test]
    fn direct_bundle_rejects_patch_outside_image() {
        let fields: [u32; 9] = [0, 256, 0x8500, 0x6200, 1, 0x100, 0x8400, 0x100, 0];
        let src = direct_src(fields, &[0u8; 16], 0xFF00);
        assert!(matches!(build_bundle_direct(&src), Err(BooterError::SignaturePatchOutsideDmem)));
    }

    fn v4_desc_src(desc: [u32; 19], image_len: usize) -> String {
        let mut d = Vec::new();
        for w in desc {
            d.extend_from_slice(&w.to_le_bytes());
        }
        format!(
            "{}{}",
            label_src("UCODE_IMAGE", &vec![0u8; image_len]),
            label_src("UCODE_DESC", &d),
        )
    }

    #[test]
    fn v4_desc_accepts_tu102_shape() {
        // TU102: bootloader 0..0x488, param 0x488+0x10, all else zero.
        let mut desc = [0u32; 19];
        desc[0] = 4;
        desc[2] = 0x488;
        desc[3] = 0x488;
        desc[4] = 0x10;
        let (image, parsed) = build_gsp_boot_v4(&v4_desc_src(desc, 4096)).expect("v4 desc parses");
        assert_eq!(image.len(), 4096);
        assert_eq!(parsed.len(), 76);
    }

    #[test]
    fn v4_desc_rejects_v5_and_out_of_range() {
        let mut desc = [0u32; 19];
        desc[0] = 5;
        assert!(build_gsp_boot_v4(&v4_desc_src(desc, 4096)).is_err());
        desc[0] = 4;
        desc[2] = 0x1100; // bootloader size 4352 exceeds the 4096-byte image
        assert!(matches!(
            build_gsp_boot_v4(&v4_desc_src(desc, 4096)),
            Err(BooterError::DescriptorRangeOutsideImage(0))
        ));
    }
}
