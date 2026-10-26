//! Dual logger: console + `\loader.log` on the boot volume (USB).
//!
//! The UEFI Shell's `> file` redirection proved unreliable on the target
//! firmware (empty capture), so we own the file ourselves. This also
//! pre-implements M1's "write samples to USB FAT" requirement.
//!
//! Constitution note: the log file lives on OUR boot volume and is the only
//! thing this app ever writes; the ESP stays untouched.

use alloc::format;
use core::cell::UnsafeCell;
use log::{Level, Log};
use uefi::boot;
use uefi::proto::media::file::{File, FileAttribute, FileHandle, FileMode, RegularFile};
use uefi::{cstr16, system, CStr16, CString16};

pub const LOG_PATH: &CStr16 = cstr16!(r"\loader.log");

struct DualLogger {
    file: UnsafeCell<Option<RegularFile>>,
    // set after a failed open so we log the reason exactly once
    file_error_reported: UnsafeCell<bool>,
}

static LOGGER: DualLogger = DualLogger {
    file: UnsafeCell::new(None),
    file_error_reported: UnsafeCell::new(false),
};

// Safety: UEFI applications are single-threaded during boot services.
unsafe impl Sync for DualLogger {}
unsafe impl Send for DualLogger {}

impl Log for DualLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "[{:5}] {}: {}\r\n",
            record.level(),
            record.target(),
            record.args()
        );

        // Console mirror.
        if let Ok(s) = CString16::try_from(line.as_str()) {
            system::with_stdout(|out| {
                let _ = out.output_string_lossy(&s);
            });
        }

        // File mirror.
        // Safety: single-threaded boot services; pointer use is confined here.
        unsafe {
            if let Some(f) = (*self.file.get()).as_mut() {
                if f.write(line.as_bytes()).is_err() {
                    // Media pulled: stop trying, keep console alive.
                    *self.file.get() = None;
                }
            }
        }
    }

    fn flush(&self) {}
}

/// Install the dual logger and open a fresh `\loader.log` on the boot volume.
/// Replaces `uefi::helpers::init()` (which only installs uefi's own
/// console-only logger; allocator and panic handler need no init call).
pub fn init(version: &str) {
    let file = open_fresh_log();
    if file.is_none() {
        *unsafe { &mut *LOGGER.file_error_reported.get() } = true;
    }
    // Safety: called once, before any logging.
    unsafe {
        *LOGGER.file.get() = file;
    }
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Info);

    if file_open() {
        log::info!("log file {} opened on boot volume", LOG_PATH);
    } else {
        log::info!("log file unavailable (console-only); open failed");
    }
    log::info!("nv-uefi-unloader v{version} (M0 skeleton: read-only)");
}

/// Close the log file (flush) before chain-loading. Idempotent.
/// Call right before handing control away; later lines go to console only.
pub fn finish() {
    // Safety: single-threaded; take() drops (close = flush per spec).
    let taken = unsafe { (*LOGGER.file.get()).take() };
    drop(taken);
}

fn file_open() -> bool {
    unsafe { (*LOGGER.file.get()).is_some() }
}

fn open_fresh_log() -> Option<RegularFile> {
    let mut fs = boot::get_image_file_system(boot::image_handle()).ok()?;
    let mut root: uefi::proto::media::file::Directory = fs.open_volume().ok()?;

    // Remove a stale log from a previous boot so each run starts clean.
    if let Ok(handle) = root.open(LOG_PATH, FileMode::ReadWrite, FileAttribute::empty()) {
        // A regular file handle only; directories cannot be deleted blindly.
        if let Some(reg) = handle.into_regular_file() {
            let _ = reg.delete();
        }
    }

    let handle: FileHandle = root
        .open(LOG_PATH, FileMode::CreateReadWrite, FileAttribute::empty())
        .ok()?;
    handle.into_regular_file()
}
