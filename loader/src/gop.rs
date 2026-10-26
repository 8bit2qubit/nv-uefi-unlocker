//! GOP handle enumeration (read-only).
//!
//! M3 will DisconnectController/ConnectController around the transaction
//! (TOTORONG craft); M0 only reports which handles carry GOP and whether one
//! of them is the NVIDIA GPU — on a MUX (dGPU-direct) laptop the panel is
//! driven by the dGPU's GOP, which is exactly the fragile case.

use log::info;
use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::console::gop::GraphicsOutput;
use uefi::{Handle, Identify};

use crate::pci::{self, PciIo};

pub fn enumerate_gop() {
    info!("PHASE: GOP-PROBE (read-only)");
    let handles = match boot::locate_handle_buffer(SearchType::ByProtocol(&GraphicsOutput::GUID)) {
        Ok(h) => h,
        Err(e) => {
            info!("GOP-PROBE: locate failed: {:?}", e.status());
            return;
        }
    };
    info!("GOP-PROBE: {} GOP handles", handles.len());

    for handle in handles.iter() {
        // Safety: GetProtocol, read-only access during boot.
        let gop = match unsafe {
            boot::open_protocol::<GraphicsOutput>(
                OpenProtocolParams {
                    handle: *handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        } {
            Ok(g) => g,
            Err(e) => {
                info!("GOP-PROBE: open failed on {:?}: {:?}", handle, e.status());
                continue;
            }
        };
        let mi = gop.current_mode_info();
        let (w, h) = mi.resolution();
        let vendor_note = match nvidia_vendor_on(*handle) {
            Some(v) if v == pci::NVIDIA_VENDOR_ID => " [NVIDIA GOP]",
            Some(_) => " [other PCI device]",
            None => "",
        };
        info!(
            "GOP handle {:?}: {}x{} {:?}{}",
            handle,
            w,
            h,
            mi.pixel_format(),
            vendor_note
        );
    }
}

/// Vendor ID if this handle also carries PciIo (GOP producer == GPU handle
/// when the console driver bound the GPU itself).
fn nvidia_vendor_on(handle: Handle) -> Option<u16> {
    // Safety: GetProtocol, config reads only.
    let proto = unsafe {
        boot::open_protocol::<PciIo>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
    .ok()?;
    Some(proto.config_read_u16(0x00))
}
