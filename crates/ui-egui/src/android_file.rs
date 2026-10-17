//! Android's in-app file dialogs: Open, Save and Folder picks without a system picker.
//!
//! Android has no rfd backend, so [`Services::file_dialog`](crate::Services::file_dialog) stays
//! empty there. When the app-private storage directory is known
//! ([`Services::app_files_dir`](crate::Services::app_files_dir), resolved by the host from
//! `AndroidApp::internal_data_path`), [`PhotocraftApp::poll_file_dialog`] serves the commands'
//! dialogs here instead of failing with "no file dialog available": saves open in `Documents/`,
//! and Open and folder picks browse the app-private directory (or shared storage once the
//! "All files access" grant holds; see `apps/photocraft-android`).
//!
//! The browser browses a single [`StorageRoot`] at a time and can never leave it: entries are
//! only ever *chosen* from listings, never typed, and "Up" is clamped to the root's top, so no
//! answer can escape the root the user is browsing. Save answers reduce the typed name to a
//! single file name inside the dialog's folder ([`sanitize_name`]).
//!
//! The whole dialog compiles on every platform — a host enables it by setting
//! [`Services::app_files_dir`](crate::Services::app_files_dir) (the Android host does; desktop
//! hosts leave it `None` and keep their blocking pickers), so the modals and the listing run in
//! the host's own tests.

use std::path::{Path, PathBuf};

use crate::{FileDialogAnswer, FileDialogReply, FileDialogRequest, PhotocraftApp};

/// Where a save lands when no folder was chosen: `Documents/` below the app-private base.
pub fn save_dir(base: &Path) -> PathBuf {
    base.join("Documents")
}

/// A user-typed name reduced to a single file name inside the dialog's folder: surrounding
/// whitespace and any typed directories are dropped so the answer can never escape the base
/// directory. Empty (or separator-only) input falls back to `fallback`.
pub fn sanitize_name(input: &str, fallback: &str) -> String {
    let name = Path::new(input.trim()).file_name().and_then(|n| n.to_str()).unwrap_or("").trim().to_string();
    if name.is_empty() || name == "." || name == ".." { fallback.to_string() } else { name }
}

/// The full save path for a [`FileDialogRequest::Save`] answered with `name` in `dir`.
pub fn save_path_in(dir: &Path, name: &str, fallback: &str) -> PathBuf {
    dir.join(sanitize_name(name, fallback))
}

/// The sub-folders of `dir`, sorted case-insensitively. Unreadable directories and non-directories
/// are skipped, never an error.
pub fn list_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries.flatten().filter_map(|e| e.path().is_dir().then(|| e.path())).collect();
    out.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    out
}

/// The files in `dir` whose extension is in `exts` (case-insensitive; an empty `exts` matches
/// every file), sorted case-insensitively. Directories and unreadable entries are skipped, never
/// an error.
pub fn list_files(dir: &Path, exts: &[String]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let is_file = path.is_file();
            let matches = exts.is_empty() || path.extension().and_then(|x| x.to_str()).is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e)));
            (is_file && matches).then_some(path)
        })
        .collect();
    out.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    out
}

/// Whether `dir` can be listed (it exists and is readable). The browser uses this to tell an
/// unreadable root apart from an empty one and offer the "All files access" grant.
pub fn readable(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok()
}

/// The root the browser is currently inside: the app-private base, or shared internal storage
/// once the "All files access" grant holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageRoot {
    /// The app-private directory (`AndroidApp::internal_data_path`), always browsable.
    App(PathBuf),
    /// Shared internal storage (typically `/storage/emulated/0`), browsable only while the grant
    /// holds.
    Shared(PathBuf),
}

impl StorageRoot {
    /// The directory this root starts at.
    pub fn dir(&self) -> &Path {
        match self {
            StorageRoot::App(base) | StorageRoot::Shared(base) => base,
        }
    }

    /// The directory "Up" leads to, clamped to this root's top: the parent while it stays inside
    /// the root, else the root itself (so the browser can never climb out).
    pub fn up_from(&self, dir: &Path) -> PathBuf {
        let root = self.dir();
        match dir.parent() {
            Some(parent) if parent.starts_with(root) && parent != root => parent.to_path_buf(),
            _ => root.to_path_buf(),
        }
    }
}

/// The in-app dialog in progress. Empty when none is open.
#[derive(Default)]
pub struct State {
    /// The request being served.
    pub(crate) request: Option<FileDialogRequest>,
    /// The reply to answer when done (`None` once answered or abandoned).
    reply: Option<FileDialogReply>,
    /// The app-private base, copied when the dialog opened.
    base: Option<PathBuf>,
    /// The save filename field.
    name: String,
    /// The new-folder field.
    new_name: String,
    /// The folder a save lands in (`None`: [`save_dir`] of the base).
    folder: Option<PathBuf>,
    /// The directory the browser is showing.
    browser: Option<PathBuf>,
    /// The root the browser is clamped to (`None`: the app base).
    root: Option<StorageRoot>,
    /// A file pick's chosen files so far.
    selected: Vec<PathBuf>,
    /// A file pick's extensions to list (empty: every file).
    exts: Vec<String>,
    /// A file pick may choose several files.
    many: bool,
    /// The egui frame the modal was last drawn in. eframe runs both `logic` and `ui` every frame
    /// and both poll the file dialog, so without this the modal would be created twice in one
    /// frame, which egui reports as an ID clash (and would process a click twice).
    drawn_frame: Option<u64>,
}

impl State {
    /// Whether the in-app browser owns a dialog right now (blocks menus and edits like a modal).
    pub fn is_open(&self) -> bool {
        self.request.is_some()
    }
}

/// Serve a file-dialog request with the in-app prompt: stash it, and answer through `reply` once
/// the user is done (on a later frame). `Err`-less: a host without app storage is served nothing
/// (the caller only calls this when [`Services::app_files_dir`](crate::Services::app_files_dir)
/// is set).
pub fn open(app: &mut PhotocraftApp, request: FileDialogRequest, reply: FileDialogReply) {
    let Some(base) = app.services.app_files_dir.clone() else {
        reply.send(Some(FileDialogAnswer::Unavailable("file dialogs need app storage on this platform".into())));
        return;
    };
    if app.android_file.is_open() {
        reply.send(Some(FileDialogAnswer::Unavailable("a file dialog is already open".into())));
        return;
    }
    let (name, exts, many, browser) = match &request {
        FileDialogRequest::Save { suggested } => {
            (Path::new(suggested).file_name().and_then(|n| n.to_str()).unwrap_or(suggested.as_str()).to_string(), Vec::new(), false, None)
        }
        FileDialogRequest::Open { extensions, multiple, initial_dir } => {
            (String::new(), extensions.clone().unwrap_or_default(), *multiple, start_dir(&base, initial_dir.as_deref()))
        }
        FileDialogRequest::Folder { initial_dir } => (String::new(), Vec::new(), false, start_dir(&base, initial_dir.as_deref())),
    };
    app.android_file = State { request: Some(request), reply: Some(reply), base: Some(base), browser, name, many, exts, ..Default::default() };
}

/// The directory the browser opens at: `initial_dir` when it sits inside `base`, else `base`
/// itself.
fn start_dir(base: &Path, initial_dir: Option<&str>) -> Option<PathBuf> {
    let dir = initial_dir.map(PathBuf::from)?;
    let dir = if dir.is_dir() { dir } else { dir.parent().map(Path::to_path_buf)? };
    dir.starts_with(base).then_some(dir)
}

/// Answer the pending request (or Cancel with `None`) and clear the dialog.
fn finish(app: &mut PhotocraftApp, answer: Option<FileDialogAnswer>) {
    if let Some(reply) = app.android_file.reply.take() {
        reply.send(answer);
    }
    app.android_file = State::default();
}

/// Draw the in-app browser for the pending request, if any. Called every frame from
/// [`PhotocraftApp::poll_file_dialog`].
pub fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if app.android_file.request.is_none() {
        return;
    }
    // eframe runs `logic` and `ui` every frame, and both end in `poll_file_dialog`; draw the
    // modal once, from whichever runs first (the second would be an egui ID clash).
    let frame = ctx.cumulative_frame_nr();
    if app.android_file.drawn_frame == Some(frame) {
        return;
    }
    app.android_file.drawn_frame = Some(frame);
    // The wait was abandoned (the pending dialog went away): drop the draft without answering.
    if app.file_dialog.is_none() {
        app.android_file = State::default();
        return;
    }
    let Some(base) = app.android_file.base.clone() else {
        finish(app, None);
        return;
    };
    let Some(request) = app.android_file.request.clone() else {
        return;
    };
    // A browser is open (a file pick, a folder pick, or a save's Location row): it owns the answer.
    if let Some(dir) = app.android_file.browser.clone() {
        browser(app, ctx, &base, dir, request);
        return;
    }
    match request {
        FileDialogRequest::Open { .. } => {
            app.android_file.browser = Some(app.android_file.folder.clone().unwrap_or_else(|| save_dir(&base)));
        }
        FileDialogRequest::Folder { .. } => {
            app.android_file.browser = Some(app.android_file.folder.clone().unwrap_or_else(|| save_dir(&base)));
        }
        FileDialogRequest::Save { suggested } => {
            save_prompt(app, ctx, &base, &suggested);
        }
    }
}

/// The Save prompt: a folder (with a Change… button that opens the browser) and a name field.
fn save_prompt(app: &mut PhotocraftApp, ctx: &egui::Context, base: &Path, suggested: &str) {
    let fallback = Path::new(suggested).file_name().and_then(|n| n.to_str()).unwrap_or("Untitled").to_string();
    let loc = app.android_file.folder.clone().unwrap_or_else(|| save_dir(base));
    // The soft keyboard appears when the field is tapped; winit raises the IME.
    let mut name = std::mem::take(&mut app.android_file.name);
    let mut save = false;
    let mut cancel = false;
    let mut change = false;
    egui::Modal::new(egui::Id::new("android-file-save")).show(ctx, |ui| {
        ui.set_min_width(360.0);
        ui.label(format!("{} {}", tl!("Location:"), loc.to_string_lossy()));
        if ui.button(tl!("Change…")).clicked() {
            change = true;
        }
        ui.add_space(8.0);
        ui.label(tl!("File name:"));
        ui.add(egui::TextEdit::singleline(&mut name).hint_text(&fallback).desired_width(f32::INFINITY));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            save = ui.button(tl!("Save")).clicked();
            cancel = ui.button(tl!("Cancel")).clicked();
        });
    });
    app.android_file.name = name;
    if change {
        app.android_file.folder = Some(loc.clone());
        app.android_file.browser = Some(loc);
    } else if save {
        let path = save_path_in(&loc, &app.android_file.name, &fallback);
        finish(app, Some(FileDialogAnswer::SaveTo(path.to_string_lossy().into_owned())));
    } else if cancel {
        finish(app, None);
    }
}

/// The file/folder browser: enter sub-folders, create one, choose files (a file pick) or the
/// shown directory (a folder pick). Clamped to the current [`StorageRoot`].
fn browser(app: &mut PhotocraftApp, ctx: &egui::Context, base: &Path, dir: PathBuf, request: FileDialogRequest) {
    let shared = app.services.shared_storage_root.clone();
    let granted = app.services.shared_storage_granted.as_ref().is_some_and(|g| g());
    let root = app.android_file.root.clone().unwrap_or_else(|| StorageRoot::App(base.to_path_buf()));
    let is_file = matches!(request, FileDialogRequest::Open { .. });

    let mut enter: Option<PathBuf> = None;
    let mut up = false;
    let mut choose = false;
    let mut cancel = false;
    let mut create = false;
    let mut new_name = std::mem::take(&mut app.android_file.new_name);
    let mut toggle: Option<usize> = None;
    let mut to_shared = false;
    let mut to_app = false;
    let mut ask_grant = false;

    let dirs = list_dirs(&dir);
    let files: Vec<PathBuf> = if is_file { list_files(&dir, &app.android_file.exts) } else { Vec::new() };
    let selected = app.android_file.selected.clone();

    egui::Modal::new(egui::Id::new("android-file-browser")).show(ctx, |ui| {
        ui.set_min_width(400.0);
        // Root switcher: the browsed root highlighted (shared storage only when the host offers it).
        ui.horizontal(|ui| {
            let in_shared = matches!(root, StorageRoot::Shared(_));
            if ui.add(egui::Button::new(tl!("App storage")).selected(!in_shared)).clicked() {
                to_app = true;
            }
            if shared.is_some() {
                let label = if granted { tl!("Shared storage") } else { tl!("Shared storage (grant access)") };
                if ui.add(egui::Button::new(label).selected(in_shared)).clicked() {
                    if granted {
                        to_shared = true;
                    } else {
                        ask_grant = true;
                    }
                }
            }
        });
        // The status toast is painted beneath this modal, so a failure here would look like
        // nothing happened: show it in the location row instead.
        let shown = if app.ui.status.is_empty() { format!("{} {}", tl!("Location:"), dir.to_string_lossy()) } else { app.ui.status.clone() };
        ui.label(shown);
        if dir != root.dir() && ui.button(tl!("Up")).clicked() {
            up = true;
        }
        if dirs.is_empty() && files.is_empty() {
            if matches!(root, StorageRoot::Shared(_)) && !readable(&dir) {
                ui.label(tl!("This folder can't be read yet. Grant PhotoCraft access to all files."));
                if ui.button(tl!("Grant access")).clicked() {
                    ask_grant = true;
                }
            } else {
                ui.label(tl!("(empty)"));
            }
        }
        egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
            for sub in &dirs {
                let label = sub.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                if ui.button(format!("{label}/")).clicked() {
                    enter = Some(sub.clone());
                }
            }
            for (i, file) in files.iter().enumerate() {
                let label = file.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                let mark = if is_file && selected.contains(file) { "✓ " } else { "" };
                if ui.button(format!("{mark}{label}")).clicked() {
                    toggle = Some(i);
                }
            }
        });
        if is_file {
            let n = selected.len();
            ui.label(if n == 0 { tl!("Tap files to select them.").to_string() } else { format!("{n} ") + tl!("selected") });
        } else {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut new_name).hint_text(tl!("New folder")).desired_width(160.0));
                create = ui.button(tl!("Create")).clicked();
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            choose = ui.button(if is_file { tl!("Open") } else { tl!("Choose this folder") }).clicked();
            cancel = ui.button(tl!("Cancel")).clicked();
        });
    });
    app.android_file.new_name = new_name;

    if ask_grant {
        if let Some(ask) = app.services.shared_storage_grant.as_ref() {
            ask();
            app.ui.status = tl!("Allow \"All files access\" for PhotoCraft, then tap Shared storage.").into();
        } else {
            app.ui.status = tl!("This build can't request shared-storage access.").into();
        }
    } else if to_app {
        app.android_file.root = Some(StorageRoot::App(base.to_path_buf()));
        app.android_file.browser = Some(base.to_path_buf());
    } else if to_shared && let Some(sroot) = shared {
        app.android_file.root = Some(StorageRoot::Shared(sroot.clone()));
        app.android_file.browser = Some(sroot);
    } else if let Some(i) = toggle
        && let Some(file) = files.get(i)
    {
        if app.android_file.many {
            if let Some(pos) = app.android_file.selected.iter().position(|f| f == file) {
                app.android_file.selected.remove(pos);
            } else {
                app.android_file.selected.push(file.clone());
            }
        } else {
            let picked = vec![file.to_string_lossy().into_owned()];
            finish(app, Some(FileDialogAnswer::Paths(picked)));
        }
    } else if let Some(sub) = enter {
        app.android_file.browser = Some(sub);
    } else if up {
        app.android_file.browser = Some(root.up_from(&dir));
    } else if create {
        let name = sanitize_name(&app.android_file.new_name, "");
        if name.is_empty() {
            app.ui.status = tl!("Name the new folder first").into();
        } else {
            let made = dir.join(&name);
            match std::fs::create_dir_all(&made) {
                Ok(()) => app.android_file.browser = Some(made),
                Err(e) => app.ui.status = format!("{} {e}", tl!("Could not create the folder:")),
            }
        }
    } else if choose {
        if is_file {
            let picked: Vec<String> = app.android_file.selected.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            if picked.is_empty() {
                app.ui.status = tl!("Select at least one file.").into();
            } else {
                finish(app, Some(FileDialogAnswer::Paths(picked)));
            }
        } else if matches!(request, FileDialogRequest::Save { .. }) {
            // A save's Location browser (Change…): remember the folder and return to the name
            // prompt — a save is answered with `SaveTo`, never with the folder itself.
            app.android_file.folder = Some(dir);
            app.android_file.browser = None;
        } else {
            finish(app, Some(FileDialogAnswer::FolderTo(dir.to_string_lossy().into_owned())));
        }
    } else if cancel {
        finish(app, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_default_to_documents() {
        assert_eq!(save_dir(Path::new("/files")), PathBuf::from("/files/Documents"));
    }

    #[test]
    fn names_cannot_escape_the_dialog_folder() {
        assert_eq!(sanitize_name("Scene.psd", "F.psd"), "Scene.psd");
        assert_eq!(sanitize_name("  spaced .psd  ", "F.psd"), "spaced .psd");
        assert_eq!(sanitize_name("../../evil.psd", "F.psd"), "evil.psd");
        assert_eq!(sanitize_name("/abs/path/x.psd", "F.psd"), "x.psd");
        assert_eq!(sanitize_name("", "Fallback.psd"), "Fallback.psd");
        assert_eq!(sanitize_name("   ", "Fallback.psd"), "Fallback.psd");
        assert_eq!(sanitize_name("..", "Fallback.psd"), "Fallback.psd");
    }

    #[test]
    fn save_paths_stay_inside_the_chosen_folder() {
        let dir = Path::new("/files/Documents");
        assert_eq!(save_path_in(dir, "Frame.png", "F.png"), PathBuf::from("/files/Documents/Frame.png"));
        assert_eq!(save_path_in(dir, "../x.psd", "F.psd"), PathBuf::from("/files/Documents/x.psd"));
    }

    #[test]
    fn up_is_clamped_to_the_root() {
        let app = StorageRoot::App("/files".into());
        assert_eq!(app.dir(), Path::new("/files"));
        assert_eq!(app.up_from(Path::new("/files/Documents/Deep")), PathBuf::from("/files/Documents"));
        assert_eq!(app.up_from(Path::new("/files/Documents")), PathBuf::from("/files"));
        assert_eq!(app.up_from(Path::new("/files")), PathBuf::from("/files"));
        let shared = StorageRoot::Shared("/storage/emulated/0".into());
        assert_eq!(shared.up_from(Path::new("/storage/emulated/0/DCIM/Cam")), PathBuf::from("/storage/emulated/0/DCIM"));
        assert_eq!(shared.up_from(Path::new("/storage/emulated/0")), PathBuf::from("/storage/emulated/0"));
    }

    #[test]
    fn files_list_filtered_by_extension_sorted() {
        let dir = std::env::temp_dir().join(format!("photocraft-android-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("Clip.PSD"), "x").unwrap();
        std::fs::write(dir.join("still.png"), "x").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let psd: Vec<String> = vec!["psd".to_string()];
        assert_eq!(list_files(&dir, &psd), vec![dir.join("Clip.PSD")]);
        // An empty filter matches every file, sorted, still excluding directories.
        assert_eq!(list_files(&dir, &[]), vec![dir.join("Clip.PSD"), dir.join("notes.txt"), dir.join("still.png")]);
        assert!(list_files(&dir.join("missing"), &[]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_browser_lists_only_directories_sorted() {
        let dir = std::env::temp_dir().join(format!("photocraft-android-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zeta")).unwrap();
        std::fs::create_dir_all(dir.join("Alpha")).unwrap();
        std::fs::write(dir.join("a-file.txt"), "x").unwrap();
        assert_eq!(list_dirs(&dir), vec![dir.join("Alpha"), dir.join("zeta")]);
        assert!(list_dirs(&dir.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_dir_only_accepts_one_inside_the_base() {
        let base = std::env::temp_dir().join(format!("photocraft-android-start-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Documents")).unwrap();
        let inside = base.join("Documents").to_string_lossy().into_owned();
        assert_eq!(start_dir(&base, Some(&inside)), Some(base.join("Documents")));
        // Outside the base (a desktop path from recent files) is ignored.
        assert_eq!(start_dir(&base, Some("/etc")), None);
        assert_eq!(start_dir(&base, None), None);
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// End-to-end tests: the app's own `Services` (no `file_dialog`, `app_files_dir` set) drives the
/// prompting and the answering, so these cover `poll_file_dialog`'s routing and the modals
/// together, exactly as the Android host runs them.
#[cfg(test)]
mod ui_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    use super::*;
    use crate::Services;

    /// A file pick's captured answers.
    type Picked = Rc<RefCell<Vec<(String, Vec<u8>)>>>;

    struct Fixture {
        h: Harness<'static, PhotocraftApp>,
        base: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    /// A harness whose `Services` have no system picker but the app-private directory `tag`, with
    /// `ask` queueing the dialog to test before the first frame.
    fn fixture(tag: &str, ask: impl FnOnce(&mut PhotocraftApp) + 'static) -> Fixture {
        let base = std::env::temp_dir().join(format!("photocraft-android-ui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Documents")).unwrap();
        let services = Services { app_files_dir: Some(base.clone()), ..Default::default() };
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), services);
        ask(&mut app);
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).with_step_dt(1.0 / 60.0).build_ui_state(
            |ui, app: &mut PhotocraftApp| {
                let ctx = ui.ctx().clone();
                // Fonts set up after the first frame only apply from the next one.
                if !ctx.fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                    return;
                }
                egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
                // eframe polls the file dialog from both `logic` and `ui`; do the same so the
                // once-per-frame guard is exercised the way the app runs it.
                app.poll_file_dialog(&ctx, None);
                app.poll_file_dialog(&ctx, None);
            },
            app,
        );
        PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
        h.run_steps(4);
        Fixture { h, base }
    }

    #[test]
    fn a_save_prompt_writes_into_documents() {
        let out: Rc<RefCell<Option<String>>> = Rc::default();
        let slot = out.clone();
        let mut f = fixture("save", move |app| {
            app.pick_save("Scene.psd", move |_app, path| {
                *slot.borrow_mut() = Some(path);
                Ok(json!("ok"))
            })
            .unwrap();
        });
        assert!(f.h.state().android_file.is_open(), "the in-app Save prompt is up");
        f.h.get_by_label("Save").click();
        f.h.run_steps(3);
        assert!(!f.h.state().android_file.is_open(), "the prompt closed");
        let expected = f.base.join("Documents/Scene.psd").to_string_lossy().into_owned();
        assert_eq!(out.borrow().as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn a_save_name_cannot_escape_documents() {
        let out: Rc<RefCell<Option<String>>> = Rc::default();
        let slot = out.clone();
        let mut f = fixture("save-escape", move |app| {
            app.pick_save("Scene.psd", move |_app, path| {
                *slot.borrow_mut() = Some(path);
                Ok(json!("ok"))
            })
            .unwrap();
        });
        // Typing a path is reduced to its file name, inside Documents/.
        f.h.state_mut().android_file.name = "../../escape.psd".into();
        f.h.run_steps(1);
        f.h.get_by_label("Save").click();
        f.h.run_steps(3);
        let expected = f.base.join("Documents/escape.psd").to_string_lossy().into_owned();
        assert_eq!(out.borrow().as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn a_saves_change_button_picks_a_folder_then_saves_into_it() {
        let out: Rc<RefCell<Option<String>>> = Rc::default();
        let slot = out.clone();
        let mut f = fixture("save-change", move |app| {
            app.pick_save("Scene.psd", move |_app, path| {
                *slot.borrow_mut() = Some(path);
                Ok(json!("ok"))
            })
            .unwrap();
        });
        std::fs::create_dir_all(f.base.join("Documents/Art")).unwrap();
        f.h.get_by_label("Change…").click();
        // The browser modal does an egui sizing pass on its first frame (invisible/disabled);
        // run past it before interacting, like the fixture does for the opening prompt.
        f.h.run_steps(4);
        f.h.get_by_label("Art/").click();
        f.h.run_steps(2);
        f.h.get_by_label("Choose this folder").click();
        f.h.run_steps(2);
        assert!(f.h.state().android_file.is_open(), "choosing a folder returns to the name prompt, not an answer");
        f.h.get_by_label("Save").click();
        f.h.run_steps(3);
        assert!(!f.h.state().android_file.is_open(), "the save then completed");
        let expected = f.base.join("Documents/Art/Scene.psd").to_string_lossy().into_owned();
        assert_eq!(out.borrow().as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn cancelling_answers_nothing() {
        let out: Rc<RefCell<Option<String>>> = Rc::default();
        let slot = out.clone();
        let mut f = fixture("cancel", move |app| {
            app.pick_save("Scene.psd", move |_app, path| {
                *slot.borrow_mut() = Some(path);
                Ok(json!("ok"))
            })
            .unwrap();
        });
        f.h.get_by_label("Cancel").click();
        f.h.run_steps(3);
        assert!(!f.h.state().android_file.is_open());
        assert!(out.borrow().is_none(), "Cancel runs nothing");
    }

    #[test]
    fn an_open_prompt_lists_and_returns_a_file() {
        let out: Picked = Rc::default();
        let slot = out.clone();
        let mut f = fixture("open", move |app| {
            app.pick_file_bytes_filtered(&["psd"], move |_app, name, bytes| {
                slot.borrow_mut().push((name, bytes));
                Ok(json!("ok"))
            })
            .unwrap();
        });
        std::fs::write(f.base.join("Documents/Scene.psd"), b"pixels").unwrap();
        f.h.run_steps(2);
        f.h.get_by_label("Scene.psd").click();
        f.h.run_steps(1);
        f.h.get_by_label("Open").click();
        f.h.run_steps(3);
        assert!(!f.h.state().android_file.is_open(), "the prompt closed");
        let expected = f.base.join("Documents/Scene.psd").to_string_lossy().into_owned();
        assert_eq!(*out.borrow(), vec![(expected, b"pixels".to_vec())]);
    }

    #[test]
    fn an_open_prompt_hides_other_extensions() {
        let out: Picked = Rc::default();
        let slot = out.clone();
        let mut f = fixture("open-filter", move |app| {
            app.pick_file_bytes_filtered(&["psd"], move |_app, name, bytes| {
                slot.borrow_mut().push((name, bytes));
                Ok(json!("ok"))
            })
            .unwrap();
        });
        std::fs::write(f.base.join("Documents/notes.txt"), b"x").unwrap();
        f.h.run_steps(2);
        assert!(f.h.query_by_label("notes.txt").is_none(), "a .txt is not offered for a .psd pick");
    }

    #[test]
    fn a_folder_prompt_returns_the_browsed_directory() {
        let out: Rc<RefCell<Option<String>>> = Rc::default();
        let slot = out.clone();
        let mut f = fixture("folder", move |app| {
            app.ask_folder(move |_app, path| {
                *slot.borrow_mut() = Some(path);
                Ok(json!("ok"))
            })
            .unwrap();
        });
        f.h.get_by_label("Choose this folder").click();
        f.h.run_steps(3);
        let expected = f.base.join("Documents").to_string_lossy().into_owned();
        assert_eq!(out.borrow().as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn creating_a_folder_browses_into_it() {
        let out: Rc<RefCell<Option<String>>> = Rc::default();
        let slot = out.clone();
        let mut f = fixture("mkdir", move |app| {
            app.ask_folder(move |_app, path| {
                *slot.borrow_mut() = Some(path);
                Ok(json!("ok"))
            })
            .unwrap();
        });
        f.h.state_mut().android_file.new_name = "New".into();
        f.h.run_steps(1);
        f.h.get_by_label("Create").click();
        f.h.run_steps(2);
        assert!(f.base.join("Documents/New").is_dir(), "the folder was created");
        f.h.get_by_label("Choose this folder").click();
        f.h.run_steps(3);
        let expected = f.base.join("Documents/New").to_string_lossy().into_owned();
        assert_eq!(out.borrow().as_deref(), Some(expected.as_str()));
    }
}
