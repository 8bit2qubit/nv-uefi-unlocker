//! Single-key confirmation with timeout (TOTORONG interaction pattern).

use core::time::Duration;
use uefi::proto::console::text::Key;
use uefi::{boot, system};

pub enum KeyOrTimeout {
    Key(Key),
    Timeout,
}

/// Poll the console input roughly every 50 ms.
///
/// M0 semantics: any key proceeds early, timeout proceeds anyway (M0 is
/// read-only). From M3 on, timeout must mean ABORT into chain-load.
pub fn wait_any_key_or_timeout(seconds: u64) -> KeyOrTimeout {
    let ticks = seconds * 20; // 50 ms each
    for _ in 0..ticks {
        let key = system::with_stdin(|stdin| stdin.read_key());
        if let Ok(Some(key)) = key {
            return KeyOrTimeout::Key(key);
        }
        boot::stall(Duration::from_millis(50));
    }
    KeyOrTimeout::Timeout
}
