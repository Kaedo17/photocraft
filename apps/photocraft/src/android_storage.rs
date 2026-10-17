//! Android's app-private storage, below the `AndroidApp` data directory.
//!
//! Android has no system file picker for this app (rfd has no backend there), so saves, exports
//! and settings default to private storage: no permissions and no Storage Access Framework
//! round-trip, though invisible to the Files app (the in-app browser can additionally reach
//! shared storage once "All files access" is granted; see `apps/photocraft-android`). Pure path
//! logic, unit-tested on the host; the base directory comes from `run_android`
//! (`apps/photocraft/src/lib.rs`).
//!
//! Two folders below the base: `Documents/` (a place for the in-app browser to start and for
//! saves to land) and `config/` (settings, preset stores and crash-recovery autosaves — what
//! [`crate::app_dirs::config_dir`] resolves to on Android).

use std::path::{Path, PathBuf};

/// The storage folders below the app-private base directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Where the in-app browser starts and where saves and exports default to.
    pub documents: PathBuf,
    /// Settings, preset stores and crash-recovery state (the settings directory).
    pub config: PathBuf,
}

/// Split the app-private `base` into its folders.
pub fn layout(base: &Path) -> Layout {
    Layout { documents: base.join("Documents"), config: base.join("config") }
}

/// Create the folders (`Err` names the first failure; the app then keeps that feature off rather
/// than crashing).
pub fn ensure(layout: &Layout) -> Result<(), String> {
    for dir in [&layout.documents, &layout.config] {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.to_string_lossy()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("photocraft-android-storage-{tag}-{}", std::process::id()))
    }

    #[test]
    fn layout_splits_the_base_into_documents_and_config() {
        let base = Path::new("/data/data/ai.storyteller.photocraft/files");
        assert_eq!(layout(base), Layout { documents: base.join("Documents"), config: base.join("config") });
    }

    #[test]
    fn ensure_creates_every_folder() {
        let base = temp_base("ok");
        let _ = std::fs::remove_dir_all(&base);
        let l = layout(&base);
        ensure(&l).unwrap();
        assert!(l.documents.is_dir() && l.config.is_dir());
        // Re-running over existing folders is fine.
        ensure(&l).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }
}
