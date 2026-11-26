//! The Color panel tab is Photoshop's spectrum field in every theme (#4367, #6000): the light
//! themes used to show HSB sliders only, then a short field with the sliders under it. A 色盘 face
//! draws the field and nothing else.

use egui_kittest::kittest::Queryable;

use crate::PhotocraftApp;
use crate::dock::Group;
use crate::theme::{ThemeKind, Tokens};

/// The Color tab of `kind`, alone in a 300 pt wide panel, starting from a red foreground.
fn color_tab(kind: ThemeKind) -> egui_kittest::Harness<'static, PhotocraftApp> {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.session.tools.foreground = [1.0, 0.0, 0.0, 1.0];
    // These tests are about Photoshop's saturation/brightness field, not the C2 default layout.
    app.ui.color_panel.mode = crate::state::ColorPanelMode::HueCube;
    let tab = Group::Color.tabs(Tokens::for_kind(kind).pro).iter().position(|name| *name == "Color").unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(300.0, 400.0)).build_ui_state(
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

/// Drag on the saturation/brightness field at `y` below its top, on its left edge (no saturation).
fn pick_at(h: &mut egui_kittest::Harness<'static, PhotocraftApp>, y: f32) -> [f32; 4] {
    let field = h.get_by_label("Saturation / Brightness").rect();
    let p = field.left_top() + egui::vec2(1.0, y);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(2);
    h.state().session.tools.foreground
}

#[test]
fn every_theme_shows_the_same_spectrum_field() {
    for kind in ThemeKind::ALL {
        let mut h = color_tab(kind);
        // Halfway down its 168 pt: no saturation, half brightness.
        let [r, g, b, _] = pick_at(&mut h, 84.0);
        assert!((r - g).abs() < 0.02 && (g - b).abs() < 0.02 && (0.3..0.7).contains(&r), "{kind:?}: the field picks a mid grey, got {r} {g} {b}");
        // No theme swaps the field for sliders or adds them under it (#6000).
        for slider in ["Hue", "Saturation", "Brightness"] {
            assert!(h.query_all_by_label(slider).next().is_none(), "{kind:?}: no {slider} slider");
        }
    }
}
