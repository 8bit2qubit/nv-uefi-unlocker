//! M0 skeleton: banner -> single-key confirm (30s timeout) -> PCI enumeration
//! (read-only) -> chain-load Windows Boot Manager. Zero GPU writes by design.

#![no_main]
#![no_std]

extern crate alloc;
extern crate log;

use log::info;
use uefi::prelude::*;
use uefi::CStr16;

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

/// CONFIRM semantics switch (armed mode): when this file exists on the
/// stick root, the operator has declared intent ON THE STICK ITSELF - the
/// wait becomes a short countdown that auto-proceeds, any key aborts.
const ARMED_MARKER: &CStr16 = cstr16!(r"\i_know_what_i_am_doing.txt");
const CONFIRM_WAIT: u64 = 30;
const ARMED_WAIT: u64 = 5;

#[entry]
fn main() -> Status {
    logging::init(VERSION);
    banner::print(VERSION);

    // Phase: CONFIRM. TX builds: ONLY Space proceeds - timeout or any other
    // key aborts into chain-load with zero GPU writes (issue #4.1: the old
    // code proceeded on any key AND on timeout, so a stick left in a machine
    // could run the transaction unattended). Read-only builds keep the M0
    // semantics. Armed mode (marker file on the stick root) inverts this:
    // a 5s countdown auto-proceeds, any key aborts - the file is the
    // operator's explicit declaration of intent, so unattended runs are
    // opt-in per stick, never a default.
    let confirmed = if chainload::boot_volume_has(ARMED_MARKER) {
        info!(
            "CONFIRM: \\i_know_what_i_am_doing.txt present - ARMED: {ARMED_WAIT}s countdown auto-runs, ANY key aborts"
        );
        keywait::wait_confirm_armed(ARMED_WAIT)
    } else {
        info!("CONFIRM: no armed marker - Space/{CONFIRM_WAIT}s confirm semantics");
        keywait::wait_confirm(CONFIRM_WAIT)
    };

    // Phase: ENUMERATE (read-only). ALL NVIDIA display adapters, in
    // firmware enumeration order.
    let gpus = pci::enumerate();

    // Phase: GOP-PROBE (read-only; which console driver owns which GPU).
    gop::enumerate_gop();

    // Phase: SAMPLE (M1: read-only cold-state register sweep, first card).
    if let Some(gpu) = gpus.first() {
        sample::run(gpu);
    } else {
        info!("SAMPLE: no NVIDIA GPU present, skipped (negative control path)");
    }

    // Phase: TX (feature-gated transaction; only in explicit tx builds).
    // NOTE: inside tx_phase each card's console drivers are detached
    // (TOTORONG craft) - the screen goes dark from that card's transaction
    // until Windows boots. The log file keeps recording and is the
    // evidence record. Multi-GPU: every supported card runs in order.
    #[cfg(feature = "tx")]
    if let keywait::Confirm::Proceed = confirmed {
        tx_phase(&gpus);
    }
    #[cfg(not(feature = "tx"))]
    let _ = confirmed;

    // Phase: CHAIN. Read-only LoadImage/StartImage of the ESP bootmgfw.efi.
    // The verdict lines above are in \loader.log; no pause before handing
    // off (the log file is the evidence record, not the screen).
    match chainload::chain_load_bootmgfw() {
        Ok(()) => info!("CHAIN: bootmgfw returned unexpectedly"),
        Err(e) => info!("CHAIN: failed: {:?}", e.status()),
    }

    info!("FINAL: read-only run complete; nothing was written to the GPU, nothing to restore. Safe to reboot.");
    logging::finish();
    Status::SUCCESS
}


#[cfg(feature = "tx")]
fn tx_phase(gpus: &[pci::FoundGpu]) {
    let targets = pci::select_tx_targets(gpus);
    if targets.is_empty() {
        info!("TX: no transaction target (selection marker refused, or no supported GPU) - chain-loading unchanged Windows");
        return;
    }
    tx::list_root_entries();
    for (i, g) in targets.iter().enumerate() {
        info!(
            "TX: ===== target {}/{}: 10de:{:04x} @ {:04x}:{:02x}:{:02x}.{} =====",
            i + 1,
            targets.len(),
            g.device_id,
            g.seg,
            g.bus,
            g.dev,
            g.fun
        );
        tx_one_card(g);
    }
    info!("TX: multi-GPU pass complete ({} target(s) processed)", targets.len());
}

/// One card's full transaction. Every outcome continues the multi-GPU
/// loop: EALREADY/EWarm are skip-class, a hard failure gets its own
/// emergency FLR on THIS card only - the next card still runs.
#[cfg(feature = "tx")]
fn tx_one_card(g: &pci::FoundGpu) {
    use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams};
    let proto = unsafe {
        boot::open_protocol::<pci::PciIo>(
            OpenProtocolParams { handle: g.handle, agent: boot::image_handle(), controller: None },
            OpenProtocolAttributes::GetProtocol,
        )
    };
    let Ok(pci) = proto else {
        info!("TX: open PciIo failed");
        return;
    };
    let gpu = tx::TxGpu { pci: &pci, bar: 0 };

    // Per-family asset selection by PCI device ID (constitution 5: no
    // cross-family payloads — a GA102 payload on AD102 HALTs and opens
    // nothing). Names on the boot volume: <asset>_<family>.bin.
    let dev = g.device_id;
    let family: &str = match dev {
        // AD10x (Ada): the full upstream ad102_family() routing table -
        // every listed ID uses the same pinned AD102 booter/payload.
        // Silicon-verified upstream: 2684/2685/2705/2805/2882/28a0/28e1;
        // verified here: 28e0; the rest are upstream-admitted IDs. 28a1 is
        // NVIDIA's other 4050 Laptop ID (same AD107M die as verified-silicon
        // 28e1, same 28xx block) but missing from upstream's table.
        // 2820 is the AD106M 4070 Laptop (also upstream-absent); the same
        // family assets as the verified 2803 desktop AD106 apply.
        // TODO(28a1,2820): first-silicon run should re-verify checklist
        // evidence.
        0x2684 | 0x2685 | 0x2689  // AD102 4090 / 4090D / 4070TiS
        | 0x2702 | 0x2704 | 0x2705 | 0x2709  // AD103 4080S/4080/4070TiS/4070
        | 0x2757  // AD103M 4090 Laptop
        | 0x2782 | 0x2783 | 0x2786  // AD104 4070Ti/4070S/4070
        | 0x2803 | 0x2805 | 0x2820  // AD106 4060Ti / 4060Ti 16GB / 4070M
        | 0x2882 | 0x28a0 | 0x28a1 | 0x28e0 | 0x28e1  // AD107 4060/4060L/4050L/4060M/4050M
        => "ad102",
        // GA10x (Ampere): 3090/3080/3070/3060/3050/CPM170HX...
        0x2204 | 0x2206 | 0x2484..=0x248c | 0x2501 | 0x2503 | 0x2504 | 0x2505
        | 0x2507 | 0x2508 | 0x2509 | 0x252f | 0x2544 | 0x2560 | 0x2584
        | 0x20c2 | 0x20f1 => "ga102",
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
    let bl = if family == "tu10x" {
        tx::read_boot_file("\\sec2_bl_tu10x.bin")
    } else {
        None
    };
    let (Some(booter), Some(gsp_boot), Some(gsp_image), Some(stock_sig)) =
        (booter, gsp_boot, gsp_image, stock_sig) else {
        info!("TX: assets missing for family '{family}' (booter_load_*/gsp_boot_*/fwimage_*/fwsignature_*) - skipping");
        return;
    };
    if v67.is_none() {
        info!("TX: NO V67 file - chain-replay mode (no unlock writes)");
    } else {
        info!("TX: V67 present - unlock mode (family-specific write chain)");
    }
    if family == "tu10x" && bl.is_none() {
        info!("TX: tu10x missing \\sec2_bl_tu10x.bin (WITH_LOADER FWSEC needs the generic falcon BL) - skipping");
        return;
    }
    // Turing track enabled 2026-10-27 (user M3 go): the chain is the
    // cmp50hx-proven TU106 recipe - FWSEC BOOT_WITH_LOADER via the generic
    // falcon BL, BOOT_DIRECT booter fire, replay verdict mbox0==0 + WPR2.
    // First fire = chain-replay (no V67 at root) unless the payload file is
    // staged deliberately.
    let assets = tx::TxAssets { family, booter, boot_desc, gsp_boot, gsp_image, stock_sig, v67, bl };
    // Size validation before ANY GPU interaction (issue #4.3): a damaged
    // stick must be refused here - the DMA copies use fixed windows.
    if let Err(e) = tx::validate_asset_sizes(&assets) {
        info!("TX: asset validation failed ({e:?}) - nothing was touched, chain-loading Windows");
        return;
    }
    match tx::run_transaction(&gpu, &assets, g.handle) {
        Ok(()) => {
            info!("SELECTOR RESULT: transaction complete");
            tx::handoff_out(&pci);
        }
        // Already-open PLM = already unlocked or not cold: skip (success
        // class, gpu-unlock-ada da96e3f), definitely no FLR - it would wipe
        // a live unlock.
        Err(tx::TxError::EAlready) => {
            info!("TX: already unlocked / not cold (EALREADY) - skipped, nothing to unwind");
            tx::handoff_out(&pci);
        }
        // Warm entry with the require-D0 switch: nothing was written, the
        // card was not even reset - the clean "come back after a Restart"
        // outcome. No emergency FLR (FLR from a non-D0 state is exactly the
        // undefined case).
        Err(tx::TxError::EWarm) => {
            info!("TX: aborted on warm/resume entry before any GPU write - chain-loading unchanged Windows (use a Restart or cold boot)");
            tx::handoff_out(&pci);
        }
        Err(e) => {
            info!("TX: transaction failed: {e:?} - NOT retrying this session");
            // Failure unwind: FLR wipes our half-states (engine resets, BCR,
            // DMA queues) so the Windows driver gets a cold-boot card.
            tx::emergency_flr(g.handle);
            tx::handoff_out(&pci);
        }
    }
}
