//! The Color panel's faces: the reference layout (C2, [`super::color_c2`]), Photoshop's Hue Cube
//! ([`crate::panels::color_field`]) and Color Wheel, and the 色条 sliders (HSB, Lab).
//!
//! The faces come in two kinds and a face is only ever one of them. A disc puts the colour on a
//! field — the C2 square and its hue strip, the hue cube, the color wheel — and carries no sliders.
//! A strip shows the colour as sliders and carries no field. The panel menu groups them 色盘 and
//! 色条, and a panel never mixes the two, so the colour area is one thing or the other.

use std::f32::consts::TAU;

use egui::{Color32, Mesh, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use crate::PhotocraftApp;
use crate::color_picker_ui::{hex, out_of_gamut};
use crate::panels::{field_chips, field_hsva, srgb_hsva};
use crate::state::{ColorPanelMode, ColorPanelState};
use crate::theme::Tokens;
use crate::widgets;
use photocraft_color::convert;
use photocraft_color::convert::{hsb_to_rgb, rgb_to_hsb};

/// Width of the chips column, as the cube field's.
const CHIPS_W: f32 = 38.0;
/// Height of one slider row.
const ROW_H: f32 = 22.0;
/// Width of a slider row's value field.
const FIELD_W: f32 = 44.0;
/// Width of the unit after a field ("°", "%").
const UNIT_W: f32 = 14.0;

/// The colour model a strip edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Model {
    Hsb,
    Lab,
}

impl Model {
    pub(crate) fn of(mode: ColorPanelMode) -> Model {
        match mode {
            ColorPanelMode::LabSliders => Model::Lab,
            _ => Model::Hsb,
        }
    }

    /// Each slider: label, range and unit, in Photoshop's order.
    pub(crate) fn channels(self) -> &'static [(&'static str, f32, f32, &'static str)] {
        match self {
            Model::Hsb => &[("H", 0.0, 360.0, "°"), ("S", 0.0, 100.0, "%"), ("B", 0.0, 100.0, "%")],
            Model::Lab => &[("L", 0.0, 100.0, ""), ("a", -128.0, 127.0, ""), ("b", -128.0, 127.0, "")],
        }
    }

    /// The sRGB colour of slider values `v` (in the units the fields show).
    pub(crate) fn to_rgb(self, v: [f32; 4]) -> [f32; 3] {
        let [a, b, c, _] = v.map(|x| if x.is_finite() { x } else { 0.0 });
        let rgb = match self {
            Model::Hsb => hsb_to_rgb([a, b / 100.0, c / 100.0]),
            Model::Lab => convert::lab_to_srgb([a, b, c]),
        };
        rgb.map(|x| if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 })
    }

    /// Slider values of `rgb`, rounded to whole numbers as Photoshop's fields show them.
    pub(crate) fn values_of(self, rgb: [f32; 3]) -> [f32; 4] {
        let v = match self {
            Model::Hsb => {
                let [h, s, b] = rgb_to_hsb(rgb);
                [h, s * 100.0, b * 100.0, 0.0]
            }
            Model::Lab => {
                let [l, a, b] = convert::srgb_to_lab(rgb);
                [l, a, b, 0.0]
            }
        };
        v.map(|x| if x.is_finite() { x.round() } else { 0.0 })
    }
}

/// The colour the panel edits.
pub(crate) fn active(app: &PhotocraftApp) -> [f32; 3] {
    let c = if app.ui.color_panel.background { app.session.tools.background } else { app.session.tools.foreground };
    [c[0], c[1], c[2]].map(|x| x.clamp(0.0, 1.0))
}

/// Set the background colour, or the foreground and recolour what is selected as the tool in hand
/// says (`apply_color`); the picks of one drag or typed value are one history step.
///
/// Picking a solid colour while 图案 is armed keeps 图案 on: the picked colour paints the stroke
/// instead of the patch sampled earlier, so the mode can output a single colour too. 图案 itself
/// (or a new sample) brings the patch back.
pub(crate) fn set_color(app: &mut PhotocraftApp, ctx: &egui::Context, background: bool, rgb: [f32; 3]) {
    let c = [rgb[0], rgb[1], rgb[2], 1.0];
    if background {
        app.session.tools.background = c;
    } else {
        app.session.tools.foreground = c;
        crate::apply_color::foreground_changed(app, crate::apply_color::gesture(ctx));
    }
}

/// The key of the slider values last shown for `model`, kept while they still give the colour:
/// a grey has no hue, and Lab values don't survive an sRGB round trip (#3228).
fn values_key(model: Model, background: bool) -> egui::Id {
    egui::Id::unique(("color-panel-values", model, background))
}

/// The key of the hue cube's remembered hue and saturation (see [`field_hsva`]).
fn field_key(background: bool) -> egui::Id {
    egui::Id::unique(("color-field-hue", background))
}

/// Slider values for the colour the panel edits: the ones last shown while they still give it.
fn current_values(ui: &egui::Ui, model: Model, background: bool, rgb: [f32; 3]) -> [f32; 4] {
    let stored: Option<[f32; 4]> = ui.data(|d| d.get_temp(values_key(model, background)));
    if let Some(v) = stored.filter(|v| hex(model.to_rgb(*v)) == hex(rgb)) {
        return v;
    }
    if model == Model::Hsb {
        // Share the cube's hue and saturation, so a grey keeps its hue across faces.
        let c = [rgb[0], rgb[1], rgb[2], 1.0];
        let hsva = srgb_hsva(c);
        let remembered: (f32, f32) = ui.data(|d| d.get_temp(field_key(background))).unwrap_or((hsva.h, hsva.s));
        let h = field_hsva(hsva, remembered);
        return [h.h * 360.0, h.s * 100.0, h.v * 100.0, 0.0].map(f32::round);
    }
    model.values_of(rgb)
}

/// Apply edited slider values: the colour, and the values (and the cube's hue) remembered.
fn apply_values(app: &mut PhotocraftApp, ui: &egui::Ui, model: Model, v: [f32; 4]) {
    let background = app.ui.color_panel.background;
    let rgb = model.to_rgb(v);
    set_color(app, ui.ctx(), background, rgb);
    ui.data_mut(|d| d.insert_temp(values_key(model, background), v));
    if model == Model::Hsb && v[2] > 0.5 {
        ui.data_mut(|d| d.insert_temp(field_key(background), (v[0] / 360.0, v[1] / 100.0)));
    }
}

/// Track colours of slider `i` at `steps` + 1 points, the other sliders at `v`.
fn track_stops(model: Model, v: [f32; 4], i: usize, steps: usize) -> Vec<Color32> {
    let Some(&(_, lo, hi, _)) = model.channels().get(i) else { return Vec::new() };
    (0..=steps)
        .map(|k| {
            let mut w = v;
            if let Some(slot) = w.get_mut(i) {
                *slot = lo + (hi - lo) * k as f32 / steps as f32;
            }
            // HSB's hue track is the full spectrum, whatever the saturation and brightness.
            if model == Model::Hsb && i == 0 {
                w = [w[0], 100.0, 100.0, 0.0];
            }
            rgb32(model.to_rgb(w))
        })
        .collect()
}

pub(crate) fn rgb32(c: [f32; 3]) -> Color32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(q(c[0]), q(c[1]), q(c[2]))
}

/// The accessible name of a slider row's number field. Its channel letter alone ("a") says nothing
/// when it is read out, so the model's own word names the field.
fn channel_name(model: Model, label: &'static str) -> String {
    let full = match (model, label) {
        (Model::Hsb, "H") => tl!("Hue"),
        (Model::Hsb, "S") => tl!("Saturation"),
        (Model::Hsb, "B") => tl!("Brightness"),
        (_, "L") => tl!("Lightness"),
        (_, other) => other,
    };
    full.to_string()
}

/// The name of a slider row's number field: the model's word when `named`, its letter otherwise
/// (see [`slider_rows`]).
fn field_name(model: Model, label: &'static str, named: bool) -> String {
    // The track beside it carries the letter ("H"): the field's own name needs the word to be
    // unique as well as said out loud, so a named face reads "Hue", a lettered one "H value".
    if named { channel_name(model, label) } else { format!("{label} value") }
}

/// The slider rows of `model`. Returns the edited values, if any. `named` gives each row's field
/// the model's full word ("Hue", "Saturation") instead of its letter; the strip faces keep
/// Photoshop's letters, where "H" is what the row is.
pub(super) fn slider_rows(ui: &mut egui::Ui, model: Model, v: [f32; 4], background: bool, dynamic: bool, named: bool) -> Option<[f32; 4]> {
    let t = Tokens::get(ui.ctx());
    // Dynamic Color Sliders off: the tracks keep the colours they had when a drag began.
    let stops_key = egui::Id::unique(("color-panel-stops", model, background));
    let dragging = ui.input(|i| i.pointer.any_down());
    let stops_from = if dynamic || !dragging { v } else { ui.data(|d| d.get_temp::<[f32; 4]>(stops_key)).unwrap_or(v) };
    if !dragging {
        ui.data_mut(|d| d.insert_temp(stops_key, v));
    }
    let mut out = None;
    let mut values = v;
    for (i, &(label, lo, hi, unit)) in model.channels().iter().enumerate() {
        let Some(&current) = values.get(i) else { continue };
        let mut x = current;
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.set_height(ROW_H);
            let (lr, _) = ui.allocate_exact_size(vec2(12.0, ROW_H), Sense::hover());
            ui.painter().text(lr.left_center(), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(12.0), t.text_dim);
            let track_w = (ui.available_width() - FIELD_W - UNIT_W - 8.0).max(24.0);
            let (tr, resp) = ui.allocate_exact_size(vec2(track_w, ROW_H), Sense::click_and_drag());
            let stops = track_stops(model, stops_from, i, 24);
            changed |= track(ui, tr, &resp, &mut x, lo, hi, &stops);
            resp.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), f64::from(x), label));
            if widgets::value_field_named(ui, &field_name(model, label, named), &mut x, lo..=hi, "", FIELD_W).changed() {
                changed = true;
            }
            let (ur, _) = ui.allocate_exact_size(vec2(UNIT_W, ROW_H), Sense::hover());
            ui.painter().text(ur.left_center(), egui::Align2::LEFT_CENTER, unit, egui::FontId::proportional(12.0), t.text_faint);
        });
        if changed && let Some(slot) = values.get_mut(i) {
            *slot = x.clamp(lo, hi).round();
            out = Some(values);
        }
    }
    out
}

/// A slider track with Photoshop's triangle thumb under it. Returns true when dragged or clicked
/// to a new value.
fn track(ui: &egui::Ui, r: Rect, resp: &egui::Response, value: &mut f32, lo: f32, hi: f32, stops: &[Color32]) -> bool {
    let t = Tokens::get(ui.ctx());
    let bar = Rect::from_min_max(pos2(r.left() + 4.0, r.center().y - 6.0), pos2(r.right() - 4.0, r.center().y + 1.0));
    let mut mesh = Mesh::default();
    let n = stops.len().saturating_sub(1).max(1);
    for (k, c) in stops.iter().enumerate() {
        let x = bar.left() + bar.width() * k as f32 / n as f32;
        mesh.colored_vertex(pos2(x, bar.top()), *c);
        mesh.colored_vertex(pos2(x, bar.bottom()), *c);
    }
    for k in 0..stops.len().saturating_sub(1) as u32 {
        let a = k * 2;
        mesh.add_triangle(a, a + 1, a + 3);
        mesh.add_triangle(a, a + 3, a + 2);
    }
    ui.painter().add(mesh);
    ui.painter().rect_stroke(bar, 0.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
    let mut changed = false;
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
        && bar.width() > 0.0
        && hi > lo
    {
        let f = ((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
        let v = (lo + (hi - lo) * f).round().clamp(lo, hi);
        if v != *value {
            *value = v;
            changed = true;
        }
    }
    let f = if hi > lo { ((*value - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
    let x = bar.left() + bar.width() * f;
    let y = bar.bottom() + 1.0;
    let thumb = vec![pos2(x, y), pos2(x + 5.0, y + 7.0), pos2(x - 5.0, y + 7.0)];
    ui.painter().add(egui::Shape::convex_polygon(thumb, t.text, Stroke::new(1.0, t.field_border)));
    changed
}

/// The 色条 faces: chips (with the out-of-gamut alert under them) beside the sliders. No ramp and
/// no field of any kind under them: a strip is nothing but strips.
fn sliders(app: &mut PhotocraftApp, ui: &mut egui::Ui, mode: ColorPanelMode) {
    let model = Model::of(mode);
    let background = app.ui.color_panel.background;
    let rgb = active(app);
    let v = current_values(ui, model, background, rgb);
    let dynamic = app.session.prefs().interface.dynamic_color_sliders;
    let rows_h = model.channels().len() as f32 * (ROW_H + ui.spacing().item_spacing.y);
    let top_h = rows_h.max(CHIPS_W + 40.0);
    let mut edited = None;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (col, _) = ui.allocate_exact_size(vec2(CHIPS_W, top_h), Sense::hover());
        field_chips(app, ui, col);
        alerts(app, ui, col, rgb);
        ui.vertical(|ui| {
            edited = slider_rows(ui, model, v, background, dynamic, false);
        });
    });
    if let Some(v) = edited {
        apply_values(app, ui, model, v);
    }
}

/// Photoshop's alert under the chips: ⚠ when the colour can't print, and a click takes the nearest
/// printable colour.
pub(crate) fn alerts(app: &mut PhotocraftApp, ui: &mut egui::Ui, col: Rect, rgb: [f32; 3]) {
    let t = Tokens::get(ui.ctx());
    let background = app.ui.color_panel.background;
    let y = col.top() + CHIPS_W + 2.0;
    let Some(near) = out_of_gamut(rgb) else { return };
    const TIP: &str = "Out of gamut for printing: click for the nearest printable color";
    let icon = Rect::from_min_size(pos2(col.left() + 1.0, y), vec2(14.0, 13.0));
    let chip = Rect::from_min_size(pos2(col.left() + 18.0, y), vec2(13.0, 13.0));
    crate::color_picker_ui::warning_triangle(ui.painter(), icon, t.text_dim, t.card);
    ui.painter().rect_filled(chip, 0.0, rgb32(near));
    ui.painter().rect_stroke(chip, 0.0, Stroke::new(1.0, t.text_dim), StrokeKind::Outside);
    let resp = ui.interact(icon.union(chip), ui.scope_id().with("color-panel-alert"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), TIP));
    if resp.on_hover_text(tl!(TIP)).clicked() {
        set_color(app, ui.ctx(), background, near);
    }
}

/// The Color Wheel: a hue ring (red at the right, running counter-clockwise through yellow and
/// green) around a triangle whose tip is the hue, with white and black at its other corners; the
/// add-to-swatches button sits under it. A disc of its own: no sliders above it.
fn wheel(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let background = app.ui.color_panel.background;
    let rgb = active(app);
    let mut v = current_values(ui, Model::Hsb, background, rgb);
    let mut changed = false;
    // The chips row: the foreground and background the wheel edits, nothing else.
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (col, _) = ui.allocate_exact_size(vec2(CHIPS_W, CHIPS_W), Sense::hover());
        field_chips(app, ui, col);
    });
    ui.add_space(6.0);
    // The wheel takes the palette's square, as every face does: the reference layout's 168 pt
    // picker at the dock's width, never the height left in the group, so switching faces never
    // changes the colour area's size or shape.
    let w = ui.available_width();
    let side = crate::panels::palette_picker_side(w);
    let (area, resp) = ui.allocate_exact_size(vec2(w, side), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Unknown, ui.is_enabled(), "Color Wheel"));
    let (center, inner, outer) = wheel_geometry(area);
    let ring_w = outer - inner;
    let (h, s, b) = (v[0] / 360.0, v[1] / 100.0, v[2] / 100.0);
    ui.painter().add(ring_mesh(center, inner, outer));
    let tri = triangle(center, inner - 1.0, h);
    ui.painter().add(triangle_mesh(tri, h));
    // Which part a drag began on: the ring sets the hue, the triangle saturation and brightness.
    // The ring or the triangle, from where the press began (kept for frames without a press
    // origin), so a drag that leaves the ring keeps turning the hue.
    let drag_key = ui.scope_id().with("color-wheel-drag");
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        // A click is where it landed; a drag goes by where its press began.
        let origin = if resp.clicked() { Some(p) } else { ui.input(|i| i.pointer.press_origin()) };
        // `None`: the press began beside the wheel (the area spans the panel), which picks nothing.
        let part = match origin {
            Some(origin) => {
                let d = origin.distance(center);
                // The hue knob reaches past the ring's edge on a small wheel.
                let part = (d <= outer + 7.0).then_some(d >= inner - 2.0);
                ui.data_mut(|data| data.insert_temp(drag_key, part));
                part
            }
            None => ui.data(|d| d.get_temp::<Option<bool>>(drag_key)).unwrap_or(Some(false)),
        };
        match part {
            Some(true) => {
                let a = (center.y - p.y).atan2(p.x - center.x).rem_euclid(TAU);
                v[0] = (a / TAU * 360.0).round().rem_euclid(360.0);
                changed = true;
            }
            Some(false) => {
                if let Some((s2, b2)) = triangle_sv(tri, p) {
                    v[1] = (s2 * 100.0).round();
                    v[2] = (b2 * 100.0).round();
                }
                changed = true;
            }
            None => {}
        }
    }
    // Knobs: on the ring at the hue, in the triangle at the saturation and brightness.
    let a = h * TAU;
    let ring_knob = center + vec2(a.cos(), -a.sin()) * (inner + ring_w / 2.0);
    let tri_knob = triangle_point(tri, s, b);
    for k in [ring_knob, tri_knob] {
        ui.painter().circle_stroke(k, 5.0, Stroke::new(1.5, Color32::WHITE));
        ui.painter().circle_stroke(k, 6.5, Stroke::new(1.0, Color32::from_black_alpha(160)));
    }
    // Photoshop's add-to-swatches button under the wheel.
    let add = Rect::from_min_size(pos2(area.right() - 16.0, area.bottom() - 16.0), vec2(14.0, 14.0));
    ui.painter().rect_stroke(add, 1.0, Stroke::new(1.0, t.text_dim), StrokeKind::Inside);
    ui.painter().line_segment([pos2(add.center().x, add.top() + 3.5), pos2(add.center().x, add.bottom() - 3.5)], Stroke::new(1.0, t.text_dim));
    ui.painter().line_segment([pos2(add.left() + 3.5, add.center().y), pos2(add.right() - 3.5, add.center().y)], Stroke::new(1.0, t.text_dim));
    let add_resp = ui.interact(add, ui.scope_id().with("color-wheel-add"), Sense::click());
    add_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), "Add to Swatches"));
    if add_resp.on_hover_text(tl!("Create new swatch")).clicked()
        && let Err(e) = app.run("swatches.add", json!({"color": hex(rgb)}))
    {
        app.ui.status = e;
        app.ui.status_error = true;
    }
    if changed {
        apply_values(app, ui, Model::Hsb, v);
    }
}

/// The wheel in `area`: its centre and the hue ring's inner and outer radii.
pub(crate) fn wheel_geometry(area: Rect) -> (Pos2, f32, f32) {
    let outer = (area.width().min(area.height()) / 2.0 - 2.0).max(1.0);
    let ring_w = (outer * 0.09).clamp(6.0, 22.0).min(outer * 0.5);
    (area.center(), outer - ring_w, outer)
}

/// The wheel's hue ring between radii `inner` and `outer`.
fn ring_mesh(center: Pos2, inner: f32, outer: f32) -> Mesh {
    let mut m = Mesh::default();
    let steps = 96u32;
    for k in 0..=steps {
        let f = k as f32 / steps as f32;
        let a = f * TAU;
        let c = rgb32(hsb_to_rgb([f * 360.0, 1.0, 1.0]));
        let dir = vec2(a.cos(), -a.sin());
        m.colored_vertex(center + dir * inner, c);
        m.colored_vertex(center + dir * outer, c);
    }
    for k in 0..steps {
        let a = k * 2;
        m.add_triangle(a, a + 1, a + 3);
        m.add_triangle(a, a + 3, a + 2);
    }
    m
}

/// The triangle's corners for hue `h` (0..1): the pure hue at the hue's angle, white 120° on and
/// black 240° on, inside radius `r`.
fn triangle(center: Pos2, r: f32, h: f32) -> [Pos2; 3] {
    let at = |turns: f32| {
        let a = (h + turns) * TAU;
        center + vec2(a.cos(), -a.sin()) * r
    };
    [at(0.0), at(1.0 / 3.0), at(2.0 / 3.0)]
}

/// The point of saturation `s` and brightness `b` in the triangle: black + b·(white − black) +
/// b·s·(hue − white).
fn triangle_point([hue, white, black]: [Pos2; 3], s: f32, b: f32) -> Pos2 {
    black + (white - black) * b + (hue - white) * (b * s)
}

/// Saturation and brightness at `p`, clamped into the triangle; `None` for a degenerate one or a
/// non-finite `p`.
pub(crate) fn triangle_sv([hue, white, black]: [Pos2; 3], p: Pos2) -> Option<(f32, f32)> {
    let (a, c, d) = (white - black, hue - white, p - black);
    let det = a.x * c.y - a.y * c.x;
    if det.abs() < 1e-6 {
        return None;
    }
    let (b, bs) = ((d.x * c.y - d.y * c.x) / det, (a.x * d.y - a.y * d.x) / det);
    // A NaN `b` would panic as a clamp bound.
    if !b.is_finite() || !bs.is_finite() {
        return None;
    }
    let b = b.clamp(0.0, 1.0);
    let bs = bs.clamp(0.0, b);
    Some((if b > 0.0 { bs / b } else { 0.0 }, b))
}

/// The triangle shaded through saturation and brightness at hue `h`.
fn triangle_mesh(tri: [Pos2; 3], h: f32) -> Mesh {
    let n = 16usize;
    let mut m = Mesh::default();
    for j in 0..=n {
        for i in 0..=n {
            let (s, b) = (i as f32 / n as f32, j as f32 / n as f32);
            m.colored_vertex(triangle_point(tri, s, b), rgb32(hsb_to_rgb([h * 360.0, s, b])));
        }
    }
    let row = (n + 1) as u32;
    for j in 0..n as u32 {
        for i in 0..n as u32 {
            let a = j * row + i;
            m.add_triangle(a, a + 1, a + row + 1);
            m.add_triangle(a, a + row + 1, a + row);
        }
    }
    m
}

/// `color="#rrggbb"`, what Copy Color as HTML puts on the clipboard.
pub(crate) fn html_color(rgb: [f32; 3]) -> String {
    format!("color=\"{}\"", hex(rgb))
}

/// `rrggbb`, what Copy Color's Hex Code puts on the clipboard.
pub(crate) fn hex_code(rgb: [f32; 3]) -> String {
    hex(rgb).trim_start_matches('#').to_string()
}

/// A panel menu row with Photoshop's check mark column: one full-width row, its text left, so the
/// whole row takes the click and the menu keeps its narrow width.
fn check_item(ui: &mut egui::Ui, on: bool, label: &str) -> bool {
    let t = Tokens::get(ui.ctx());
    let text = format!("{}{}", if on { "✔ " } else { "   " }, tl!(label));
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    ui.painter().text(rect.left_center() + vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, &text, egui::FontId::proportional(13.0), t.text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), text.clone()));
    resp.clicked()
}

/// The Color tab: whatever the panel menu picked. A 色盘 face draws a field, a 色条 face draws
/// sliders; neither face draws the other kind.
pub(crate) fn panel(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    match app.ui.color_panel.mode {
        ColorPanelMode::C2 => super::color_c2::show(app, ui),
        ColorPanelMode::HueCube => crate::panels::color_field(app, ui),
        ColorPanelMode::ColorWheel => wheel(app, ui),
        mode => sliders(app, ui, mode),
    }
    crate::apply_color::track_focus(ui.ctx());
}

/// The width of the Color panel's menu, in points. A right-click on the panel anchors the popup to
/// the whole panel's rect and egui would stretch the menu to it, so the menu pins its own width.
const MENU_WIDTH: f32 = 200.0;

/// A section heading in the panel menu (色盘, 色条): dim text over the items it titles, with no
/// check column of its own, so it reads as a group rather than a choice.
fn section(ui: &mut egui::Ui, label: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(2.0);
    ui.label(egui::RichText::new(tl!(label)).color(t.text_dim).size(12.0));
}

/// The Color panel's panel menu, Photoshop's: the faces in two groups (色盘 and 色条), the
/// mixing strength, the copy commands. Two narrow pages — the faces and the brush control, then
/// the commands — with a page dot under them, so the menu never grows into a column wider or
/// taller than the panel it belongs to.
pub(crate) fn panel_menu(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    ui.set_width(MENU_WIDTH);
    ui.set_max_width(MENU_WIDTH);
    const PAGES: usize = 2;
    // The page the menu is on, kept across frames while it stays open.
    let page_id = ui.make_persistent_id("color_panel_menu_page");
    let page = ui.data(|d| d.get_temp::<usize>(page_id)).unwrap_or(0);
    if page == 0 {
        // 色盘: the faces that put the colour on a disc or a square (the reference layout first and
        // the default). A pick is remembered in the session.
        section(ui, "Color Discs");
        for mode in ColorPanelMode::DISC {
            if check_item(ui, app.ui.color_panel.mode == mode, mode.label()) {
                app.ui.color_panel.mode = mode;
                ui.close();
            }
        }
        ui.separator();
        // 色条: the faces that show the colour as sliders, and only as sliders.
        section(ui, "Color Strips");
        for mode in ColorPanelMode::STRIP {
            if check_item(ui, app.ui.color_panel.mode == mode, mode.label()) {
                app.ui.color_panel.mode = mode;
                ui.close();
            }
        }
        ui.separator();
        // 混合强度: the brush's pigment mixing, one value for every brush, beside the colour field
        // as the reference application keeps it. The slider writes the `painting.pigmentMixStrength`
        // preference and the engine copies it onto the brush, so it survives a restart with the rest
        // of the preferences.
        let mut strength = app.session.prefs().painting.pigment_mix_strength;
        if widgets::slider_row(ui, tl!("Mixing Strength"), &mut strength, 0.0..=100.0, "%", None).changed() {
            if let Err(e) = app.run("prefs.set", json!({"path": "painting.pigmentMixStrength", "value": strength})) {
                app.ui.status = e;
                app.ui.status_error = true;
            }
        }
    } else {
        let rgb = active(app);
        if ui.button(tl!("Copy Color as HTML")).clicked() {
            ui.ctx().copy_text(html_color(rgb));
            ui.close();
        }
        if ui.button(tl!("Copy Color's Hex Code")).clicked() {
            ui.ctx().copy_text(hex_code(rgb));
            ui.close();
        }
    }
    page_dots(ui, page_id, page, PAGES);
}

/// The page dots under a paged panel menu: one dot per page, the current one filled. A click
/// turns the menu to that page.
fn page_dots(ui: &mut egui::Ui, id: egui::Id, page: usize, pages: usize) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.separator();
    ui.horizontal(|ui| {
        let dot = 9.0;
        let gap = 6.0;
        let total = pages as f32 * dot + (pages.saturating_sub(1)) as f32 * gap;
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        for i in 0..pages {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(dot, dot), egui::Sense::click());
            let on = i == page;
            ui.painter().circle_filled(rect.center(), dot / 2.0, if on { t.text } else { t.text_faint });
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &format!("{} {}", tl!("Page"), i + 1)));
            if response.clicked() {
                ui.data_mut(|d| d.insert_temp(id, i));
                ui.ctx().request_repaint();
            }
            if i + 1 < pages {
                ui.add_space(gap);
            }
        }
    });
}

/// Remember the Color panel's options in the preferences when they change, so they survive a
/// restart (#9592). Cheap: a small JSON compare per frame.
pub fn persist(app: &mut PhotocraftApp) {
    let stored = app.session.prefs().color_panel.clone();
    // Changed outside the panel since the last sync (`prefs.set`, a reset): the panel takes it.
    if app.ui.color_panel_synced.as_ref().is_some_and(|synced| *synced != stored) {
        restore(app);
        app.ui.color_panel_synced = Some(stored);
        return;
    }
    let Ok(now) = serde_json::to_value(app.ui.color_panel) else { return };
    if stored != now {
        app.session.prefs.edit(|p| p.color_panel = now.clone());
    }
    app.ui.color_panel_synced = Some(now);
}

/// Restore the remembered Color panel options (at launch, or after a preference change made
/// outside the panel); none stored (a reset) or unreadable ones are the defaults. The display face
/// is the reference layout (C2) at launch whatever the file says: the panel menu's pick lasts the
/// session, a stored face never comes back.
pub fn restore(app: &mut PhotocraftApp) {
    let mut state: ColorPanelState = serde_json::from_value(app.session.prefs().color_panel.clone()).unwrap_or_default();
    state.mode = ColorPanelMode::C2;
    app.ui.color_panel = state;
}
