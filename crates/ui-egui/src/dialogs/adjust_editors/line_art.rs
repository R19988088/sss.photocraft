//! Line art (线条强化) properties: how much ink the edges take, how sharp and how thick the
//! lines are, which tones they touch, their colour, and how they meet the artwork.

use super::basic::sliders;
use super::{Edit, EditorCx, Value, adjust_params, color_button, json, widgets};

/// C3's four blend modes, by params value and label.
const BLENDS: [(&str, &str); 4] = [("normal", "Normal"), ("multiply", "Multiply"), ("screen", "Screen"), ("overlay", "Overlay")];

pub(super) fn line_art(ui: &mut egui::Ui, v: &mut Value, cx: &EditorCx) -> Edit {
    let mut e = Edit::default();

    e.add(sliders(
        ui,
        v,
        &[
            ("intensity", "Intensity", 0.0, 300.0, 100.0, "%", None),
            ("sharpness", "Sharpness", 0.0, 100.0, 100.0, "%", None),
            ("thickness", "Thickness", 0.1, 10.0, 13.0, " px", Some(1)),
        ],
    ));

    // Which tones of the artwork the lines may touch.
    e.add(sliders(
        ui,
        v,
        &[
            ("shadows", "Shadows", 0.0, 100.0, 24.0, "%", None),
            ("midtones", "Midtones", 0.0, 100.0, 32.0, "%", None),
            ("highlights", "Highlights", 0.0, 100.0, 90.0, "%", None),
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
