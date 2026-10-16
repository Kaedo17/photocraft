//! Android entry point for PhotoCraft.
//!
//! Android's `NativeActivity` loads this library and calls [`android_main`] on a dedicated
//! thread. It opens the shared app ([`photocraft::run_android`], the same code the desktop binary
//! runs) and returns when the window closes, ending the activity's main-loop thread.
//!
//! This crate exists because that export must be unmangled, and on edition 2024 the only spelling
//! (`#[unsafe(no_mangle)]`) trips the workspace-forbidden `unsafe_code` lint — see `Cargo.toml`
//! for why the crate carries its own lints instead. It holds no unsafe logic: one export attribute
//! on a safe trampoline.
//!
//! Android limitations (v1): no system file or message dialogs (rfd has no Android backend), so
//! the file hooks stay empty and the commands fail with "no file dialog available"; settings,
//! preset stores and crash recovery need a config directory, which Android has none of yet, so
//! they are skipped.
//!
//! On every other target this crate is empty: only Android's loader ever looks for `android_main`.

/// NativeActivity's main-loop thread entry point (required; see the `android-activity` docs).
/// winit pumps the activity's lifecycle and input events inside [`photocraft::run_android`],
/// which needs the activity's handle to create its event loop; when the activity is destroyed
/// that returns and this returns with it.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: android_activity::AndroidApp) {
    if let Err(e) = photocraft::run_android(app) {
        log::error!("photocraft exited with an error: {e}");
    }
}
