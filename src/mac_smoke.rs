//! Runs AppKit integration checks on the OS main thread; no screen permission or saved settings changes.
#![allow(dead_code, unused_imports)]
#[cfg(target_os = "macos")]
mod anim;
#[cfg(target_os = "macos")]
mod annotate;
#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod card;
#[cfg(target_os = "macos")]
mod hotkeys;
#[cfg(target_os = "macos")]
mod image_io;
#[cfg(target_os = "macos")]
mod longshot;
#[cfg(target_os = "macos")]
mod ocr;
#[cfg(target_os = "macos")]
mod overlay;
#[cfg(target_os = "macos")]
mod pin;
#[cfg(target_os = "macos")]
#[path = "plat/mod.rs"]
mod platform;
#[cfg(target_os = "macos")]
mod plat {
    pub use crate::platform::*;
    pub fn config_dir() -> Option<std::path::PathBuf> {
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-tmp/mac-smoke"))
    }
    pub fn api_key(_: &str) -> String {
        String::new()
    }
}
#[cfg(target_os = "macos")]
mod record;
#[cfg(target_os = "macos")]
mod settings;
#[cfg(target_os = "macos")]
mod settings_ui;
#[cfg(target_os = "macos")]
mod tray;
#[cfg(target_os = "macos")]
mod snap;
#[cfg(target_os = "macos")]
mod toast;
#[cfg(target_os = "macos")]
mod toolbar;
#[cfg(target_os = "macos")]
mod translate;
#[cfg(target_os = "macos")]
mod ui;

#[cfg(not(target_os = "macos"))]
fn main() {}
#[cfg(target_os = "macos")]
fn main() {
    smoke::run();
}
#[cfg(target_os = "macos")]
#[path = "plat/mac/smoke.rs"]
mod smoke;
