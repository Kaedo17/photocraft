//! PhotoCraft desktop app binary. The app itself is the library (`lib.rs`, [`photocraft::run`]);
//! Android's NativeActivity entry point is the `photocraft-android` crate.
// Release builds on Windows are GUI-subsystem apps, so launching from the Start Menu or Explorer
// doesn't open a console window. (`--version` output then only shows when redirected.)
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

fn main() -> eframe::Result {
    photocraft::run()
}
