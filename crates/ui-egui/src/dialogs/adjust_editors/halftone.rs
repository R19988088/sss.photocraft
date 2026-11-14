//! Halftone (网点 / comic screentone) properties: the screen's shape, its size and angle, the
//! tone it prints, and whether each channel gets its own screen. The panel follows the studio
//! theme's rows and sliders like every other adjustment editor, so a Halftone adjustment layer
//! reads as part of the same Properties panel.

use super::basic::sliders;
use super::{Edit, EditorCx, Value, flag, json, widgets};

/// Every dot shape, by its params key and its (translated) label.
const SHAPES: [(&str, &str); 6] = [("round", "Dot"), ("diamond", "Diamond"), ("ellipse", "Ellipse"), ("square", "Square"), ("cross", "Cross"), ("line", "Line")];

pub(super) fn halftone(ui: &mut egui::Ui, v: &mut Value, cx: &EditorCx) -> Edit {
    let mut e = Edit::default();

    // Shape of the dot (Photoshop's Bitmap › Halftone Screen set).
    let mut shape = v.get("shape").and_then(Value::as_str).unwrap_or("round").to_string();
    if !SHAPES.iter().any(|(key, _)| *key == shape) {
        shape = "round".to_string();
    }
    let opts: Vec<(String, &str)> = SHAPES.iter().map(|(key, label)| ((*key).to_string(), *label)).collect();
    if widgets::dropdown(ui, &format!("{:?}-ht-shape", cx.mem), "Shape:", &mut shape, &opts, 150.0) {
        v["shape"] = json!(shape);
        e.add(Edit::discrete(true));
    }

    // One screen on the grey tone, or one per channel at its own angle.
    let mut color = flag(v, "color", false);
    let (mut key, screens) = (if color { "color" } else { "mono" }, [("mono", "Grayscale"), ("color", "Color")]);
    if widgets::dropdown(ui, &format!("{:?}-ht-screen", cx.mem), "Screen:", &mut key, &screens, 150.0) {
        color = key == "color";
        v["color"] = json!(color);
        e.add(Edit::discrete(true));
    }

    // The screen itself: period and angle, the tone's response and the dot's gain.
    e.add(sliders(
        ui,
        v,
        &[
            ("size", "Screen Size", 1.0, 200.0, 8.0, " px", None),
            ("angle", "Screen Angle", -180.0, 180.0, 45.0, "°", None),
            ("density", "Dot Gain", 0.0, 100.0, 50.0, "%", None),
            ("brightness", "Brightness", -100.0, 100.0, 0.0, "", None),
            ("contrast", "Contrast", -100.0, 100.0, 0.0, "", None),
            ("jitter", "Roughness", 0.0, 100.0, 0.0, "%", None),
            // Tolerances: how much of the lightest and darkest tone prints as flat paper and ink.
            ("white", "White Strength", 0.0, 100.0, 0.0, "%", None),
            ("black", "Black Strength", 0.0, 100.0, 0.0, "%", None),
        ],
    ));

    let mut preserve = flag(v, "preserveWhite", true);
    if widgets::checkbox(ui, &mut preserve, "Preserve White").changed() {
        v["preserveWhite"] = json!(preserve);
        e.add(Edit::discrete(true));
    }
    e
}
