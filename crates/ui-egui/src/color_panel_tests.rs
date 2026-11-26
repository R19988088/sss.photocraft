//! The Color panel's faces and panel menu (#6000, #2569): every face draws and edits the colour
//! in every theme, a 色盘 face carries no sliders and a 色条 face carries no field, and the
//! choice is the panel menu's, never the theme's.

use egui::{Modifiers, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::PhotocraftApp;
use crate::color_panel_ui::{Model, hex_code, html_color};
use crate::dock::Group;
use crate::state::{ColorPanelMode, ColorStrip};
use crate::theme::{ThemeKind, Tokens};

/// The Color tab of `kind` in `mode` (a 色盘) with the given 色条 switched on, alone in a
/// 300 x 400 pt panel.
fn color_tab_strips(kind: ThemeKind, mode: ColorPanelMode, strips: &[ColorStrip], fg: [f32; 4]) -> Harness<'static, PhotocraftApp> {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.session.tools.foreground = fg;
    app.ui.color_panel.mode = mode;
    for strip in strips {
        app.ui.color_panel.toggle_strip(*strip);
    }
    color_tab_of(kind, app)
}

/// [`color_tab_strips`] with no 色条.
fn color_tab(kind: ThemeKind, mode: ColorPanelMode, fg: [f32; 4]) -> Harness<'static, PhotocraftApp> {
    color_tab_strips(kind, mode, &[], fg)
}

/// The Color tab of `kind` for `app`, as [`color_tab`].
fn color_tab_of(kind: ThemeKind, app: PhotocraftApp) -> Harness<'static, PhotocraftApp> {
    let tab = Group::Color.tabs(Tokens::for_kind(kind).pro).iter().position(|name| *name == "Color").unwrap();
    let mut h = Harness::builder().with_size(vec2(300.0, 400.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        move |ui, app: &mut PhotocraftApp| {
            if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                crate::panels::dock_body(app, ui, Group::Color, tab);
            }
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, kind);
    h.run_steps(3);
    h
}

/// Click at `p` with `modifiers` held.
fn press(h: &mut Harness<'_, PhotocraftApp>, p: Pos2, modifiers: Modifiers) {
    h.hover_at(p);
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers });
        h.run_steps(1);
    }
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

fn bytes(c: [f32; 4]) -> [u8; 3] {
    [c[0], c[1], c[2]].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn type_into(h: &mut Harness<'_, PhotocraftApp>, role: egui::accesskit::Role, nth: usize, text: &str) {
    // Photoshop's number fields open on the second click (fields.rs `number_edit`): the first
    // only arms the field.
    h.query_all_by_role(role).nth(nth).unwrap().click();
    h.run_steps(1);
    h.query_all_by_role(role).nth(nth).unwrap().click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    for ch in text.chars() {
        h.event(egui::Event::Text(ch.to_string()));
        h.run_steps(1);
    }
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
}

#[test]
fn every_model_round_trips_its_colours() {
    let colours = [[0.2, 0.6, 0.8], [1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.5, 0.25, 0.75]];
    for model in [Model::Hsb, Model::Lab] {
        for c in colours {
            let back = model.to_rgb(model.values_of(c));
            let worst = (0..3).map(|i| (back[i] - c[i]).abs()).fold(0.0, f32::max);
            // Whole-number fields: HSB within a step, Lab within a few.
            let tolerance = match model {
                Model::Hsb => 2.5 / 255.0,
                Model::Lab => 0.03,
            };
            assert!(worst <= tolerance, "{model:?} {c:?} -> {back:?}");
        }
    }
    // Hostile values give a valid colour.
    for model in [Model::Hsb, Model::Lab] {
        for v in [[f32::NAN; 4], [f32::INFINITY; 4], [-1e9; 4], [1e9; 4]] {
            assert!(model.to_rgb(v).iter().all(|x| (0.0..=1.0).contains(x)), "{model:?} {v:?}");
        }
        assert_eq!(model.channels().len(), 3, "{model:?}");
    }
}

#[test]
fn copy_color_formats() {
    assert_eq!(html_color([0.2, 0.6, 0.8]), "color=\"#3399cc\"");
    assert_eq!(hex_code([0.2, 0.6, 0.8]), "3399cc");
}

/// Every face draws in every theme, and a click in it sets the colour.
#[test]
fn every_mode_works_in_every_theme() {
    for kind in ThemeKind::ALL {
        for mode in ColorPanelMode::ALL {
            let mut h = color_tab(kind, mode, [1.0, 0.0, 0.0, 1.0]);
            match mode {
                ColorPanelMode::C2 => {
                    // The C2 layout: the source switch, the chips and the hex readout.
                    for label in ["Solid", "Pattern", "Foreground Color", "Background Color", "#"] {
                        assert!(h.query_by_label(label).is_some(), "{kind:?} {mode:?}: {label}");
                    }
                    let field = h.get_by_label("Saturation / Brightness").rect().center();
                    press(&mut h, field, Modifiers::NONE);
                    assert_ne!(h.state().session.tools.foreground, [1.0, 0.0, 0.0, 1.0], "{kind:?} {mode:?}: the field picks");
                }
                ColorPanelMode::HueCube => {
                    assert!(h.query_by_label("#").is_some(), "{kind:?} {mode:?}: the readout");
                    let field = h.get_by_label("Saturation / Brightness").rect().center();
                    press(&mut h, field, Modifiers::NONE);
                    assert_ne!(h.state().session.tools.foreground, [1.0, 0.0, 0.0, 1.0], "{kind:?} {mode:?}: the field picks");
                }
                ColorPanelMode::ColorWheel => {
                    for label in ["Color Wheel", "Add to Swatches"] {
                        assert!(h.query_by_label(label).is_some(), "{kind:?} {mode:?}: {label}");
                    }
                }
            }
            // A 色盘 carries no 色条: strips are switched on under the disc, not built into it.
            for name in ["H value", "S value", "B value", "L value", "a value", "b value"] {
                assert!(h.query_by_label(name).is_none(), "{kind:?} {mode:?}: no {name} row in a disc");
            }
            // Every disc keeps the picture's 色板 row under it.
            assert!(h.query_all_by_label("Image Palette Color").next().is_some(), "{kind:?} {mode:?}: the 色板 row");
        }
        // A 色条 switched on draws its own three rows and keeps the disc above it whole.
        for strip in ColorStrip::ALL {
            let mut h = color_tab_strips(kind, ColorPanelMode::C2, &[strip], [1.0, 0.0, 0.0, 1.0]);
            for (label, ..) in Model::of(strip).channels() {
                assert!(h.query_by_label(label).is_some(), "{kind:?} {strip:?}: the {label} slider");
            }
            assert!(h.query_by_label("Saturation / Brightness").is_some(), "{kind:?} {strip:?}: the disc stays above its strip");
            assert!(h.query_all_by_label("Image Palette Color").next().is_some(), "{kind:?} {strip:?}: the 色板 row stays between them");
        }
    }
}

/// Black has no hue or saturation of its own: typed H and S must stick while B is still 0.
#[test]
fn hsb_sliders_take_values_on_black() {
    let mut h = color_tab_strips(ThemeKind::Studio, ColorPanelMode::C2, &[ColorStrip::Hsb], [0.0, 0.0, 0.0, 1.0]);
    for (field, typed) in [(0, "120"), (1, "100"), (2, "100")] {
        type_into(&mut h, egui::accesskit::Role::SpinButton, field, typed);
    }
    assert_eq!(bytes(h.state().session.tools.foreground), [0, 255, 0]);
}

#[test]
fn hsb_slider_track_sets_its_channel() {
    let mut h = color_tab_strips(ThemeKind::AdwaitaDark, ColorPanelMode::C2, &[ColorStrip::Hsb], [1.0, 0.0, 0.0, 1.0]);
    let s = h.get_by_label("S").rect();
    press(&mut h, pos2(s.right() - 2.0, s.center().y), Modifiers::NONE);
    // Saturation 100 of red is still red; the field takes a brightness next.
    assert_eq!(bytes(h.state().session.tools.foreground), [255, 0, 0]);
    let b = h.get_by_label("B").rect();
    press(&mut h, pos2(b.left() + 4.0, b.center().y), Modifiers::NONE);
    assert_eq!(bytes(h.state().session.tools.foreground), [0, 0, 0], "brightness 0 is black");
}

#[test]
fn hue_cube_warns_when_the_colour_cannot_print() {
    // #8672: Photoshop's Hue Cube shows the ⚠ and the nearest printable colour under the chips;
    // a click takes that colour.
    const GAMUT: &str = "Out of gamut for printing: click for the nearest printable color";
    let mut h = color_tab(ThemeKind::Classic, ColorPanelMode::HueCube, [0.0, 1.0, 0.0, 1.0]);
    let near = crate::color_picker_ui::out_of_gamut([0.0, 1.0, 0.0]).expect("pure green can't print");
    let alert = h.get_by_label(GAMUT).rect().center();
    press(&mut h, alert, Modifiers::NONE);
    assert_eq!(bytes(h.state().session.tools.foreground), bytes([near[0], near[1], near[2], 1.0]));
    h.run_steps(2);
    assert!(h.query_by_label(GAMUT).is_none(), "the printable colour still warns");
    // A printable colour shows no alert.
    let h = color_tab(ThemeKind::Classic, ColorPanelMode::HueCube, [0.5, 0.5, 0.5, 1.0]);
    assert!(h.query_by_label(GAMUT).is_none());
    // CodeRabbit on #9154: with the background chip active, the click sets the background.
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.session.tools.foreground = [0.5, 0.5, 0.5, 1.0];
    app.session.tools.background = [0.0, 1.0, 0.0, 1.0];
    app.ui.color_panel.mode = ColorPanelMode::HueCube;
    app.ui.color_panel.background = true;
    let mut h = color_tab_of(ThemeKind::Classic, app);
    let alert = h.get_by_label(GAMUT).rect().center();
    press(&mut h, alert, Modifiers::NONE);
    let tools = &h.state().session.tools;
    assert_eq!(bytes(tools.background), bytes([near[0], near[1], near[2], 1.0]), "background");
    assert_eq!(bytes(tools.foreground), bytes([0.5, 0.5, 0.5, 1.0]), "the foreground is left alone");
}

/// A non-finite pointer (a huge synthetic `ui.move`) misses the triangle instead of panicking.
#[test]
fn triangle_ignores_non_finite_points() {
    let tri = [pos2(100.0, 0.0), pos2(0.0, 0.0), pos2(0.0, 100.0)];
    for p in [pos2(f32::INFINITY, f32::INFINITY), pos2(f32::NAN, 1.0), pos2(f32::NEG_INFINITY, f32::INFINITY)] {
        assert_eq!(crate::color_panel_ui::triangle_sv(tri, p), None, "{p:?}");
    }
    assert!(crate::color_panel_ui::triangle_sv(tri, pos2(10.0, 10.0)).is_some());
}

#[test]
fn color_wheel_ring_sets_hue_and_triangle_sets_saturation_and_brightness() {
    let mut h = color_tab(ThemeKind::Pro, ColorPanelMode::ColorWheel, [0.0, 0.0, 1.0, 1.0]);
    let area = h.get_by_label("Color Wheel").rect();
    let (center, inner, outer) = crate::color_panel_ui::wheel_geometry(area);
    let mid = (inner + outer) / 2.0;
    // Up on the ring: hue 90° (yellow-green). Blue is fully saturated and bright, so it keeps both.
    press(&mut h, pos2(center.x, center.y - mid), Modifiers::NONE);
    let [r, g, bl] = bytes(h.state().session.tools.foreground);
    assert!(r > 100 && g > 240 && bl < 10, "hue 90: {r} {g} {bl}");
    // The triangle's centre: some saturation, some brightness, same hue.
    press(&mut h, center, Modifiers::NONE);
    let after = bytes(h.state().session.tools.foreground);
    let hsv = photocraft_color::convert::rgb_to_hsb(after.map(|v| f32::from(v) / 255.0));
    assert!((hsv[0] - 90.0).abs() < 4.0 && hsv[1] < 0.9 && hsv[2] < 0.95, "{after:?} {hsv:?}");
}

/// A right-click on the panel opens the menu at the menu's own narrow width: the popup is anchored
/// to the whole panel's rect, so without the menu pinning its width it stretched to the dock.
#[test]
fn the_panel_menu_keeps_its_narrow_width_over_the_panel() {
    let mut h = color_tab(ThemeKind::Studio, ColorPanelMode::C2, [0.2, 0.4, 0.6, 1.0]);
    let at = h.get_by_label("Saturation / Brightness").rect().center();
    h.event(egui::Event::PointerMoved(at));
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
    let row = h.get_by_label("✔ C2 Color").rect();
    assert!(row.width() <= 210.0, "the menu keeps its own width: {row:?}");
    assert_eq!(h.query_all_by_label("Color Discs").count(), 1);
    assert_eq!(h.query_all_by_label("Color Strips").count(), 1);
}

/// The panel menu lists the faces in two groups — 色盘 (the discs) over 色条 (the strips) — with
/// the reference layout checked first; picking a disc switches the panel, a strip is switched on
/// beside it, and a theme change keeps both. The copy commands still work on the second page.
#[test]
fn panel_menu_lists_the_faces_in_two_groups() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.session.tools.foreground = [0.2, 0.6, 0.8, 1.0];
    let mut h = Harness::builder().with_size(vec2(260.0, 760.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                crate::color_panel_ui::panel_menu(app, ui);
            }
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, ThemeKind::AdwaitaDark);
    h.run_steps(2);
    for group in ["Color Discs", "Color Strips"] {
        assert_eq!(h.query_all_by_label(group).count(), 1, "the {group} heading");
    }
    for mode in ColorPanelMode::ALL {
        let label = format!("{}{}", if mode == ColorPanelMode::C2 { "✔ " } else { "   " }, mode.label());
        assert_eq!(h.query_all_by_label(&label).count(), 1, "{mode:?} is offered");
    }
    for strip in ColorStrip::ALL {
        assert_eq!(h.query_all_by_label(&format!("   {}", strip.label())).count(), 1, "{strip:?} is offered unchecked");
    }
    assert_eq!(h.state().ui.color_panel.mode, ColorPanelMode::C2);
    // A 色条 is switched on, not swapped for the disc: the disc stays and the strip comes under it.
    h.get_by_label("   HSB Sliders").click();
    h.run_steps(2);
    assert_eq!(h.state().ui.color_panel.mode, ColorPanelMode::C2, "the disc stays");
    assert!(h.state().ui.color_panel.hsb_strip, "the strip is on");
    assert_eq!(h.query_all_by_label("✔ HSB Sliders").count(), 1, "the menu shows it on");
    // The copy commands live on the menu's second page, reached from the dot under the first.
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    h.get_by_label("Copy Color's Hex Code").click();
    h.run_steps(1);
    let copied: Vec<_> =
        h.output().platform_output.commands.iter().filter_map(|c| if let egui::OutputCommand::CopyText(s) = c { Some(s.clone()) } else { None }).collect();
    assert_eq!(copied, ["3399cc"]);
    // Switching theme keeps the disc and the strip the author picked.
    let ctx = h.ctx.clone();
    h.state_mut().set_theme(&ctx, ThemeKind::Pro);
    h.run_steps(2);
    assert_eq!(h.state().ui.color_panel.mode, ColorPanelMode::C2);
    assert!(h.state().ui.color_panel.hsb_strip);
}

/// `ui.set colorPanel` merges, so setting the mode keeps the edited chip; bad values change nothing.
#[test]
fn control_sets_the_mode() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    let ctx = egui::Context::default();
    let call = |app: &mut PhotocraftApp, params: serde_json::Value| {
        let (req, _rx) = crate::control::ControlRequest::new("ui.set", params);
        match crate::control::handle(app, &ctx, &req) {
            crate::control::Outcome::Done(v) => v["ok"] == true,
            _ => panic!("expected an immediate reply"),
        }
    };
    assert!(call(&mut app, json!({"colorPanel": {"background": true}})));
    assert!(call(&mut app, json!({"colorPanel": {"mode": "colorWheel"}})));
    let cp = app.ui.color_panel;
    assert!(cp.background, "the chip is kept");
    assert_eq!(cp.mode, ColorPanelMode::ColorWheel);
    // The faces the panel no longer has, and values of the wrong type: rejected.
    for bad in [json!({"colorPanel": {"mode": "hueWheel"}}), json!({"colorPanel": {"mode": "grayscaleSlider"}}), json!({"colorPanel": {"background": "yes"}})] {
        assert!(!call(&mut app, bad.clone()), "{bad}");
        assert_eq!(app.ui.color_panel, cp, "{bad}: nothing changes");
    }
}

/// Only Web Colors is `webOnly` now that the state is camelCase; state saved before still loads.
#[test]
fn web_only_reads_its_old_snake_case_key() {
    let old: crate::state::ColorPanelState = serde_json::from_value(json!({"background": true, "web_only": true})).unwrap();
    assert!(old.background && old.web_only);
    let new = serde_json::to_value(old).unwrap();
    assert_eq!((new.get("webOnly"), new.get("web_only")), (Some(&json!(true)), None));
}

/// A click beside the Color Wheel (its area spans the panel) leaves the colour alone.
#[test]
fn color_wheel_ignores_clicks_beside_it() {
    let mut h = color_tab(ThemeKind::Pro, ColorPanelMode::ColorWheel, [0.0, 0.0, 1.0, 1.0]);
    let area = h.get_by_label("Color Wheel").rect();
    let (center, _, outer) = crate::color_panel_ui::wheel_geometry(area);
    let corner = area.left_top() + vec2(3.0, 3.0);
    assert!(corner.distance(center) > outer + 20.0, "the corner is beside the wheel");
    press(&mut h, corner, Modifiers::NONE);
    assert_eq!(bytes(h.state().session.tools.foreground), [0, 0, 255]);
}

/// 色板区域固定比例: every disc face draws the same square in the same panel — the C2 layout's
/// 168 pt picker, the hue cube's field and the color wheel's area all measure that square — so
/// switching faces never changes the colour area's size or shape.
#[test]
fn every_face_keeps_the_palette_square() {
    for width in [300.0, 420.0] {
        let c2 = mode_panel(width, ColorPanelMode::C2).get_by_label("Saturation / Brightness").rect();
        assert_eq!((c2.width().round(), c2.height().round()), (168.0, 168.0), "the C2 picker at {width}");
        let wheel = mode_panel(width, ColorPanelMode::ColorWheel).query_by_label("Color Wheel").unwrap_or_else(|| panic!("{width}: no Color Wheel")).rect();
        assert_eq!(wheel.height().round(), c2.height().round(), "the wheel's area is the palette square at {width}");
        // The Hue Cube's field is the 17 x 17 mesh.
        let h = mode_panel(width, ColorPanelMode::HueCube);
        let mut field = None;
        for clipped in &h.output().shapes {
            if let egui::Shape::Mesh(m) = &clipped.shape
                && m.vertices.len() == 17 * 17
            {
                field = Some(m.calc_bounds());
            }
        }
        let field = field.unwrap_or_else(|| panic!("{width}: no hue cube field"));
        assert_eq!((field.width().round(), field.height().round()), (c2.width().round(), c2.height().round()), "the cube's field is the palette square at {width}");
    }
}

/// The panel that draws `mode` alone at `width`, as the Color tab would.
fn mode_panel(width: f32, mode: ColorPanelMode) -> Harness<'static, PhotocraftApp> {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.session.tools.foreground = [1.0, 0.0, 0.0, 1.0];
    app.ui.color_panel.mode = mode;
    let mut h = Harness::builder().with_size(vec2(width, 400.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                crate::color_panel_ui::panel(app, ui);
            }
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, ThemeKind::Studio);
    h.run_steps(3);
    h
}

/// The Color tab in `mode` with a document holding one selected black rectangle, the Rectangle tool
/// in hand, foreground blue and background red, editing the background chip if `background`.
fn color_tab_with_a_shape(mode: ColorPanelMode, strips: &[ColorStrip], background: bool) -> Harness<'static, PhotocraftApp> {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 200, "height": 100})).unwrap();
    app.sync_views();
    app.run("shape.create", json!({"kind": "rect", "rect": [20, 20, 50, 50], "fill": "#000000"})).unwrap();
    app.ui.tool = crate::state::Tool::Rectangle;
    app.session.tools.foreground = [0.0, 0.0, 1.0, 1.0];
    app.session.tools.background = [1.0, 0.0, 0.0, 1.0];
    app.ui.color_panel.mode = mode;
    for strip in strips {
        app.ui.color_panel.toggle_strip(*strip);
    }
    app.ui.color_panel.background = background;
    color_tab_of(ThemeKind::Studio, app)
}

/// Types `text` into the first slider field (Hue) and commits it with Enter.
fn type_first_slider(h: &mut Harness<'_, PhotocraftApp>, text: &str) {
    // The number field opens on the second click (fields.rs `number_edit`).
    h.query_all_by_role(egui::accesskit::Role::SpinButton).next().unwrap().click();
    h.run_steps(1);
    h.query_all_by_role(egui::accesskit::Role::SpinButton).next().unwrap().click();
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.event(egui::Event::Text(text.into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
}

fn shape_fill(app: &PhotocraftApp) -> [u8; 3] {
    let st = app.session.active().unwrap();
    let photocraft_doc::LayerContent::Shape(sh) = &st.doc.layer(st.active_layer.unwrap()).unwrap().content else { panic!("shape layer") };
    let Some(photocraft_doc::Fill::Solid(c)) = &sh.fill else { panic!("solid fill") };
    bytes([c.c[0], c.c[1], c.c[2], 1.0])
}

/// The HSB sliders edit the chip the panel is editing (#5842): with the background chip active
/// they change the background, and the selected shape (which takes the foreground with a shape
/// tool in hand) keeps its fill. On the foreground chip the shape follows.
#[test]
fn hsb_sliders_edit_the_active_chip_and_only_the_foreground_recolours_shapes() {
    let mut h = color_tab_with_a_shape(ColorPanelMode::C2, &[ColorStrip::Hsb], true);
    type_first_slider(&mut h, "120");
    let app = h.state();
    assert_eq!(bytes(app.session.tools.background), [0, 255, 0], "the background takes the hue");
    assert_eq!(bytes(app.session.tools.foreground), [0, 0, 255], "the foreground is left alone");
    assert_eq!(shape_fill(app), [0, 0, 0], "a background edit recolours nothing");

    let mut h = color_tab_with_a_shape(ColorPanelMode::C2, &[ColorStrip::Hsb], false);
    type_first_slider(&mut h, "120");
    let app = h.state();
    assert_eq!(bytes(app.session.tools.foreground), [0, 255, 0]);
    assert_eq!(bytes(app.session.tools.background), [255, 0, 0]);
    assert_eq!(shape_fill(app), [0, 255, 0], "the selected shape takes the new foreground");
}

/// The strips recolour the selected shape like the disc's field and the swatches do
/// (`apply_color`), in one history step, and only for a foreground edit.
#[test]
fn the_strips_recolour_the_selected_shape_with_the_foreground() {
    let mut h = color_tab_with_a_shape(ColorPanelMode::C2, &[ColorStrip::Hsb], false);
    let before = h.state().session.active().unwrap().history.entries().len();
    type_first_slider(&mut h, "120");
    let app = h.state();
    assert_eq!(bytes(app.session.tools.foreground), [0, 255, 0]);
    assert_eq!(shape_fill(app), [0, 255, 0]);
    assert_eq!(app.session.active().unwrap().history.entries().len(), before + 1, "one history step");

    let mut h = color_tab_with_a_shape(ColorPanelMode::C2, &[ColorStrip::Lab], false);
    type_first_slider(&mut h, "70");
    let app = h.state();
    assert_ne!(bytes(app.session.tools.foreground), [0, 0, 255], "the Lab strip edits the foreground");
    assert_eq!(shape_fill(app), bytes(app.session.tools.foreground), "and the shape follows it");
}

/// A 色盘 face carries no slider rows under its field: 颜色数值 lives in the disc's own hex
/// readout at its top right, never as strips below the colour area.
#[test]
fn the_disc_carries_no_sliders() {
    let mut h = color_tab(ThemeKind::Studio, ColorPanelMode::C2, [0.772, 0.361, 0.361, 1.0]);
    assert_eq!(h.query_all_by_role(egui::accesskit::Role::SpinButton).count(), 0, "no H/S/B rows under the C2 disc");
    for name in ["Hue", "Saturation", "Brightness"] {
        assert!(h.query_by_label(name).is_none(), "no {name} row");
    }
    // The hex field is there, at the panel's top right, above the field and the swatches.
    let hex = h.get_by_label("#").rect();
    let picker = h.get_by_label("Saturation / Brightness").rect();
    let swatch = h.query_all_by_label("Image Palette Color").next().expect("a swatch").rect();
    assert!(hex.bottom() <= picker.top() && hex.top() < swatch.top(), "the value sits at the top: {hex:?} {picker:?}");
    assert_eq!(h.query_all_by_label("Image Palette Color").count(), 10, "the swatch row is still there");
    h.run_steps(3);
}

/// A 色条 is a strip only: its rows go below the 色盘, it draws no field of its own, and the 色板
/// row sits between the two.
#[test]
fn the_strips_carry_no_field() {
    for strip in ColorStrip::ALL {
        let h = color_tab_strips(ThemeKind::Studio, ColorPanelMode::C2, &[strip], [0.2, 0.4, 0.6, 1.0]);
        for name in ["Color Ramp", "White", "Black", "Color Wheel"] {
            assert!(h.query_by_label(name).is_none(), "{strip:?}: no {name}");
        }
        assert_eq!(h.query_all_by_label("Image Palette Color").count(), 10, "{strip:?}: 色板 stays above the strips");
        for (label, ..) in Model::of(strip).channels() {
            assert_eq!(h.query_all_by_label(label).count(), 1, "{strip:?}: the {label} strip is there");
        }
    }
}

/// 色板 在上、色条在下: a strip keeps the swatch row above its sliders, under the 色盘, so
/// switching faces never takes the palette away.
#[test]
fn the_strips_keep_the_palette_above_them() {
    let h = color_tab_strips(ThemeKind::Studio, ColorPanelMode::C2, &[ColorStrip::Hsb], [0.2, 0.4, 0.6, 1.0]);
    let swatch = h.query_all_by_label("Image Palette Color").next().expect("a swatch").rect();
    let picker = h.get_by_label("Saturation / Brightness").rect();
    let hue = h.get_by_label("H").rect();
    assert!(picker.bottom() <= swatch.top(), "the 色盘 sits above the palette: {picker:?} vs {swatch:?}");
    assert!(swatch.bottom() <= hue.top(), "the palette sits above the strips: {swatch:?} vs {hue:?}");
}

/// #9592: the Color panel's options survive a restart (they ride in the preferences), as in
/// Photoshop; preferences without them keep the defaults.
#[test]
fn color_panel_options_survive_a_restart() {
    let mut first = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    first.ui.color_panel.recent[0] = [0.1, 0.2, 0.3, 1.0];
    crate::color_panel_ui::persist(&mut first);
    let saved = first.session.prefs().clone();

    let mut next = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    next.session.prefs.edit(|p| *p = saved);
    crate::color_panel_ui::restore(&mut next);
    // The options survive; the display does not, because the panel opens on the reference layout.
    assert_eq!(next.ui.color_panel.recent[0], [0.1, 0.2, 0.3, 1.0]);
    assert_eq!(next.ui.color_panel.mode, ColorPanelMode::C2, "the display never comes back stored");

    let mut fresh = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    crate::color_panel_ui::restore(&mut fresh);
    assert_eq!(fresh.ui.color_panel.mode, ColorPanelMode::C2, "no saved options: the default");
}

/// Whatever the preferences record — a hand edit, an older file's face or
/// `ui.set colorPanel.mode` — the panel comes back as the reference layout, and the preference
/// is rewritten to match: one starting face, no fallback.
#[test]
fn the_panel_never_falls_back_to_a_traditional_mode() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    let stored = serde_json::json!({"background": false, "mode": "colorWheel", "recent": [[0.1, 0.2, 0.3, 1.0], [0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0]]});
    app.session.prefs.edit(|p| p.color_panel = stored);
    crate::color_panel_ui::restore(&mut app);
    assert_eq!(app.ui.color_panel.mode, ColorPanelMode::C2, "the stored face is dropped");
    assert_eq!(app.ui.color_panel.recent[0], [0.1, 0.2, 0.3, 1.0], "the rest of the file is kept");
    crate::color_panel_ui::persist(&mut app);
    let stored: crate::state::ColorPanelState = serde_json::from_value(app.session.prefs().color_panel.clone()).unwrap();
    assert_eq!(stored.mode, ColorPanelMode::C2, "and the preference no longer asks for it");
}

/// CodeRabbit on #9606: a preference change made outside the panel (`prefs.set`, a reset) is kept.
/// The next frame takes it into the live panel instead of writing the panel's old options back.
#[test]
fn an_outside_preference_change_reaches_the_live_panel() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    crate::color_panel_ui::persist(&mut app);
    // As prefs.set or a reset to defaults would: the stored options change under the panel.
    let mut other = app.ui.color_panel;
    other.recent[1] = [0.4, 0.5, 0.6, 1.0];
    app.session.prefs.edit(|p| p.color_panel = serde_json::to_value(other).unwrap());
    crate::color_panel_ui::persist(&mut app);
    assert_eq!(app.ui.color_panel.recent[1], [0.4, 0.5, 0.6, 1.0], "the live panel follows the preference");
    let stored: crate::state::ColorPanelState = serde_json::from_value(app.session.prefs().color_panel.clone()).unwrap();
    assert_eq!(stored.recent[1], [0.4, 0.5, 0.6, 1.0], "and the preference isn't overwritten");
    // A stored face is the one option that never reaches the panel.
    let mut mode_change = app.ui.color_panel;
    mode_change.mode = ColorPanelMode::ColorWheel;
    mode_change.hsb_strip = true;
    app.session.prefs.edit(|p| p.color_panel = serde_json::to_value(mode_change).unwrap());
    crate::color_panel_ui::persist(&mut app);
    assert_eq!(app.ui.color_panel.mode, ColorPanelMode::C2, "the stored face is not restored");
    // A reset to defaults clears the stored options: the panel goes back to its defaults too.
    app.session.prefs.edit(|p| p.color_panel = serde_json::Value::Null);
    crate::color_panel_ui::persist(&mut app);
    assert_eq!(app.ui.color_panel.mode, ColorPanelMode::C2, "reset: the default mode");
    crate::color_panel_ui::persist(&mut app);
    assert_eq!(app.ui.color_panel.mode, ColorPanelMode::C2, "and it stays reset");
}

/// C3 draws its palette at a fixed scale (PaletteLayerPanels: `scale = 1`, core 168): the
/// picker stays a 168 pt square whatever the dock's width, so the panel's height never follows
/// the dock.
#[test]
fn the_c2_picker_keeps_a_168_pt_square_at_any_dock_width() {
    let mut sides = Vec::new();
    for width in [300.0, 420.0] {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.session.tools.foreground = [0.2, 0.4, 0.6, 1.0];
        app.ui.color_panel.mode = ColorPanelMode::C2;
        let mut h = Harness::builder().with_size(vec2(width, 400.0)).with_step_dt(1.0 / 60.0).build_ui_state(
            |ui, app: &mut PhotocraftApp| {
                if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                    crate::color_panel_ui::panel(app, ui);
                }
            },
            app,
        );
        PhotocraftApp::setup_context(&h.ctx, ThemeKind::Studio);
        h.run_steps(3);
        let field = h.get_by_label("Saturation / Brightness").rect();
        sides.push((field.width().round(), field.height().round()));
    }
    assert_eq!(sides, [(168.0, 168.0), (168.0, 168.0)], "the picker is fixed, not stretched with the dock");
}

/// The C2 layout's 实线/图案 switch at the top of the panel: 图案 arms the brush for a sampled
/// pattern, 实线 takes it off again.
#[test]
fn the_c2_source_switch_tops_the_panel() {
    let mut h = color_tab(ThemeKind::Studio, ColorPanelMode::C2, [1.0, 0.0, 1.0, 1.0]);
    assert!(!h.state().session.tools.brush.pattern_enabled, "实线 until asked");
    h.get_by_label("Pattern").click();
    h.run_steps(2);
    assert!(h.state().session.tools.brush.pattern_enabled, "图案 arms the pattern");
    h.get_by_label("Solid").click();
    h.run_steps(2);
    assert!(!h.state().session.tools.brush.pattern_enabled, "实线 turns it off");
}

/// Picking a solid colour keeps 图案 on: the picked colour paints the stroke (a monochrome
/// stroke) instead of the patch, and 图案 brings the patch back.
#[test]
fn picking_a_solid_colour_keeps_pattern_and_paints_it() {
    let mut h = color_tab_with_a_shape(ColorPanelMode::C2, &[], false);
    h.state_mut().session.tools.foreground = [1.0, 0.0, 0.0, 1.0];
    h.state_mut().run("document.samplePattern", json!({"x": 4, "y": 4, "size": 4})).unwrap();
    h.run_steps(2);
    assert!(h.state().session.tools.brush.pattern_enabled);
    assert!(!h.state().session.tools.brush.pattern_solid, "图案 paints the patch at first");
    let patch = h.state().session.tools.brush.sampled_pattern.clone();
    let field = h.get_by_label("Saturation / Brightness").rect().center();
    press(&mut h, field, Modifiers::NONE);
    assert!(h.state().session.tools.brush.pattern_enabled, "a solid pick stays in 图案");
    assert!(h.state().session.tools.brush.pattern_solid, "and the picked colour paints, not the patch");
    assert_eq!(h.state().session.tools.brush.sampled_pattern, patch, "the patch is kept");
    // 图案 again (the button, or a new sample) brings the patch back.
    h.get_by_label("Pattern").click();
    h.run_steps(2);
    assert!(!h.state().session.tools.brush.pattern_solid, "图案 paints the patch again");
}

/// 混合强度 lives in the Color tab's panel menu and drives the brush's pigment response
/// through the `painting.pigmentMixStrength` preference, which survives a restart.
#[test]
fn the_panel_menu_sets_the_brush_mixing_strength() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    let mut h = Harness::builder().with_size(vec2(320.0, 500.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                crate::color_panel_ui::panel_menu(app, ui);
            }
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, ThemeKind::Studio);
    h.run_steps(3);
    assert_eq!(h.query_all_by_label("Mixing Strength").count(), 2, "the row: its caption and its slider");
    assert_eq!(h.state().session.prefs().painting.pigment_mix_strength, 0.0);
    type_into(&mut h, egui::accesskit::Role::SpinButton, 0, "70");
    assert_eq!(h.state().session.prefs().painting.pigment_mix_strength, 70.0);
    assert!((h.state().session.tools.brush.pigment_mix_strength - 0.7).abs() < 1e-6, "the engine copies it onto the brush");
}
