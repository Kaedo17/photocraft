//! A finger on a layer row pans the Layers list; it does not pick the row up (Android).
//!
//! The rows sense drags, so egui hands the row the gesture the scroll area underneath would pan
//! the list with. On a tablet a swipe up the list therefore picked a layer up and dropped it
//! somewhere else (measured). `widgets::touch_row` settles the press — moving before the hold
//! scrolls, holding then moving drags — and this test drives the real panel with Android-style
//! touch input (an `Event::Touch` plus the pointer events egui-winit sends for it).

use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use super::*;

/// Twelve named layers in a 260 pt tall panel: the list is taller than its viewport.
fn harness() -> Harness<'static, PhotocraftApp> {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    for i in 1..=12 {
        app.run("layer.new.layer", json!({"name": format!("row {i}")})).unwrap();
    }
    let mut ready = false;
    let mut h = Harness::builder().with_size(vec2(320.0, 260.0)).build_ui_state(
        move |ui, app: &mut PhotocraftApp| {
            if !ready {
                PhotocraftApp::setup_context(ui.ctx(), theme::ThemeKind::ProMedium);
                ready = true;
                return;
            }
            layers(app, ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

/// The shown document's layers, top of the stack first — the panel's own order.
fn names(h: &Harness<'_, PhotocraftApp>) -> Vec<String> {
    let doc = &h.state().session.active().unwrap().doc;
    crate::layer_tree_ui::panel_rows(h.state(), doc).into_iter().map(|(_, l)| l.name.clone()).collect()
}

fn touch(phase: egui::TouchPhase, pos: egui::Pos2) -> egui::Event {
    egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos, force: Some(0.5) }
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

/// The finger first meant to scroll: it moved before the hold expired, so the list pans and no
/// layer moves — the release that follows must not reorder either.
#[test]
fn a_finger_swipe_on_a_layer_row_scrolls_the_list_and_moves_no_layer() {
    let mut h = harness();
    let before = names(&h);
    assert!(before.len() >= 12, "the harness built the layers: {before:?}");
    let at = h.get_by_role_and_label(Role::Button, "row 10").rect().center();
    assert!(at.y > 0.0 && at.y < 260.0, "the pressed row is on screen: {at:?}");
    // A warm-up pass: egui hit-tests a press against the previous pass's widgets.
    h.run_steps(1);
    h.event(touch(egui::TouchPhase::Start, at));
    h.event(egui::Event::PointerMoved(at));
    h.event(button(at, true));
    h.run_steps(1);
    let mut last = at;
    for k in 1..=5 {
        last = egui::pos2(at.x, at.y - 14.0 * k as f32);
        h.event(touch(egui::TouchPhase::Move, last));
        h.event(egui::Event::PointerMoved(last));
        h.run_steps(1);
    }
    h.event(touch(egui::TouchPhase::End, last));
    h.event(button(last, false));
    h.event(egui::Event::PointerGone);
    h.run_steps(2);

    assert_eq!(names(&h), before, "a swipe never reorders (last command: {:?})", h.state().perf.last_command);
    assert_ne!(h.state().perf.last_command, "layer.moveTo");
    let after = h.get_by_role_and_label(Role::Button, "row 10").rect().center();
    assert!(after.y < at.y - 20.0, "the list itself panned: {} -> {}", at.y, after.y);
}
