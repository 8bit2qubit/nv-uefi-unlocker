//! Embed the build timestamp so stale-artifact mixups are visible on screen.

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let ts = Command::new("cmd")
        .args(["/C", "echo %date% %time%"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let out_dir = env::var("OUT_DIR").unwrap();
    let out = Path::new(&out_dir);
    fs::write(out.join("build_ts.txt"), &ts).unwrap();
    println!("cargo:rustc-env=BUILD_TS={ts}");
    println!("cargo:rerun-if-changed=build.rs");
}
