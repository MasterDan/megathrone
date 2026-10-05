//! Replicates the `desktop` / `mobile` cfg flags tauri-build sets for the
//! app crate, so the library's `#[cfg(desktop)]` gates compile identically
//! no matter which shell (tauri app, web server) pulls it in.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    let mobile = target.contains("android") || target.contains("ios");
    println!("cargo:rustc-check-cfg=cfg(desktop, mobile)");
    if mobile {
        println!("cargo:rustc-cfg=mobile");
    } else {
        println!("cargo:rustc-cfg=desktop");
    }
}
