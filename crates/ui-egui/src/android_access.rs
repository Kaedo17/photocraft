//! The launch-time ask for Android's "All files access" grant (`MANAGE_EXTERNAL_STORAGE`).
//!
//! Android 11+ hands an app the shared storage tree only after the user allows "All files
//! access" on a system Settings page. The in-app browser has its own "Shared storage (grant
//! access)" row (`android_file`), which is easy to miss on a first launch, so a host that can
//! both check and ask for the grant
//! ([`Services::shared_storage_granted`](crate::Services::shared_storage_granted) and
//! [`Services::shared_storage_grant`](crate::Services::shared_storage_grant), set by the Android
//! entry crate) calls [`maybe_ask`] once the first frames have rendered (from its `started`
//! hook). This module then shows one short prompt:
//!
//! - **Grant access** opens Android's "All files access" page — the same ask the browser's row
//!   makes — so the user can flip the switch and come back to a granted browser;
//! - **Not now** is remembered (`workspace.androidSkipAllFilesAccessPrompt`) and the prompt
//!   stops appearing at launch; the browser's own row still asks on demand.
//!
//! Nothing is asked while the grant already holds, and hosts that cannot check the grant (the
//! desktop) never ask: without `shared_storage_granted`, [`maybe_ask`] does nothing at all, so
//! the whole module is inert off Android. It compiles everywhere, so the modals and the pref
//! round-trip run in the host's own tests.

use serde_json::json;

use crate::PhotocraftApp;

/// The stored "Not now": [`photocraft_engine::prefs::Workspace`]'s field of the same name.
pub(crate) const SKIP_PATH: &str = "workspace.androidSkipAllFilesAccessPrompt";

/// The launch-time prompt's state. Empty on hosts that never ask.
#[derive(Default)]
pub struct State {
    /// The prompt is up (or waiting for its first frame).
    pub(crate) requested: bool,
    /// The egui frame the modal was last drawn in. eframe runs both `logic` and `ui` every frame
    /// and both end in `poll_file_dialog`, so without this the modal would be drawn twice in one
    /// frame, which egui reports as an ID clash (see `android_file::State::drawn_frame`).
    drawn_frame: Option<u64>,
}

impl State {
    /// Whether the prompt owns the window right now (blocks menus and edits like other modals).
    pub fn is_open(&self) -> bool {
        self.requested
    }
}

/// Ask at launch, unless the grant already holds or the user chose "Not now" before. Called
/// once, from the host's `started` hook. `Err`-less: a host that cannot check the grant never
/// asks (no closure to call), and a stored preference that cannot be read falls back to asking.
pub fn maybe_ask(app: &mut PhotocraftApp) {
    match app.services.shared_storage_granted.as_ref() {
        Some(granted) if !granted() => {}
        // No way to check the grant (the desktop, or a build without the JNI): never ask. So is
        // a grant that already holds.
        _ => return,
    }
    if app.session.prefs().workspace.android_skip_all_files_access_prompt {
        return;
    }
    app.android_access.requested = true;
}

/// Draw the launch prompt while it is up. Called every frame from
/// [`PhotocraftApp::poll_file_dialog`], like the in-app browser's modals.
pub fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if !app.android_access.requested {
        return;
    }
    // Drawn once per frame, from whichever of `logic` and `ui` polls first.
    let frame = ctx.cumulative_frame_nr();
    if app.android_access.drawn_frame == Some(frame) {
        return;
    }
    app.android_access.drawn_frame = Some(frame);
    let mut answer = None;
    egui::Modal::new(egui::Id::new("android-all-files-access")).show(ctx, |ui| {
        ui.set_min_width(380.0);
        ui.label(tl!("Allow access to all files?"));
        ui.add_space(6.0);
        ui.label(tl!("PhotoCraft needs \"All files access\" to open and save files in shared storage."));
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            use crate::widgets::{ButtonRole, DialogButton};
            let buttons = [DialogButton::new(ButtonRole::Default, tl!("Grant access"), 130.0), DialogButton::new(ButtonRole::Cancel, tl!("Not now"), 96.0)];
            if let Some(role) = crate::widgets::dialog_buttons(ui, &buttons) {
                answer = Some(role == ButtonRole::Default);
            }
        });
    });
    if let Some(allow) = answer {
        answer_once(app, allow);
    }
}

/// Act on the user's answer: `true` opens Android's "All files access" page (the browser
/// re-checks the grant on its next visit), `false` remembers "Not now" so the launch ask stops.
/// Separate from [`show`] so a test can answer without an egui frame.
pub(crate) fn answer_once(app: &mut PhotocraftApp, allow: bool) {
    app.android_access.requested = false;
    if allow {
        match app.services.shared_storage_grant.as_ref() {
            Some(ask) => {
                ask();
                app.ui.status = tl!("Allow \"All files access\" for PhotoCraft, then tap Shared storage.").into();
            }
            // The browser's own row says the same thing for a build without the JNI.
            None => app.ui.status = tl!("This build can't request shared-storage access.").into(),
        }
    } else if app.session.execute("prefs.set", json!({"path": SKIP_PATH, "value": true})).is_err() {
        // Only loses the "remembered" part: the prompt returns at the next launch.
        log::warn!("android access: couldn't remember \"Not now\"");
    }
}

/// End-to-end tests: the prompt driven through the app's own `Services`, the way the Android host
/// sets them up (a `shared_storage_granted` closure and a `shared_storage_grant` closure).
#[cfg(test)]
mod ui_tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::Services;

    /// A host that can check and ask for the grant: `granted` is what a Settings round-trip would
    /// flip, and every ask lands in the returned counter.
    fn android_services(granted: bool) -> (Services, Rc<Cell<bool>>, Rc<Cell<u32>>) {
        let flag = Rc::new(Cell::new(granted));
        let asks = Rc::new(Cell::new(0));
        let (g, a) = (flag.clone(), asks.clone());
        let services = Services {
            shared_storage_root: Some(std::path::PathBuf::from("/storage/emulated/0")),
            shared_storage_granted: Some(Box::new(move || g.get())),
            shared_storage_grant: Some(Box::new(move || a.set(a.get() + 1))),
            ..Default::default()
        };
        (services, flag, asks)
    }

    fn app(granted: bool) -> (PhotocraftApp, Rc<Cell<bool>>, Rc<Cell<u32>>) {
        let (services, flag, asks) = android_services(granted);
        (PhotocraftApp::new(photocraft_engine::Session::new(), services), flag, asks)
    }

    #[test]
    fn a_missing_grant_asks_at_launch() {
        let (mut app, _, _) = app(false);
        maybe_ask(&mut app);
        assert!(app.android_access.is_open());
    }

    #[test]
    fn a_held_grant_never_asks() {
        let (mut app, _, _) = app(true);
        maybe_ask(&mut app);
        assert!(!app.android_access.is_open());
    }

    #[test]
    fn a_host_that_cannot_check_the_grant_never_asks() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Services::default());
        maybe_ask(&mut app);
        assert!(!app.android_access.is_open(), "the desktop must not show the Android prompt");
    }

    #[test]
    fn not_now_is_remembered_and_stops_the_launch_ask() {
        let (mut app, _, _) = app(false);
        maybe_ask(&mut app);
        assert!(app.android_access.is_open());
        answer_once(&mut app, false);
        assert!(!app.android_access.is_open());
        assert!(app.session.prefs().workspace.android_skip_all_files_access_prompt);
        maybe_ask(&mut app);
        assert!(!app.android_access.is_open(), "the remembered \"Not now\" stops the launch ask");
    }

    #[test]
    fn grant_access_opens_the_system_page_through_the_services() {
        let (mut app, granted, asks) = app(false);
        maybe_ask(&mut app);
        answer_once(&mut app, true);
        assert_eq!(asks.get(), 1, "the ask reaches the host's JNI");
        assert!(!granted.get(), "the grant only holds after the user flips it in Settings");
        assert!(!app.android_access.is_open());
        assert!(!app.session.prefs().workspace.android_skip_all_files_access_prompt);
    }

    /// The prompt drawn in a real frame: the two buttons answer it.
    fn harness(granted: bool) -> (Harness<'static, PhotocraftApp>, Rc<Cell<bool>>, Rc<Cell<u32>>) {
        let (app, flag, asks) = app(granted);
        let mut h = Harness::builder().with_size(egui::vec2(900.0, 420.0)).with_step_dt(1.0 / 60.0).build_ui_state(
            |ui, app: &mut PhotocraftApp| {
                let ctx = ui.ctx().clone();
                // Fonts set up after the first frame only apply from the next one.
                if !ctx.fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                    return;
                }
                egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
                // eframe polls the file dialog from both `logic` and `ui`; do the same so the
                // once-per-frame guard is exercised the way the app runs it.
                if app.android_access.is_open() {
                    app.poll_file_dialog(&ctx, None);
                    app.poll_file_dialog(&ctx, None);
                }
            },
            app,
        );
        PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
        h.run_steps(4);
        maybe_ask(h.state_mut());
        h.run_steps(2);
        (h, flag, asks)
    }

    #[test]
    fn the_prompt_draws_and_not_now_answers_it() {
        let (mut h, _, _) = harness(false);
        assert!(h.state().android_access.is_open(), "the launch prompt is up");
        h.get_by_label("Not now").click();
        h.run_steps(3);
        assert!(!h.state().android_access.is_open(), "the prompt closed");
        assert!(h.state().session.prefs().workspace.android_skip_all_files_access_prompt);
    }

    #[test]
    fn the_prompt_never_draws_on_a_granted_launch() {
        let (h, _, asks) = harness(true);
        assert!(!h.state().android_access.is_open());
        assert_eq!(asks.get(), 0);
    }
}
