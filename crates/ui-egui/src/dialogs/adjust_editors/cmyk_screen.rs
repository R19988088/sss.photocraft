//! CMYK screen print (CMYK 网点) properties: the dot period, its angle, and how much ink each
//! plate lays down. C3 keeps this as its own adjustment kind, so it is its own layer here too.

use super::basic::sliders;
use super::{Edit, EditorCx, Value};

pub(super) fn cmyk_screen(ui: &mut egui::Ui, v: &mut Value, _cx: &EditorCx) -> Edit {
    sliders(
        ui,
        v,
        &[
            ("dotSize", "Dot Size", 2.0, 96.0, 82.0, " px", None),
            ("angle", "Screen Angle", -180.0, 180.0, 15.0, "°", None),
            // Each plate's ink; negative values thin the ink instead of adding it (C3's own
            // strength range), so 0 is "as the artwork has it".
            ("cyan", "Cyan", -100.0, 100.0, 0.0, "%", None),
            ("magenta", "Magenta", -100.0, 100.0, 0.0, "%", None),
            ("yellow", "Yellow", -100.0, 100.0, 0.0, "%", None),
            ("black", "Black", -100.0, 100.0, 0.0, "%", None),
        ],
    )
}
