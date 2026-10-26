//! Version banner (TOTORONG-style phase banner).

use log::info;

pub fn print(version: &str) {
    info!("==================================================");
    info!(" nv-uefi-unlocker v{version} (built {})", env!("BUILD_TS"));
    #[cfg(feature = "tx")]
    {
        info!(" ** TX BUILD: CONTAINS GPU-WRITE PATHS **");
        info!(" transaction: claim->gfw->fwsec->booter->selectors->flr");
        info!(" 30s no-key = ABORT to Windows; failure = no retry");
    }
    #[cfg(not(feature = "tx"))]
    {
        info!(" read-only build: enumerate + sample + chain-load");
        info!(" console output only; no GPU writes by design");
    }
    info!("==================================================");
    info!("PHASE: CONFIRM");
}
