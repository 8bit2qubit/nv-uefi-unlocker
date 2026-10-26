//! V67 signature-image payload generator (data-only; the words are ROP
//! gadget operands, semantics per gpu-unlock docs + cmp170hx rop-chain).
//!
//! Reference target: GA102 / 610.43.03 rejoin15 (pinned SHA-256 in
//! manifest). TU106 payloads, if ever built, come from a Turing-pinned
//! asset audit — never by editing these constants (task brief §9.1).

use crate::manifest::ga102_610::V67_PAYLOAD_SHA256;
use crate::manifest::sha256_hex;

pub const PAYLOAD_SIZE: usize = 0xFA00;
pub const FILL_DWORD: u32 = 0x0000_019C; // self-loop instruction (observable stop)

/// (offset, value) — offset in BYTES within the payload image.
pub const PATCHED_WORDS_GA102: &[(usize, u32)] = &[
    (0x1100, 0x00000007),
    (0xF948, 0xFFFFFFFF), // write value
    (0xF950, 0x00000D44),
    (0xF960, 0x00823804), // FEAT_OVR_PLM address
    (0xF968, 0x00001FCE),
    (0xF974, 0x00000000),
    (0xF97C, 0x00001101),
    (0xF980, 0x000084C8),
    (0xF984, 0x00008E18),
    (0xF98C, 0x000084C8),
    (0xF990, 0x00000000),
    (0xF998, 0x00001FCE),
    (0xF9A4, 0x0000FFBC),
    (0xF9AC, 0x00005789),
    (0xF9BC, 0x00000D44),
    (0xF9CC, 0x00000003),
    (0xF9D4, 0x00001FCE),
    (0xF9E8, 0x00000D52),
    (0xF9EC, 0x000081EE), // terminator
];

pub fn build_payload(fill: u32, size: usize, patched: &[(usize, u32)]) -> Vec<u8> {
    assert_eq!(size % 4, 0, "payload size must be dword-aligned");
    let mut out = vec![0u8; size];
    for (i, chunk) in out.chunks_exact_mut(4).enumerate() {
        chunk.copy_from_slice(&fill.to_le_bytes()[..4]);
        let _ = i;
    }
    for &(off, value) in patched {
        assert!(off + 4 <= size, "patch word outside payload");
        out[off..off + 4].copy_from_slice(&value.to_le_bytes());
    }
    out
}

/// Build the pinned GA102 rejoin15 payload and verify its SHA-256.
pub fn build_pinned_ga102() -> Result<Vec<u8>, String> {
    let payload = build_payload(FILL_DWORD, PAYLOAD_SIZE, PATCHED_WORDS_GA102);
    let digest = sha256_hex(&payload);
    if digest != V67_PAYLOAD_SHA256 {
        return Err(format!("generated V67 image differs from pinned SHA-256 ({digest})"));
    }
    Ok(payload)
}

/// AD102 four-write chain (gpu-unlock-ada generate_v67.py, TOTORONG-derived):
/// ONE fire performs four chained PLM writes + the mailbox marker.
/// Gadgets: 0x21b2, 0x33fc, 0x256f, 0x7a02 (+ tail 0x31b5/0x771e).
/// Note WPR_CFG opens to 0xFFFFF0FF — bits [11:0] region deliberately not
/// all-ones (cmp170hx caution); FBPA/WPR/FEAT go to 0xFFFFFFFF.
pub const PATCHED_WORDS_AD102: &[(usize, u32)] = &[
    (0x1100, 0x00000007),
    (0xF948, 0xFFFFF0FF), // write value #1 (WPR_CFG)
    (0xF950, 0x000021B2), // gadget
    (0xF960, 0x001FA7CC), // addr #1 WPR_CFG
    (0xF968, 0x000033FC), // gadget
    (0xF974, 0xFFFFFFFF), // value #2 (FBPA)
    (0xF97C, 0x0000256F), // gadget
    (0xF980, 0x00007A02), // gadget
    (0xF984, 0x009A0148), // addr #2 FBPA
    (0xF98C, 0x000033FC),
    (0xF998, 0xFFFFFFFF), // value #3 (WPR)
    (0xF9A0, 0x0000256F),
    (0xF9A4, 0x00007A02),
    (0xF9A8, 0x001FA7C4), // addr #3 WPR
    (0xF9B0, 0x000033FC),
    (0xF9BC, 0xFFFFFFFF), // value #4 (FEAT)
    (0xF9C4, 0x0000256F),
    (0xF9C8, 0x00007A02),
    (0xF9CC, 0x00823804), // addr #4 FEAT
    (0xF9D4, 0x000033FC),
    (0xF9E0, 0x67B00001), // mailbox marker
    (0xF9E8, 0x0000256F),
    (0xF9EC, 0x00007A02),
    (0xF9F8, 0x000031B5),
    (0xF9FC, 0x0000771E),
];

use crate::manifest::ad102_610::V67_AD102_PAYLOAD_SHA256;

pub fn build_pinned_ad102() -> Result<Vec<u8>, String> {
    let payload = build_payload(FILL_DWORD, PAYLOAD_SIZE, PATCHED_WORDS_AD102);
    let digest = sha256_hex(&payload);
    if digest != V67_AD102_PAYLOAD_SHA256 {
        return Err(format!("AD102 payload differs from pinned SHA-256 ({digest})"));
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ga102_reference_payload_reproduces_pinned_sha256() {
        // GPL-2.0-only behavioral port check: identical bytes to the
        // reference generator output.
        let payload = build_pinned_ga102().expect("pinned payload must reproduce");
        assert_eq!(payload.len(), 0xFA00);
        assert_eq!(&payload[0..4], &0x19C_u32.to_le_bytes());
        assert_eq!(&payload[0xF9EC..0xF9F0], &0x81EE_u32.to_le_bytes());
    }

    #[test]
    fn small_generator_semantics() {
        let p = build_payload(0xAA, 16, &[(4, 0x11223344)]);
        assert_eq!(&p[0..4], &0xAA_u32.to_le_bytes());
        assert_eq!(&p[4..8], &0x11223344_u32.to_le_bytes());
        assert_eq!(&p[12..16], &0xAA_u32.to_le_bytes());
    }

    #[test]
    #[should_panic]
    fn out_of_range_patch_rejected() {
        build_payload(0, 8, &[(6, 1)]);
    }
}
