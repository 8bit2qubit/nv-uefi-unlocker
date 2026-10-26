//! Falcon DMA transfer validation + falcon_hs preconditions — behavioral
//! port of unlock_610.c falcon_dma_valid / falcon_hs validation blocks.
//! All checks run BEFORE any register write or transfer submission: a bad
//! geometry must never leave a half-submitted IMEM DMA behind.

/// DMATRFMOFFS is 24 bits, FBOFFS is 32 bits, BASE is 49 bits.
/// Reject truncation/wrap before submitting any part of a transfer.
pub fn falcon_dma_valid(dest: u32, mem_off: u32, src: u64, size: u32) -> bool {
    size != 0
        && (size as u64 | dest as u64 | mem_off as u64 | src) & 255 == 0
        && dest as u64 + size as u64 <= 1 << 24
        && mem_off as u64 + size as u64 <= 1 << 32
        && src < 1 << 49
        && mem_off as u64 + size as u64 <= (1u64 << 49) - src
}

#[derive(Debug, PartialEq)]
pub enum HsError {
    /// Image offset arithmetic out of range.
    Range,
    /// DMA geometry invalid.
    Inval,
}

/// falcon_hs prevalidation. Returns the code source DMA address.
pub fn hs_prevalidate(
    image: u64,
    code_off: u32,
    code_size: u32,
    code_va: u32,
    code_pa: u32,
    data_off: u32,
    data_size: u32,
    data_pa: u32,
) -> Result<u64, HsError> {
    let (code_off, code_va, data_off) = (code_off as u64, code_va as u64, data_off as u64);
    if image > u64::MAX - code_off
        || image + code_off < code_va
        || image > u64::MAX - data_off
    {
        return Err(HsError::Range);
    }
    let code_src = image + code_off - code_va;
    if !falcon_dma_valid(code_pa, code_va as u32, code_src, code_size)
        || (data_size != 0 && !falcon_dma_valid(data_pa, 0, image + data_off, data_size))
    {
        return Err(HsError::Inval);
    }
    Ok(code_src)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dma_ok_baseline() {
        assert!(falcon_dma_valid(0, 0, 0x100000, 512));
    }

    /// C main() bad=0..4: each must be rejected with -EINVAL BEFORE writes.
    #[test]
    fn dma_matrix_rejects_truncation_and_wrap() {
        // bad=0: dest beyond 24-bit DMATRFMOFFS
        assert!(!falcon_dma_valid(0xffff00, 0, 0x100000, 512));
        // bad=1: mem_off wraps 32 bits
        assert!(!falcon_dma_valid(0, 0xffffff00, 0x100000, 512));
        // bad=2: src+size overflows the 49-bit BASE
        assert!(!falcon_dma_valid(0, 0, (1u64 << 49) - 256, 512));
        // bad=3: size not a 256 multiple
        assert!(!falcon_dma_valid(0, 0, 0x100000, 257));
        // bad=4: src unaligned
        assert!(!falcon_dma_valid(0, 0, 0x100001, 512));
    }

    /// C main() falcon_hs mode loop tail: ERANGE before any transfer.
    #[test]
    fn hs_range_rejects_va_above_image() {
        // (code_off=0, size=256, code_va=0x200000, ...) image+code_off < code_va
        assert_eq!(
            hs_prevalidate(0x100000, 0, 256, 0x200000, 0, 256, 256, 0),
            Err(HsError::Range)
        );
    }

    /// Bad DMEM geometry must not leave an already-submitted IMEM DMA:
    /// pure prevalidation => no state exists by construction.
    #[test]
    fn hs_inval_on_bad_dmem_geometry() {
        assert_eq!(
            hs_prevalidate(0x100000, 0, 256, 0, 0, 256, 257, 0),
            Err(HsError::Inval)
        );
    }

    #[test]
    fn hs_ok_computes_code_src() {
        let src = hs_prevalidate(0x100000, 0x1000, 256, 0x100, 0x300, 0x2000, 256, 0x400)
            .expect("valid hs");
        assert_eq!(src, 0x100000 + 0x1000 - 0x100);
    }
}
