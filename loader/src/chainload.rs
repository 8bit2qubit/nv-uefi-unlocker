//! Read-only chain-load of the ESP's `\EFI\Microsoft\Boot\bootmgfw.efi`
//! (constitution §1.3: LoadImage/StartImage only, never overwrite the ESP).
//!
//! Strategy: probe every SimpleFileSystem for the bootmgfw file, take that
//! filesystem's device path, splice a FilePath node onto it, and load from
//! the resulting full device path so bootmgfw keeps its device context
//! (needed for it to find its BCD store).

use alloc::vec::Vec;
use log::info;
use uefi::boot::{self, LoadImageSource, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::BootPolicy;
use uefi::proto::device_path::DevicePath;
use uefi::proto::media::file::{File, FileAttribute, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::{cstr16, CStr16, Identify, Result};

const BOOTMGFW_PATH: &CStr16 = cstr16!(r"\EFI\Microsoft\Boot\bootmgfw.efi");

/// Device path node header: type, subtype, u16 length (includes header).
const NODE_TYPE_MEDIA: u8 = 0x04;
const NODE_SUBTYPE_FILE: u8 = 0x04;
/// END_ENTIRE node: type 0x7f, subtype 0xff, length 0x0004 (little endian).
const END_ENTIRE: [u8; 4] = [0x7f, 0xff, 0x04, 0x00];

pub fn chain_load_bootmgfw() -> Result<()> {
    info!("PHASE: CHAIN");
    let fs_handles = boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))?;
    info!("CHAIN: {} SimpleFileSystem handles", fs_handles.len());

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

        let mut dir = match fs.open_volume() {
            Ok(d) => d,
            Err(_) => continue,
        };
        // Existence probe: open read-only and drop it immediately.
        let found = dir
            .open(BOOTMGFW_PATH, FileMode::Read, FileAttribute::empty())
            .is_ok();
        if !found {
            continue;
        }
        info!("CHAIN: bootmgfw found on {:?}", handle);

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

        let full = match splice_file_node(&dp) {
            Some(v) => v,
            None => {
                info!("CHAIN: device path splice failed on {:?}", handle);
                continue;
            }
        };

        // Safety: `full` outlives the load_image call; firmware parses it.
        let full_dp = unsafe { DevicePath::from_ffi_ptr(full.as_ptr().cast()) };
        info!("CHAIN: LoadImage bootmgfw (FromDevicePath)");
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
        info!("CHAIN: StartImage -> handing control to Windows Boot Manager");
        let r = boot::start_image(loaded);
        info!("CHAIN: bootmgfw returned: {:?}", r);
        return Ok(());
    }

    info!("CHAIN: bootmgfw not found on any filesystem");
    Err(uefi::Status::NOT_FOUND.into())
}

/// `base` + FilePath(BOOTMGFW_PATH) + END_ENTIRE, as raw device-path bytes.
fn splice_file_node(base: &DevicePath) -> Option<Vec<u8>> {
    let data = base.as_bytes();
    if data.len() < 4 || data[data.len() - 4..] != END_ENTIRE {
        return None; // unexpected layout; refuse to splice
    }
    let mut out = Vec::with_capacity(data.len() + 8 + 64);
    out.extend_from_slice(&data[..data.len() - 4]); // strip END_ENTIRE

    let mut name: Vec<u16> = BOOTMGFW_PATH.as_slice().iter().map(|c| u16::from(*c)).collect();
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
