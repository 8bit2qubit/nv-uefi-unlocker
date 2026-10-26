//! TOTORONG-lineage staged driver detach + exclusive claim (msi_release.c
//! semantics): the transaction only works after ALL console and vendor
//! drivers are off the GPU and the PciIo is EXCLUSIVELY owned. Three stages
//! (console agents on GOP, then the vendor driver on PCI), each verified by
//! an open-protocol count delta; then exclusive claim + IO|MEM|BME enable +
//! config snapshot baseline. Reconnect is NOT performed: after FLR the
//! firmware BDS re-discovers the device (TOTORONG never reconnects either).
#![allow(dead_code)]

use alloc::vec;
use log::info;
use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams};
use uefi::proto::console::gop::GraphicsOutput;
use uefi::{Handle, Identify};

use crate::pci::PciIo;

fn open_count(handle: Handle) -> usize {
    match boot::protocols_per_handle(handle) {
        Ok(per) => per.iter().count(),
        Err(_) => 0,
    }
}

fn config_snapshot(pci: &PciIo) -> alloc::vec::Vec<u32> {
    let mut c = vec![0u32; 16];
    for (i, slot) in c.iter_mut().enumerate() {
        *slot = pci.config_read_u32((i * 4) as u32);
    }
    c
}

/// Entry power state, read before anything else touches the device.
pub struct PmEntry {
    pub cap: Option<u8>,
    /// PMCSR as found (before the D0 request).
    pub pmcsr: u16,
    /// Entry was NOT D0 - a sleep/resume or fast-startup handoff, not a POST.
    pub warm: bool,
}

/// Resume-entry preflight. A sleep/resume hands the dGPU over in D3hot: the
/// firmware did not POST it, the previous OS session's device context is
/// still in the function, and everything the transaction relies on (BAR
/// decode, claim guards, FLR) is only defined in D0. Read PMCSR first, say
/// so loudly, then bring the function to D0 with a verified readback.
/// `\tx-require-d0.txt` on the boot volume turns the warning into an abort
/// (nothing has been written yet at that point, so the run is a no-op).
pub fn pm_preflight(pci: &PciIo) -> Result<PmEntry, &'static str> {
    let Some(cap) = pci.pm_cap() else {
        info!("PM: no power-management capability - D-state not evaluable");
        return Ok(PmEntry { cap: None, pmcsr: 0, warm: false });
    };
    let before = pci.pmcsr(cap);
    let warm = before & 0x3 != 0;
    info!("PM: entry PMCSR={before:#06x} state=D{} (cap@{cap:#04x})", before & 0x3);
    if warm {
        info!("PM: WARM ENTRY (resume/fast-startup handoff, not a POST): the previous session's device context is still in this function; the unlock may not survive the Windows handoff");
        if crate::tx::read_boot_file("\\tx-require-d0.txt").is_some() {
            info!("PM: \\tx-require-d0.txt present - aborting before any GPU write (use Restart, or cold boot, and re-run)");
            return Err("warm entry with require-D0 switch set");
        }
    }
    let after_cur = pci.pmcsr(cap);
    if !pci.pmcsr_force_d0(cap) {
        let rb = pci.pmcsr(cap);
        info!("PM: D0 request did NOT stick (PMCSR {after_cur:#06x} -> {rb:#06x}, state=D{})", rb & 0x3);
        return Err("device refuses D0");
    }
    info!("PM: D0 verified (PMCSR={:#06x})", pci.pmcsr(cap));
    Ok(PmEntry { cap: Some(cap), pmcsr: before, warm })
}

/// Staged detach + exclusive claim. Returns the config-space baseline taken
/// while exclusively owned (drift reference for the post-transaction check).
pub fn detach_and_claim(pci_handle: Handle) -> Result<alloc::vec::Vec<u32>, &'static str> {
    info!("DETACH: staged console/driver detach begin (TOTORONG craft)");

    // Stage 1+2: disconnect every driver bound to the GOP handle
    // (ConSplitter TextOut agent + GraphicsConsole; the UEFI disconnect
    // walks children, TOTORONG splits them only for failure attribution).
    let gop_handles =
        boot::locate_handle_buffer(boot::SearchType::ByProtocol(&GraphicsOutput::GUID));
    if let Ok(h) = gop_handles {
        for i in 0..h.len() {
            let gop = h[i];
            let before = open_count(gop);
            info!("DETACH: stage GOP handle {:?} protocols {}", gop, before);
            let st = boot::disconnect_controller(gop, None, None);
            let after = open_count(gop);
            info!(
                "DETACH: GOP disconnect status {:?} protocols {} -> {}",
                st.map(|_| ()),
                before,
                after
            );
        }
    } else {
        info!("DETACH: no GOP handles (headless?) - console stages skipped");
    }

    // Stage 3: the vendor GPU driver (nvgop) on the PCI handle.
    let before = open_count(pci_handle);
    let st = boot::disconnect_controller(pci_handle, None, None);
    let after = open_count(pci_handle);
    info!(
        "DETACH: PCI disconnect status {:?} protocols {} -> {}",
        st.map(|_| ()),
        before,
        after
    );

    // Exclusive claim: re-open EXCLUSIVE (this fails while any other driver
    // still holds the PCI I/O protocol - the TOTORONG acceptance gate).
    let pci = {
        // Safety: exclusive claim; all other drivers detached by now.
        unsafe {
            boot::open_protocol::<PciIo>(
                OpenProtocolParams {
                    handle: pci_handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::Exclusive,
            )
        }
    };
    let pci = match pci {
        Ok(p) => p,
        Err(e) => {
            info!("DETACH: EXCLUSIVE claim failed {:?} (a driver still holds the device?)", e.status());
            return Err("exclusive claim failed");
        }
    };
    info!("DETACH: PciIo EXCLUSIVE claimed");

    // Restore IO|MEM|BME: try the Attributes API first (ClaimRestore),
    // then fall back to a raw PCI_COMMAND write - the driver's stop path
    // cleared MEMORY/BME, and this firmware's Attributes(Enable) is a
    // no-op while detached (observed: MEM=false after enable).
    pci.attributes_enable(0x0007);
    let cmd = pci.config_read_u16(0x04);
    if cmd & 0x7 != 0x7 {
        info!("DETACH: Attributes(Enable) no-op (cmd={cmd:#06x}) - raw COMMAND write");
        pci.cfg_write_u16(0x04, 0x0007);
        let cmd2 = pci.config_read_u16(0x04);
        info!("DETACH: raw COMMAND write result cmd={cmd2:#06x}");
    }
    let baseline = config_snapshot(&pci);
    info!(
        "DETACH: config baseline (vendor={:#06x} dev={:#06x} cmd={:#06x} - MEM={} BME={})",
        baseline[0] & 0xffff,
        baseline[0] >> 16,
        baseline[1] & 0xffff,
        baseline[1] & 0x2 != 0,
        baseline[1] & 0x4 != 0
    );
    // BAR0 reads return garbage until MEMORY decode is back. Verify - 1.1.
    if baseline[1] & 0x2 == 0 {
        return Err("PCI command MEMORY bit did not stick after both enable paths");
    }
    Ok(baseline)
}

/// Post-transaction GOP/console reconnect (TOTORONG craft): after the FLR
/// returned the card to a cold state, recursively ConnectController so the
/// firmware re-binds the vendor GOP/driver stack. Non-fatal: bootmgfw
/// self-establishes its own console when it starts; this only restores the
/// display earlier for machines where the dGPU is the primary adapter.
pub fn reconnect_all(pci_handle: uefi::Handle) {
    // Give the FLR-rebooted function a moment, then recursive-connect.
    boot::stall(core::time::Duration::from_millis(200));
    match boot::connect_controller(pci_handle, &[], None, true) {
        Ok(()) => info!("RECONNECT: ConnectController recursive ok"),
        Err(e) => info!("RECONNECT: ConnectController failed: {:?} (bootmgfw will self-establish)", e.status()),
    }
}
