//! WPR2/FRTS reservation layout — behavioral port of layout_610.c
//! gpu_unlock_layout (exact arithmetic) + the register adapter from
//! unlock_transaction_layout (FB/VGA/MMU-lock interpretation, 256 MiB
//! pre-scrubbed floor, VGA fallback rules).

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutIn {
    pub usable_fb: u64,
    pub vga_base: u64,
    pub mmu_lock_valid: bool,
    pub mmu_lock_low: u64,
    pub frts_size: u64,
    pub boot_size: u64,
    pub image_size: u64,
    pub nonwpr_size: u64,
    pub heap_request: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutOut {
    pub reserved_top: u64,
    pub wpr_end: u64,
    pub frts_offset: u64,
    pub boot_offset: u64,
    pub gsp_offset: u64,
    pub heap_offset: u64,
    pub heap_size: u64,
    pub wpr_start: u64,
    pub nonwpr_start: u64,
}

#[derive(Debug, PartialEq)]
pub enum LayoutError {
    /// usable FB below the 256 MiB pre-scrubbed floor.
    FbTooSmall,
    /// VGA base at or above usable FB.
    VgaAboveFb,
    /// Reservation arithmetic underflowed or ordering violated.
    Range,
    /// Reservations dip below the pre-scrubbed top 256 MiB.
    BelowScrubTop,
    /// Register inputs unreadable.
    EnODEV,
}

fn align_down(v: u64, align: u64) -> Option<u64> {
    if align == 0 || align & (align - 1) != 0 || v < align {
        return None;
    }
    Some(v & !(align - 1))
}
fn align_up(v: u64, align: u64) -> Option<u64> {
    if align == 0 || align & (align - 1) != 0 || v > u64::MAX - (align - 1) {
        return None;
    }
    align_down(v + align - 1, align)
}
fn sub(v: u64, amount: u64) -> Option<u64> {
    v.checked_sub(amount)
}

pub fn gpu_unlock_layout(input: &LayoutIn, out: &mut LayoutOut) -> Result<(), LayoutError> {
    if input.usable_fb == 0 || input.vga_base == 0 || input.frts_size == 0
        || input.boot_size == 0 || input.image_size == 0
    {
        return Err(LayoutError::Range);
    }
    let reserved = if input.mmu_lock_valid && input.mmu_lock_low < input.vga_base {
        input.mmu_lock_low
    } else {
        input.vga_base
    };
    let wpr_end = align_down(reserved, 128 * 1024).ok_or(LayoutError::Range)?;
    let frts = sub(wpr_end, input.frts_size).ok_or(LayoutError::Range)?;
    let boot = sub(frts, input.boot_size).ok_or(LayoutError::Range)?;
    let boot = align_down(boot, 4096).ok_or(LayoutError::Range)?;
    let gsp = sub(boot, input.image_size).ok_or(LayoutError::Range)?;
    let gsp = align_down(gsp, 64 * 1024).ok_or(LayoutError::Range)?;
    let nonwpr = align_up(input.nonwpr_size, 1024 * 1024).ok_or(LayoutError::Range)?;
    let meta = align_up(256, 1024 * 1024).ok_or(LayoutError::Range)?;
    let heap = align_up(input.heap_request, 1024 * 1024).ok_or(LayoutError::Range)?;
    let heap_off = sub(gsp, heap).ok_or(LayoutError::Range)?;
    let heap_off = align_down(heap_off, 1024 * 1024).ok_or(LayoutError::Range)?;
    let wpr_start = sub(heap_off, meta).ok_or(LayoutError::Range)?;
    let nonwpr_start = sub(wpr_start, nonwpr).ok_or(LayoutError::Range)?;

    if nonwpr_start >= input.usable_fb
        || wpr_end > input.usable_fb
        || wpr_start >= wpr_end
        || gsp >= boot
        || boot >= frts
        || frts >= wpr_end
    {
        return Err(LayoutError::Range);
    }
    *out = LayoutOut {
        reserved_top: reserved,
        wpr_end,
        frts_offset: frts,
        boot_offset: boot,
        gsp_offset: gsp,
        heap_offset: heap_off,
        heap_size: (gsp - heap_off) & !((1u64 << 20) - 1),
        wpr_start,
        nonwpr_start,
    };
    Ok(())
}

/// Register adapter: FB size / VGA workspace / MMU lock registers ->
/// LayoutIn. Encodes the interpretation rules from
/// unlock_transaction_layout (top-lock rule, VGA fallbacks, floors).
pub fn layout_from_registers(
    fb_mib: u32,
    vga: u32,
    mmu_plm: u32,
    mmu_lo_reg: u32,
    mmu_hi_reg: u32,
    image_size: u64,
    boot_size: u64,
) -> Result<(LayoutIn, LayoutOut), LayoutError> {
    if fb_mib == 0 || fb_mib == u32::MAX || vga == u32::MAX || mmu_plm == u32::MAX
        || mmu_lo_reg == u32::MAX || mmu_hi_reg == u32::MAX || mmu_plm & 1 == 0
    {
        return Err(LayoutError::EnODEV);
    }
    let usable_fb = (fb_mib as u64) << 20;
    if usable_fb < 256 << 20 {
        return Err(LayoutError::FbTooSmall);
    }
    // kgspPopulateWprMeta_TU102: PRAMIN is the top 1 MiB fallback.
    let mut vga_base = usable_fb - (1 << 20);
    if vga & (1 << 3) != 0 {
        vga_base = ((vga >> 8) as u64) << 16;
        if vga_base < usable_fb - (1 << 20) {
            vga_base = usable_fb - 0x20000;
        }
    }
    if vga_base >= usable_fb {
        return Err(LayoutError::VgaAboveFb);
    }
    let mmu_lock_low = ((mmu_lo_reg >> 4) as u64) << 12;
    let mmu_hi = ((mmu_hi_reg >> 4) as u64) << 12;
    let mmu_top = if mmu_hi > u64::MAX - 0xffff {
        u64::MAX
    } else {
        (mmu_hi + 0xffff) & !0xffffu64
    };
    // The lock counts only when its exclusive high address IS the FB top.
    let mmu_lock_valid = mmu_hi >= mmu_lock_low && (mmu_top >> 20) == (usable_fb >> 20);

    let input = LayoutIn {
        usable_fb,
        vga_base,
        mmu_lock_valid,
        mmu_lock_low,
        frts_size: 1 << 20,
        boot_size,
        image_size,
        nonwpr_size: 1 << 20,
        heap_request: 88 << 20,
    };
    let mut out = LayoutOut::default();
    gpu_unlock_layout(&input, &mut out)?;
    // GA102 has no scrubber: reservations must fit the pre-scrubbed top.
    if out.nonwpr_start < usable_fb - (256 << 20) {
        return Err(LayoutError::BelowScrubTop);
    }
    Ok((input, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE: u64 = 0x5053_000; // pinned GA102 gsp .fwimage span
    const BOOT: u64 = 24576;

    fn case(fb_mib: u32, vga: u32, plm: u32, lo: u32, hi: u32) -> Result<(LayoutIn, LayoutOut), LayoutError> {
        layout_from_registers(fb_mib, vga, plm, lo, hi, IMAGE, BOOT)
    }

    fn invariants(r: &Result<(LayoutIn, LayoutOut), LayoutError>) {
        let (i, o) = r.as_ref().unwrap();
        assert!(o.nonwpr_start >= i.usable_fb - (256 << 20));
        assert!(o.wpr_end <= i.vga_base);
        assert!(o.heap_size >= 88 << 20);
    }

    /// The 8 register scenarios from C main(), inputs verbatim.
    #[test]
    fn sc0_default_pramin_fallback() {
        let r = case(24 * 1024, 0, 1, 0xfffffff0, 0);
        invariants(&r);
    }

    #[test]
    fn sc1_vga_workspace_at_fb_top_minus_1m() {
        let vga = (((((24u64 << 30) - (1 << 20)) >> 16) as u32) << 8) | 8;
        let r = case(24 * 1024, vga, 1, 0xfffffff0, 0);
        invariants(&r);
    }

    #[test]
    fn sc2_vga_fallback_below() {
        let r = case(24 * 1024, 8, 1, 0xfffffff0, 0);
        invariants(&r);
    }

    #[test]
    fn sc3_mmu_top_lock_used() {
        let low = (24u64 << 30) - (2 << 20);
        let r = case(
            24 * 1024,
            0,
            1,
            (((low >> 12) as u32) << 4),
            (((((24u64 << 30) - 4096) >> 12) as u32) << 4),
        );
        let (i, _) = r.as_ref().unwrap();
        assert!(i.mmu_lock_valid, "top lock (2 MiB window) must be honored");
        invariants(&r);
    }

    #[test]
    fn sc4_reservations_below_scrub_top() {
        let low = (24u64 << 30) - (200 << 20);
        let r = case(
            24 * 1024,
            0,
            1,
            (((low >> 12) as u32) << 4),
            (((((24u64 << 30) - 4096) >> 12) as u32) << 4),
        );
        assert_eq!(r.unwrap_err(), LayoutError::BelowScrubTop);
    }

    #[test]
    fn sc5_fb_below_256m() {
        let r = case(128, 0, 1, 0xfffffff0, 0);
        assert_eq!(r.unwrap_err(), LayoutError::FbTooSmall);
    }

    #[test]
    fn sc6_vga_above_fb() {
        let vga = ((((24u64 << 30) >> 16) as u32) << 8) | 8;
        let r = case(24 * 1024, vga, 1, 0xfffffff0, 0);
        assert_eq!(r.unwrap_err(), LayoutError::VgaAboveFb);
    }

    /// Captured cold GA102 lock values: lock present but NOT at FB top —
    /// must be ignored (mmu lock ignored), layout proceeds via VGA top.
    #[test]
    fn sc7_cold_ga102_lock_not_at_top_is_ignored() {
        let r = case(0x6000, 1, 0x45cb8f, 0x100000, 0x5cffff0);
        let (i, _) = r.as_ref().unwrap();
        assert!(!i.mmu_lock_valid, "lock high 0x5d000000 is not the 24G FB top");
        invariants(&r);
    }

    #[test]
    fn layout_arithmetic_matches_reference_shape() {
        // GA102 pinned geometry: image 0x5053000, boot 24576, 24G FB.
        let (_, o) = case(0x6000, 1, 0x45cb8f, 0x100000, 0x5cffff0).unwrap();
        assert_eq!(o.wpr_end, (24u64 << 30) - (1 << 20), "reserved = PRAMIN top 1M");
        assert_eq!(o.frts_offset, o.wpr_end - (1 << 20));
        assert!(o.boot_offset <= o.frts_offset - BOOT);
        assert!(o.gsp_offset + IMAGE <= o.boot_offset);
    }
}
