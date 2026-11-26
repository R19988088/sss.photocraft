use super::{Color32, PhotocraftApp, Rect, RichText, Sense, Stroke, StrokeKind, Tokens, c32, pos2, theme, vec2, widgets};

/// The reference layout's palette strip: 54 (source chips) : 8 : 168 (the picker) : 8 : 25 (the
/// hue strip) — 263 pt wide, with the picker a 168 pt square. Every face of the Color panel draws
/// that same square, so switching faces never changes the colour area's size or shape.
pub(crate) const PALETTE_WIDTH: f32 = 263.0;

/// The palette's square, the reference layout's 168 pt picker.
pub(crate) const PALETTE_PICKER: f32 = 168.0;

/// The one scale every face draws its palette at: the dock's width over the reference strip,
/// capped at 1 so a wider dock centres the strip instead of enlarging it. The dock's height plays
/// no part — a short Color group scrolls its content rather than reshaping the colour area.
pub(crate) fn palette_scale(width: f32) -> f32 {
    (width.max(1.0) / PALETTE_WIDTH).min(1.0)
}

/// Side of the palette's square at a dock `width`: 168 pt in a dock at least as wide as the
/// reference strip, the same fraction of the width below that.
pub(crate) fn palette_picker_side(width: f32) -> f32 {
    PALETTE_PICKER * palette_scale(width)
}

/// The Color panel's foreground and background chips. A click picks the colour the field edits,
/// framed; a double-click opens the Color Picker on it.
pub(crate) fn field_chips(app: &mut PhotocraftApp, ui: &mut egui::Ui, chips: Rect) {
    let t = Tokens::get(ui.ctx());
    let bgr = Rect::from_min_size(chips.min + vec2(13.0, 13.0), vec2(22.0, 22.0));
    let fgr = Rect::from_min_size(chips.min + vec2(3.0, 3.0), vec2(22.0, 22.0));
    let frame = Stroke::new(1.0, t.text_dim);
    let bg_active = app.ui.color_panel.background;
    let p = ui.painter();
    p.rect_filled(bgr, 2.0, c32(app.session.tools.background));
    p.rect_stroke(bgr, 2.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
    if bg_active {
        p.rect_stroke(bgr.expand(2.0), 2.0, frame, StrokeKind::Outside);
    }
    p.rect_filled(fgr, 2.0, c32(app.session.tools.foreground));
    p.rect_stroke(fgr, 2.0, Stroke::new(1.0, Color32::from_gray(210)), StrokeKind::Outside);
    if !bg_active {
        p.rect_stroke(fgr.expand(2.0), 2.0, frame, StrokeKind::Outside);
    }
    // The foreground is on top, so it takes the clicks where the two overlap.
    let bg_resp = ui.interact(bgr, ui.scope_id().with("field-bg"), Sense::click());
    let fg_resp = ui.interact(fgr, ui.scope_id().with("field-fg"), Sense::click());
    let picked = if fg_resp.clicked() { false } else { bg_resp.clicked() || bg_active };
    app.ui.color_panel.background = picked;
    if fg_resp.double_clicked() || bg_resp.double_clicked() {
        crate::color_picker_ui::open(app, if picked { "background" } else { "foreground" });
    }
}

/// The colour the SV field draws for `hsva`, with what the field showed last (`hue`, `saturation`)
/// filling in what the colour itself cannot carry: a grey has no hue of its own, and black has
/// neither hue nor saturation — so a drag that bottoms out into black would otherwise round-trip
/// through RGB as `s = 0` and drop the knob from the bottom-right corner to the bottom-left one
/// (#3228).
pub(crate) fn field_hsva(mut hsva: egui::ecolor::Hsva, remembered: (f32, f32)) -> egui::ecolor::Hsva {
    if hsva.v < 0.01 {
        hsva.h = remembered.0;
        hsva.s = remembered.1;
    } else if hsva.s < 0.01 {
        // A grey: the saturation really is zero (that is where the knob belongs), only the hue
        // is undefined.
        hsva.h = remembered.0;
    }
    hsva
}

/// Photoshop's Color panel Hue Cube: a saturation/brightness field with its hue strip, drawn as
/// shaded meshes, over a hex/RGB readout. A 色盘 face: the strip belongs to the field, nothing
/// below it is a slider.
pub(crate) fn color_field(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let bg_active = app.ui.color_panel.background;
    let key = egui::Id::unique(("color-field-hue", bg_active));
    let mut hsva = srgb_hsva(if bg_active { app.session.tools.background } else { app.session.tools.foreground });
    // What the field showed last: a grey carries no hue, black neither hue nor saturation (#3228).
    let remembered: (f32, f32) = ui.data(|d| d.get_temp(key)).unwrap_or((hsva.h, hsva.s));
    hsva = field_hsva(hsva, remembered);
    let w = ui.available_width();
    let strip_w = 14.0;
    // Every face draws the same square: the reference layout's picker at the dock's width alone
    // (`palette_picker_side`), never the group's height — the hue and brightness cubes match the
    // C2 layout and the color wheel, so switching faces keeps the colour area's size and shape.
    let side = palette_picker_side(w);
    let block = 38.0 + 8.0 + side + 8.0 + strip_w;
    let pad = ((w - block) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(pad);
        let (chips, _) = ui.allocate_exact_size(vec2(38.0, side), Sense::hover());
        field_chips(app, ui, chips);
        // Photoshop's ⚠ under the chips when the colour can't print, as in the slider modes
        // (#8672); a compact field too short to hold it below the chips leaves it out.
        if chips.height() >= 38.0 + 15.0 {
            let rgb = crate::color_panel_ui::active(app);
            crate::color_panel_ui::alerts(app, ui, chips, rgb);
        }
        // Saturation across and brightness up, hue on the strip.
        let (field, fresp) = ui.allocate_exact_size(vec2(side, side), Sense::click_and_drag());
        let at = |x: f32, y: f32| egui::ecolor::Hsva::new(hsva.h, x, 1.0 - y, 1.0);
        ui.painter().add(color_grid(field, 16, |x, y| hsva_color32(at(x, y))));
        ui.painter().rect_stroke(field, 0.0, Stroke::new(1.0, t.separator), StrokeKind::Outside);
        // The same pair of names as the C2 layout's own field and strip, so the faces read alike.
        fresp.widget_info(|| egui::WidgetInfo::slider(true, f64::from(hsva.v), "Saturation / Brightness"));
        let field_pick = widgets::pick_pos(&fresp);
        if let Some(p) = field_pick {
            let x = ((p.x - field.left()) / field.width()).clamp(0.0, 1.0);
            let y = ((p.y - field.top()) / field.height()).clamp(0.0, 1.0);
            hsva.s = x;
            hsva.v = 1.0 - y;
        }
        let (kx, ky) = (hsva.s, 1.0 - hsva.v);
        let knob = pos2(field.left() + kx * field.width(), field.top() + ky * field.height());
        // At the extremes the knob sits on the edge: clip it to the field (#8968).
        let painter = ui.painter().with_clip_rect(field);
        painter.circle_stroke(knob, 5.0, Stroke::new(1.5, Color32::WHITE));
        painter.circle_stroke(knob, 6.5, Stroke::new(1.0, Color32::from_black_alpha(160)));
        // The hue strip: red at both ends, as Photoshop's.
        let (strip, sresp) = ui.allocate_exact_size(vec2(strip_w, side), Sense::click_and_drag());
        ui.painter().add(color_grid(strip, 24, |_, y| hsva_color32(egui::ecolor::Hsva::new(1.0 - y, 1.0, 1.0, 1.0))));
        sresp.widget_info(|| egui::WidgetInfo::slider(true, f64::from(hsva.h), "Hue Strip"));
        let strip_pick = widgets::pick_pos(&sresp);
        if let Some(p) = strip_pick {
            let f = ((p.y - strip.top()) / strip.height()).clamp(0.0, 1.0);
            hsva.h = 1.0 - f.min(0.9999);
        }
        let y = strip.top() + (1.0 - hsva.h) * strip.height();
        let tri = vec![pos2(strip.right() + 1.0, y), pos2(strip.right() + 6.0, y - 4.0), pos2(strip.right() + 6.0, y + 4.0)];
        ui.painter().add(egui::Shape::convex_polygon(tri, t.text, Stroke::NONE));
        if field_pick.is_some() || strip_pick.is_some() {
            if bg_active {
                app.session.tools.background = hsva_srgb(hsva);
            } else if app.session.tools.foreground != hsva_srgb(hsva) {
                // A held, unmoved press repeats the pick: restyle selected type only on a change.
                app.session.tools.foreground = hsva_srgb(hsva);
                crate::apply_color::foreground_changed(app, crate::apply_color::gesture(ui.ctx()));
            }
            ui.data_mut(|d| d.insert_temp(key, (hsva.h, hsva.s)));
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let mut c = if bg_active { app.session.tools.background } else { app.session.tools.foreground };
        if color_readout(ui, ui.scope_id().with(("color-field", bg_active)), &mut c) {
            if bg_active {
                app.session.tools.background = c;
            } else {
                app.session.tools.foreground = c;
                crate::apply_color::foreground_changed(app, crate::apply_color::gesture(ui.ctx()));
            }
            // Keep the hue for greys, so the field marker doesn't jump to red; the saturation with
            // it, so a later black lands on the corner it was last dragged to (#3228).
            let h = srgb_hsva(c);
            if h.s >= 0.01 && h.v >= 0.01 {
                ui.data_mut(|d| d.insert_temp(key, (h.h, h.s)));
            }
        }
    });
    crate::apply_color::track_focus(ui.ctx());
}

/// A smooth-shaded `n` x `n` grid over `rect`, coloured by `color(x, y)` with x and y in 0..1
/// (y down).
pub(crate) fn color_grid(rect: Rect, n: usize, color: impl Fn(f32, f32) -> Color32) -> egui::Mesh {
    let n = n.max(1);
    let mut mesh = egui::Mesh::default();
    for j in 0..=n {
        for i in 0..=n {
            let (x, y) = (i as f32 / n as f32, j as f32 / n as f32);
            mesh.colored_vertex(pos2(rect.left() + x * rect.width(), rect.top() + y * rect.height()), color(x, y));
        }
    }
    let row = (n + 1) as u32;
    for j in 0..n as u32 {
        for i in 0..n as u32 {
            let a = j * row + i;
            mesh.add_triangle(a, a + 1, a + row + 1);
            mesh.add_triangle(a, a + row + 1, a + row);
        }
    }
    mesh
}

/// Tool colours are sRGB-encoded floats; egui's Hsva works on sRGB bytes via these helpers
/// (its `from_rgba_unmultiplied` expects *linear* RGB, which gave wrong readouts).
pub(crate) fn srgb_bytes(c: [f32; 4]) -> [u8; 3] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [q(c[0]), q(c[1]), q(c[2])]
}

/// The Color panel's HSB, as Photoshop's and the Color Picker's: on the sRGB-encoded values,
/// so a 50% grey has Brightness 50%. egui's `Hsva` conversions work in linear light, which put
/// that grey at 22% and drew the field's middle as 188 (#3323); `Hsva` only holds the numbers.
pub(crate) fn srgb_hsva(c: [f32; 4]) -> egui::ecolor::Hsva {
    let [h, s, v] = photocraft_color::convert::rgb_to_hsb([c[0], c[1], c[2]].map(|v| v.clamp(0.0, 1.0)));
    egui::ecolor::Hsva::new(h / 360.0, s, v, 1.0)
}

/// The inverse of srgb_hsva: the Color panel's HSB back to sRGB-encoded values.
pub(crate) fn hsva_srgb(h: egui::ecolor::Hsva) -> [f32; 4] {
    let [r, g, b] = photocraft_color::convert::hsb_to_rgb([h.h * 360.0, h.s.clamp(0.0, 1.0), h.v.clamp(0.0, 1.0)]);
    [r, g, b, 1.0]
}

/// The colour to draw for Color panel HSB `h` (see [`srgb_hsva`]).
pub(crate) fn hsva_color32(h: egui::ecolor::Hsva) -> Color32 {
    let [r, g, b] = srgb_bytes(hsva_srgb(h));
    Color32::from_rgb(r, g, b)
}

/// The Color panel's editable colour readout: a hex field and R, G, B fields, all drawn like the
/// dock's [`widgets::value_field`] so they sit in the theme. Editing any of them sets `color`
/// (sRGB-encoded floats) and returns `true`.
pub(crate) fn color_readout(ui: &mut egui::Ui, id: egui::Id, color: &mut [f32; 4]) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut changed = false;
    // Wrapped so the fields fold onto a second line in a narrow dock instead of being clipped.
    ui.horizontal_wrapped(|ui| {
        // Docks zero the item spacing; keep the fields apart.
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new("#").font(theme::mono(12.0)).color(t.text_faint));
        changed = hex_edit(ui, id.with("hex"), color);
        let [mut r, mut g, mut b] = srgb_bytes(*color).map(f32::from);
        let mut rgb = false;
        for (label, name, v) in [("R", tl!("Red"), &mut r), ("G", tl!("Green"), &mut g), ("B", tl!("Blue"), &mut b)] {
            ui.label(RichText::new(label).font(theme::mono(11.0)).color(t.text_faint));
            rgb |= widgets::value_field(ui, v, 0.0..=255.0, "", 38.0).accessible_name(name).changed();
        }
        if rgb {
            *color = [r / 255.0, g / 255.0, b / 255.0, 1.0];
            changed = true;
        }
    });
    changed
}

/// The hex field of [`color_readout`]: type a colour with or without the leading `#`. Valid input
/// sets `color` and returns `true`. While it has focus it shows exactly what is typed, so partial
/// or invalid input isn't overwritten by the colour; an invalid entry snaps back when focus leaves.
fn hex_edit(ui: &mut egui::Ui, id: egui::Id, color: &mut [f32; 4]) -> bool {
    hex_field(ui, id, color, vec2(64.0, 24.0))
}

/// [`hex_edit`] at a given box size: the C2 panel puts the colour value over the hue strip, so
/// the field shrinks with the panel.
pub(crate) fn hex_field(ui: &mut egui::Ui, id: egui::Id, color: &mut [f32; 4], size: egui::Vec2) -> bool {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    widgets::surface(ui, rect, t.field, false);
    if !t.bevel {
        ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    }
    let [r, g, b] = srgb_bytes(*color);
    let typing = if ui.memory(|m| m.has_focus(id)) { ui.data(|d| d.get_temp::<String>(id)) } else { None };
    let mut text = typing.unwrap_or_else(|| format!("{r:02X}{g:02X}{b:02X}"));
    let field = rect.shrink2(vec2(5.0, 2.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field).layout(egui::Layout::left_to_right(egui::Align::Center)));
    // Room for a pasted "#rrggbb"; the parse trims it, and the field shows plain digits otherwise.
    // `Frame::NONE`: the themed surface above is this field's frame.
    let resp = child
        .add(egui::TextEdit::singleline(&mut text).id(id).char_limit(7).desired_width(field.width()).frame(egui::Frame::NONE).font(theme::mono(12.0)))
        .accessible_name(tl!("Color"));
    if resp.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    if resp.changed()
        && let Some(c) = crate::color_picker_ui::parse_hex(&text)
    {
        *color = [c[0], c[1], c[2], 1.0];
        return true;
    }
    false
}
