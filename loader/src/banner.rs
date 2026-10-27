//! Version banner (TOTORONG-style phase banner).

use log::info;

pub fn print(version: &str) {
    info!("==================================================");
    info!(" nv-uefi-unlocker v{version} (built {})", env!("BUILD_TS"));
    #[cfg(feature = "tx")]
    {
        info!(" ** TX BUILD: CONTAINS GPU-WRITE PATHS **");
        info!(" transaction: claim->gfw->fwsec->booter->selectors->flr");
        info!(" SPACE = run; any other key or 30s no-key = ABORT to Windows");
        info!(" armed: \\i_know_what_i_am_doing.txt = 5s auto-run, any key aborts");
        info!(" multi-GPU: no marker = every supported card in order;");
        info!("            \\tx-select-bdf.txt (bb:dd.f) pins exactly one");
        info!(" failure = no retry (per card; the next card still runs)");
    }
    #[cfg(not(feature = "tx"))]
    {
        info!(" read-only build: enumerate + sample + chain-load");
        info!(" console output only; no GPU writes by design");
    }
    info!("==================================================");
    info!("PHASE: CONFIRM");
}
