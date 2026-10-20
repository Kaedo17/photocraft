//! `touch_row`: a finger on a draggable row of a scrollable panel scrolls the panel, unless the
//! finger held the row first. Fed real Android-style touch input (an `Event::Touch` plus the
//! emulated pointer events egui-winit sends).

use super::TouchRow;

/// A scrollable area whose rows all sense drags (the way the Layers, Brushes and Actions panels
/// draw theirs) and all consult the arbiter. Every row stacks the drag its own code would have
/// begun (`resp.drag_started()` gated on the arbiter, the pattern the panels use), and the area's
/// saved offset after each frame.
///
/// A press on row 7 is the finger under test.
#[derive(Default)]
struct Bench {
    /// Per frame: would a row have begun its drag?
    would_grab: Vec<bool>,
    /// Per frame: would a row have dragged continuously (the live reorder path)?
    would_drag: Vec<bool>,
    /// The area's saved offset after each frame.
    offset_y: Vec<f32>,
    /// Draw click-only rows (a Button, like the History panel's older states) instead of
    /// drag-sensing rows.
    click_only: bool,
}

impl Bench {
    fn frame(&mut self, ctx: &egui::Context, events: Vec<egui::Event>, at: f64) {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 200.0));
        let input = egui::RawInput { screen_rect: Some(screen), time: Some(at), events, ..Default::default() };
        let mut offset = 0.0;
        let mut my_grab = false;
        let mut my_drag = false;
        let mut run = ctx.run_ui(input, |ui| {
            // The app's own area: `panel_scroll` pans on any drag (`DragScroll::Always`).
            let out = crate::widgets::panel_scroll().id_salt("touch-bench").auto_shrink([false, false]).show(ui, |ui| {
                for i in 0..20 {
                    let r = if self.click_only {
                        ui.add_sized([ui.available_width(), 18.0], egui::Button::new(format!("row {i}")))
                    } else {
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::click_and_drag()).1
                    };
                    // What every panel here does: ask the arbiter, and only act on the row's own
                    // drag when the finger meant the row (a mouse press stays `None`).
                    let answer = crate::widgets::touch_row(ui, &r);
                    let scrolling = answer == TouchRow::Scroll;
                    let grab = r.drag_started() && !scrolling;
                    let drag = r.dragged() && !scrolling;
                    if i == 7 {
                        my_grab = grab;
                        my_drag = drag;
                    }
                }
            });
            offset = out.state.offset.y;
        });
        run.textures_delta.clear();
        self.would_grab.push(my_grab);
        self.would_drag.push(my_drag);
        self.offset_y.push(offset);
    }

    fn grabbed(&self) -> bool {
        self.would_grab.iter().any(|g| *g)
    }

    fn last_offset(&self) -> f32 {
        *self.offset_y.last().unwrap_or(&0.0)
    }
}

fn touch(phase: egui::TouchPhase, pos: egui::Pos2) -> egui::Event {
    egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos, force: Some(0.5) }
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

/// A finger that moves on a draggable row before the hold expires scrolls the list and never
/// starts the row's drag — even though egui hands the row the gesture (the row senses drags, so it
/// is egui's drag target and the area underneath it would otherwise never pan).
#[test]
fn an_early_finger_move_on_a_drag_row_scrolls_instead_of_grabbing() {
    let ctx = egui::Context::default();
    let mut b = Bench::default();
    let at = egui::pos2(120.0, 150.0);
    // A warm-up pass: egui hit-tests a press against the previous pass's widgets.
    b.frame(&ctx, vec![], -0.1);
    b.frame(&ctx, vec![touch(egui::TouchPhase::Start, at), egui::Event::PointerMoved(at), button(at, true)], 0.0);
    for k in 1..=5 {
        let p = egui::pos2(at.x, at.y - 15.0 * k as f32);
        b.frame(&ctx, vec![touch(egui::TouchPhase::Move, p), egui::Event::PointerMoved(p)], 0.1 * k as f64);
    }
    b.frame(&ctx, vec![], 0.6);
    assert!(!b.grabbed(), "a moving finger never begins the row's drag: {:?}", b.would_grab);
    assert!(!b.would_drag.iter().any(|d| *d), "and never reorders live: {:?}", b.would_drag);
    assert!(b.last_offset() > 20.0, "the area really panned: {:?}", b.offset_y);
}

/// A finger held on a draggable row without moving never begins the drag: the row leaves the press
/// to egui, whose own long-press opens the row's context menu. Moving *after* the hold begins the
/// drag, and the area stays put throughout.
#[test]
fn a_still_hold_is_left_to_the_long_press_and_only_a_move_after_it_grabs() {
    let ctx = egui::Context::default();
    let mut b = Bench::default();
    let at = egui::pos2(120.0, 150.0);
    b.frame(&ctx, vec![], -0.1);
    b.frame(&ctx, vec![touch(egui::TouchPhase::Start, at), egui::Event::PointerMoved(at), button(at, true)], 0.0);
    for k in 1..=8 {
        b.frame(&ctx, vec![], 0.1 * k as f64);
    }
    assert!(!b.grabbed(), "a still hold never begins the drag: {:?}", b.would_grab);
    assert_eq!(b.last_offset(), 0.0, "and does not pan the area");
    // Moving while still down, after the hold: the row's drag, and still no pan.
    let p = egui::pos2(at.x, at.y - 30.0);
    b.frame(&ctx, vec![touch(egui::TouchPhase::Move, p), egui::Event::PointerMoved(p)], 1.0);
    assert!(b.grabbed(), "a move after the hold begins the drag: {:?}", b.would_grab);
    b.frame(&ctx, vec![], 1.05);
    assert_eq!(b.last_offset(), 0.0, "a grabbed press does not pan the area");
    // It keeps dragging for the rest of the press, however far the finger travels.
    let q = egui::pos2(at.x, at.y - 60.0);
    b.frame(&ctx, vec![touch(egui::TouchPhase::Move, q), egui::Event::PointerMoved(q)], 1.1);
    assert!(*b.would_drag.last().unwrap_or(&false), "a held finger keeps its drag: {:?}", b.would_drag);
    b.frame(&ctx, vec![], 1.15);
    assert_eq!(b.last_offset(), 0.0, "a grabbed press does not pan the area");
    // Lift.
    b.frame(&ctx, vec![touch(egui::TouchPhase::End, q), button(q, false), egui::Event::PointerGone], 1.2);
    assert!(!*b.would_grab.last().unwrap_or(&true) && !*b.would_drag.last().unwrap_or(&true), "the press is over");
}

/// egui's own long-press opens the row's context menu once the finger has been still past its
/// timeout; a finger that then moves must not pick the row up underneath the open menu.
#[test]
fn a_move_under_the_long_press_menu_does_not_grab() {
    let ctx = egui::Context::default();
    let mut b = Bench::default();
    let at = egui::pos2(120.0, 150.0);
    b.frame(&ctx, vec![], -0.1);
    b.frame(&ctx, vec![touch(egui::TouchPhase::Start, at), egui::Event::PointerMoved(at), button(at, true)], 0.0);
    // Still, past egui's long-press timeout (0.8 s): its "secondary click" now owns the press.
    for k in 1..=9 {
        b.frame(&ctx, vec![], 0.1 * k as f64);
    }
    let p = egui::pos2(at.x, at.y - 30.0);
    b.frame(&ctx, vec![touch(egui::TouchPhase::Move, p), egui::Event::PointerMoved(p)], 1.05);
    assert!(!b.grabbed(), "under the open menu the row does not grab: {:?}", b.would_grab);
    b.frame(&ctx, vec![], 1.1);
    assert_eq!(b.last_offset(), 0.0, "and it does not pan either");
    b.frame(&ctx, vec![touch(egui::TouchPhase::End, p), button(p, false), egui::Event::PointerGone], 1.15);
}

/// Every row of the area asks the arbiter — that is how the panels draw them — but one finger
/// scrolls the area once per frame, never once per row: twenty rows each adding their own copy of
/// the delta would pin the area to its end.
#[test]
fn every_row_asking_still_pans_the_area_once() {
    let ctx = egui::Context::default();
    let mut b = Bench::default();
    let at = egui::pos2(120.0, 150.0);
    b.frame(&ctx, vec![], -0.1);
    b.frame(&ctx, vec![touch(egui::TouchPhase::Start, at), egui::Event::PointerMoved(at), button(at, true)], 0.0);
    for k in 1..=5 {
        let p = egui::pos2(at.x, at.y - 15.0 * k as f32);
        b.frame(&ctx, vec![touch(egui::TouchPhase::Move, p), egui::Event::PointerMoved(p)], 0.1 * k as f64);
    }
    b.frame(&ctx, vec![], 0.6);
    let moved = b.last_offset();
    // The finger travelled 75 px; the area holds 20 rows of 18 px in a 200 px viewport.
    assert!(moved > 20.0, "the area panned: {moved}");
    assert!(moved < 100.0, "one pan per frame, not one per row: {moved}");
}

/// A mouse press on the same row is `None` every frame: the desktop's drag path answers from the
/// response itself, untouched, and the arbiter never pans.
#[test]
fn a_mouse_press_is_not_arbitrated() {
    let ctx = egui::Context::default();
    let mut b = Bench::default();
    let at = egui::pos2(120.0, 150.0);
    // A warm-up pass: egui hit-tests a press against the previous pass's widgets.
    b.frame(&ctx, vec![egui::Event::PointerMoved(at)], -0.1);
    b.frame(&ctx, vec![egui::Event::PointerMoved(at), button(at, true)], 0.0);
    b.frame(&ctx, vec![], 0.6);
    let p = egui::pos2(at.x, at.y - 40.0);
    b.frame(&ctx, vec![egui::Event::PointerMoved(p)], 0.7);
    b.frame(&ctx, vec![button(p, false)], 0.8);
    assert!(b.grabbed(), "a mouse drag still begins at once: {:?}", b.would_grab);
    assert_eq!(b.last_offset(), 0.0, "the arbiter never pans for a mouse: {:?}", b.offset_y);
}

/// A touch press that does not move at all, released quickly, is a plain click: the row keeps
/// answering clicks and nothing grabs or pans.
#[test]
fn a_quick_tap_is_neither_a_grab_nor_a_pan() {
    let ctx = egui::Context::default();
    let mut b = Bench::default();
    let at = egui::pos2(120.0, 150.0);
    b.frame(&ctx, vec![], -0.1);
    b.frame(&ctx, vec![touch(egui::TouchPhase::Start, at), egui::Event::PointerMoved(at), button(at, true)], 0.0);
    b.frame(&ctx, vec![touch(egui::TouchPhase::End, at), button(at, false), egui::Event::PointerGone], 0.08);
    assert!(!b.grabbed(), "a tap never begins a drag: {:?}", b.would_grab);
    assert_eq!(b.last_offset(), 0.0, "a tap never pans");
}

/// A click-only row (a Button, and the History panel's older states) leaves the drag to the scroll
/// area, which pans by itself: the arbiter must answer `None` there and never add a second copy of
/// the finger's delta, or the list would scroll twice as fast over those rows.
#[test]
fn click_only_rows_pan_once_through_the_area_itself() {
    let ctx = egui::Context::default();
    let mut b = Bench { click_only: true, ..Default::default() };
    let at = egui::pos2(120.0, 150.0);
    // Two warm-up passes: the area registers its own drag surface only once it knows its content
    // is taller than the viewport, and a press is hit-tested against the previous pass's widgets.
    b.frame(&ctx, vec![], -0.2);
    b.frame(&ctx, vec![], -0.1);
    b.frame(&ctx, vec![touch(egui::TouchPhase::Start, at), egui::Event::PointerMoved(at), button(at, true)], 0.0);
    for k in 1..=5 {
        let p = egui::pos2(at.x, at.y - 15.0 * k as f32);
        b.frame(&ctx, vec![touch(egui::TouchPhase::Move, p), egui::Event::PointerMoved(p)], 0.1 * k as f64);
    }
    b.frame(&ctx, vec![], 0.6);
    let moved = b.last_offset();
    // The finger travelled 75 px; the area pans by exactly that, once.
    assert!((60.0..=100.0).contains(&moved), "the area panned by the finger's travel, not twice it: {moved}");
}
