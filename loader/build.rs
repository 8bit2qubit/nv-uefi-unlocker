//! Embed the build timestamp so stale-artifact mixups are visible on screen.

use std::env;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let ts = local_timestamp().unwrap_or_else(utc_timestamp);
    let out_dir = env::var("OUT_DIR").unwrap();
    let out = Path::new(&out_dir);
    fs::write(out.join("build_ts.txt"), &ts).unwrap();
    println!("cargo:rustc-env=BUILD_TS={ts}");
    println!("cargo:rerun-if-changed=build.rs");
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// `Ddd YYYY-MM-DD HH:MM:SS +HH:MM` from local fields and the UTC offset in
/// minutes.
#[allow(clippy::too_many_arguments)]
fn format_local(wday: usize, y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32, off: i64) -> String {
    let sign = if off < 0 { '-' } else { '+' };
    let off = off.abs();
    format!(
        "{} {y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} {sign}{:02}:{:02}",
        WEEKDAYS[wday],
        off / 60,
        off % 60
    )
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct SysTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetLocalTime(st: *mut SysTime);
    fn GetSystemTime(st: *mut SysTime);
}

/// Local time from GetLocalTime; the offset is local minus GetSystemTime,
/// rounded to 15 minutes.
#[cfg(windows)]
fn local_timestamp() -> Option<String> {
    let mut local = SysTime::default();
    let mut utc = SysTime::default();
    // Safety: both calls only fill the SYSTEMTIME they are given.
    unsafe {
        GetLocalTime(&mut local);
        GetSystemTime(&mut utc);
    }
    let minutes = |t: &SysTime| {
        days_from_civil(t.year.into(), t.month.into(), t.day.into()) * 1_440
            + i64::from(t.hour) * 60
            + i64::from(t.minute)
    };
    let off = ((minutes(&local) - minutes(&utc)) as f64 / 15.0).round() as i64 * 15;
    let wday = usize::from(local.day_of_week);
    (wday < 7).then(|| {
        format_local(
            wday,
            local.year.into(),
            local.month.into(),
            local.day.into(),
            local.hour.into(),
            local.minute.into(),
            local.second.into(),
            off,
        )
    })
}

#[cfg(all(unix, target_pointer_width = "64"))]
#[repr(C)]
struct Tm {
    sec: std::ffi::c_int,
    min: std::ffi::c_int,
    hour: std::ffi::c_int,
    mday: std::ffi::c_int,
    mon: std::ffi::c_int,
    year: std::ffi::c_int,
    wday: std::ffi::c_int,
    yday: std::ffi::c_int,
    isdst: std::ffi::c_int,
    gmtoff: std::ffi::c_long,
    zone: *const std::ffi::c_char,
}

#[cfg(all(unix, target_pointer_width = "64"))]
extern "C" {
    fn localtime_r(t: *const i64, result: *mut Tm) -> *mut Tm;
}

/// Local time and UTC offset from localtime_r.
#[cfg(all(unix, target_pointer_width = "64"))]
fn local_timestamp() -> Option<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let mut tm = std::mem::MaybeUninit::<Tm>::uninit();
    // Safety: localtime_r fills `tm` and returns it, or returns null on error.
    let tm = unsafe {
        if localtime_r(&now, tm.as_mut_ptr()).is_null() {
            return None;
        }
        tm.assume_init()
    };
    let wday = usize::try_from(tm.wday).ok().filter(|&w| w < 7)?;
    Some(format_local(
        wday,
        i64::from(tm.year) + 1900,
        u32::try_from(tm.mon + 1).ok()?,
        u32::try_from(tm.mday).ok()?,
        u32::try_from(tm.hour).ok()?,
        u32::try_from(tm.min).ok()?,
        u32::try_from(tm.sec).ok()?,
        i64::from(tm.gmtoff) / 60,
    ))
}

#[cfg(not(any(windows, all(unix, target_pointer_width = "64"))))]
fn local_timestamp() -> Option<String> {
    None
}

/// UTC time, `Ddd YYYY-MM-DD HH:MM:SS UTC`, from the system clock alone.
fn utc_timestamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    // 1970-01-01 was a Thursday.
    let wd = WEEKDAYS[(days + 4).rem_euclid(7) as usize];
    let s = secs % 86_400;
    format!("{wd} {y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", s / 3600, s % 3600 / 60, s % 60)
}

/// Days since 1970-01-01 to (year, month, day) in the proleptic Gregorian
/// calendar (Howard Hinnant's civil_from_days).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// (year, month, day) to days since 1970-01-01, the inverse of
/// civil_from_days (Howard Hinnant's days_from_civil).
#[cfg(windows)]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
