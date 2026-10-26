//! Scenario batch 1 — port of the core test_unlock_adapter.c behaviors.
//! Scenario naming mirrors the C file's intent; each maps to a knob.
//!
//! Lesson baked into these tests: a dropped write is only observable via
//! read-back when the baseline differs from the target value (fixture SS0
//! is the unlocked value 0x88888888, so scenarios force a locked baseline
//! before asserting write paths).

use crate::*;

#[test]
fn scenario_01_happy_path_unlock() {
    let mut gpu = FakeGpu::new_unlocked_fixture();
    // Locked cold state: PLM at factory value.
    gpu.force_write(regs::FEAT_PLM, 0xffff_ff8f);
    assert!(plm_cold_guard(&mut gpu).is_ok());
    assert!(wpr2_cold_guard(&mut gpu).is_ok());

    // FWSEC-FRTS fire establishes WPR2.
    gpu.fwsec_wpr = Some((0x1fff_fe00, 0x0400_0000));
    gpu.write(regs::GFW_CPUCTL, 1);
    assert_eq!(gpu.read(regs::WPR2_HI), 0x0400_0000, "WPR2 established");

    // SEC2 boot with the payload effect opens the PLM.
    gpu.write(regs::SEC2_CPUCTL, 1);
    assert_eq!(gpu.read(regs::FEAT_PLM), 0xffff_ffff, "PLM open after boot");

    // Host writes the speed selectors; read-back is the verdict.
    gpu.force_write(regs::FEAT_SS0, 0x0000_0001); // GA102 locked default
    gpu.force_write(regs::FEAT_SS1, 0x0000_0001);
    assert_eq!(write_verified(&mut gpu, regs::FEAT_SS0, 0x8888_8888).unwrap(), 0x8888_8888);
    assert_eq!(write_verified(&mut gpu, regs::FEAT_SS1, 0x8).unwrap(), 0x8);
}

#[test]
fn scenario_02_ss1_is_4bit() {
    let mut gpu = FakeGpu::new_unlocked_fixture();
    gpu.write(regs::FEAT_SS1, 0xffff_ffff);
    assert_eq!(gpu.read(regs::FEAT_SS1), 0xf, "SS1 masks to 4 bits");
    // Full-write roundtrip must therefore use 0x8, never 0xffffffff.
    assert!(write_verified(&mut gpu, regs::FEAT_SS1, 0x8888_8888).is_err());
}

#[test]
fn scenario_03_selector_write_dropped() {
    let mut gpu = FakeGpu::new_unlocked_fixture();
    gpu.force_write(regs::FEAT_SS0, 0x0000_0001); // observable baseline
    gpu.selector_bad = 1; // SS0 writes silently dropped
    assert!(write_verified(&mut gpu, regs::FEAT_SS0, 0x8888_8888).is_err(),
        "read-back catches the silent privilege drop");
    assert_eq!(gpu.read(regs::FEAT_SS0), 0x0000_0001);
}

#[test]
fn scenario_04_locked_plm_drops_writes() {
    let mut gpu = FakeGpu::new_unlocked_fixture();
    gpu.force_write(regs::FEAT_SS0, 0x0000_0001);
    gpu.force_write(regs::FEAT_PLM, 0xffff_ff8f); // locked baseline
    gpu.write(regs::FEAT_SS0, 0x1111_1111);
    assert_eq!(gpu.read(regs::FEAT_SS0), 0x0000_0001,
        "host write dropped while PLM closed");
}

#[test]
fn scenario_05_wpr2_guard_busy() {
    let mut gpu = FakeGpu::new_unlocked_fixture();
    gpu.force_write(regs::WPR2_HI, 0x0400_0000); // already initialized
    assert_eq!(wpr2_cold_guard(&mut gpu), Err("EBUSY: WPR2 already initialized this power cycle"));
}

#[test]
fn scenario_06_plm_stuck_open_is_ealready() {
    let mut gpu = FakeGpu::new_unlocked_fixture();
    gpu.force_write(regs::FEAT_PLM, 0xffff_ffff);
    assert_eq!(plm_cold_guard(&mut gpu), Err("EALREADY: FEAT PLM already open (unlocked or not cold)"));
}

#[test]
fn scenario_07_reset_propagation() {
    let mut gpu = FakeGpu::new_unlocked_fixture();
    gpu.reset_delay = 5;
    gpu.write(regs::SEC2_RESET, 1);
    for _ in 0..4 {
        assert_eq!(gpu.read(regs::SEC2_RESET) & 1, 0, "reset not propagated yet");
    }
    assert_eq!(gpu.read(regs::SEC2_RESET) & 1, 1, "propagates on the Nth read");
}

#[test]
fn scenario_08_gfw_late_completion() {
    let mut gpu = FakeGpu::new_unlocked_fixture();
    gpu.force_write(regs::GFW_BOOT, 0);
    gpu.force_write(regs::FB_SIZE_MIB, 0);
    gpu.gfw_late_reads = 3;
    assert_eq!(gpu.read(regs::FB_SIZE_MIB), 0, "FB size invisible while GFW runs");
    gpu.read(regs::GFW_BOOT);
    gpu.read(regs::GFW_BOOT);
    assert_eq!(gpu.read(regs::GFW_BOOT), regs::GFW_BOOT_COMPLETED);
    assert_eq!(gpu.read(regs::FB_SIZE_MIB), 0x6000, "FB size pops after GFW completes");
}

#[test]
fn scenario_09_sec2_halt_no_plm() {
    let mut gpu = FakeGpu::new_unlocked_fixture();
    gpu.force_write(regs::FEAT_PLM, 0xffff_ff8f);
    gpu.force_write(regs::FEAT_SS0, 0x0000_0001); // observable baseline
    gpu.boot_mode = BootMode::Halt;
    gpu.write(regs::SEC2_CPUCTL, 1);
    assert_eq!(gpu.read(regs::FEAT_PLM), 0xffff_ff8f, "HALT does not open the PLM");
    assert!(write_verified(&mut gpu, regs::FEAT_SS0, 0x8888_8888).is_err());
    assert_eq!(gpu.read(regs::FEAT_SS0), 0x0000_0001);
}
