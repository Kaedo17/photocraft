//! Android entry point for PhotoCraft.
//!
//! Android's `NativeActivity` loads this library and calls [`android_main`] on a dedicated
//! thread. It opens the shared app ([`photocraft::run_android`], the same code the desktop binary
//! runs) and returns when the window closes, ending the activity's main-loop thread.
//!
//! This crate exists because that export must be unmangled, and on edition 2024 the only spelling
//! (`#[unsafe(no_mangle)]`) trips the workspace-forbidden `unsafe_code` lint — see `Cargo.toml`
//! for why the crate carries its own lints instead. It also holds `android_access`, the one piece
//! of real unsafe in the project: JNI calls to the activity's JVM to check and request the "All
//! files access" grant, whose safe closures are handed to the shared app as
//! [`photocraft::SharedStorageAccess`].
//!
//! Android limitations (v1): no system file or message dialogs (rfd has no Android backend), so
//! Save, Open, exports and folder picks are served by PhotoCraft's own in-app browser — app-private
//! storage always, shared storage (`/storage/emulated/0`) too once the user grants "All files
//! access" (`android_access`); settings, preset stores and crash recovery use that storage's
//! `config/` folder.
//!
//! On every other target this crate is empty: only Android's loader ever looks for `android_main`.

// The workspace's clippy denials, set here because this crate deliberately does not inherit the
// workspace lints (see `Cargo.toml`): Android's NativeActivity ABI needs an unmangled export, and
// edition 2024 spells that `#[unsafe(no_mangle)]`, which the workspace-forbidden `unsafe_code`
// lint rejects. Every other clippy denial is the same as `crates/*`'s.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "android")]
mod android_access;

/// NativeActivity's main-loop thread entry point (required; see the `android-activity` docs).
/// winit pumps the activity's lifecycle and input events inside [`photocraft::run_android`],
/// which needs the activity's handle to create its event loop; when the activity is destroyed
/// that returns and this returns with it.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: android_activity::AndroidApp) {
    // The "All files access" grant, checked and requested over JNI — the entry crate's unsafe
    // stays here; `run_android` only ever sees plain closures (built before the app moves).
    let shared = android_access::capture(&app).map(|a| photocraft::SharedStorageAccess {
        granted: Box::new(move || a.shared_storage_granted()),
        ask: Box::new(move || a.ask_shared_storage_grant()),
    });
    if let Err(e) = photocraft::run_android(app, shared) {
        log::error!("photocraft exited with an error: {e}");
    }
}
