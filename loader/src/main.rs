//! M0 skeleton: banner -> single-key confirm (30s timeout) -> PCI enumeration
//! (read-only) -> chain-load Windows Boot Manager. Zero GPU writes by design.

#![no_main]
#![no_std]

extern crate alloc;
extern crate log;

use log::info;
use uefi::prelude::*;

mod banner;
mod chainload;
mod gop;
mod keywait;
mod logging;
mod pci;
mod sample;
#[cfg(feature = "tx")]
mod tx;
#[cfg(feature = "tx")]
mod tx_fwsec;
#[cfg(feature = "tx")]
mod tx_booter;
#[cfg(feature = "tx")]
mod tx_detach;
#[cfg(feature = "tx")]
mod tx_portal;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[entry]
fn main() -> Status {
    logging::init(VERSION);
    banner::print(VERSION);

    // Phase: CONFIRM. Any key proceeds early; timeout proceeds anyway.
    // (M0 is read-only. From M3 on, timeout must mean ABORT into chain-load.)
    match keywait::wait_any_key_or_timeout(30) {
        keywait::KeyOrTimeout::Key(k) => info!("CONFIRM: key {:?} pressed, proceeding", k),
        keywait::KeyOrTimeout::Timeout => {
            info!("CONFIRM: 30s timeout, proceeding (M0 read-only default)")
        }
    }

    // Phase: ENUMERATE (read-only).
    let gpu = pci::enumerate();

    // Phase: GOP-PROBE (read-only; which console driver owns which GPU).
    gop::enumerate_gop();

    // Phase: SAMPLE (M1: read-only cold-state register sweep).
    match &gpu {
        Some(gpu) => sample::run(gpu),
        None => info!("SAMPLE: no NVIDIA GPU present, skipped (negative control path)"),
    }

    // Phase: TX (feature-gated transaction; only in explicit tx builds).
    // NOTE: inside tx_phase the console drivers are detached (TOTORONG
    // craft) - the screen goes dark from that point until Windows boots.
    // The log file keeps recording; the CHAIN-GATE pause still works
    // (keyboard events are handled by the firmware, not the console).
    #[cfg(feature = "tx")]
    if let Some(gpu_handle) = &gpu {
        tx_phase(gpu_handle.handle);
    }

    // Phase: CHAIN-GATE. tx builds pause before leaving the app so the
    // final screen (SELECTOR RESULT / POST-FLR / dump lines) can be read
    // or photographed before the console is torn down by bootmgfw.
    #[cfg(feature = "tx")]
    {
        info!("CHAIN-GATE: press ANY KEY to chain-load bootmgfw (photo the screen first if needed)");
        match keywait::wait_any_key_or_timeout(300) {
            keywait::KeyOrTimeout::Key(k) => info!("CHAIN-GATE: key {:?} - chaining", k),
            keywait::KeyOrTimeout::Timeout => info!("CHAIN-GATE: 300s timeout - chaining anyway"),
        }
    }

    // Phase: CHAIN. Read-only LoadImage/StartImage of the ESP bootmgfw.efi.
    match chainload::chain_load_bootmgfw() {
        Ok(()) => info!("CHAIN: bootmgfw returned unexpectedly"),
        Err(e) => info!("CHAIN: failed: {:?}", e.status()),
    }

    info!("FINAL: read-only run complete; nothing was written to the GPU, nothing to restore. Safe to reboot.");
    logging::finish();
    Status::SUCCESS
}


#[cfg(feature = "tx")]
fn tx_phase(handle: uefi::Handle) {
    use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams};
    let proto = unsafe {
        boot::open_protocol::<pci::PciIo>(
            OpenProtocolParams { handle, agent: boot::image_handle(), controller: None },
            OpenProtocolAttributes::GetProtocol,
        )
    };
    let Ok(pci) = proto else {
        info!("TX: open PciIo failed");
        return;
    };
    let gpu = tx::TxGpu { pci: &pci, bar: 0 };
    tx::list_root_entries();

    // Per-family asset selection by PCI device ID (constitution 5: no
    // cross-family payloads — a GA102 payload on AD102 HALTs and opens
    // nothing). Names on the boot volume: <asset>_<family>.bin.
    let dev = pci.config_read_u16(0x02);
    let family: &str = match dev {
        // AD10x (Ada): 4090/4090D/4070TiS/4060Ti/4060M...
        0x2684 | 0x2685 | 0x2705 | 0x2803 | 0x2805 | 0x28a0 | 0x28e0 => "ad102",
        // GA10x (Ampere): 3090/3080/3070/3060/CPM170HX...
        0x2204 | 0x2206 | 0x2484..=0x248c | 0x2503 | 0x2504 | 0x2505 | 0x2507
        | 0x2544 | 0x2560 | 0x20c2 | 0x20f1 => "ga102",
        // Turing: 2060/2070/2080, 1660-lineage, RTX 20-series laptops.
        0x1e02..=0x1fff | 0x2182..=0x2192 => "tu10x",
        other => {
            info!("TX: device 10de:{other:04x} has no asset family mapping - skipping");
            return;
        }
    };
    info!("TX: device 10de:{dev:04x} -> asset family '{family}'");

    let booter = tx::read_boot_file_fmt("\\booter_load_{}.bin", family);
    let boot_desc = tx::read_boot_file_fmt("\\gsp_boot_desc_{}.bin", family);
    let gsp_boot = tx::read_boot_file_fmt("\\gsp_boot_{}.bin", family);
    let gsp_image = tx::read_boot_file_fmt("\\fwimage_{}.bin", family);
    let stock_sig = tx::read_boot_file_fmt("\\fwsignature_{}.bin", family);
    let v67 = tx::read_boot_file_fmt("\\v67-{}-four-plm.bin", family);
    let (Some(booter), Some(gsp_boot), Some(gsp_image), Some(stock_sig)) =
        (booter, gsp_boot, gsp_image, stock_sig) else {
        info!("TX: assets missing for family '{family}' (booter_load_*/gsp_boot_*/fwimage_*/fwsignature_*) - skipping");
        return;
    };
    if v67.is_none() {
        info!("TX: NO V67 file - chain-replay mode (no unlock writes)");
    } else {
        info!("TX: V67 present - four-write unlock mode");
    }
    if family == "tu10x" {
        // Turing chain (BOOT_DIRECT booter + WITH_LOADER FWSEC + BSI
        // scrubber semantics) is implemented but not hardware-validated;
        // entering the GA10x HS chain on TU106 would write wrong states.
        // Assets above are parsed+verified as evidence only.
        info!("TX: family 'tu10x' - Turing transaction track not enabled yet, skipping (no writes)");
        return;
    }
    let assets = tx::TxAssets { family, booter, boot_desc, gsp_boot, gsp_image, stock_sig, v67 };
    match tx::run_transaction(&gpu, &assets, handle) {
        Ok(()) => info!("SELECTOR RESULT: transaction complete"),
        // Already-open PLM = already unlocked or not cold: skip (success
        // class, gpu-unlock-ada da96e3f), definitely no FLR - it would wipe
        // a live unlock.
        Err(tx::TxError::EAlready) => {
            info!("TX: already unlocked / not cold (EALREADY) - skipped, nothing to unwind");
        }
        Err(e) => {
            info!("TX: transaction failed: {e:?} - NOT retrying this session");
            // Failure unwind: FLR wipes our half-states (engine resets, BCR,
            // DMA queues) so the Windows driver gets a cold-boot card.
            tx::emergency_flr(handle);
        }
    }
}
