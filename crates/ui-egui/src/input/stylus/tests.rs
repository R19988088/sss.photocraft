use super::*;

/// Hold a touch contact still for 1.2 s over a click-sensing area, with the press-and-hold
/// handling the app does in `raw_input_hook`. Did the area see a secondary click?
fn held_contact_right_clicks(hover_first: bool, force: Option<f32>) -> bool {
    let ctx = egui::Context::default();
    let mut contacts = PenContacts::default();
    let mut right_clicked = false;
    let ev = |phase, force| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(3), phase, pos: egui::pos2(50.0, 50.0), force };
    // A first frame lays the area out, so the press hits it.
    let mut frames = vec![(0.0, vec![])];
    if hover_first {
        frames.push((0.05, vec![ev(egui::TouchPhase::Move, None), egui::Event::PointerMoved(egui::pos2(50.0, 50.0))]));
    }
    frames.push((
        0.1,
        vec![
            ev(egui::TouchPhase::Start, force),
            egui::Event::PointerMoved(egui::pos2(50.0, 50.0)),
            egui::Event::PointerButton { pos: egui::pos2(50.0, 50.0), button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
        ],
    ));
    for t in [0.4, 0.7, 1.0, 1.3] {
        frames.push((t, vec![]));
    }
    for (time, mut events) in frames {
        // egui-winit emits the touch and its emulated pointer events together.
        let mut raw =
            egui::RawInput { time: Some(time), screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 200.0))), ..Default::default() };
        raw.events.append(&mut events);
        let pen = contacts.observe(&raw.events);
        pen_hold_is_no_right_click(&ctx, pen);
        let mut out = ctx.run_ui(raw, |ui| {
            let r = ui.allocate_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 200.0)), egui::Sense::click_and_drag());
            right_clicked |= r.secondary_clicked();
        });
        out.textures_delta.clear();
    }
    right_clicked
}

#[test]
fn a_held_pen_is_not_a_right_click_but_a_held_finger_still_is() {
    assert!(held_contact_right_clicks(false, None), "a finger's press-and-hold is egui's context menu");
    assert!(!held_contact_right_clicks(true, None), "a pen that hovered first");
    assert!(!held_contact_right_clicks(false, Some(0.4)), "a contact that reports pressure");
}

#[test]
fn pen_contacts_follow_hover_force_and_lift() {
    use egui::TouchPhase::{Cancel, End, Move, Start};
    let t = |id, phase, force| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(id), phase, pos: egui::pos2(1.0, 1.0), force };
    let mut c = PenContacts::default();
    assert!(!c.observe(&[t(1, Move, None)]), "hovering is no contact");
    assert!(c.observe(&[t(1, Start, None)]));
    assert!(c.observe(&[]), "still down");
    assert!(c.observe(&[t(1, End, None)]), "the lift frame still counts");
    assert!(!c.observe(&[]));
    // A finger, until it reports a force.
    assert!(!c.observe(&[t(9, Start, None), t(9, Move, None)]));
    assert!(c.observe(&[t(9, Move, Some(0.5))]));
    assert!(c.observe(&[t(9, Cancel, None)]));
    assert!(!c.observe(&[]));
}

fn touch(phase: egui::TouchPhase, force: Option<f32>) -> egui::Event {
    egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(0), phase, pos: egui::pos2(1.0, 1.0), force }
}

fn pen(pressure: f32) -> PenSample {
    PenSample { pressure, ..Default::default() }
}

/// The pen's own positions (WinTab packets carry them) come with the frame, oldest first and
/// stamped with the frame's clock, so the canvas can lay a stroke along the pen's samples rather
/// than the pointer's one move a frame. A mouse, or a backend with no position, gives none.
#[test]
fn the_frames_pen_positions_come_with_their_times() {
    let mut s = Stylus { optimized: true, ..Default::default() };
    s.clock_ms = 500.0;
    let base = pen_clock_ms();
    s.feed.set_at(PenSample { pressure: 0.3, pos: Some([100.0, 200.0]), ..Default::default() }, base - 8.0);
    s.feed.set_at(PenSample { pressure: 0.5, pos: Some([140.0, 210.0]), ..Default::default() }, base - 4.0);
    s.feed.set_at(PenSample { pressure: 0.7, pos: Some([180.0, 220.0]), ..Default::default() }, base);
    s.update(&[]);
    let track = s.pen_track();
    assert_eq!(track.len(), 3, "every sample of the frame, not only the last");
    assert_eq!(track[0].0, [100.0, 200.0]);
    assert_eq!(track[2].0, [180.0, 220.0]);
    assert!(track[2].1 > track[1].1 && track[1].1 > track[0].1, "oldest first: {track:?}");
    assert!((track[2].1 - 500.0).abs() < 1e-6, "the newest sample is the frame's own: {track:?}");
    // A sample without a position is no position, and a mouse has none at all.
    let mut plain = Stylus { optimized: true, ..Default::default() };
    plain.clock_ms = 500.0;
    plain.feed.set_at(pen(0.4), pen_clock_ms());
    plain.update(&[]);
    assert!(plain.pen_track().is_empty());
}

#[test]
fn the_feed_queues_every_sample_in_order_and_keeps_the_latest() {
    let feed = StylusFeed::default();
    for p in [0.1, 0.2, 0.3] {
        feed.set(Some(pen(p)));
    }
    assert_eq!(feed.get().map(|s| s.pressure), Some(0.3));
    assert_eq!(feed.drain().iter().map(|(s, _)| s.pressure).collect::<Vec<_>>(), vec![0.1, 0.2, 0.3]);
    assert!(feed.drain().is_empty(), "drained once");
    feed.set(None);
    assert_eq!(feed.get(), None, "a lifted pen is not queued");
    assert!(feed.drain().is_empty());
    // A UI that stops draining keeps only the newest samples.
    for i in 0..FEED_QUEUE + 10 {
        feed.set(Some(pen((i % 100) as f32 / 100.0)));
    }
    assert_eq!(feed.drain().len(), FEED_QUEUE);
}

#[test]
fn the_tablet_optimization_is_off_by_default_and_a_frames_moves_then_split_by_index() {
    // Off (Preferences › Tablet › Tablet optimization), the earlier pairing: a frame's `n` moves
    // split its `m` samples evenly by index, whatever times the pen reported them at.
    let mut s = Stylus::default();
    assert!(!s.optimized, "the optimization is opt-in");
    s.clock_ms = 100.0;
    for (p, at) in [(0.2, 84.0), (0.5, 90.0), (0.8, 96.0)] {
        s.feed.set_at(pen(p), at);
    }
    s.update(&[]);
    let pressure = |s: &mut Stylus, k: usize| {
        s.select(k, 3);
        s.pressure()
    };
    assert!((pressure(&mut s, 0) - 0.2).abs() < 1e-6, "{}", pressure(&mut s, 0));
    assert!((pressure(&mut s, 1) - 0.5).abs() < 1e-6, "{}", pressure(&mut s, 1));
    assert!((pressure(&mut s, 2) - 0.8).abs() < 1e-6, "{}", pressure(&mut s, 2));
}

#[test]
fn a_move_takes_the_pressure_the_pen_had_at_its_own_time() {
    // The pen reports at its own rate; the frame's moves are spread over the frame's time. Each
    // move takes the sample interpolated at its time, not an index's share of the frame — the
    // pairing the old `select` guessed, which gave a fast move a slower sample's pressure.
    let mut s = Stylus { optimized: true, ..Default::default() };
    s.clock_ms = 100.0;
    for (p, at) in [(0.2, 84.0), (0.5, 90.0), (0.8, 96.0)] {
        s.feed.set_at(pen(p), at);
    }
    s.update(&[]);
    // The newest sample is this frame's end, so the three sit at 88, 94 and 100 ms.
    let at = |s: &mut Stylus, t: f64| {
        s.clock_ms = t;
        s.select(0, 1);
        s.pressure()
    };
    assert!((at(&mut s, 91.0) - 0.35).abs() < 1e-6, "{}", s.pressure());
    assert!((at(&mut s, 97.0) - 0.65).abs() < 1e-6, "{}", s.pressure());
    assert!((at(&mut s, 100.0) - 0.8).abs() < 1e-6, "{}", s.pressure());
    // A time past the frame's samples holds the last one, never a share of a later frame's.
    assert!((at(&mut s, 120.0) - 0.8).abs() < 1e-6);
}

#[test]
fn the_pressure_curve_reshapes_pen_pressure_but_not_the_mouse() {
    let mut s = Stylus::default();
    s.set_pressure_curve(&[[0.0, 0.0], [0.3, 0.6], [1.0, 1.0]]);
    s.feed.set(Some(pen(0.3)));
    s.update(&[]);
    assert!((s.pressure() - 0.6).abs() < 1e-5, "{}", s.pressure());
    s.set_pressure_curve(&[[0.0, 0.0], [1.0, 1.0]]);
    assert!((s.pressure() - 0.3).abs() < 1e-6, "linear again");
    // A mouse is full pressure whatever the curve.
    let mut m = Stylus::default();
    m.set_pressure_curve(&[[0.0, 0.0], [1.0, 0.2]]);
    m.update(&[egui::Event::PointerMoved(egui::pos2(1.0, 1.0))]);
    assert_eq!(m.pressure(), 1.0);
}

#[test]
fn a_second_update_in_the_same_frame_keeps_its_samples() {
    // The canvas runs once per view, and egui may run a frame in several passes.
    let mut s = Stylus { optimized: true, ..Default::default() };
    s.feed.set(Some(pen(0.4)));
    s.feed.set(Some(pen(0.8)));
    s.update_for_frame(7, &[]);
    s.update_for_frame(7, &[]);
    // A time before the frame's samples is the first of them (both were kept, not drained twice).
    s.clock_ms = -1.0;
    s.select(0, 2);
    assert!((s.pressure() - 0.4).abs() < 1e-6, "{}", s.pressure());
    // The frame's end is the last of them.
    s.clock_ms = 0.0;
    s.select(0, 2);
    assert!((s.pressure() - 0.8).abs() < 1e-6, "{}", s.pressure());
}

#[test]
fn a_new_stroke_never_starts_from_an_old_strokes_pressure() {
    let mut s = Stylus { optimized: true, ..Default::default() };
    s.feed.set(Some(pen(0.9)));
    s.update(&[]);
    s.feed.set(Some(pen(0.1)));
    s.update(&[]);
    s.begin_stroke();
    s.select(0, 2);
    assert!((s.pressure() - 0.1).abs() < 1e-6, "{}", s.pressure());
}

#[test]
fn a_mouse_or_use_pressure_off_paints_at_full_pressure() {
    let mut s = Stylus::default();
    s.update(&[egui::Event::PointerMoved(egui::pos2(1.0, 1.0))]);
    s.select(0, 1);
    assert_eq!(s.pressure(), 1.0);
    s.feed.set(Some(pen(0.3)));
    s.use_pressure = false;
    s.update(&[]);
    s.select(0, 1);
    assert_eq!(s.pressure(), 1.0);
}

#[test]
fn rotation_between_samples_takes_the_short_way_round() {
    let mut s = Stylus { optimized: true, ..Default::default() };
    s.clock_ms = 10.0;
    s.feed.set_at(PenSample { rotation: 350.0, ..pen(1.0) }, pen_clock_ms());
    s.update(&[]);
    s.clock_ms = 30.0;
    s.feed.set_at(PenSample { rotation: 10.0, ..pen(1.0) }, pen_clock_ms());
    s.update(&[]);
    // Halfway between the two frames' samples (10 ms and 30 ms).
    s.clock_ms = 20.0;
    s.select(0, 2);
    let r = s.sample().map_or(-1.0, |p| p.rotation);
    assert!(r.abs() < 0.5 || (r - 360.0).abs() < 0.5, "halfway from 350° to 10° is 0°, got {r}");
}

#[test]
fn mouse_is_full_pressure() {
    let mut s = Stylus::default();
    s.update(&[egui::Event::PointerMoved(egui::pos2(3.0, 4.0))]);
    assert_eq!(s.pressure(), 1.0);
    assert_eq!(s.sample(), None);
}

#[test]
fn touch_force_drives_pressure_until_lift() {
    let mut s = Stylus::default();
    s.update(&[touch(egui::TouchPhase::Start, Some(0.25))]);
    assert_eq!(s.pressure(), 0.25);
    s.update(&[touch(egui::TouchPhase::Move, Some(0.8)), touch(egui::TouchPhase::Move, Some(1.7))]);
    assert_eq!(s.pressure(), 1.0, "clamped");
    s.update(&[touch(egui::TouchPhase::Move, Some(f32::NAN))]);
    assert_eq!(s.pressure(), 1.0);
    s.update(&[touch(egui::TouchPhase::Move, Some(0.4))]);
    assert_eq!(s.pressure(), 0.4);
    // The lift frame still paints at the last force; the next frame is back to the mouse.
    s.update(&[touch(egui::TouchPhase::End, None)]);
    assert_eq!(s.pressure(), 0.4);
    s.update(&[]);
    assert_eq!(s.pressure(), 1.0);
}

fn touch_id(id: u64, phase: egui::TouchPhase, force: Option<f32>) -> egui::Event {
    egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(id), phase, pos: egui::pos2(1.0, 1.0), force }
}

#[test]
fn a_light_windows_pen_tap_is_not_full_pressure() {
    use egui::TouchPhase::{End, Move, Start};
    // winit on Windows reports a pen pressed below its first pressure level with no force
    // (pressure 0 maps to `None`). The pen hovered first, so the tap is pressure 0, not a
    // mouse click's 1.
    let mut s = Stylus::default();
    s.update(&[touch_id(5, Move, None)]);
    assert_eq!(s.pressure(), 1.0, "hovering is no contact");
    s.update(&[touch_id(5, Start, None), touch_id(5, End, None)]);
    assert_eq!(s.pressure(), 0.0);
    assert_eq!(s.tap_pressure(), 0.0);
    s.update(&[]);
    assert_eq!(s.pressure(), 1.0, "lifted");
    // A pen known from an earlier force: a stroke pressed before its first level starts at 0
    // and then follows the pen.
    let mut s = Stylus::default();
    s.update(&[touch_id(7, Start, Some(0.5)), touch_id(7, End, None)]);
    s.update(&[]);
    s.update(&[touch_id(8, Start, None)]);
    assert_eq!(s.pressure(), 0.0);
    s.update(&[touch_id(8, Move, Some(0.3))]);
    assert_eq!(s.pressure(), 0.3);
    // A force-less move mid-stroke keeps the last force.
    s.update(&[touch_id(8, Move, None)]);
    assert_eq!(s.pressure(), 0.3);
}

#[test]
fn a_finger_without_force_still_paints_at_full_pressure() {
    use egui::TouchPhase::{End, Move, Start};
    let mut s = Stylus::default();
    s.update(&[touch_id(1, Start, None), touch_id(1, Move, None)]);
    assert_eq!(s.pressure(), 1.0);
    s.update(&[touch_id(1, End, None)]);
    s.update(&[]);
    assert_eq!(s.pressure(), 1.0);
}

#[test]
fn a_new_tap_in_the_frame_an_old_one_lifted_keeps_its_force() {
    use egui::TouchPhase::{End, Start};
    let mut s = Stylus::default();
    s.update(&[touch_id(1, Start, Some(0.6)), touch_id(1, End, None), touch_id(2, Start, Some(0.2))]);
    assert_eq!(s.pressure(), 0.2);
    s.update(&[]);
    assert_eq!(s.pressure(), 0.2, "the second contact is still down");
}

#[test]
fn feed_wins_and_is_sanitized() {
    let mut s = Stylus::default();
    s.update(&[touch(egui::TouchPhase::Start, Some(0.3))]);
    s.feed.set(Some(PenSample { pressure: 0.6, tilt_x: 120.0, tilt_y: -30.0, rotation: -90.0, eraser: false, pos: None }));
    assert_eq!(s.sample(), Some(PenSample { pressure: 0.6, tilt_x: 90.0, tilt_y: -30.0, rotation: 270.0, eraser: false, pos: None }));
    s.feed.set(None);
    assert_eq!(s.pressure(), 0.3);
}

#[test]
fn pen_samples_reach_the_brush_stroke() {
    use crate::canvas::{ToolEvent, tool_event};
    use serde_json::json;
    let mut app = crate::PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.execute("file.new", json!({"width": 120, "height": 80})).unwrap();
    app.session.execute("layer.new.layer", json!({})).unwrap();
    app.ui.tool = crate::state::Tool::Brush;
    let m = egui::Modifiers::NONE;
    app.stylus.feed.set(Some(PenSample { pressure: 0.2, tilt_x: 40.0, tilt_y: 0.0, rotation: 30.0, eraser: false, pos: None }));
    let pr = app.stylus.pressure();
    tool_event(&mut app, ToolEvent::Down { x: 10.0, y: 40.0, pressure: pr }, m);
    app.stylus.feed.set(Some(PenSample { pressure: 0.9, tilt_x: 10.0, tilt_y: -5.0, rotation: 60.0, eraser: false, pos: None }));
    let pr = app.stylus.pressure();
    tool_event(&mut app, ToolEvent::Move { x: 100.0, y: 40.0, pressure: pr }, m);
    tool_event(&mut app, ToolEvent::Up { x: 100.0, y: 40.0 }, m);
    let (id, p) = app.session.journal.iter().rev().find(|(id, _)| id == "paint.stroke").cloned().unwrap();
    assert_eq!(id, "paint.stroke");
    assert_eq!(p["points"][0], json!([10.0, 40.0, 0.2f32 as f64, 40.0, 0.0, 30.0]));
    assert_eq!(p["points"][1], json!([100.0, 40.0, 0.9f32 as f64, 10.0, -5.0, 60.0]));
    // A mouse stroke stays at full pressure with plain [x, y, p] points.
    app.stylus.feed.set(None);
    let pr = app.stylus.pressure();
    tool_event(&mut app, ToolEvent::Down { x: 10.0, y: 60.0, pressure: pr }, m);
    tool_event(&mut app, ToolEvent::Up { x: 90.0, y: 60.0 }, m);
    let (_, p) = app.session.journal.iter().rev().find(|(id, _)| id == "paint.stroke").cloned().unwrap();
    assert_eq!(p["points"][0], json!([10.0, 60.0, 1.0]));
}

#[test]
fn stroke_points_carry_tilt_only_when_present() {
    let mut s = Stylus::default();
    let pts = [[0.0, 0.0, 0.5], [10.0, 0.0, 0.7], [20.0, 0.0, 0.7]];
    s.begin_stroke();
    s.record_point();
    assert_eq!(s.stroke_points(&pts)[1], vec![10.0, 0.0, 0.7]);
    s.feed.set(Some(PenSample { pressure: 0.7, tilt_x: 30.0, tilt_y: 10.0, rotation: 45.0, eraser: false, pos: None }));
    s.record_point();
    let out = s.stroke_points(&pts);
    assert_eq!(out[0], vec![0.0, 0.0, 0.5, 0.0, 0.0, 0.0]);
    assert_eq!(out[2], vec![20.0, 0.0, 0.7, 30.0, 10.0, 45.0]);
}

#[test]
fn use_pressure_off_ignores_pen_and_touch() {
    let mut s = Stylus { use_pressure: false, ..Default::default() };
    s.update(&[touch(egui::TouchPhase::Start, Some(0.3))]);
    s.feed.set(Some(PenSample { pressure: 0.2, tilt_x: 40.0, ..Default::default() }));
    assert_eq!((s.sample(), s.pressure()), (None, 1.0));
    s.begin_stroke();
    assert_eq!(s.stroke_points(&[[1.0, 2.0, 1.0]]), vec![vec![1.0, 2.0, 1.0]], "no tilt either");
}

#[test]
fn a_lift_does_not_raise_the_pressure_while_the_pen_thins() {
    // The stroke's samples go through the tablet response; its window (an older file's five
    // samples) still holds the coarser readings just before the lift. Reading that at the lift
    // made the tail's pressure jump up as the pen thinned away.
    use photocraft_paint::TabletPressureSettings;
    let mut s = Stylus { optimized: true, ..Default::default() };
    s.set_tablet_response(TabletPressureSettings { pressure_window: 5, ..Default::default() });
    s.clock_ms = 100.0;
    s.begin_stroke();
    s.begin_pressure_filter();
    for (i, p) in [0.8, 0.7, 0.6, 0.2].into_iter().enumerate() {
        s.clock_ms = 100.0 + i as f64 * 10.0;
        s.feed.set_at(pen(p), 330.0 + i as f64 * 10.0);
        s.update(&[]);
    }
    let lifted = s.release_pressure();
    // The pen's own last reading maps the same way the stroke's samples do (elasticity 6 turns
    // 0.2 into about 0.047), not the window's mean of the coarser samples above it (about 0.22).
    assert!(lifted < 0.1, "the lift reads the pen's own reading, got {lifted}");
    assert!(lifted < s.pressure() - 0.05, "the lift must not be above the stroke's windowed pressure");
}

#[test]
fn the_lift_keeps_the_pressure_the_pen_left_with() {
    // A stroke's last moves come in the frame the pen lifts in: the pen has reported nothing new,
    // and its lift has no pressure of its own (ProCraft reads the pen's own last reading there).
    // They keep the pressure the pen left with — never the mouse's full 1, which made every stroke
    // end in a sudden fat dab — and the release carries it too.
    let mut s = Stylus { optimized: true, ..Default::default() };
    s.clock_ms = 100.0;
    s.begin_stroke();
    s.feed.set_at(pen(0.35), 92.0);
    s.update(&[]);
    s.feed.set(None);
    s.clock_ms = 116.0;
    s.update(&[]);
    s.select(0, 1);
    assert!((s.pressure() - 0.35).abs() < 1e-6, "the lift frame's moves: {}", s.pressure());
    assert!((s.release_pressure() - 0.35).abs() < 1e-6, "the release: {}", s.release_pressure());
    // A mouse (no pen at all) still releases at 1.
    let mut m = Stylus::default();
    m.update(&[egui::Event::PointerMoved(egui::pos2(1.0, 1.0))]);
    assert_eq!(m.release_pressure(), 1.0);
    // The stroke is over: the next frame's pen-less events are a mouse's again.
    s.finish_pressure_filter();
    s.clock_ms = 130.0;
    s.update(&[]);
    s.select(0, 1);
    assert_eq!(s.pressure(), 1.0);
    assert_eq!(s.release_pressure(), 1.0);
}

#[test]
fn the_tablet_response_smooths_every_sample_of_a_frame() {
    // The response's window is over the pen's own samples (C3's pressure window), not over frames:
    // a 240 Hz pen is smoothed over a few milliseconds, so the pressure follows the pen to its
    // lift instead of trailing it by the frame rate (a 60 Hz window held the tail's thick
    // pressure for a fifth of a second).
    let mut s = Stylus { optimized: true, ..Default::default() };
    s.set_tablet_response(photocraft_paint::TabletPressureSettings {
        enabled: true,
        dead_zone: 0.6,
        pressure_window: 3,
        ..Default::default()
    });
    s.begin_pressure_filter();
    s.clock_ms = 100.0;
    for i in 0..5 {
        s.feed.set_at(pen(1.0), 90.0 + i as f64);
    }
    s.update(&[]);
    assert!((s.pressure() - 1.0).abs() < 1e-3, "{}", s.pressure());
    // Five light samples in one frame fall under the dead zone, so all five reach the filter
    // and its whole window empties: the pressure is 0, not a frame's single sample's share.
    s.clock_ms = 116.0;
    for i in 0..5 {
        s.feed.set_at(pen(0.5), 108.0 + i as f64);
    }
    s.update(&[]);
    s.select(0, 1);
    assert_eq!(s.pressure(), 0.0, "the frame's samples did not all reach the filter");
}

#[test]
fn a_stroke_ends_at_the_pressure_the_pen_lifted_with() {
    use crate::canvas::{ToolEvent, tool_event};
    use serde_json::json;
    let mut app = crate::PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.execute("file.new", json!({"width": 120, "height": 80})).unwrap();
    app.session.execute("layer.new.layer", json!({})).unwrap();
    app.ui.tool = crate::state::Tool::Brush;
    app.stylus.optimized = true;
    let m = egui::Modifiers::NONE;
    let mut frame = |app: &mut crate::PhotocraftApp, at: f64, pressure: Option<f32>| {
        app.stylus.clock_ms = at;
        match pressure {
            Some(p) => app.stylus.feed.set(Some(PenSample { pressure: p, ..Default::default() })),
            None => app.stylus.feed.set(None),
        }
        app.stylus.update(&[]);
    };
    frame(&mut app, 0.0, Some(0.8));
    let p0 = app.stylus.pressure();
    tool_event(&mut app, ToolEvent::Down { x: 10.0, y: 40.0, pressure: p0 }, m);
    frame(&mut app, 16.0, Some(0.5));
    let p1 = app.stylus.pressure();
    tool_event(&mut app, ToolEvent::Move { x: 60.0, y: 40.0, pressure: p1 }, m);
    app.stylus.record_point();
    // The pen thinned to 0.3 and lifted where it was: the stroke's last point takes the lift.
    frame(&mut app, 32.0, Some(0.3));
    let p2 = app.stylus.pressure();
    tool_event(&mut app, ToolEvent::Move { x: 100.0, y: 40.0, pressure: p2 }, m);
    app.stylus.record_point();
    frame(&mut app, 48.0, None);
    tool_event(&mut app, ToolEvent::Up { x: 100.0, y: 40.0 }, m);
    let p = app.session.journal.iter().rev().find(|(id, _)| id == "paint.stroke").map(|(_, p)| p.clone()).unwrap();
    let last = p["points"].as_array().unwrap().last().unwrap();
    assert!((last[2].as_f64().unwrap() - 0.3).abs() < 1e-6, "the lift painted at {}", last[2]);
}

#[test]
fn eraser_end_switches_tools_once_and_never_mid_drag() {
    use crate::canvas::{ToolEvent, tool_event};
    use crate::state::Tool;
    let mut app = crate::PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.execute("file.new", serde_json::json!({"width": 60, "height": 60})).unwrap();
    app.ui.tool = Tool::Brush;
    let pen = |eraser| Some(PenSample { pressure: 0.5, eraser, ..Default::default() });
    // A mouse never switches.
    assert!(!Stylus::sync_eraser_tool(&mut app));
    app.stylus.feed.set(pen(true));
    assert!(Stylus::sync_eraser_tool(&mut app));
    assert_eq!(app.ui.tool, Tool::Eraser);
    assert!(!Stylus::sync_eraser_tool(&mut app), "reported once");
    // Flipping back during a drag waits for the drag to end.
    tool_event(&mut app, ToolEvent::Down { x: 10.0, y: 10.0, pressure: 0.5 }, egui::Modifiers::NONE);
    app.stylus.feed.set(pen(false));
    assert!(!Stylus::sync_eraser_tool(&mut app));
    assert_eq!(app.ui.tool, Tool::Eraser);
    tool_event(&mut app, ToolEvent::Up { x: 20.0, y: 10.0 }, egui::Modifiers::NONE);
    assert!(Stylus::sync_eraser_tool(&mut app));
    assert_eq!(app.ui.tool, Tool::Brush, "the tip restores the previous tool");
    // The eraser end while the Eraser is already chosen remembers nothing to restore.
    app.ui.tool = Tool::Eraser;
    app.stylus.feed.set(pen(true));
    assert!(!Stylus::sync_eraser_tool(&mut app));
    app.stylus.feed.set(pen(false));
    assert!(!Stylus::sync_eraser_tool(&mut app));
    assert_eq!(app.ui.tool, Tool::Eraser);
}

/// Windows reports the pen's end through `Services::pen_end`, not the feed (#2285): flipping the
/// pen switches tools the same way, and a feed sample (automation, another backend) wins.
#[test]
fn the_platform_pen_end_switches_to_the_eraser_and_back() {
    use crate::state::Tool;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU8, Ordering};
    let mut app = crate::PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.execute("file.new", serde_json::json!({"width": 60, "height": 60})).unwrap();
    app.ui.tool = Tool::Brush;
    // 0 = no pen, 1 = tip, 2 = eraser end.
    let end = Arc::new(AtomicU8::new(0));
    let read = end.clone();
    app.services.pen_end = Some(Box::new(move || match read.load(Ordering::Relaxed) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }));
    assert!(!Stylus::sync_eraser_tool(&mut app), "no pen in range");
    end.store(1, Ordering::Relaxed);
    assert!(!Stylus::sync_eraser_tool(&mut app), "the tip keeps the tool");
    end.store(2, Ordering::Relaxed);
    assert!(Stylus::sync_eraser_tool(&mut app));
    assert_eq!(app.ui.tool, Tool::Eraser);
    assert!(!Stylus::sync_eraser_tool(&mut app), "reported once");
    // Out of range and back with the eraser end: still the Eraser, nothing to switch.
    end.store(0, Ordering::Relaxed);
    assert!(!Stylus::sync_eraser_tool(&mut app));
    end.store(2, Ordering::Relaxed);
    assert!(!Stylus::sync_eraser_tool(&mut app));
    end.store(1, Ordering::Relaxed);
    assert!(Stylus::sync_eraser_tool(&mut app));
    assert_eq!(app.ui.tool, Tool::Brush, "the tip restores the previous tool");
    // A feed sample is read before the platform's end.
    end.store(2, Ordering::Relaxed);
    app.stylus.feed.set(Some(PenSample { pressure: 0.5, eraser: false, ..Default::default() }));
    assert!(!Stylus::sync_eraser_tool(&mut app));
    assert_eq!(app.ui.tool, Tool::Brush);
}

/// The pen's eraser end is a tool switch like any other: Add New Artboard's placing mode ends
/// (#11386), and coming back to the Artboard tool on the tip leaves it off.
#[test]
fn the_eraser_end_ends_the_artboard_placing_mode() {
    use crate::state::Tool;
    let mut app = crate::PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.execute("file.new", serde_json::json!({"width": 60, "height": 60})).unwrap();
    app.ui.tool = Tool::Artboard;
    app.ui.artboard_tool.adding = true;
    let pen = |eraser| Some(PenSample { pressure: 0.5, eraser, ..Default::default() });
    app.stylus.feed.set(pen(true));
    assert!(Stylus::sync_eraser_tool(&mut app));
    assert!(!app.ui.artboard_tool.adding, "the eraser end ends the mode");
    app.stylus.feed.set(pen(false));
    assert!(Stylus::sync_eraser_tool(&mut app));
    assert_eq!(app.ui.tool, Tool::Artboard);
    assert!(!app.ui.artboard_tool.adding);
}
