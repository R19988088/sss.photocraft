//! Line art (线条强化) properties: how much ink the edges take, how sharp and how thick the
//! lines are, which tones they touch, their colour, and how they meet the artwork.
//!
//! The pass has two halves. The expensive one is edge strength (`maxDiff`, one ring of up to
//! ~440 taps per pixel); it depends on the artwork below and on the thickness alone, and the
//! compositor caches it as C3 does (`Compositor::sync_fields`). Everything else — intensity,
//! sharpness, the three zone opacities, the ink colour and the blend — is a pointwise read of
//! that field, which is why those controls preview live and cost nothing.
//!
//! Thickness is the one control that invalidates the field, so it does *not* preview while it is
//! dragged: the row holds the dragged value in view state, the canvas keeps showing the committed
//! one, and a single render follows the release. That is the whole rule — no field in the drag
//! loop, one field per gesture.

use super::basic::{row_value, sliders};
use super::{Edit, EditorCx, Value, adjust_params, color_button, json, widgets};
use photocraft_compose::adjust::{DEFAULT_HIGHLIGHTS, DEFAULT_INTENSITY, DEFAULT_MIDTONES, DEFAULT_SHADOWS, DEFAULT_SHARPNESS, DEFAULT_THICKNESS, THICKNESS_MAX, THICKNESS_MIN};

/// C3's four blend modes, by params value and label.
const BLENDS: [(&str, &str); 4] = [("normal", "Normal"), ("multiply", "Multiply"), ("screen", "Screen"), ("overlay", "Overlay")];

pub(super) fn line_art(ui: &mut egui::Ui, v: &mut Value, cx: &EditorCx) -> Edit {
    let mut e = Edit::default();

    // Pointwise reads of the cached field: these preview live.
    e.add(sliders(
        ui,
        v,
        &[
            ("intensity", "Intensity", 0.0, 300.0, DEFAULT_INTENSITY, "%", None),
            ("sharpness", "Sharpness", 0.0, 100.0, DEFAULT_SHARPNESS, "%", None),
        ],
    ));
    e.add(thickness_row(ui, v, cx));

    // Which tones of the artwork the lines may touch.
    e.add(sliders(
        ui,
        v,
        &[
            ("shadows", "Shadows", 0.0, 100.0, DEFAULT_SHADOWS, "%", None),
            ("midtones", "Midtones", 0.0, 100.0, DEFAULT_MIDTONES, "%", None),
            ("highlights", "Highlights", 0.0, 100.0, DEFAULT_HIGHLIGHTS, "%", None),
        ],
    ));

    // How the ink meets the artwork.
    let mut blend = v.get("blend").and_then(Value::as_str).unwrap_or("multiply").to_string();
    if !BLENDS.iter().any(|(key, _)| *key == blend) {
        blend = "multiply".to_string();
    }
    let opts: Vec<(String, &str)> = BLENDS.iter().map(|(key, label)| ((*key).to_string(), *label)).collect();
    if widgets::dropdown(ui, &format!("{:?}-la-blend", cx.mem), "Blend:", &mut blend, &opts, 150.0) {
        v["blend"] = json!(blend);
        e.add(Edit::discrete(true));
    }

    let mut color = super::rgb_of(v, "color", [0.0, 0.0, 0.0]);
    ui.horizontal(|ui| {
        super::label(ui, "Line Color:");
        let r = color_button(ui, &mut color, "Line Art");
        if r.changed {
            v["color"] = json!(adjust_params::hex(color));
        }
        e.add(r);
    });
    e
}

/// Thickness: the field's ring depends on it, so the control previews on release, never during the
/// drag. The dragged value lives in view state until the gesture ends; then it is written into the
/// params, which commits one `layer.setAdjustment` and one field rebuild.
fn thickness_row(ui: &mut egui::Ui, v: &mut Value, cx: &EditorCx) -> Edit {
    let pending = cx.mem.with("line-thickness-drag");
    let held: Option<f32> = ui.data(|d| d.get_temp(pending));
    let mut t = held.unwrap_or_else(|| super::num(v, "thickness", DEFAULT_THICKNESS).clamp(THICKNESS_MIN, THICKNESS_MAX));
    let gestures = widgets::RowGestures { reset: Some(DEFAULT_THICKNESS), wheel_step: None, ..Default::default() };
    let r = widgets::slider_row_prec(ui, "Thickness", &mut t, THICKNESS_MIN..=THICKNESS_MAX, " px", None, gestures, Some(1));
    let mut e = Edit::default();
    if r.dragged() {
        // Show the drag on the row, tell the host nothing: no field, no full-size render.
        ui.data_mut(|d| d.insert_temp(pending, t));
        return e;
    }
    if held.is_some() || r.changed() {
        ui.data_mut(|d| d.remove::<f32>(pending));
        v["thickness"] = json!(row_value(t, THICKNESS_MIN, THICKNESS_MAX, Some(1)));
        // A finished drag commits without previewing (the canvas was never showing it); a wheel or
        // arrow step commits through the ordinary path.
        if held.is_some() {
            e.commit = true;
        } else {
            e.add(Edit::of(&r));
        }
    }
    e
}
