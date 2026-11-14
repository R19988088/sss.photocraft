//! The C2 Color panel layout: the reference application's own, side by side with Photoshop's
//! modes in [`super::color_panel_ui`]. A colour/pattern source switch, the colour chips, a large
//! saturation/brightness field with its hue strip, the hex and RGB readout, the recent colours and
//! the image palette.
//!
//! The geometry is the reference's 54 : 8 : 168 : 8 : 25 strip at the panel's width, so the whole
//! panel scales with the dock instead of its height.

use std::sync::Arc;

use egui::{Color32, Mesh, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use crate::PhotocraftApp;
use crate::panels::c32;
use crate::panels::color::{color_readout, hsva_color32, hsva_srgb, srgb_bytes, srgb_hsva};
use crate::theme::Tokens;

/// The C2 layout's proportions, at the reference's 263-point panel width.
const C2_WIDTH: f32 = 263.0;

/// Foreground and background chips, complementary colour, and the recent swatches: the column at
/// the field's left.
fn sources(app: &mut PhotocraftApp, ui: &mut egui::Ui, r: Rect) {
    let t = Tokens::get(ui.ctx());
    let width = r.width();
    let scale = width / 54.0;
    let fg = Rect::from_min_size(r.min, vec2(width, width));
    let response = ui.interact(fg, ui.id().with("c2-foreground"), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Foreground Color"));
    ui.painter().rect_filled(fg, t.radius_sm, c32(app.session.tools.foreground));
    ui.painter().rect_stroke(fg, t.radius_sm, Stroke::new(if app.ui.color_panel.background { 1. } else { 2. }, t.accent), StrokeKind::Inside);
    if response.clicked() {
        app.ui.color_panel.background = false;
        let _ = app.run("tools.setColorSource", json!({"pattern": false}));
    }
    if response.double_clicked() {
        crate::color_picker_ui::open(app, "foreground");
    }
    let bg = Rect::from_min_size(r.min + vec2(0., width + 4. * scale), vec2(width, 18. * scale));
    let response = ui.interact(bg, ui.id().with("c2-background"), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Background Color"));
    ui.painter().rect_filled(bg, t.radius_sm, c32(app.session.tools.background));
    ui.painter().rect_stroke(bg, t.radius_sm, Stroke::new(if app.ui.color_panel.background { 2. } else { 1. }, t.field_border), StrokeKind::Inside);
    if response.clicked() {
        app.ui.color_panel.background = true;
    }
    if response.double_clicked() {
        crate::color_picker_ui::open(app, "background");
    }
    // The complementary colour of the foreground: click it to paint with it.
    let c = srgb_hsva(app.session.tools.foreground);
    let complement = egui::ecolor::Hsva::new((c.h + 0.5).rem_euclid(1.), c.s, c.v, 1.);
    let comp = Rect::from_min_size(pos2(r.left(), bg.bottom() + 4. * scale), vec2(width, 18. * scale));
    ui.painter().rect_filled(comp, t.radius_sm, hsva_color32(complement));
    if ui.interact(comp, ui.id().with("c2-complement"), Sense::click()).on_hover_text(tl!("Complementary Color")).clicked() {
        app.ui.color_panel.background = false;
        super::color_panel_ui::set_color(app, ui.ctx(), false, panels_srgb(complement));
    }
    // The recent colours, two to a row.
    let size = (width - 5. * scale) * 0.5;
    let recent = app.ui.color_panel.recent;
    for (i, c) in recent.into_iter().enumerate() {
        let recent = Rect::from_min_size(
            pos2(r.left() + (i % 2) as f32 * (size + 5. * scale), comp.bottom() + 4. * scale + (i / 2) as f32 * 23. * scale),
            vec2(size, 18. * scale),
        );
        ui.painter().rect_filled(recent, t.radius_sm, if c[3] > 0. { c32(c) } else { t.field });
        ui.painter().rect_stroke(recent, t.radius_sm, Stroke::new(1., t.field_border), StrokeKind::Inside);
        if c[3] > 0. && ui.interact(recent, ui.id().with(("c2-recent", i)), Sense::click()).clicked() {
            super::color_panel_ui::set_color(app, ui.ctx(), false, [c[0], c[1], c[2]]);
        }
    }
}

/// A shaded quad grid over `rect`, coloured by `color(x, y)` with x, y in 0..1 (y down).
fn gradient(ui: &egui::Ui, rect: Rect, nx: u32, ny: u32, color: impl Fn(f32, f32) -> Color32) {
    let mut mesh = Mesh::default();
    for y in 0..=ny {
        for x in 0..=nx {
            let (u, v) = (x as f32 / nx as f32, y as f32 / ny as f32);
            mesh.colored_vertex(pos2(rect.left() + u * rect.width(), rect.top() + v * rect.height()), color(u, v));
        }
    }
    for y in 0..ny {
        for x in 0..nx {
            let a = y * (nx + 1) + x;
            mesh.add_triangle(a, a + 1, a + nx + 2);
            mesh.add_triangle(a, a + nx + 2, a + nx + 1);
        }
    }
    ui.painter().add(mesh);
}

fn knob(ui: &egui::Ui, p: egui::Pos2, color: Color32) {
    ui.painter().circle_filled(p, 6.0, color);
    ui.painter().circle_stroke(p, 6.5, Stroke::new(1.5, Color32::WHITE));
    ui.painter().circle_stroke(p, 8.0, Stroke::new(1.0, Color32::from_black_alpha(150)));
}

/// The saturation/brightness square. Returns true when the pointer picked a colour.
fn sv(ui: &mut egui::Ui, r: Rect, h: &mut egui::ecolor::Hsva) -> bool {
    gradient(ui, r, 20, 20, |s, v| hsva_color32(egui::ecolor::Hsva::new(h.h, s, 1. - v, 1.)));
    let response = ui.interact(r, ui.id().with("c2-sv"), Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Saturation / Brightness"));
    let picked = (response.clicked() || response.dragged()) && response.interact_pointer_pos().is_some();
    if picked && let Some(p) = response.interact_pointer_pos() {
        h.s = ((p.x - r.left()) / r.width()).clamp(0., 1.);
        h.v = (1. - (p.y - r.top()) / r.height()).clamp(0., 1.);
    }
    knob(ui, pos2(r.left() + h.s * r.width(), r.top() + (1. - h.v) * r.height()), hsva_color32(*h));
    picked
}

/// The vertical hue strip. Returns true when the pointer picked a hue.
fn hue(ui: &mut egui::Ui, r: Rect, h: &mut egui::ecolor::Hsva) -> bool {
    gradient(ui, r, 1, 36, |_, v| hsva_color32(egui::ecolor::Hsva::new(v, 1., 1., 1.)));
    let response = ui.interact(r, ui.id().with("c2-hue"), Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::slider(true, f64::from(h.h * 360.0), "Hue"));
    let picked = (response.clicked() || response.dragged()) && response.interact_pointer_pos().is_some();
    if picked && let Some(p) = response.interact_pointer_pos() {
        h.h = ((p.y - r.top()) / r.height()).rem_euclid(1.);
    }
    let y = r.top() + h.h * r.height();
    ui.painter().line_segment([pos2(r.left() - 2., y), pos2(r.right() + 2., y)], Stroke::new(2., Tokens::get(ui.ctx()).text));
    picked
}

/// egui HSB back to the straight sRGB the tools store.
fn panels_srgb(h: egui::ecolor::Hsva) -> [f32; 3] {
    let c = hsva_srgb(h);
    [c[0], c[1], c[2]]
}

/// The 实线/图案 switch: the brush paints the solid foreground colour, or a pattern sampled from
/// the canvas with the eyedropper. A click on the sampled patch selects it too.
fn color_source(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let pattern = app.session.tools.brush.pattern_enabled;
    ui.horizontal(|ui| {
        if ui.selectable_label(!pattern, tl!("Solid")).clicked() {
            let _ = app.run("tools.setColorSource", json!({"pattern": false}));
        }
        if ui.selectable_label(pattern, tl!("Pattern")).clicked() {
            let _ = app.run("tools.setColorSource", json!({"pattern": true}));
        }
        if let Some(patch) = app.session.tools.brush.sampled_pattern.as_ref() {
            let (r, response) = ui.allocate_exact_size(vec2(26., 26.), Sense::click());
            ui.painter().rect_filled(r, t.radius_sm, t.field);
            let tex = pattern_preview(ui, patch);
            ui.painter().image(tex.id(), r.shrink(1.), Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)), Color32::WHITE);
            ui.painter().rect_stroke(r, t.radius_sm, Stroke::new(1., if pattern { t.accent } else { t.field_border }), StrokeKind::Inside);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Sampled Pattern"));
            if response.on_hover_text(tl!("Use Sampled Pattern")).clicked() {
                let _ = app.run("tools.setColorSource", json!({"pattern": true}));
            }
        }
    });
    if pattern && app.session.tools.brush.sampled_pattern.is_none() {
        ui.label(tl!("Use the eyedropper on the canvas to sample a pattern."));
    }
    ui.add_space(4.);
}

/// Cells across the pattern preview. The patch can be 2048 x 2048, so the preview is a texture
/// built once per sampled pattern rather than one filled quad per cell on every frame.
const PREVIEW_CELLS: usize = 28;

/// The sampled pattern as a small nearest-neighbour texture, cached until the pattern changes.
fn pattern_preview(ui: &egui::Ui, patch: &photocraft_paint::brush::ColorPattern) -> Arc<egui::TextureHandle> {
    type Key = (usize, u32, u32);
    let id = egui::Id::unique("c2-source-pattern-preview");
    let key: Key = (patch.pixels.as_ptr() as usize, patch.width, patch.height);
    if let Some((cached, tex)) = ui.data(|d| d.get_temp::<(Key, Arc<egui::TextureHandle>)>(id))
        && cached == key
    {
        return tex;
    }
    let n = PREVIEW_CELLS;
    let mut image = egui::ColorImage::new([n, n], vec![Color32::TRANSPARENT; n * n]);
    for y in 0..n {
        for x in 0..n {
            // Cell centres, as the pixelated preview has always sampled them.
            let sx = (x as u32 * patch.width / n as u32).min(patch.width.saturating_sub(1));
            let sy = (y as u32 * patch.height / n as u32).min(patch.height.saturating_sub(1));
            let at = sy as usize * patch.width as usize + sx as usize;
            if let Some(c) = patch.pixels.get(at) {
                image[(x, y)] = c32(*c);
            }
        }
    }
    let tex = Arc::new(ui.ctx().load_texture("c2-source-pattern", image, egui::TextureOptions::NEAREST));
    ui.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
    tex
}

/// The C2 Color tab.
/// The 实线/图案 switch at the top of the C2 layout, with the sampled pattern's preview. The
/// pattern itself comes from the eyedropper on the canvas (`document.samplePattern`), so the
/// panel says where a pattern comes from while it has none.
pub(crate) fn show(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    color_source(app, ui);
    let background = app.ui.color_panel.background;
    let color = if background { app.session.tools.background } else { app.session.tools.foreground };
    let key = egui::Id::new(("c2-hue", background));
    let raw = srgb_hsva(color);
    // A grey carries no hue of its own and black neither hue nor saturation: remember what the
    // field last showed so its knob does not jump (#3228).
    let remembered = ui.data(|d| d.get_temp::<(f32, f32)>(key)).unwrap_or((raw.h, raw.s));
    let mut h = raw;
    if h.s < 0.01 {
        h.h = remembered.0;
    }
    if h.v < 0.04 && h.s < remembered.1 {
        h.s += (remembered.1 - h.s) * (1. - h.v / 0.04).clamp(0., 1.);
    }
    // The reference's 54 : 8 : 168 : 8 : 25 strip is a fixed point size (PaletteLayerPanels:
    // `scale = 1`, core 168, source 54, hue 25, gap 8). The picker stays a 168 pt square
    // whatever the dock's width, so the panel's height never follows the dock; a narrower dock
    // shrinks the strip evenly instead of clipping, and a wider one centres it.
    let width = ui.available_width().max(1.);
    let scale = (width / C2_WIDTH).min(1.);
    let left = 54. * scale;
    let side = 168. * scale;
    let pad = ((width - C2_WIDTH * scale) / 2.).max(0.);
    let (rect, response) = ui.allocate_exact_size(vec2(width, side), Sense::hover());
    sources(app, ui, Rect::from_min_size(rect.min + vec2(pad, 0.), vec2(left, side)));
    let square = Rect::from_min_size(pos2(rect.min.x + pad + left + 8. * scale, rect.min.y), vec2(side, side));
    let mut changed = sv(ui, square, &mut h);
    changed |= hue(ui, Rect::from_min_size(pos2(square.right() + 8. * scale, square.top()), vec2(25. * scale, square.height())), &mut h);
    if changed {
        super::color_panel_ui::set_color(app, ui.ctx(), background, panels_srgb(h));
    }
    if changed || (raw.s > 0.001 && (raw.v > 0.04 || raw.s > remembered.1)) {
        ui.data_mut(|d| d.insert_temp(key, (h.h, h.s)));
    }
    // Store completed selections, not every intermediate colour along a drag.
    if !ui.input(|i| i.pointer.any_down()) {
        let current = if background { app.session.tools.background } else { app.session.tools.foreground };
        let recent = &mut app.ui.color_panel.recent;
        if current[3] > 0. && srgb_bytes(recent[0]) != srgb_bytes(current) {
            recent.rotate_right(1);
            recent[0] = current;
        }
    }
    image_palette(app, ui, scale);
    ui.add_space(6.);
    let mut c = if background { app.session.tools.background } else { app.session.tools.foreground };
    if color_readout(ui, ui.id().with(("c2-readout", background)), &mut c) {
        super::color_panel_ui::set_color(app, ui.ctx(), background, [c[0], c[1], c[2]]);
        let h = srgb_hsva(c);
        if h.s > 0.001 && h.v > 0.04 {
            ui.data_mut(|d| d.insert_temp(key, (h.h, h.s)));
        }
    }
    // The panel menu, from a right-click anywhere on the panel.
    let popup = egui::Popup::context_menu(&response);
    let open = ui.input(|i| {
        i.pointer.button_clicked(egui::PointerButton::Secondary)
            && i.pointer.interact_pos().is_some_and(|p| Rect::from_min_max(rect.min, ui.min_rect().max).contains(p))
    });
    popup.open_memory(open.then_some(egui::SetOpenCommand::Bool(true))).show(|ui| super::color_panel_ui::panel_menu(app, ui));
    crate::apply_color::track_focus(ui.ctx());
}

/// The document's ten most telling colours, as a row of swatches under the field.
fn image_palette(app: &mut PhotocraftApp, ui: &mut egui::Ui, scale: f32) {
    let Some(st) = app.session.active() else { return };
    let key = (st.doc.id, st.revision);
    let id = egui::Id::new("c2-image-palette");
    let cached = ui.data(|d| d.get_temp::<((photocraft_doc::DocId, u64), Vec<[f32; 4]>)>(id));
    let colors = if let Some((_, v)) = cached.filter(|(k, _)| *k == key) {
        v
    } else {
        let image = photocraft_compose::thumbnail(&st.doc, 96);
        let colors = extract_colors(&image.pixels);
        ui.data_mut(|d| d.insert_temp(id, (key, colors.clone())));
        colors
    };
    ui.add_space(6. * scale);
    let t = Tokens::get(ui.ctx());
    let size = ((ui.available_width() - 9. * 2. * scale) / 10.).max(0.);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2. * scale;
        for i in 0..10 {
            let (r, response) = ui.allocate_exact_size(vec2(size, 22. * scale), Sense::click());
            ui.painter().rect_filled(r, t.radius_sm, colors.get(i).copied().map(c32).unwrap_or(t.field));
            ui.painter().rect_stroke(r, t.radius_sm, Stroke::new(1., t.field_border), StrokeKind::Inside);
            if response.on_hover_text(tl!("Image Palette Color")).clicked()
                && let Some(c) = colors.get(i)
            {
                super::color_panel_ui::set_color(app, ui.ctx(), false, [c[0], c[1], c[2]]);
            }
        }
    });
}

/// Ten well-separated colours of a thumbnail: the most common bins, scored for hue distance from
/// the ones already chosen so a flat image does not give ten near-duplicates.
fn extract_colors(pixels: &[u8]) -> Vec<[f32; 4]> {
    let mut bins = std::collections::BTreeMap::<(i32, i32, i32), ([f32; 4], u32, [f32; 3])>::new();
    for p in pixels.chunks_exact(4).step_by((pixels.len() / 4 / 220).max(1)) {
        if p[3] < 21 {
            continue;
        }
        let rgb = [f32::from(p[0]) / 255., f32::from(p[1]) / 255., f32::from(p[2]) / 255.];
        let h = srgb_hsva([rgb[0], rgb[1], rgb[2], 1.]);
        let h = [h.h, h.s, h.v];
        let key = ((h[0] * 24.).round() as i32, (h[1] * 8.).round() as i32, (h[2] * 8.).round() as i32);
        let entry = bins.entry(key).or_insert(([rgb[0], rgb[1], rgb[2], 1.], 0, h));
        entry.1 += 1;
    }
    let mut candidates: Vec<_> = bins.into_values().collect();
    candidates.sort_by_key(|v| std::cmp::Reverse(v.1));
    let mut selected = Vec::<([f32; 4], u32, [f32; 3])>::new();
    while !candidates.is_empty() && selected.len() < 10 {
        let score = |v: &([f32; 4], u32, [f32; 3])| {
            let nearest = selected
                .iter()
                .map(|p| {
                    let h = (p.2[0] - v.2[0]).abs();
                    h.min(1. - h) * 0.56 + (p.2[2] - v.2[2]).abs() * 0.32 + (p.2[1] - v.2[1]).abs() * 0.12
                })
                .reduce(f32::min)
                .unwrap_or(0.);
            (v.1 as f32 + 1.).ln() * 0.42 + v.2[1] * 0.18 + v.2[2] * 0.12 + nearest * 2.4
        };
        let at = candidates.iter().enumerate().max_by(|(_, a), (_, b)| score(a).total_cmp(&score(b))).map_or(0, |(i, _)| i);
        selected.push(candidates.remove(at));
    }
    selected.into_iter().map(|v| v.0).collect()
}
