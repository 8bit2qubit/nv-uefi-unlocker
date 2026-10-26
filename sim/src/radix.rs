//! Three-level radix page table for the GSP image DMA window + WPR meta
//! field layout — behavioral port of unlock_transaction_radix and the
//! fake's independent walker in check_published_image (field offsets from
//! that check: magic@0, ver@8, level0@16, image_bytes@24, v67@72..80,
//! layout ranges at 112/136/144/152/168/176).

pub const RADIX_PAGE: usize = 4096;

#[derive(Debug, PartialEq)]
pub struct RadixPlan {
    /// pages[0]=L0, [1]=L1 tables, [2]=L2 tables, [3]=image leaves.
    pub pages: [usize; 4],
    pub table_pages: usize,
}

pub fn plan_for(image_size: u64) -> RadixPlan {
    let leaves = image_size.div_ceil(RADIX_PAGE as u64) as usize;
    let l2 = leaves.div_ceil(512);
    let l1 = l2.div_ceil(512).max(1);
    let l0 = 1;
    RadixPlan {
        pages: [l0, l1, l2, leaves],
        table_pages: l0 + l1 + l2,
    }
}

/// Built radix: page frames in C order (tables first, then image leaves).
pub struct Radix {
    pub plan: RadixPlan,
    /// dma address of page i (all frames are RADIX_PAGE sized).
    pub dma_base: u64,
    pub frames: Vec<[u8; RADIX_PAGE]>,
}

impl Radix {
    pub fn build(image: &[u8], dma_base: u64) -> Self {
        let plan = plan_for(image.len() as u64);
        let total = plan.table_pages + plan.pages[3];
        let mut frames: Vec<[u8; RADIX_PAGE]> = vec![[0; RADIX_PAGE]; total];
        // Leaves carry the image bytes, zero-padded to page size.
        for i in 0..plan.pages[3] {
            let off = i * RADIX_PAGE;
            let n = (image.len() - off).min(RADIX_PAGE);
            frames[plan.table_pages + i][..n].copy_from_slice(&image[off..off + n]);
        }
        // Tables: 8-byte child pointers (dma addresses), C fill order.
        let page_addr = |i: usize| dma_base + (i as u64) * RADIX_PAGE as u64;
        let mut first = 0usize;
        for level in 0..3 {
            let child = first + plan.pages[level];
            for i in 0..plan.pages[level] {
                let at = i * (RADIX_PAGE / 8);
                let n = (plan.pages[level + 1] - at).min(RADIX_PAGE / 8);
                for e in 0..n {
                    let addr = page_addr(child + at + e);
                    frames[first + i][e * 8..e * 8 + 8].copy_from_slice(&addr.to_le_bytes());
                }
            }
            first = child;
        }
        Self { plan, dma_base, frames }
    }

    fn page_of(&self, addr: u64) -> usize {
        assert!(addr >= self.dma_base && (addr - self.dma_base) % RADIX_PAGE as u64 == 0);
        ((addr - self.dma_base) / RADIX_PAGE as u64) as usize
    }

    /// Independent consumer walk (the firmware's view): L0 -> L1 -> L2 -> leaf.
    pub fn walk_leaf_addr(&self, page: u64) -> u64 {
        let get64 = |frame: usize, entry: usize| -> u64 {
            let b = &self.frames[frame][entry * 8..entry * 8 + 8];
            u64::from_le_bytes(b.try_into().unwrap())
        };
        let l0 = get64(0, (page >> 18) as usize);
        let l1 = get64(self.page_of(l0), ((page >> 9) & 511) as usize);
        let l2 = get64(self.page_of(l1), (page & 511) as usize);
        l2
    }

    pub fn leaf(&self, page: u64) -> &[u8; RADIX_PAGE] {
        &self.frames[self.page_of(self.walk_leaf_addr(page))]
    }
}

/// WPR meta field offsets (from the C verifier).
pub mod wpr_meta {
    pub const MAGIC_OFF: usize = 0;
    pub const VERSION_OFF: usize = 8;
    pub const LEVEL0_OFF: usize = 16;
    pub const IMAGE_BYTES_OFF: usize = 24;
    pub const V67_DMA_OFF: usize = 72;
    pub const V67_SIZE_OFF: usize = 80;
    pub const STOCK_SIG_DMA_OFF: usize = 72; // after restore, same slot
    pub const STOCK_SIG_SIZE_OFF: usize = 80;
    // Captured GA102 layout ranges the C check pins:
    pub const RANGE_HEAP_OFF: usize = 112;
    pub const RANGE_NONWPR_OFF: usize = 136;
    pub const RANGE_WPR_START_OFF: usize = 144;
    pub const RANGE_FRTS_OFF: usize = 152;
    pub const RANGE_WPR_END_OFF: usize = 168;
    pub const RANGE_FB_TOP_OFF: usize = 176;

    pub fn put_u64(meta: &mut [u8], off: usize, v: u64) {
        meta[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }
    pub fn get_u64(meta: &[u8], off: usize) -> u64 {
        u64::from_le_bytes(meta[off..off + 8].try_into().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors check_published_image: every published page byte-exact vs
    /// the source image, zero padding in the tail page, including a
    /// transition across the 512-entry L2 boundary (page 511 -> 512).
    #[test]
    fn radix_publishes_image_byte_exact_across_l2_boundary() {
        // 540 pages (~2.1 MiB): crosses exactly one L2 table boundary.
        let mut image = vec![0u8; 540 * RADIX_PAGE - 1234];
        for (i, b) in image.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let radix = Radix::build(&image, 0x1_0000_0000);
        assert_eq!(radix.plan.pages, [1, 1, 2, 540], "L2 count for 540 leaves");
        assert_eq!(radix.plan.table_pages, 4);

        let pages = image.len().div_ceil(RADIX_PAGE);
        for page in 0..pages {
            let leaf = radix.leaf(page as u64);
            let off = page * RADIX_PAGE;
            let n = (image.len() - off).min(RADIX_PAGE);
            assert_eq!(&leaf[..n], &image[off..off + n], "page {page} mismatch");
            for b in &leaf[n..] {
                assert_eq!(*b, 0, "tail padding of page {page} must be zero");
            }
        }
        // The boundary leaves must come from DIFFERENT L2 tables.
        let a = radix.walk_leaf_addr(511);
        let b = radix.walk_leaf_addr(512);
        let l2_of = |addr: u64| (radix.page_of(addr) - radix.plan.table_pages) / 512;
        assert_ne!(l2_of(a), l2_of(b), "pages 511/512 must straddle L2 tables");
    }

    #[test]
    fn wpr_meta_roundtrip() {
        let mut meta = vec![0u8; 256];
        wpr_meta::put_u64(&mut meta, wpr_meta::MAGIC_OFF, 0xCAFE_BABE);
        wpr_meta::put_u64(&mut meta, wpr_meta::IMAGE_BYTES_OFF, 84226048);
        assert_eq!(wpr_meta::get_u64(&meta, wpr_meta::MAGIC_OFF), 0xCAFE_BABE);
        assert_eq!(wpr_meta::get_u64(&meta, wpr_meta::IMAGE_BYTES_OFF), 84226048);
    }
}
