use super::*;
use egui::{Event, PointerButton, pos2};
use egui_kittest::Harness;
use serde_json::json;

#[test]
fn wrap_deg_folds_huge_and_hostile_angles() {
    assert_eq!(wrap_deg(0.0), 0.0);
    assert_eq!(wrap_deg(180.0), 180.0);
    assert_eq!(wrap_deg(-180.0), 180.0);
    assert!((wrap_deg(190.0) + 170.0).abs() < 1e-4);
    assert!((wrap_deg(-190.0) - 170.0).abs() < 1e-4);
    assert!((wrap_deg(360.0)).abs() < 1e-4);
    assert!((wrap_deg(1.0e10)).abs() <= 180.0);
    assert_eq!(wrap_deg(f32::NAN), 0.0);
    assert_eq!(wrap_deg(f32::INFINITY), 0.0);
    assert_eq!(wrap_deg(f32::NEG_INFINITY), 0.0);
}

#[test]
fn snap_deg_lands_on_fifteens() {
    assert_eq!(snap_deg(0.0, SNAP_DEG), 0.0);
    assert_eq!(snap_deg(7.0, SNAP_DEG), 0.0);
    assert_eq!(snap_deg(8.0, SNAP_DEG), 15.0);
    assert_eq!(snap_deg(-7.0, SNAP_DEG), 0.0);
    assert_eq!(snap_deg(-8.0, SNAP_DEG), -15.0);
    assert_eq!(snap_deg(40.0, SNAP_DEG), 45.0);
    assert_eq!(snap_deg(f32::NAN, SNAP_DEG), 0.0);
    assert_eq!(snap_deg(40.0, 0.0), wrap_deg(40.0));
    assert_eq!(snap_deg(40.0, f32::NAN), wrap_deg(40.0));
}

#[test]
fn apply_params_sets_adds_resets_and_rejects() {
    assert_eq!(apply_params(0.0, &json!({"angle": 30.0})).unwrap(), 30.0);
    assert_eq!(apply_params(30.0, &json!({"delta": 15.0})).unwrap(), 45.0);
    assert_eq!(apply_params(45.0, &json!({"reset": true})).unwrap(), 0.0);
    assert_eq!(apply_params(170.0, &json!({"delta": 20.0})).unwrap(), -170.0);
    assert!(apply_params(0.0, &json!({})).is_err());
    assert!(apply_params(0.0, &Value::Null).is_err());
    assert!(apply_params(0.0, &json!("30")).is_err());
    assert!(apply_params(0.0, &json!({"angle": f64::NAN})).is_err());
    assert!(apply_params(0.0, &json!({"angle": f64::INFINITY})).is_err());
    assert!(apply_params(0.0, &json!({"delta": f64::INFINITY})).is_err());
    // JSON can contain finite f64 values whose f32 conversion overflows to infinity.
    // Silently wrapping infinity to 0 would reset a rotated view.
    assert!(apply_params(32.0, &json!({"angle": 1.0e100})).is_err());
    assert!(apply_params(32.0, &json!({"delta": -1.0e100})).is_err());
    assert!(apply_params(0.0, &json!({"reset": "yes"})).is_err());
    assert!(apply_params(0.0, &json!({"zoom": 1})).is_err());
    let huge = apply_params(0.0, &json!({"angle": 1.0e20})).unwrap();
    assert!(huge.is_finite() && huge.abs() <= 180.0);
}

#[test]
fn native_rotate_event_reaches_canvas_when_trackpad_rotation_is_enabled() {
    let ctx = egui::Context::default();
    let mut enabled = None;
    let mut disabled = None;
    let raw = egui::RawInput { events: vec![Event::Rotate(std::f32::consts::FRAC_PI_6)], ..Default::default() };
    let mut output = ctx.run_ui(raw, |ui| {
        enabled = trackpad_delta(ui.ctx(), true, true);
        disabled = trackpad_delta(ui.ctx(), false, true);
    });
    output.textures_delta.clear();

    assert!(enabled.is_some_and(|degrees| (degrees - 30.0).abs() < 1e-3), "{enabled:?}");
    assert_eq!(disabled, None);
}

/// Preferences › Enhanced Controls › Enable Gestures (#204): off, a two-finger twist on a
/// touch screen doesn't turn the view; the trackpad's `Rotate` event still does.
#[test]
fn enable_gestures_switches_touch_screen_rotation() {
    use egui::TouchPhase::{Move, Start};
    let touch = |id: u64, phase, pos| Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(id), phase, pos, force: None };
    let twist = |touch_gestures: bool, extra: Vec<Event>| {
        let ctx = egui::Context::default();
        let frames = [
            // egui only starts a multi-touch gesture once it knows where the pointer is.
            vec![Event::PointerMoved(pos2(50.0, 50.0))],
            vec![touch(1, Start, pos2(40.0, 50.0)), touch(2, Start, pos2(60.0, 50.0))],
            // A quarter turn about the centre between the fingers.
            [vec![touch(1, Move, pos2(50.0, 40.0)), touch(2, Move, pos2(50.0, 60.0))], extra].concat(),
        ];
        let mut last = None;
        for (n, events) in (0u32..).zip(frames) {
            let raw = egui::RawInput { time: Some(f64::from(n) / 60.0), events, ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| last = trackpad_delta(ui.ctx(), true, touch_gestures));
            out.textures_delta.clear();
        }
        last
    };
    let on = twist(true, Vec::new());
    assert!(on.is_some_and(|d| (d.abs() - 90.0).abs() < 1e-3), "on: the fingers turn the view: {on:?}");
    assert_eq!(twist(false, Vec::new()), None, "off: the fingers don't");
    let trackpad = twist(false, vec![Event::Rotate(std::f32::consts::FRAC_PI_6)]);
    assert!(trackpad.is_some_and(|d| (d - 30.0).abs() < 1e-3), "off: a trackpad rotate still turns: {trackpad:?}");
}

#[test]
fn trackpad_pref_gates_rotation() {
    assert_eq!(apply_trackpad(10.0, false, 0.5), None);
    assert_eq!(apply_trackpad(10.0, true, 0.0), None);
    assert_eq!(apply_trackpad(10.0, true, f32::NAN), None);
    assert_eq!(apply_trackpad(10.0, true, f32::INFINITY), None);
    let a = apply_trackpad(10.0, true, std::f32::consts::FRAC_PI_2).expect("finite delta");
    assert!((a - 100.0).abs() < 0.05, "{a}");
}

#[test]
fn view_xform_identity_matches_unrotated_mapping() {
    for zoom in [0.5, 1.0, 2.5] {
        for center in [[0.0, 0.0], [320.0, 240.0], [10.0, -4.0]] {
            for flip in [false, true] {
                let rect = Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 600.0));
                let a = ViewXform { rect, zoom, center, flip, rotation: 0.0, aspect: 1.0 };
                let sx = if flip { -1.0 } else { 1.0 };
                let want = rect.center() + vec2((10.0 - center[0]) * zoom * sx, (20.0 - center[1]) * zoom);
                let got = a.to_screen(10.0, 20.0);
                assert!((got.x - want.x).abs() < 1e-4 && (got.y - want.y).abs() < 1e-4, "{got:?} vs {want:?}");
            }
        }
    }
}

#[test]
fn view_xform_roundtrip_and_ninety_degrees() {
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
    for rot in [15.0, 90.0, -45.0, 180.0] {
        for flip in [false, true] {
            let xf = ViewXform { rect, zoom: 2.0, center: [80.0, 40.0], flip, rotation: rot, aspect: 1.0 };
            let s = xf.to_screen(10.0, 20.0);
            let d = xf.to_doc(s);
            assert!((d[0] - 10.0).abs() < 1e-3 && (d[1] - 20.0).abs() < 1e-3, "rot={rot} flip={flip} {d:?}");
        }
    }
    // 90°: a point on +X in document space lands on +Y on screen (Y-down = clockwise).
    let xf = ViewXform { rect, zoom: 1.0, center: [0.0, 0.0], flip: false, rotation: 90.0, aspect: 1.0 };
    let s = xf.to_screen(1.0, 0.0);
    let c = rect.center();
    assert!((s.x - c.x).abs() < 1e-3 && (s.y - (c.y + 1.0)).abs() < 1e-3, "{s:?} vs {c:?}");
    // Flip after rotate: 0° +X goes left.
    let xf = ViewXform { rect, zoom: 1.0, center: [0.0, 0.0], flip: true, rotation: 0.0, aspect: 1.0 };
    assert!(xf.to_screen(1.0, 0.0).x < rect.center().x);
    let xf = ViewXform { rect, zoom: 1.0, center: [0.0, 0.0], flip: true, rotation: 90.0, aspect: 1.0 };
    let s = xf.to_screen(1.0, 0.0);
    assert!((s.x - c.x).abs() < 1e-3 && (s.y - (c.y + 1.0)).abs() < 1e-3, "flip then rotate {s:?}");
}

/// Screen angle, about the canvas centre, of the document point that was under `p0` at the press.
fn followed_angle(h: &Harness<'static, PhotocraftApp>, p0: Pos2, start_rotation: f32) -> f32 {
    let st = h.state();
    let (rect, v, flip) = (st.last_canvas_rect, &st.ui.views[0], st.ui.view.flip_horizontal);
    let before = ViewXform { rect, zoom: 1.0, center: v.center, flip, rotation: start_rotation, aspect: 1.0 };
    let d = before.to_doc(p0);
    let after = ViewXform { rotation: v.rotation, ..before };
    let s = after.to_screen(d[0] as f32, d[1] as f32) - rect.center();
    wrap_deg(s.y.atan2(s.x).to_degrees() - pointer_deg(p0, rect.center()))
}

#[test]
fn drag_follows_the_pointer_in_a_flipped_view() {
    for flip in [false, true] {
        let mut h = harness();
        h.state_mut().ui.view.flip_horizontal = flip;
        h.run_steps(2);
        let c = h.state().last_canvas_rect.center();
        let p0 = c + vec2(120.0, 0.0);
        h.event(Event::PointerMoved(p0));
        h.step();
        press(&mut h, p0, true, false);
        // 30° clockwise about the pivot: the image turns 30° clockwise on screen.
        let p1 = c + vec2(120.0 * 30f32.to_radians().cos(), 120.0 * 30f32.to_radians().sin());
        h.event(Event::PointerMoved(p1));
        h.step();
        let got = followed_angle(&h, p0, 0.0);
        assert!((got - 30.0).abs() < 0.75, "flip={flip}: the image turned {got}°, the pointer 30°");
        press(&mut h, p1, false, false);
    }
}

#[test]
fn screen_delta_mirrors_only_in_a_flipped_view() {
    assert_eq!(screen_delta(12.5, false), 12.5);
    assert_eq!(screen_delta(12.5, true), -12.5);
    assert_eq!(screen_delta(-180.0, true), 180.0);
    assert_eq!(screen_delta(f32::NAN, true), 0.0);
    assert_eq!(screen_delta(f32::INFINITY, false), 0.0);
    // A flipped view still lands on the snapped angle with Shift.
    assert_eq!(snap_deg(0.0 + screen_delta(-40.0, true), SNAP_DEG), 45.0);
}

fn app_docs(n: usize) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    for i in 0..n {
        app.run("file.new", json!({"width": 200 + i as u32 * 10, "height": 100})).unwrap();
    }
    app.sync_views();
    app
}

#[test]
fn commands_rotate_reset_and_reject_without_a_document() {
    let ctx = egui::Context::default();
    let mut empty = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    assert_eq!(crate::menus::invoke(&mut empty, &ctx, "view.rotateView", json!({"angle": 30})).unwrap_err(), "no document open");
    assert_eq!(crate::menus::invoke(&mut empty, &ctx, "view.resetView", json!({})).unwrap_err(), "no document open");

    let mut app = app_docs(1);
    let r = crate::menus::invoke(&mut app, &ctx, "view.rotateView", json!({"angle": 30})).unwrap();
    assert_eq!(r["rotation"], 30.0);
    assert_eq!(app.ui.views[0].rotation, 30.0);
    let r = crate::menus::invoke(&mut app, &ctx, "view.rotateView", json!({"delta": 15})).unwrap();
    assert_eq!(r["rotation"], 45.0);
    let z = app.ui.views[0].zoom;
    let c = app.ui.views[0].center;
    let r = crate::menus::invoke(&mut app, &ctx, "view.resetView", json!({})).unwrap();
    assert_eq!(r["rotation"], 0.0);
    assert_eq!(app.ui.views[0].rotation, 0.0);
    assert_eq!(app.ui.views[0].zoom, z);
    assert_eq!(app.ui.views[0].center, c);
    assert!(crate::menus::invoke(&mut app, &ctx, "view.rotateView", json!({})).is_err());
    assert!(crate::menus::invoke(&mut app, &ctx, "view.rotateView", json!({"angle": f64::NAN})).is_err());
    assert_eq!(app.ui.views[0].rotation, 0.0);
    assert_eq!(app.session.documents()[0].history.past_len(), 0);
}

#[test]
fn ui_set_selects_the_tool_and_sets_rotation() {
    let ctx = egui::Context::default();
    let mut app = app_docs(1);
    let (req, _) = crate::control::ControlRequest::new("ui.set", json!({"tool": "RotateView", "rotation": 30}));
    let out = crate::control::handle(&mut app, &ctx, &req);
    let crate::control::Outcome::Done(v) = out else { panic!("done") };
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(app.ui.tool, crate::state::Tool::RotateView);
    assert_eq!(app.ui.views[0].rotation, 30.0);
    let (req, _) = crate::control::ControlRequest::new("ui.inspect", json!({}));
    let crate::control::Outcome::Done(ins) = crate::control::handle(&mut app, &ctx, &req) else { panic!("inspect") };
    assert_eq!(ins["result"]["tool"], "RotateView");
    assert_eq!(ins["result"]["views"][0]["rotation"], 30.0);
    let (req, _) = crate::control::ControlRequest::new("ui.set", json!({"rotation": f64::NAN}));
    let crate::control::Outcome::Done(bad) = crate::control::handle(&mut app, &ctx, &req) else { panic!("nan") };
    assert_eq!(bad["ok"], false);
    assert_eq!(app.ui.views[0].rotation, 30.0);
    let mut empty = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    let (req, _) = crate::control::ControlRequest::new("ui.set", json!({"tool": "RotateView"}));
    let crate::control::Outcome::Done(ok) = crate::control::handle(&mut empty, &ctx, &req) else { panic!("tool") };
    assert_eq!(ok["ok"], true, "{ok}");
    assert_eq!(empty.ui.tool, crate::state::Tool::RotateView);
    let (req, _) = crate::control::ControlRequest::new("ui.set", json!({"rotation": 15}));
    let crate::control::Outcome::Done(err) = crate::control::handle(&mut empty, &ctx, &req) else { panic!("no doc") };
    assert_eq!(err["ok"], false);
    assert!(err["error"].as_str().unwrap().contains("no document open"), "{err}");
}

#[test]
fn match_rotation_copies_the_angle_and_needs_two_documents() {
    let ctx = egui::Context::default();
    let one = app_docs(1);
    assert_eq!(crate::view_cmds::is_enabled(&one, "window.arrange.matchRotation"), Some(false));
    let mut app = app_docs(2);
    assert_eq!(crate::view_cmds::is_enabled(&app, "window.arrange.matchRotation"), Some(true));
    app.ui.views[1].rotation = 30.0;
    let r = crate::menus::invoke(&mut app, &ctx, "window.arrange.matchRotation", json!({})).unwrap();
    assert_eq!(r["rotation"], 30.0);
    assert!(app.ui.views.iter().all(|v| (v.rotation - 30.0).abs() < 1e-4));
    app.ui.views[1].rotation = 12.0;
    app.ui.views[1].zoom = 2.0;
    crate::menus::invoke(&mut app, &ctx, "window.arrange.matchAll", json!({})).unwrap();
    assert!(app.ui.views.iter().all(|v| (v.rotation - 12.0).abs() < 1e-4 && (v.zoom - 2.0).abs() < 1e-4));
}

fn harness() -> Harness<'static, PhotocraftApp> {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 400, "height": 300})).unwrap();
    app.sync_views();
    app.ui.extras.rulers = false;
    app.ui.tool = crate::state::Tool::RotateView;
    let mut h = Harness::builder().with_size(vec2(1000.0, 700.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            if !ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                return;
            }
            egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
    h.run_steps(4);
    h
}

fn press(h: &mut Harness<'static, PhotocraftApp>, p: Pos2, pressed: bool, shift: bool) {
    let modifiers = egui::Modifiers { shift, ..Default::default() };
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers });
    h.step();
}

#[test]
fn shift_drag_snaps_to_fifteen_degrees() {
    let mut h = harness();
    let r = h.state().last_canvas_rect;
    let c = r.center();
    let p0 = c + vec2(120.0, 0.0);
    h.event(Event::PointerMoved(p0));
    h.step();
    h.event(Event::ModifiersChanged(egui::Modifiers::SHIFT));
    h.step();
    press(&mut h, p0, true, true);
    // ~40° clockwise: Shift rounds to 45°.
    let p1 = c + vec2(120.0 * 40f32.to_radians().cos(), 120.0 * 40f32.to_radians().sin());
    h.event(Event::PointerMoved(p1));
    h.step();
    let rot = h.state().ui.views[0].rotation;
    assert!((rot - 45.0).abs() < 0.75, "{rot}");
    press(&mut h, p1, false, true);
    h.run_steps(2);
    assert_eq!(h.state().session.documents()[0].history.past_len(), 0);
    assert_eq!(crate::state::Tool::from_name("RotateView"), Some(crate::state::Tool::RotateView));
    assert_eq!(crate::state::Tool::RotateView.key(), 'R');
}

#[test]
fn escape_and_reset_zero_the_angle_without_history() {
    let ctx = egui::Context::default();
    let mut app = app_docs(1);
    crate::menus::invoke(&mut app, &ctx, "view.rotateView", json!({"angle": 33})).unwrap();
    assert_eq!(app.session.documents()[0].history.past_len(), 0);
    crate::menus::invoke(&mut app, &ctx, "view.resetView", json!({})).unwrap();
    assert_eq!(app.ui.views[0].rotation, 0.0);
    assert_eq!(app.session.documents()[0].history.past_len(), 0);

    let mut h = harness();
    h.state_mut().ui.views[0].rotation = 40.0;
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.step();
    h.run_steps(2);
    assert!((h.state().ui.views[0].rotation).abs() < 0.05, "{}", h.state().ui.views[0].rotation);
    assert_eq!(h.state().session.documents()[0].history.past_len(), 0);
}

#[test]
fn rotated_view_keeps_engine_selection_in_document_space() {
    let mut app = app_docs(1);
    app.ui.views[0].rotation = 30.0;
    app.run("select.rect", json!({"x": 10, "y": 20, "width": 40, "height": 10})).unwrap();
    let st = app.session.active().unwrap();
    let b = st.doc.selection.as_ref().and_then(|s| photocraft_compose::bounds::content_bounds(s).into()).expect("selection");
    // Axis-aligned in the file: the outline is drawn rotated via ViewXform.
    assert!(b.width() >= 40 && b.height() >= 10, "{b:?}");
    let xf =
        ViewXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0)), zoom: 1.0, center: [100.0, 50.0], flip: false, rotation: 30.0, aspect: 1.0 };
    let a = xf.to_screen(10.0, 20.0);
    let c = xf.to_screen(50.0, 20.0);
    let v = c - a;
    let ang = v.y.atan2(v.x).to_degrees();
    assert!((wrap_deg(ang) - 30.0).abs() < 0.5, "{ang}");
    assert_eq!(st.history.past_len(), 1, "only the selection is a history step");
}

fn brush_harness() -> Harness<'static, PhotocraftApp> {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 400, "height": 300})).unwrap();
    app.sync_views();
    app.ui.extras.rulers = false;
    app.ui.tool = crate::state::Tool::Brush;
    let mut h = Harness::builder().with_size(vec2(1000.0, 700.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            if !ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                return;
            }
            egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
    h.run_steps(4);
    h
}

fn hold_escape(h: &mut Harness<'static, PhotocraftApp>) {
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.step();
}

/// The view as the tests see it (the harness runs at one physical pixel per point).
fn testing_xf(h: &Harness<'static, PhotocraftApp>) -> ViewXform {
    let st = h.state();
    let v = &st.ui.views[0];
    ViewXform { rect: st.last_canvas_rect, zoom: v.zoom, center: v.center, flip: st.ui.view.flip_horizontal, rotation: v.rotation, aspect: 1.0 }
}

/// Hold the key, then press at `p0`: the press's document point is what the gesture must keep
/// under the pen.
fn begin_at(h: &mut Harness<'static, PhotocraftApp>, p0: Pos2) -> [f64; 2] {
    h.event(Event::PointerMoved(p0));
    h.step();
    hold_escape(h);
    let doc = testing_xf(h).to_doc(p0);
    h.event(Event::PointerButton { pos: p0, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    doc
}

fn drag_by(h: &mut Harness<'static, PhotocraftApp>, p: &mut Pos2, d: egui::Vec2) {
    *p += d;
    h.event(Event::PointerMoved(*p));
    h.step();
}

/// Let the pen up, then the key: the gesture ends with the view where the drag left it, and
/// nothing was painted.
fn lift(mut h: Harness<'static, PhotocraftApp>, p: Pos2) -> Harness<'static, PhotocraftApp> {
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.step();
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: Default::default() });
    h.run_steps(2);
    h
}

#[test]
fn esc_drag_up_zooms_about_the_press_point() {
    let mut h = brush_harness();
    let p0 = h.state().last_canvas_rect.center() + vec2(80.0, 60.0);
    let doc = begin_at(&mut h, p0);
    let mut p = p0;
    for _ in 0..8 {
        drag_by(&mut h, &mut p, vec2(0.0, -15.0));
    }
    let st = h.state();
    let v = &st.ui.views[0];
    assert!(v.rotation.abs() < 0.01, "a vertical drag must not turn the view: {}", v.rotation);
    assert!(v.zoom > 1.5, "120 points up nearly doubles the zoom, got {}", v.zoom);
    // The pen tip's document point never moves: it is the centre of the zoom.
    let s = testing_xf(&h).to_screen(doc[0] as f32, doc[1] as f32);
    assert!((s - p0).length() < 0.5, "the press point moved to {s:?} from {p0:?}");
    let h = lift(h, p);
    assert_eq!(h.state().session.documents()[0].history.past_len(), 0, "the gesture must not paint");
    assert!(h.state().drag.is_none(), "the press left no tool drag behind");
    assert!(h.state().ui.views[0].zoom > 1.5, "the release keeps the zoom");
}

#[test]
fn esc_drag_sideways_turns_about_the_press_point() {
    let mut h = brush_harness();
    let p0 = h.state().last_canvas_rect.center() + vec2(80.0, 60.0);
    let doc = begin_at(&mut h, p0);
    let mut p = p0;
    for _ in 0..6 {
        drag_by(&mut h, &mut p, vec2(20.0, 0.0));
    }
    let st = h.state();
    let v = &st.ui.views[0];
    assert!((v.zoom - 1.0).abs() < 1e-3, "a sideways drag must not zoom: {}", v.zoom);
    assert!(v.rotation > 30.0, "120 points right turn the view clockwise, got {}", v.rotation);
    let s = testing_xf(&h).to_screen(doc[0] as f32, doc[1] as f32);
    assert!((s - p0).length() < 0.5, "the view turned about something else: {s:?} vs {p0:?}");
    let h = lift(h, p);
    assert_eq!(h.state().session.documents()[0].history.past_len(), 0, "the gesture must not paint");
    assert!(h.state().drag.is_none(), "the press left no tool drag behind");
    assert!(h.state().ui.views[0].rotation > 30.0, "the release keeps the angle");
}

/// Dragging back the other way reverses the same gesture: no lift, no second press needed.
#[test]
fn esc_drag_reverses_the_zoom_without_lifting() {
    let mut h = brush_harness();
    let p0 = h.state().last_canvas_rect.center() + vec2(80.0, 60.0);
    let doc = begin_at(&mut h, p0);
    let mut p = p0;
    for _ in 0..8 {
        drag_by(&mut h, &mut p, vec2(0.0, -15.0));
    }
    let peak = h.state().ui.views[0].zoom;
    assert!(peak > 1.5, "the first half zooms in: {peak}");
    for _ in 0..16 {
        drag_by(&mut h, &mut p, vec2(0.0, 15.0));
    }
    let back = h.state().ui.views[0].zoom;
    assert!(back < 1.0, "dragging back down zooms out past where it started: {back}");
    assert!(h.state().ui.views[0].rotation.abs() < 0.01, "still no turning");
    let s = testing_xf(&h).to_screen(doc[0] as f32, doc[1] as f32);
    assert!((s - p0).length() < 0.5, "the press point stays put through both directions: {s:?}");
}

/// The axis is judged every frame, not locked by the first pixels: a vertical drag followed by a
/// horizontal one in the same press turns the view, and the zoom stops where it was.
#[test]
fn esc_drag_switches_from_zoom_to_turn_in_one_press() {
    let mut h = brush_harness();
    let p0 = h.state().last_canvas_rect.center() + vec2(80.0, 60.0);
    let _doc = begin_at(&mut h, p0);
    let mut p = p0;
    for _ in 0..6 {
        drag_by(&mut h, &mut p, vec2(0.0, -15.0));
    }
    let zoom_at_turn = h.state().ui.views[0].zoom;
    assert!(zoom_at_turn > 1.4, "the vertical part zooms: {zoom_at_turn}");
    assert!(h.state().ui.views[0].rotation.abs() < 0.01, "and does not turn");
    for _ in 0..6 {
        drag_by(&mut h, &mut p, vec2(20.0, 0.0));
    }
    let st = h.state();
    assert!((st.ui.views[0].zoom - zoom_at_turn).abs() < 1e-3, "the turn keeps the zoom it had: {}", st.ui.views[0].zoom);
    assert!(st.ui.views[0].rotation > 30.0, "the horizontal part turns: {}", st.ui.views[0].rotation);
}

/// The pen leaves the canvas altogether between presses (a pen taken off the tablet), the key
/// still held: the mode must survive, and the next press must be a fresh gesture.
#[test]
fn esc_mode_survives_the_pen_leaving_the_canvas() {
    let mut h = brush_harness();
    let p0 = h.state().last_canvas_rect.center() + vec2(60.0, 40.0);
    let _ = begin_at(&mut h, p0);
    let mut p = p0;
    for _ in 0..6 {
        drag_by(&mut h, &mut p, vec2(0.0, -15.0));
    }
    let zoom1 = h.state().ui.views[0].zoom;
    assert!(zoom1 > 1.4, "the first press zooms: {zoom1}");
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.step();
    // Off the canvas: the pen lifted and carried away, nothing hovering at all.
    let out = h.state().last_canvas_rect.left_top() - vec2(40.0, 40.0);
    h.event(Event::PointerMoved(out));
    h.run_steps(2);
    // Back on, the key never released: a new gesture about the new point.
    let p1 = h.state().last_canvas_rect.center() + vec2(-70.0, 90.0);
    h.event(Event::PointerMoved(p1));
    h.step();
    let doc1 = testing_xf(&h).to_doc(p1);
    h.event(Event::PointerButton { pos: p1, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    assert!(h.state().drag.is_none(), "the press after the pen came back is still the mode's");
    let mut q = p1;
    for _ in 0..6 {
        drag_by(&mut h, &mut q, vec2(20.0, 0.0));
    }
    let st = h.state();
    assert!((st.ui.views[0].zoom - zoom1).abs() < 1e-3, "the turn keeps the zoom: {}", st.ui.views[0].zoom);
    assert!(st.ui.views[0].rotation > 30.0, "the second press turns: {}", st.ui.views[0].rotation);
    let s = testing_xf(&h).to_screen(doc1[0] as f32, doc1[1] as f32);
    assert!((s - p1).length() < 0.5, "the second press turns about its own point, not the first's: {s:?} vs {p1:?}");
}

/// The key stays down between presses: each press is its own gesture, each about its own point.
/// The pen leaves the tablet and hovers on the way to the second press, as a real pen does.
#[test]
fn esc_hold_lets_a_second_press_gesture() {
    let mut h = brush_harness();
    let p0 = h.state().last_canvas_rect.center() + vec2(60.0, 40.0);
    let _doc0 = begin_at(&mut h, p0);
    let mut p = p0;
    for _ in 0..6 {
        drag_by(&mut h, &mut p, vec2(0.0, -15.0));
    }
    let zoom1 = h.state().ui.views[0].zoom;
    assert!(zoom1 > 1.4, "the first press zooms: {zoom1}");
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.step();
    let p1 = p0 + vec2(-40.0, 180.0);
    // The pen comes off the tablet and hovers to the new spot before it lands again.
    for i in 1..=4 {
        h.event(Event::PointerMoved(p + (p1 - p) * (i as f32 / 4.0)));
        h.step();
    }
    assert!(h.state().drag.is_none(), "hovering over the canvas starts nothing");
    let doc1 = testing_xf(&h).to_doc(p1);
    h.event(Event::PointerButton { pos: p1, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    assert!(h.state().drag.is_none(), "the second press is still the mode's, not the brush's");
    let mut q = p1;
    for _ in 0..6 {
        drag_by(&mut h, &mut q, vec2(20.0, 0.0));
    }
    let st = h.state();
    assert!((st.ui.views[0].zoom - zoom1).abs() < 1e-3, "the second press keeps the first press's zoom: {}", st.ui.views[0].zoom);
    assert!(st.ui.views[0].rotation > 30.0, "the second press turns: {}", st.ui.views[0].rotation);
    let s = testing_xf(&h).to_screen(doc1[0] as f32, doc1[1] as f32);
    assert!((s - p1).length() < 0.5, "the second press turns about its own point: {s:?} vs {p1:?}");
    let h = lift(h, q);
    assert_eq!(h.state().session.documents()[0].history.past_len(), 0, "neither press painted");
    assert!(h.state().drag.is_none(), "no tool drag left behind");
    assert!(h.state().ui.views[0].zoom > 1.4, "the zoom outlives the mode");
    assert!(h.state().ui.views[0].rotation > 30.0, "and so does the turn");
}

/// Three presses in one hold: zoom, turn, zoom again. Nothing is locked after the first.
#[test]
fn esc_mode_takes_every_press_while_it_lasts() {
    let mut h = brush_harness();
    let p0 = h.state().last_canvas_rect.center() + vec2(60.0, 40.0);
    let _ = begin_at(&mut h, p0);
    let mut p = p0;
    for _ in 0..6 {
        drag_by(&mut h, &mut p, vec2(0.0, -15.0));
    }
    let zoom1 = h.state().ui.views[0].zoom;
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.step();
    let p1 = p0 + vec2(-30.0, 120.0);
    h.event(Event::PointerMoved(p1));
    h.step();
    h.event(Event::PointerButton { pos: p1, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    let mut q = p1;
    for _ in 0..4 {
        drag_by(&mut h, &mut q, vec2(25.0, 0.0));
    }
    h.event(Event::PointerButton { pos: q, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.step();
    let p2 = p1 + vec2(50.0, -60.0);
    h.event(Event::PointerMoved(p2));
    h.step();
    h.event(Event::PointerButton { pos: p2, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    let mut r = p2;
    for _ in 0..8 {
        drag_by(&mut h, &mut r, vec2(0.0, -15.0));
    }
    let st = h.state();
    assert!(st.ui.views[0].zoom > zoom1, "the third press zooms in again: {} against {zoom1}", st.ui.views[0].zoom);
    assert!(st.ui.views[0].rotation > 30.0, "the turn from the second press survives: {}", st.ui.views[0].rotation);
    let h = lift(h, r);
    assert_eq!(h.state().session.documents()[0].history.past_len(), 0, "none of the three presses painted");
    assert!(h.state().drag.is_none(), "no tool drag left behind");
}

/// With the key up the canvas is the tool's again: a press paints.
#[test]
fn the_key_going_up_hands_the_canvas_back_to_the_tool() {
    let mut h = brush_harness();
    let p0 = h.state().last_canvas_rect.center() + vec2(60.0, 40.0);
    let _ = begin_at(&mut h, p0);
    let mut p = p0;
    for _ in 0..4 {
        drag_by(&mut h, &mut p, vec2(0.0, -15.0));
    }
    let h = lift(h, p);
    let mut h = h;
    let c = h.state().last_canvas_rect.center();
    h.event(Event::PointerMoved(c));
    h.step();
    h.event(Event::PointerButton { pos: c, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    assert!(h.state().drag.is_some(), "without the key the brush owns the press");
}

/// Without the key the same drag is an ordinary brush stroke: the gesture claims nothing.
#[test]
fn a_plain_drag_still_reaches_the_brush() {
    let mut h = brush_harness();
    let p0 = h.state().last_canvas_rect.center() + vec2(80.0, 60.0);
    h.event(Event::PointerMoved(p0));
    h.step();
    h.event(Event::PointerButton { pos: p0, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    assert!(h.state().drag.is_some(), "the brush owns the press");
    for i in 1..=6 {
        h.event(Event::PointerMoved(p0 + vec2(10.0 * i as f32, 6.0 * i as f32)));
        h.step();
    }
    assert!(h.state().drag.is_some(), "the stroke is still the brush's");
    assert!((h.state().ui.views[0].zoom - 1.0).abs() < 1e-3, "the view must not move");
    assert!(h.state().ui.views[0].rotation.abs() < 0.01, "nor turn");
}

/// Escape pressed and let go with no press on the canvas is still the plain tap that puts the
/// turned view back upright.
#[test]
fn an_escape_tap_still_uprights_the_view() {
    let mut h = brush_harness();
    let c = h.state().last_canvas_rect.center();
    h.event(Event::PointerMoved(c));
    h.step();
    h.state_mut().ui.views[0].rotation = 40.0;
    hold_escape(&mut h);
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: Default::default() });
    h.run_steps(2);
    assert!(h.state().ui.views[0].rotation.abs() < 0.05, "{}", h.state().ui.views[0].rotation);
}

#[test]
fn gpu_path_stays_on_with_rotation() {
    // Flip is the only CPU-blit fallback; rotation is a shader uniform.
    assert!(crate::gpu_canvas::gpu_view_ok(false, 45.0, 1.0));
    assert!(!crate::gpu_canvas::gpu_view_ok(true, 45.0, 1.0));
    assert!(crate::gpu_canvas::gpu_view_ok(false, 0.0, 1.0));
}
