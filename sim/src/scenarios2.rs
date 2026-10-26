//! Transaction scenarios (batch 2): full state machine + fault injection.
//! Each scenario asserts BOTH the phase error (when expected) AND the
//! unwind contract (no DMA leak, BME restored) — a failing transaction
//! that leaks resources is a bug, not a fault-path success.

use crate::transaction::{Faults, PhaseError, Transaction};
use crate::{regs, FakeGpu};

fn run(faults: Faults) -> Result<crate::transaction::Report, PhaseError> {
    let mut gpu = FakeGpu::new_locked_cold(None);
    Transaction::new(&mut gpu, faults).run()
}

#[test]
fn t01_full_happy_path() {
    let report = run(Faults::default()).expect("happy path");
    assert!(report.unwind_clean && report.selectors_written && report.flr_done);
    assert_eq!(report.dma_pages_allocated, 2);
    assert_eq!(report.dma_pages_freed, 2);
}

#[test]
fn t02_plm_already_open_refused() {
    let mut gpu = FakeGpu::new_locked_cold(None);
    gpu.force_write(regs::FEAT_PLM, 0xffff_ffff);
    let err = Transaction::new(&mut gpu, Faults::default()).run().unwrap_err();
    assert_eq!(err.phase, "claim");
    assert!(err.reason.contains("EALREADY"));
}

#[test]
fn t03_wpr2_pre_armed_refused() {
    let mut gpu = FakeGpu::new_locked_cold(None);
    gpu.force_write(regs::WPR2_HI, 0x0400_0000);
    let err = Transaction::new(&mut gpu, Faults::default()).run().unwrap_err();
    assert_eq!(err.phase, "claim");
    assert!(err.reason.contains("EBUSY"));
}

#[test]
fn t04_marker_missing_fails_triad_and_unwinds() {
    let err = run(Faults { marker_missing: true, ..Default::default() }).unwrap_err();
    assert_eq!(err.phase, "verify");
    assert!(err.reason.contains("triad"));
}

#[test]
fn t05_halt_not_set_fails_triad() {
    let err = run(Faults { halt_not_set: true, ..Default::default() }).unwrap_err();
    assert_eq!(err.phase, "verify");
}

#[test]
fn t06_selector_write_dropped_fails_with_readback() {
    let mut gpu = FakeGpu::new_locked_cold(None);
    gpu.selector_bad = 1;
    let err = Transaction::new(&mut gpu, Faults::default()).run().unwrap_err();
    assert_eq!(err.phase, "selectors");
    assert!(err.reason.contains("SS0"));
}

#[test]
fn t07_wpr2_geometry_mismatch_detected() {
    let err = run(Faults { wpr2_mismatch: true, ..Default::default() }).unwrap_err();
    assert_eq!(err.phase, "fwsec");
    assert!(err.reason.contains("mismatch"));
}

#[test]
fn t08_alloc_fail_first_page() {
    let err = run(Faults { fail_alloc_at: Some(1), ..Default::default() }).unwrap_err();
    assert_eq!(err.phase, "dma");
}

#[test]
fn t09_alloc_fail_second_page() {
    let err = run(Faults { fail_alloc_at: Some(2), ..Default::default() }).unwrap_err();
    assert_eq!(err.phase, "dma");
}

#[test]
fn t10_set_master_fails_at_claim() {
    let err = run(Faults { master_fail: true, ..Default::default() }).unwrap_err();
    assert_eq!(err.phase, "claim");
    assert!(err.reason.contains("set_master"));
}

#[test]
fn t11_flr_failure_still_unwinds() {
    let err = run(Faults { fail_flr: true, ..Default::default() }).unwrap_err();
    assert_eq!(err.phase, "flr");
}

#[test]
fn t12_bcr_switch_stuck() {
    let err = run(Faults { switch_stuck: true, ..Default::default() }).unwrap_err();
    assert_eq!(err.phase, "booter");
    assert!(err.reason.contains("BCR"));
}

#[test]
fn t13_gfw_never_completes() {
    let mut gpu = FakeGpu::new_locked_cold(None);
    gpu.force_write(regs::GFW_BOOT, 0);
    gpu.gfw_late_reads = 5_000; // beyond the transaction's poll budget
    let err = Transaction::new(&mut gpu, Faults::default()).run().unwrap_err();
    assert_eq!(err.phase, "gfw_wait");
    assert!(err.reason.contains("ETIMEDOUT"));
}

#[test]
fn t14_reset_never_propagates() {
    let mut gpu = FakeGpu::new_locked_cold(None);
    gpu.reset_delay = 100; // beyond the 16-read assert budget
    let err = Transaction::new(&mut gpu, Faults::default()).run().unwrap_err();
    assert_eq!(err.phase, "booter");
    assert!(err.reason.contains("reset"));
}

#[test]
fn t15_post_flr_alien_wpr() {
    let mut gpu = FakeGpu::new_locked_cold(None);
    let mut faults = Faults::default();
    faults.post_flr_alien_wpr = true;
    // Fake the alien value appearing at FLR.
    // (The fake clears WPR2 at force_write(0); emulate alien by making hi non-zero
    // through the fault branch in flr_and_post — modeled as fault -> error.)
    let err = Transaction::new(&mut gpu, faults).run().unwrap_err();
    assert_eq!(err.phase, "post_flr");
}

#[test]
fn t16_ss_drift_does_not_break_writeback() {
    // AD107 measurement: SS0/SS1 cold values drift between boots; the
    // transaction's explicit writes + read-backs are drift-proof.
    let mut gpu = FakeGpu::new_locked_cold(Some((0x1714_2071, 0x5)));
    assert_eq!(gpu.read(regs::FEAT_SS0), 0x1714_2071, "drifted cold value");
    let report = Transaction::new(&mut gpu, Faults::default()).run().expect("drift tolerated");
    assert!(report.selectors_written);
    assert_eq!(gpu.read(regs::FEAT_SS0), 0x8888_8888, "explicit target lands");
    assert_eq!(gpu.read(regs::FEAT_SS1), 0x8);
}

#[test]
fn t17_sec2_halt_mode_fails_triad() {
    let mut gpu = FakeGpu::new_locked_cold(None);
    gpu.boot_mode = crate::BootMode::Halt;
    let err = Transaction::new(&mut gpu, Faults::default()).run().unwrap_err();
    assert_eq!(err.phase, "verify");
}

#[test]
fn t18_unwind_contract_on_every_failure() {
    // A resource leak surfaces as phase "unwind"; every fault must instead
    // report its own phase with the unwind contract honored internally.
    for faults in [
        Faults { marker_missing: true, ..Default::default() },
        Faults { wpr2_mismatch: true, ..Default::default() },
        Faults { fail_alloc_at: Some(2), ..Default::default() },
        Faults { switch_stuck: true, ..Default::default() },
        Faults { fail_flr: true, ..Default::default() },
        Faults { halt_not_set: true, ..Default::default() },
    ] {
        let mut gpu = FakeGpu::new_locked_cold(None);
        let err = Transaction::new(&mut gpu, faults).run().expect_err("fault must fail");
        assert_ne!(err.phase, "unwind", "leak masked the real failure: {err:?}");
    }
}

/// gpu-unlock-ada AD102-family four-write chain on AD107M: single fire
/// opens WPR_CFG (0xfffff0ff — bits [11:8] deliberately clear), FBPA, WPR
/// and FEAT PLMs; read-backs decide, including the not-all-ones value.
#[test]
fn t19_ada_four_write_chain() {
    let mut gpu = FakeGpu::new_locked_cold(None);
    gpu.boot_mode = crate::BootMode::AdaFourWrite;
    let report = Transaction::new(&mut gpu, Faults::default()).run().expect("ada chain");
    assert!(report.unwind_clean);
    assert_eq!(gpu.read(regs::WPRCFG_PLM), 0xffff_f0ff, "WPR_CFG: not all-ones!");
    assert_eq!(gpu.read(regs::FBPA_PLM), 0xffff_ffff);
    assert_eq!(gpu.read(regs::WPR_PLM), 0xffff_ffff);
    assert_eq!(gpu.read(regs::FEAT_PLM), 0xffff_ffff);
    assert_eq!(gpu.read(regs::FEAT_SS0), 0x8888_8888);
    assert_eq!(gpu.read(regs::FEAT_SS1), 0x8);
}
