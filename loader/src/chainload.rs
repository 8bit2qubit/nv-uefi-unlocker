//! Read-only chain-load of the OS bootloader (constitution §1.3:
//! LoadImage/StartImage only, never overwrite the ESP).
//!
//! Strategy: probe every SimpleFileSystem for the candidate loaders in
//! order (Windows first, then the common Linux paths, then the removable-
//! media default), splice a FilePath node onto the winning filesystem's
//! device path, and load from the full device path so the loader keeps its
//! device context (bootmgfw needs it for its BCD store; shim/grub for
//! their config). Our own volume is skipped for the generic BOOTX64 path
//! so we never relaunch ourselves.

use alloc::vec::Vec;
use log::info;
use uefi::boot::{self, LoadImageSource, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::BootPolicy;
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{File, FileAttribute, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::{cstr16, CStr16, Identify, Result};

/// Candidate OS loaders, tried in order on every SimpleFileSystem volume.
const CANDIDATES: &[&CStr16] = &[
    cstr16!(r"\EFI\Microsoft\Boot\bootmgfw.efi"),
    cstr16!(r"\EFI\ubuntu\shimx64.efi"),
    cstr16!(r"\EFI\ubuntu\grubx64.efi"),
    cstr16!(r"\EFI\fedora\shimx64.efi"),
    cstr16!(r"\EFI\debian\shimx64.efi"),
    cstr16!(r"\EFI\arch\grubx64.efi"),
    cstr16!(r"\EFI\Manjaro\grubx64.efi"),
    cstr16!(r"\EFI\opensuse\shimx64.efi"),
    cstr16!(r"\EFI\grub\grubx64.efi"),
    cstr16!(r"\EFI\systemd\systemd-bootx64.efi"),
    cstr16!(r"\EFI\shim\shimx64.efi"),
    cstr16!(r"\EFI\BOOT\BOOTX64.EFI"), // generic fallback LAST (may be us)
];

/// Device path node header: type, subtype, u16 length (includes header).
const NODE_TYPE_MEDIA: u8 = 0x04;
const NODE_SUBTYPE_FILE: u8 = 0x04;
/// END_ENTIRE node: type 0x7f, subtype 0xff, length 0x0004 (little endian).
const END_ENTIRE: [u8; 4] = [0x7f, 0xff, 0x04, 0x00];

pub fn chain_load_bootmgfw() -> Result<()> {
    info!("PHASE: CHAIN");
    let fs_handles = boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))?;
    info!("CHAIN: {} SimpleFileSystem handles", fs_handles.len());

    // Our own volume: the generic \EFI\BOOT\BOOTX64.EFI candidate must
    // never resolve to US (infinite relaunch loop).
    let own_volume = own_volume_handle();

    for handle in fs_handles.iter() {
        // Safety: ByHandleProtocol, protocol lives as long as the handle.
        let mut fs = match unsafe {
            boot::open_protocol::<SimpleFileSystem>(
                OpenProtocolParams {
                    handle: *handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        } {
            Ok(fs) => fs,
            Err(e) => {
                info!("CHAIN: open fs {:?} failed: {:?}", handle, e.status());
                continue;
            }
        };

        let is_own = own_volume == Some(*handle);
        let mut dir = match fs.open_volume() {
            Ok(d) => d,
            Err(_) => continue,
        };
        let mut chosen: Option<&CStr16> = None;
        for path in CANDIDATES {
            if *path == CANDIDATES[CANDIDATES.len() - 1] && is_own {
                continue; // never relaunch ourselves from our own stick
            }
            if dir
                .open(path, FileMode::Read, FileAttribute::empty())
                .is_ok()
            {
                chosen = Some(path);
                break;
            }
        }
        let Some(path) = chosen else { continue };
        info!("CHAIN: loader found on {:?}: {}", handle, path);

        // Safety: same ByHandleProtocol discipline for the device path.
        let dp = match unsafe {
            boot::open_protocol::<DevicePath>(
                OpenProtocolParams {
                    handle: *handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        } {
            Ok(dp) => dp,
            Err(e) => {
                info!("CHAIN: no device path on {:?}: {:?}", handle, e.status());
                continue;
            }
        };

        let full = match splice_file_node(&dp, path) {
            Some(v) => v,
            None => {
                info!("CHAIN: device path splice failed on {:?}", handle);
                continue;
            }
        };

        // Safety: `full` outlives the load_image call; firmware parses it.
        let full_dp = unsafe { DevicePath::from_ffi_ptr(full.as_ptr().cast()) };
        info!("CHAIN: LoadImage (FromDevicePath)");
        // Flush and close the USB log before handing control away; lines
        // logged after this point go to console only.
        crate::logging::finish();
        let loaded = boot::load_image(
            boot::image_handle(),
            LoadImageSource::FromDevicePath {
                device_path: full_dp,
                boot_policy: BootPolicy::ExactMatch,
            },
        )?;
        info!("CHAIN: StartImage -> handing control to the OS loader");
        let r = boot::start_image(loaded);
        info!("CHAIN: loader returned: {:?}", r);
        return Ok(());
    }

    info!("CHAIN: no known OS loader found on any filesystem.");
    info!("CHAIN: boot the machine into its UEFI boot menu / built-in shell");
    info!("CHAIN: and start the desired OS manually - the GPU is already in");
    info!("CHAIN: its post-transaction cold state; no further tool action needed.");
    Err(uefi::Status::NOT_FOUND.into())
}

/// The volume our own image was loaded from (to skip it in candidate
/// probing), or None if the LoadedImage protocol does not expose one.
fn own_volume_handle() -> Option<uefi::Handle> {
    // Safety: GetProtocol on our own image handle during boot services.
    let li = unsafe {
        boot::open_protocol::<LoadedImage>(
            OpenProtocolParams {
                handle: boot::image_handle(),
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
    .ok()?;
    li.device()
}

/// `base` + FilePath(path) + END_ENTIRE, as raw device-path bytes.
fn splice_file_node(base: &DevicePath, path: &CStr16) -> Option<Vec<u8>> {
    let data = base.as_bytes();
    if data.len() < 4 || data[data.len() - 4..] != END_ENTIRE {
        return None; // unexpected layout; refuse to splice
    }
    let mut out = Vec::with_capacity(data.len() + 8 + 64);
    out.extend_from_slice(&data[..data.len() - 4]); // strip END_ENTIRE

    let mut name: Vec<u16> = path.as_slice().iter().map(|c| u16::from(*c)).collect();
    name.push(0); // NUL terminator
    let node_len = (4 + 2 * name.len()) as u16;
    out.push(NODE_TYPE_MEDIA);
    out.push(NODE_SUBTYPE_FILE);
    out.extend_from_slice(&node_len.to_le_bytes());
    for u in name {
        out.extend_from_slice(&u.to_le_bytes());
    }

    out.extend_from_slice(&END_ENTIRE);
    Some(out)
}
