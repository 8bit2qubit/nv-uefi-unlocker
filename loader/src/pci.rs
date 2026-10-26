//! PCI enumeration via a hand-written raw `EFI_PCI_IO_PROTOCOL` wrapper.
//!
//! uefi 0.26 (latest on crates.io at project start) ships no `PciIo` protocol
//! type, and neither does `uefi_raw`; per task brief §5 we drop to the raw
//! protocol definition. Layout follows UEFI spec 2.x, PCI I/O Protocol.
//!
//! M0 scope: config-space READS only (vendor/device/class + capability walk).
//! There is deliberately no write helper in this file.

use core::ffi::c_void;
#[cfg(feature = "tx")]
use core::time::Duration;
use log::info;
use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::unsafe_protocol;
use uefi::{Identify, Status};

pub const NVIDIA_VENDOR_ID: u16 = 0x10de;

/// EFI_PCI_IO_PROTOCOL_WIDTH (only the fixed-width variants are named here).
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Uint64 arrives with M1 BAR0 64-bit reads
#[repr(u32)]
pub enum PciWidth {
    Uint8 = 0,
    Uint16,
    Uint32,
    Uint64,
}

/// Safety: `this` must point to a live `PciIoRaw` for the call duration.
type AccessMemFn =
    unsafe extern "efiapi" fn(*mut PciIoRaw, PciWidth, bar_index: u8, offset: u64, count: usize, buffer: *mut c_void) -> Status;
/// Safety: `this` must point to a live `PciIoRaw` for the call duration.
type AccessCfgFn =
    unsafe extern "efiapi" fn(*mut PciIoRaw, PciWidth, offset: u32, count: usize, buffer: *mut c_void) -> Status;

#[repr(C)]
struct MemAccess {
    read: AccessMemFn,
    write: AccessMemFn,
}

#[repr(C)]
struct CfgAccess {
    read: AccessCfgFn,
    write: AccessCfgFn,
}

/// Safety: `this` must point to a live `PciIoRaw` for the call duration.
pub type MapFn = unsafe extern "efiapi" fn(
    *mut PciIoRaw,
    operation: u32, // EFI_PCI_IO_PROTOCOL_OPERATION
    host: *mut c_void,
    bytes: *mut usize,
    device: *mut u64,
    mapping: *mut *mut c_void,
) -> Status;
pub type UnmapFn = unsafe extern "efiapi" fn(*mut PciIoRaw, mapping: *mut c_void) -> Status;
pub type AllocBufFn = unsafe extern "efiapi" fn(
    *mut PciIoRaw,
    alloc_type: u32,   // EFI_ALLOCATE_TYPE
    memory_type: u32,  // EFI_MEMORY_TYPE
    pages: usize,
    host: *mut *mut c_void,
    attributes: u64,
) -> Status;
pub type FreeBufFn = unsafe extern "efiapi" fn(*mut PciIoRaw, pages: usize, host: *mut c_void) -> Status;

#[repr(C)]
pub struct PciIoRaw {
    /// EDK2 MdePkg/Include/Protocol/PciIo.h: the struct opens with TWO poll
    /// fields (PollMem, PollIo). A previous revision had only one, shifting
    /// every slot: `pci.read` landed on `Io.Write` (an unintended write
    /// primitive, rejected by firmware as UNSUPPORTED on BAR-less devices)
    /// and `get_location` landed on `Flush` (SUCCESS + untouched out-params).
    /// Offsets below are pinned by compile-time asserts.
    _poll_mem: *const c_void,
    _poll_io: *const c_void,
    mem: MemAccess,
    _io: MemAccess,
    pci: CfgAccess,
    _copy_mem: *const c_void,
    map: MapFn,
    unmap: UnmapFn,
    allocate_buffer: AllocBufFn,
    free_buffer: FreeBufFn,
    _flush: *const c_void,
    get_location:
        unsafe extern "efiapi" fn(*mut PciIoRaw, *mut usize, *mut usize, *mut usize, *mut usize) -> Status,
    /// EfiPciIoAttributeOperation(Get/Enable/...), arg, arg, status out
    attributes: unsafe extern "efiapi" fn(*mut PciIoRaw, u32, u64, u64, *mut Status) -> Status,
    _get_bar_attributes: *const c_void,
    _set_bar_attributes: *const c_void,
    rom_size: u64,
    rom_image: *const c_void,
}

// Layout pins against EDK2 PciIo.h (pointer = 8 bytes on x86_64 UEFI).
const _: () = assert!(core::mem::offset_of!(PciIoRaw, pci) == 6 * 8); // slots: PollMem,PollIo,Mem{r,w},Io{r,w} -> Pci.Read
const _: () = assert!(core::mem::offset_of!(PciIoRaw, get_location) == 14 * 8);
const _: () = assert!(core::mem::offset_of!(PciIoRaw, attributes) == 15 * 8);
const _: () = assert!(core::mem::offset_of!(PciIoRaw, rom_size) == 18 * 8);
const _: () = assert!(core::mem::size_of::<PciIoRaw>() == 20 * 8);

#[repr(transparent)]
#[unsafe_protocol("4cf5b200-68b8-4ca5-9eec-b23e3f50029a")]
pub struct PciIo(PciIoRaw);

impl PciIo {
    /// Config-space read of `count` scalars of the given width.
    /// Status is returned to the caller; helpers below log failures
    /// (constitution §1: never swallow a read-back result silently).
    fn config_read_raw(&self, width: PciWidth, offset: u32, count: usize, buf: *mut c_void) -> Status {
        // Safety: exclusive-borrowed protocol, buffers outlive the call.
        unsafe { (self.0.pci.read)(&self.0 as *const _ as *mut _, width, offset, count, buf) }
    }

    pub fn config_read_u16(&self, offset: u32) -> u16 {
        let mut v = 0u16;
        let st = self.config_read_raw(PciWidth::Uint16, offset, 1, &mut v as *mut _ as *mut c_void);
        if st != Status::SUCCESS {
            log::error!("cfg_read16({:#06x}) status {:?}", offset, st);
        }
        v
    }

    pub fn config_read_u32(&self, offset: u32) -> u32 {
        let mut v = 0u32;
        let st = self.config_read_raw(PciWidth::Uint32, offset, 1, &mut v as *mut _ as *mut c_void);
        if st != Status::SUCCESS {
            log::error!("cfg_read32({:#06x}) status {:?}", offset, st);
        }
        v
    }

    pub fn config_read_u8(&self, offset: u32) -> u8 {
        let mut v = 0u8;
        let st = self.config_read_raw(PciWidth::Uint8, offset, 1, &mut v as *mut _ as *mut c_void);
        if st != Status::SUCCESS {
            log::error!("cfg_read8({:#06x}) status {:?}", offset, st);
        }
        v
    }

    /// BAR memory read of `count` u32 scalars via `Mem.Read` (M1 sampler's
    /// only window into the GPU: BAR0 MMIO). Status is returned; helpers log.
    fn mem_read_raw(&self, width: PciWidth, bar: u8, offset: u64, count: usize, buf: *mut c_void) -> Status {
        // Safety: exclusive-borrowed protocol, buffers outlive the call.
        unsafe { (self.0.mem.read)(&self.0 as *const _ as *mut _, width, bar, offset, count, buf) }
    }

    /// BAR memory write of `count` u32 scalars via `Mem.Write` (the only
    /// host-to-VRAM bulk path: BAR1 into the framebuffer).
    fn mem_write_raw(&self, width: PciWidth, bar: u8, offset: u64, count: usize, buf: *mut c_void) -> Status {
        // Safety: exclusive-borrowed protocol; caller owns the source buffer.
        unsafe { (self.0.mem.write)(&self.0 as *const _ as *mut _, width, bar, offset, count, buf) }
    }

    /// Read one u32 from a BAR window (BAR0 for all M1 registers).
    pub fn bar_read_u32(&self, bar: u8, offset: u64) -> u32 {
        let mut v = 0u32;
        let st = self.mem_read_raw(PciWidth::Uint32, bar, offset, 1, &mut v as *mut _ as *mut c_void);
        if st != Status::SUCCESS {
            log::error!("bar_read32(bar {bar}, {offset:#x}) status {st:?}");
        }
        v
    }

    /// Read one u32, returning None when the device rejects the access
    /// (misaligned offset, decode error). Used by sweep loops that must
    /// distinguish "read failed" from "value zero".
    pub fn bar_try_read_u32(&self, bar: u8, offset: u64) -> Option<u32> {
        let mut v = 0u32;
        let st = self.mem_read_raw(PciWidth::Uint32, bar, offset, 1, &mut v as *mut _ as *mut c_void);
        if st != Status::SUCCESS {
            log::error!("bar_read32(bar {bar}, {offset:#x}) status {st:?}");
            return None;
        }
        Some(v)
    }

    /// PCI attributes enable (tx builds: TOTORONG ClaimRestore step).
    #[cfg(feature = "tx")]

    #[cfg(feature = "tx")]
    pub fn attributes_enable(&self, mask: u64) {
        // op 1 = EfiPciIoAttributeOperationEnable; Arg1 = mask
        let mut st = Status::SUCCESS;
        unsafe {
            let _called = (self.0.attributes)(&self.0 as *const _ as *mut _, 1, mask, 0, &mut st);
        }
        if st != Status::SUCCESS {
            log::error!("attributes_enable({mask:#x}) status {st:?}");
        }
    }

    /// THE BAR write primitive — consumed only by `tx` builds. M0/M1
    /// binaries contain no call sites (constitution §1.7 audit anchor).
    #[cfg(feature = "tx")]
    pub fn bar_write_u32(&self, bar: u8, offset: u64, v: u32) {
        // Safety: exclusive-borrowed protocol, buffers outlive the call.
        unsafe {
            let _ = (self.0.mem.write)(&self.0 as *const _ as *mut _, PciWidth::Uint32, bar, offset, 1, &v as *const _ as *mut c_void);
        };
    }

    /// PCI config-space dword write (tx builds: BAR restore after FLR).
    #[cfg(feature = "tx")]
    pub fn cfg_write_u32(&self, offset: u32, v: u32) {
        // Safety: exclusively borrowed protocol; offset must be 4-aligned.
        unsafe {
            let _ = (self.0.pci.write)(&self.0 as *const _ as *mut _, PciWidth::Uint32, offset, 1, &v as *const _ as *mut c_void);
        };
    }

    /// PCI config-space write (tx builds: BME clear + FLR trigger +
    /// COMMAND restore when the Attributes API is refused).
    #[cfg(feature = "tx")]
    pub fn cfg_write_u16(&self, offset: u32, v: u16) {
        // Safety: exclusively borrowed protocol.
        unsafe {
            let _ = (self.0.pci.write)(&self.0 as *const _ as *mut _, PciWidth::Uint16, offset, 1, &v as *const _ as *mut c_void);
        }
    }

    /// Walk the legacy capability list for the Power Management capability
    /// (ID 0x01). Read-only. `None` = no PM cap, which means the D-state
    /// cannot be evaluated at all (the device is then assumed powered on).
    #[cfg(feature = "tx")]
    pub fn pm_cap(&self) -> Option<u8> {
        if self.config_read_u16(0x06) & 0x10 == 0 {
            return None; // STATUS.CAP_LIST absent
        }
        let mut ptr = self.config_read_u8(0x34) & 0xfc;
        let mut guard = 0;
        while ptr != 0 && guard < 48 {
            if self.config_read_u8(ptr.into()) == 0x01 {
                return Some(ptr);
            }
            ptr = self.config_read_u8(ptr as u32 + 1) & 0xfc;
            guard += 1;
        }
        None
    }

    /// PMCSR read (PM capability + 4). Bits 1:0 = PowerState.
    #[cfg(feature = "tx")]
    pub fn pmcsr(&self, cap: u8) -> u16 {
        self.config_read_u16(cap as u32 + 4)
    }

    /// PMCSR write, caller supplies the exact value.
    #[cfg(feature = "tx")]
    pub fn pmcsr_write(&self, cap: u8, v: u16) {
        self.cfg_write_u16(cap as u32 + 4, v);
    }

    /// Request D0 by clearing PMCSR.PowerState (all other bits preserved)
    /// and verify by readback. True only when the state field reads back 0.
    /// A D3hot -> D0 transition may take up to 10 ms (PCI PM spec).
    #[cfg(feature = "tx")]
    pub fn pmcsr_force_d0(&self, cap: u8) -> bool {
        let cur = self.pmcsr(cap);
        if cur & 0x3 == 0 {
            return true;
        }
        self.pmcsr_write(cap, cur & !0x3);
        boot::stall(Duration::from_millis(20));
        self.pmcsr(cap) & 0x3 == 0
    }

    /// Coherent common-buffer DMA allocation (AllocateBuffer + Map).
    /// TOTORONG craft uses EfiReservedMemoryType, but consumer firmware
    /// (observed on AD107M) rejects it with INVALID_PARAMETER — fall back
    /// through legal coherent memory types before giving up.
    #[cfg(feature = "tx")]
    pub fn dma_alloc(&self, len: usize) -> Result<DmaBuffer, Status> {
        // Canonical EFI_MEMORY_TYPE numbering (UEFI spec): BS-Data=4,
        // RS-Data=6. EfiReserved(0) is included only to reproduce the
        // documented EDK2 rejection for the record; the legal set for
        // PciIo.AllocateBuffer is exactly {EfiBootServicesData,
        // EfiRuntimeServicesData} (PciRootBridgeIo.c).
        const TYPES: [(u32, &str); 3] = [
            (4, "EfiBootServicesData"),
            (6, "EfiRuntimeServicesData"),
            (0, "EfiReserved(spec-rejected)"),
        ];
        let pages = len.div_ceil(4096);
        for (mt, name) in TYPES {
            let mut host: *mut c_void = core::ptr::null_mut();
            // Safety: out-params outlive the call; protocol exclusively borrowed.
            let st = unsafe {
                (self.0.allocate_buffer)(
                    &self.0 as *const _ as *mut _,
                    0, // AllocateAnyPages
                    mt,
                    pages,
                    &mut host,
                    0,
                )
            };
            if st != Status::SUCCESS || host.is_null() {
                log::info!("DMA alloc: memtype {name} rejected ({st:?})");
                continue;
            }
            let mut bytes = pages * 4096;
            let mut device: u64 = 0;
            let mut mapping: *mut c_void = core::ptr::null_mut();
            // Safety: as above; host block is valid for `pages` pages.
            let st = unsafe {
                (self.0.map)(
                    &self.0 as *const _ as *mut _,
                    2, // EfiPciOperationBusMasterCommonBuffer
                    host,
                    &mut bytes,
                    &mut device,
                    &mut mapping,
                )
            };
            if st != Status::SUCCESS {
                log::info!("DMA map: memtype {name} failed ({st:?})");
                let _st = unsafe { (self.0.free_buffer)(&self.0 as *const _ as *mut _, pages, host) };
                continue;
            }
            log::info!("DMA alloc: {} pages memtype={name} dev={:#x}", pages, device);
            return Ok(DmaBuffer { host: host as *mut u8, dev: device, mapping, pages, len, released: false });
        }
        Err(Status::OUT_OF_RESOURCES)
    }

    /// Bulk u32 read from a BAR window (used for the PROM CRC pass).
    pub fn bar_read_u32_buf(&self, bar: u8, offset: u64, buf: &mut [u32]) -> Status {
        self.mem_read_raw(PciWidth::Uint32, bar, offset, buf.len(), buf.as_mut_ptr() as *mut c_void)
    }

    /// Bulk u32 write into a BAR window (VRAM fallback staging via BAR1).
    /// `buf.len()` must be a multiple of 4.
    pub fn bar_write_buf(&self, bar: u8, offset: u64, buf: &[u8]) -> Status {
        self.mem_write_raw(PciWidth::Uint32, bar, offset, buf.len() / 4, buf.as_ptr() as *mut c_void)
    }

    /// (segment, bus, device, function) as UINTN scalars per spec.
    pub fn get_location(&self) -> Option<(usize, usize, usize, usize)> {
        let (mut seg, mut bus, mut dev, mut fun) = (0usize, 0usize, 0usize, 0usize);
        // Safety: out-params outlive the call and are correctly typed.
        let st = unsafe {
            (self.0.get_location)(&self.0 as *const _ as *mut _, &mut seg, &mut bus, &mut dev, &mut fun)
        };
        if st != Status::SUCCESS {
            log::error!("get_location status {:?}", st);
            return None;
        }
        Some((seg, bus, dev, fun))
    }
}

/// Coherent DMA buffer. Release is EXPLICIT (`release`) so the transaction's
/// unwind can verify nobody freed twice or leaked — no silent Drop.
#[cfg(feature = "tx")]
#[allow(dead_code)] // consumed by the fwsec/booter wiring tranche
pub struct DmaBuffer {
    pub host: *mut u8,
    pub dev: u64,
    pub mapping: *mut c_void,
    pub pages: usize,
    pub len: usize,
    released: bool,
}

#[cfg(feature = "tx")]
impl DmaBuffer {
    /// Unmap + free. Idempotent (second call is a no-op returning true).
    /// Returns false if the firmware rejected the unmap — caller must log
    /// and refuse to proceed (unwind contract).
    #[allow(dead_code)] // consumed by the fwsec/booter wiring tranche
    pub fn release(&mut self, pci: &PciIo) -> bool {
        if self.released {
            return true;
        }
        self.released = true;
        // Safety: buffer still mapped/allocated; protocol outlives boot.
        unsafe {
            let s1 = (pci.0.unmap)(&pci.0 as *const _ as *mut _, self.mapping);
            let s2 = (pci.0.free_buffer)(&pci.0 as *const _ as *mut _, self.pages, self.host as *mut c_void);
            s1 == Status::SUCCESS && s2 == Status::SUCCESS
        }
    }
}

/// The first NVIDIA display device found by [`enumerate`] (function 0 only).
pub struct FoundGpu {
    pub handle: uefi::Handle,
    pub device_id: u16,
    pub seg: usize,
    pub bus: usize,
    pub dev: usize,
    pub fun: usize,
}

/// Enumerate all PCI(e) devices; NVIDIA (10de) ones get the capability walk.
/// Read-only: config-space reads only, zero writes (constitution §1.7/M0).
/// Returns the first NVIDIA GPU (class 03xx, function 0) for M1 sampling.
pub fn enumerate() -> Option<FoundGpu> {
    info!("PHASE: ENUMERATE (read-only)");
    let handles = match boot::locate_handle_buffer(SearchType::ByProtocol(&PciIo::GUID)) {
        Ok(h) => h,
        Err(e) => {
            info!("ENUMERATE: locate_handle_buffer failed: {:?}", e.status());
            return None;
        }
    };
    info!("ENUMERATE: {} PciIo handles", handles.len());

    let mut found: Option<FoundGpu> = None;
    for handle in handles.iter() {
        // Safety: GetProtocol performs no driver binding; protocol
        // interface stays valid while the handle lives (all of boot time).
        let proto = unsafe {
            boot::open_protocol::<PciIo>(
                OpenProtocolParams {
                    handle: *handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        };
        let pci = match proto {
            Ok(p) => p,
            Err(e) => {
                info!("ENUMERATE: open_protocol failed on {:?}: {:?}", handle, e.status());
                continue;
            }
        };

        // Location first: it doubles as an interface sanity probe.
        let (seg, bus, dev, fun) = match pci.get_location() {
            Some(loc) => loc,
            None => {
                info!("ENUMERATE: no location on {:?}, skip", handle);
                continue;
            }
        };

        let vendor = pci.config_read_u16(0x00);
        let device = pci.config_read_u16(0x02);
        if vendor == 0xffff || vendor == 0x0000 {
            info!("PCI {:04x}:{:02x}:{:02x}.{} empty slot (vendor {:#06x})", seg, bus, dev, fun, vendor);
            continue;
        }
        let class = pci.config_read_u32(0x08) >> 16; // class<<8 | subclass
        info!(
            "PCI {:04x}:{:02x}:{:02x}.{} {:04x}:{:04x} class {:06x}",
            seg, bus, dev, fun, vendor, device, class
        );

        if vendor == NVIDIA_VENDOR_ID {
            if found.is_none() && fun == 0 {
                found = Some(FoundGpu {
                    handle: *handle,
                    device_id: device,
                    seg, bus, dev, fun,
                });
            }
            describe_nvidia(&pci, seg, bus, dev, fun, device);
        }
    }
    found
}

/// Logs the GPU's command, status and device control, then the IDs, command,
/// status, bus numbers and bridge control of every PCI bridge. Read-only.
#[cfg(feature = "tx")]
pub fn log_dma_path(tag: &str, gpu: &PciIo) {
    // PCIe cap at 0x78: Device Control at +0x08.
    info!(
        "DMA-PATH[{tag}]: gpu loc={:?} command={:#06x} status={:#06x} devctl={:#06x}",
        gpu.get_location(),
        gpu.config_read_u16(0x04),
        gpu.config_read_u16(0x06),
        gpu.config_read_u16(0x78 + 0x08)
    );
    let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&PciIo::GUID)) else {
        info!("DMA-PATH[{tag}]: locate_handle_buffer failed");
        return;
    };
    for handle in handles.iter() {
        // Safety: GetProtocol performs no driver binding (same as enumerate()).
        let Ok(pci) = (unsafe {
            boot::open_protocol::<PciIo>(
                OpenProtocolParams { handle: *handle, agent: boot::image_handle(), controller: None },
                OpenProtocolAttributes::GetProtocol,
            )
        }) else {
            continue;
        };
        if pci.config_read_u16(0x00) == 0xffff || pci.config_read_u8(0x0e) & 0x7f != 0x01 {
            continue;
        }
        let loc = pci.get_location();
        let buses = pci.config_read_u32(0x18);
        info!(
            "DMA-PATH[{tag}]: bridge {:04x}:{:04x} loc={loc:?} command={:#06x} status={:#06x} pri/sec/sub={}/{}/{} bridgectl={:#06x}",
            pci.config_read_u16(0x00),
            pci.config_read_u16(0x02),
            pci.config_read_u16(0x04),
            pci.config_read_u16(0x06),
            buses & 0xff,
            buses >> 8 & 0xff,
            buses >> 16 & 0xff,
            pci.config_read_u16(0x3e)
        );
    }
}

/// Bus-master guard for the PCI bridges between the root complex and the GPU.
///
/// `enable` sets COMMAND bit 2 (bus master) on every PCI-to-PCI bridge in the
/// GPU's segment whose secondary..subordinate range contains the GPU's bus,
/// only where the bit is clear, and records the bridges whose readback shows it
/// set. Drop clears bit 2 again on exactly those bridges.
#[cfg(feature = "tx")]
pub struct BridgeMaster {
    /// (bridge handle, location for logs) of every bridge this guard enabled.
    enabled: alloc::vec::Vec<(uefi::Handle, Option<(usize, usize, usize, usize)>)>,
}

#[cfg(feature = "tx")]
impl BridgeMaster {
    pub fn enable(gpu: &PciIo) -> BridgeMaster {
        let mut enabled = alloc::vec::Vec::new();
        let Some((gpu_seg, gpu_bus, _, _)) = gpu.get_location() else {
            info!("BRIDGE-BME: GPU location unavailable - not touching any bridge");
            return BridgeMaster { enabled };
        };
        let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&PciIo::GUID)) else {
            info!("BRIDGE-BME: locate_handle_buffer failed");
            return BridgeMaster { enabled };
        };
        for handle in handles.iter() {
            // Safety: GetProtocol performs no driver binding (same as enumerate()).
            let Ok(pci) = (unsafe {
                boot::open_protocol::<PciIo>(
                    OpenProtocolParams { handle: *handle, agent: boot::image_handle(), controller: None },
                    OpenProtocolAttributes::GetProtocol,
                )
            }) else {
                continue;
            };
            if pci.config_read_u16(0x00) == 0xffff || pci.config_read_u8(0x0e) & 0x7f != 0x01 {
                continue;
            }
            let loc = pci.get_location();
            if loc.map(|(seg, _, _, _)| seg) != Some(gpu_seg) {
                continue;
            }
            let buses = pci.config_read_u32(0x18);
            let (sec, sub) = ((buses >> 8 & 0xff) as usize, (buses >> 16 & 0xff) as usize);
            if gpu_bus < sec || gpu_bus > sub {
                continue;
            }
            let before = pci.config_read_u16(0x04);
            if before & 0x4 != 0 {
                info!("BRIDGE-BME: {loc:?} already has bus master (command={before:#06x})");
                continue;
            }
            pci.cfg_write_u16(0x04, before | 0x4);
            let after = pci.config_read_u16(0x04);
            info!("BRIDGE-BME: {loc:?} command {before:#06x} -> {after:#06x}");
            if after & 0x4 != 0 {
                enabled.push((*handle, loc));
            }
        }
        BridgeMaster { enabled }
    }
}

#[cfg(feature = "tx")]
impl Drop for BridgeMaster {
    fn drop(&mut self) {
        for (handle, loc) in self.enabled.drain(..) {
            // Safety: GetProtocol performs no driver binding.
            let Ok(pci) = (unsafe {
                boot::open_protocol::<PciIo>(
                    OpenProtocolParams { handle, agent: boot::image_handle(), controller: None },
                    OpenProtocolAttributes::GetProtocol,
                )
            }) else {
                info!("BRIDGE-BME: restore {loc:?}: protocol gone");
                continue;
            };
            let now = pci.config_read_u16(0x04);
            pci.cfg_write_u16(0x04, now & !0x4);
            info!("BRIDGE-BME: restored {loc:?} command {now:#06x} -> {:#06x}", pci.config_read_u16(0x04));
        }
    }
}

fn describe_nvidia(pci: &PciIo, seg: usize, bus: usize, dev: usize, fun: usize, device: u16) {
    info!(
        "NVIDIA {} {:04x}:{:02x}:{:02x}.{} device {:04x}",
        if fun == 0 { "GPU?" } else { "fn" },
        seg, bus, dev, fun, device
    );
    let header_type = pci.config_read_u8(0x0e) & 0x7f;
    if header_type != 0x00 {
        info!("  header type {:#x}: not type 0, skip caps walk", header_type);
        return;
    }

    // BARs as programmed by firmware (read-only report; no size probing).
    let mut i = 0u32;
    while i < 6 {
        let lo_off = 0x10 + 4 * i;
        let lo = pci.config_read_u32(lo_off);
        if lo == 0 {
            i += 1;
            continue;
        }
        let is_mem = lo & 1 == 0;
        if is_mem && (lo >> 1 & 3) == 2 {
            // 64-bit BAR: consumes this and the next slot
            let hi = pci.config_read_u32(lo_off + 4);
            info!("  BAR{} 64-bit mem base {:#010x}{:08x}", i, hi, lo & !0xf);
            i += 2;
        } else if is_mem {
            info!("  BAR{} 32-bit mem base {:#010x}", i, lo & !0xf);
            i += 1;
        } else {
            info!("  BAR{} io base {:#06x}", i, lo & !3);
            i += 1;
        }
    }

    // Legacy capability list at 0x34.
    let status = pci.config_read_u16(0x06);
    if status & 0x10 == 0 {
        info!("  no capability list");
    } else {
        let mut cap_ptr = pci.config_read_u8(0x34) & 0xfc;
        while cap_ptr != 0 {
            let cap_id = pci.config_read_u8(cap_ptr.into());
            match cap_id {
                0x10 => {
                    let devcap = pci.config_read_u32(cap_ptr as u32 + 0x04);
                    let link_cap = pci.config_read_u32(cap_ptr as u32 + 0x0c);
                    info!(
                        "  PCIe cap @{:#04x}: DEVCAP {:#010x} (FLR {}) LINKCAP {:#010x}",
                        cap_ptr,
                        devcap,
                        if devcap & (1 << 15) != 0 { "yes" } else { "no" },
                        link_cap
                    );
                }
                other => {
                    info!("  cap {:#04x} @{:#04x}", other, cap_ptr);
                }
            }
            cap_ptr = pci.config_read_u8(cap_ptr as u32 + 1);
        }
    }

    // Extended capability list at 0x100 (ReBAR lives here).
    let mut off: u32 = 0x100;
    loop {
        let hdr = pci.config_read_u32(off);
        let id = (hdr & 0xffff) as u16;
        if id == 0x0000 {
            break; // list terminator
        }
        let next = ((hdr >> 20) & 0xfff) as u32;
        match id {
            0x0015 => {
                let ctrl = pci.config_read_u32(off + 4);
                let n_bars = ctrl & 0x1f;
                info!("  ReBAR cap @{:#06x}: {} resizable BAR(s)", off, n_bars);
                for b in 0..n_bars as u32 {
                    let bc = pci.config_read_u32(off + 8 + 8 * b);
                    let bs_ = pci.config_read_u32(off + 12 + 8 * b);
                    let size_idx = bs_ & 0x1f;
                    // sizes are 1MB << index per spec
                    info!(
                        "    BAR{} ctrl {:#010x} status {:#010x} -> {} MiB (idx {})",
                        bc & 0x1f,
                        bc,
                        bs_,
                        1u64 << (20 + size_idx),
                        size_idx
                    );
                }
            }
            other => info!("  ext cap {:#06x} @{:#06x}", other, off),
        }
        if next == 0 {
            break;
        }
        off = next;
    }
}
