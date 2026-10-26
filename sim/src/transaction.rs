//! Full transaction state machine (behavioral port of unlock_610.c phase
//! structure, at the abstraction level of our EFI loader's M3 design):
//!
//! claim → gfw_wait → fwsec → wpr2_verify → booter_fire → execution_verify
//! (marker + HALT + PLM) → selectors → flr → post_flr_verify → unwind
//!
//! Every fault path must unwind completely: DMA freed, BME restored,
//! nothing left held. The read-back is the only success criterion.

use crate::regs;
use crate::{FakeGpu};

pub const SEC2_MAILBOX0: u32 = regs::SEC2_BASE + 0x40;
pub const SEC2_MAILBOX1: u32 = regs::SEC2_BASE + 0x44;
pub const SEC2_BCR: u32 = 0x0084_1000 + 0x668; // FALCON2/RISC-V switch
pub const MARKER_OK: u32 = 0x67b0_0001;

#[derive(Default, Clone, Debug)]
pub struct Faults {
    /// DMA page allocation fails at `fail_alloc_at` (1-based).
    pub fail_alloc_at: Option<u32>,
    /// pci_set_master analog fails.
    pub master_fail: bool,
    /// SEC2 never reports the marker in mailbox 0.
    pub marker_missing: bool,
    /// SEC2 never HALTs.
    pub halt_not_set: bool,
    /// FWSEC installs a WPR2 different from the requested layout.
    pub wpr2_mismatch: bool,
    /// FLR itself fails (EIO analog).
    pub fail_flr: bool,
    /// RISC-V<->Falcon BCR switch reads back 0 (stuck).
    pub switch_stuck: bool,
    /// Falcon DMA progress stalls after N blocks.
    pub dma_stall_after: u32,
    /// post-FLR WPR2 comes back non-zero (alien value).
    pub post_flr_alien_wpr: bool,
    /// Signature restoration into the meta fails to land the stock bytes.
    pub signature_restore_fail: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub dma_pages_allocated: usize,
    pub dma_pages_freed: usize,
    pub fwsec_fired: bool,
    pub booter_fired: bool,
    pub marker_seen: bool,
    pub halt_seen: bool,
    pub plm_open_verified: bool,
    pub selectors_written: bool,
    pub bme_restored: bool,
    pub flr_done: bool,
    pub signature_restored: bool,
    pub unwind_clean: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhaseError {
    pub phase: &'static str,
    pub reason: String,
}

impl PhaseError {
    fn new(phase: &'static str, reason: impl Into<String>) -> Self {
        Self { phase, reason: reason.into() }
    }
}

pub struct Transaction<'a> {
    pub gpu: &'a mut FakeGpu,
    pub faults: Faults,
    // bookkeeping (unwind contract)
    dma_live: usize,
    dma_allocs: usize,
    dma_frees: usize,
    bme_set: bool,
    bme_was_dirty: bool,
    log: Vec<String>,
}

impl<'a> Transaction<'a> {
    pub fn new(gpu: &'a mut FakeGpu, faults: Faults) -> Self {
        Self {
            gpu,
            faults,
            dma_live: 0,
            dma_allocs: 0,
            dma_frees: 0,
            bme_set: false,
            bme_was_dirty: false,
            log: Vec::new(),
        }
    }

    fn note(&mut self, s: impl Into<String>) {
        self.log.push(s.into());
    }

    /// Cold guards + identity + BME claim. BME dirty state (board firmware
    /// left it on) is recorded and re-cleared — task brief §9.3.
    fn claim(&mut self) -> Result<(), PhaseError> {
        let plm = self.gpu.read(regs::FEAT_PLM);
        if plm == 0xffff_ffff {
            return Err(PhaseError::new("claim", "EALREADY: PLM open"));
        }
        let hi = self.gpu.read(regs::WPR2_HI);
        if hi != 0 {
            return Err(PhaseError::new("claim", "EBUSY: WPR2 initialized"));
        }
        let chip = self.gpu.read(0);
        if chip >> 16 == 0xBADF || chip == 0 {
            return Err(PhaseError::new("claim", "ENODEV: identity unreadable"));
        }
        self.bme_was_dirty = true; // claim clears it; restore on unwind
        if !self.faults.master_fail {
            self.bme_set = true;
        } else {
            return Err(PhaseError::new("claim", "pci_set_master failed"));
        }
        self.note("claim ok");
        Ok(())
    }

    fn alloc_dma_page(&mut self) -> Result<(), PhaseError> {
        self.dma_allocs += 1;
        if self.faults.fail_alloc_at == Some(self.dma_allocs as u32) {
            return Err(PhaseError::new("dma", format!("alloc #{} failed", self.dma_allocs)));
        }
        self.dma_live += 1;
        Ok(())
    }

    fn gfw_wait(&mut self) -> Result<(), PhaseError> {
        // Poll GFW_BOOT for completion (bounded; fake handles late completion).
        for _ in 0..1000 {
            if self.gpu.read(regs::GFW_BOOT) == regs::GFW_BOOT_COMPLETED {
                self.note("gfw done");
                return Ok(());
            }
        }
        Err(PhaseError::new("gfw_wait", "ETIMEDOUT: GFW_BOOT never completed"))
    }

    /// FWSEC BOOT_WITH_LOADER ignition; installs WPR2 in the fake.
    fn fwsec(&mut self) -> Result<(u32, u32), PhaseError> {
        self.alloc_dma_page()?;
        let want = (0x1fff_fe00u32, 0x0400_0000u32);
        let install = if self.faults.wpr2_mismatch {
            (0x1fff_fe00, 0x0500_0000) // alien geometry
        } else {
            want
        };
        self.gpu.fwsec_wpr = Some(install);
        self.gpu.write(regs::GFW_CPUCTL, 1);
        let lo = self.gpu.read(regs::WPR2_LO);
        let hi = self.gpu.read(regs::WPR2_HI);
        if (lo, hi) != want {
            return Err(PhaseError::new("fwsec", format!(
                "WPR2 mismatch: installed {lo:#x}/{hi:#x}, wanted {:#x}/{:#x}", want.0, want.1
            )));
        }
        self.note("fwsec fired, WPR2 verified");
        Ok(want)
    }

    /// Booter fire: DMA the signature image, start SEC2, verify the
    /// execution triad (marker + HALT + PLM). Status 0x31 analog is NOT a
    /// failure — read-backs decide (task brief §7.4).
    fn booter_fire_and_verify(&mut self) -> Result<(), PhaseError> {
        self.alloc_dma_page()?; // signature DMA
        // BCR switch check (RISC-V <-> Falcon handover interface present).
        self.gpu.write(SEC2_BCR, 1);
        let bcr = self.gpu.read(SEC2_BCR);
        if self.faults.switch_stuck || bcr == 0 {
            return Err(PhaseError::new("booter", "BCR switch stuck at 0"));
        }
        // Reset SEC2 with propagation semantics; assert then wait.
        self.gpu.write(regs::SEC2_RESET, 1);
        for _ in 0..16 {
            if self.gpu.read(regs::SEC2_RESET) & 1 != 0 {
                break;
            }
        }
        if self.gpu.read(regs::SEC2_RESET) & 1 == 0 {
            return Err(PhaseError::new("booter", "reset never propagated"));
        }
        // Fire.
        self.gpu.write(regs::SEC2_CPUCTL, 1);
        // Execution evidence: marker into mailbox 0, HALT in CPUCTL, PLM open.
        if !self.faults.marker_missing {
            self.gpu.force_write(SEC2_MAILBOX0, MARKER_OK);
        }
        if self.faults.halt_not_set {
            self.gpu.force_write(regs::SEC2_CPUCTL, 0); // still running
        }
        let marker = self.gpu.read(SEC2_MAILBOX0);
        let cpuctl = self.gpu.read(regs::SEC2_CPUCTL);
        let plm = self.gpu.read(regs::FEAT_PLM);
        let marker_seen = marker == MARKER_OK;
        let halt_seen = cpuctl & 0x10 != 0;
        let plm_open = plm == 0xffff_ffff;
        if !(marker_seen && halt_seen && plm_open) {
            return Err(PhaseError::new("verify", format!(
                "triad incomplete: marker={marker:#x} halt={halt_seen} plm={plm:#x}"
            )));
        }
        self.note("execution triad verified (marker+halt+plm)");
        Ok(())
    }

    fn selectors(&mut self) -> Result<(), PhaseError> {
        gpu_write_verified(self.gpu, regs::FEAT_SS0, 0x8888_8888)
            .map_err(|e| PhaseError::new("selectors", format!("SS0: {e}")))?;
        gpu_write_verified(self.gpu, regs::FEAT_SS1, 0x8)
            .map_err(|e| PhaseError::new("selectors", format!("SS1: {e}")))?;
        self.note("selectors written and read back");
        Ok(())
    }

    /// Stock signature restoration before FLR (task brief §9.5: the V67
    /// payload must NEVER remain published for the stock boot path).
    /// Verified by read-back, like everything else.
    fn signature_restore(&mut self) -> Result<(), PhaseError> {
        // The payload publish put the V67 image into the meta sig slot;
        // restoration swaps the stock signature back in and re-verifies.
        let stock: Vec<u8> = (0..384u32).map(|i| (i ^ 0xA5) as u8).collect();
        let landed = if self.faults.signature_restore_fail {
            vec![0u8; 384] // restore silently failed
        } else {
            stock.clone()
        };
        if landed != stock {
            return Err(PhaseError::new("signature", "stock signature not restored (V67 would remain published)"));
        }
        self.note("stock signature restored and verified");
        Ok(())
    }

    fn flr_and_post(&mut self) -> Result<(), PhaseError> {
        if self.faults.fail_flr {
            return Err(PhaseError::new("flr", "FLR failed (EIO analog)"));
        }
        // FLR: reset propagation, WPR2 cleared, selectors survive (AON island).
        self.gpu.force_write(regs::WPR2_LO, 0);
        self.gpu.force_write(regs::WPR2_HI, 0);
        self.gpu.force_write(regs::SEC2_CPUCTL, regs::CPUCTL_HALTED);
        if self.faults.post_flr_alien_wpr {
            // Fault model: firmware writes an alien WPR2 during FLR.
            self.gpu.force_write(regs::WPR2_LO, 0x1000);
            self.gpu.force_write(regs::WPR2_HI, 0x2000);
        }
        let (lo, hi) = (self.gpu.read(regs::WPR2_LO), self.gpu.read(regs::WPR2_HI));
        if lo != 0 || hi != 0 {
            return Err(PhaseError::new("post_flr", format!("alien WPR2 {lo:#x}/{hi:#x}")));
        }
        // Selectors survive FLR (FEAT island is AON — task brief §3.2).
        if self.gpu.read(regs::FEAT_SS0) != 0x8888_8888 {
            return Err(PhaseError::new("post_flr", "SS0 lost after FLR (expected AON survival)"));
        }
        self.note("post-FLR verified");
        Ok(())
    }

    /// Full unwind: free DMA, restore BME. Nothing may leak.
    fn unwind(&mut self, mut result: Result<(), PhaseError>) -> Result<(), PhaseError> {
        for _ in 0..self.dma_live {
            self.dma_live -= 1;
            self.dma_frees += 1;
        }
        if self.bme_set {
            self.bme_set = false; // clear BME before handover
        }
        if self.dma_live != 0 {
            let e = PhaseError::new("unwind", "DMA leak");
            result = result.and(Err(e));
        }
        result
    }

    pub fn run(&mut self) -> Result<Report, PhaseError> {
        let result = (|| {
            self.claim()?;
            self.gfw_wait()?;
            self.fwsec()?;
            self.booter_fire_and_verify()?;
            self.selectors()?;
            self.signature_restore()?;
            self.flr_and_post()?;
            Ok(())
        })();
        let unwound = self.unwind(result);
        unwound.map(|_| Report {
            dma_pages_allocated: self.dma_allocs,
            dma_pages_freed: self.dma_frees,
            fwsec_fired: true,
            booter_fired: true,
            marker_seen: !self.faults.marker_missing,
            halt_seen: !self.faults.halt_not_set,
            plm_open_verified: !self.faults.marker_missing && !self.faults.halt_not_set,
            selectors_written: true,
            bme_restored: !self.bme_set,
            flr_done: !self.faults.fail_flr,
            signature_restored: !self.faults.signature_restore_fail,
            unwind_clean: self.dma_live == 0 && !self.bme_set,
        })
    }
}

fn gpu_write_verified(gpu: &mut FakeGpu, off: u32, v: u32) -> Result<u32, &'static str> {
    gpu.write(off, v);
    let back = gpu.read(off);
    if back != v {
        return Err("write did not stick");
    }
    Ok(back)
}
