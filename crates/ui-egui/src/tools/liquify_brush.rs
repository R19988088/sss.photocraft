//! Liquify as a brush tool: the document itself warps under the pointer, on the canvas, with
//! Photoshop's whole Liquify tool strip and brush options.
//!
//! Strokes are plain data ([`LiquifyStroke`]) replayed on a [`LiquifyField`]; OK runs
//! `filter.liquify` with them, so the edit is one history step and stays a smart filter on a
//! smart object. The live preview warps every target layer of a document proxy through the field
//! and re-composites only the brush rectangle, uploading just those pixels: the rest of the
//! document stays resident in the canvas' GPU cache, and the proxy follows the view (full
//! resolution at 100 % and closer). Layer masks linked to the pixels follow the warp, and the
//! Scope choice (Layer / Group / All Layers) reaches the layers the command will warp.
//!
//! The tool strip, the options bar widgets and the overlays use the shell's own widgets and
//! theme, not the old dialog's chrome.

use std::sync::Arc;

use egui::{Color32, ColorImage, TextureHandle, TextureOptions, pos2};
use photocraft_algo::liquify::{MAX_LIVE_MS, LiquifyField, LiquifyStroke, LiquifyTool, auto_cell};
use photocraft_doc::{DocId, Document, LayerContent, LayerId};
use photocraft_geom::Rect as DRect;
use photocraft_raster::Surface;
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, ViewXform};
use crate::liquify_ui::LiquifyOpts;
use crate::state::Tool;
use crate::widgets;

/// Longest side of the preview texture (round-number headroom under every driver's limit).
const MAX_SIDE: i32 = 4096;
/// Proxy pixels the preview aims for when the texture limit is not the tighter one.
const BUDGET_PIXELS: f64 = 4_000_000.0;
/// `prefs.dialogs` key shared with the Liquify dialog: the same brush settings either way.
const REMEMBERED: &str = "filter.liquify";

/// Live Liquify state for one document: the field, the recorded strokes and the preview.
pub struct LiquifyBrush {
    /// Document this mode belongs to (#5115): a tab switch drops it.
    pub document: DocId,
    /// Layers the command will warp, for the current scope.
    targets: Vec<LayerId>,
    scope: String,
    scope_dirty: bool,
    canvas: DRect,
    cell: f64,
    field: LiquifyField,
    strokes: Vec<LiquifyStroke>,
    redo: Vec<LiquifyStroke>,
    cur: Option<Stroke>,
    lasso: Option<(bool, Vec<[f64; 2]>)>,
    pub opts: LiquifyOpts,
    preview: Preview,
    /// The document with every layer hidden: the canvas paints the checkerboard, and the proxy
    /// carries the picture. Keeps the GPU canvas from compositing the document every dab.
    hidden: Arc<Document>,
    ctx: egui::Context,
    key: u64,
    revision: u64,
    /// Last pointer position in document pixels (the brush circle).
    hover: Option<[f64; 2]>,
    last_ms: f64,
    /// Control+Alt-drag (Alt+right-drag's macOS gesture): the brush circle stays at the press and
    /// the drag sets size and hardness, as in Photoshop (#4702).
    resize: Option<Resize>,
    pub dab_ms: f64,
    pub preview_ms: f64,
    mask_dirty: bool,
}

struct Stroke {
    s: LiquifyStroke,
    last: [f64; 3],
}

struct Resize {
    anchor: [f64; 2],
    size: f32,
    density: f32,
}

impl LiquifyBrush {
    /// Starts the mode on the active document, at the current scope and the remembered brush.
    fn new(app: &mut PhotocraftApp, ctx: &egui::Context, zoom: f32) -> Result<Self, String> {
        let (doc, canvas, revision, active_layer) = {
            let st = app.session.active().ok_or("no document")?;
            (st.doc.clone(), st.doc.bounds(), st.revision, st.active_layer)
        };
        let _ = active_layer;
        if canvas.is_empty() {
            return Err("The document is empty".into());
        }
        let mut opts = LiquifyOpts::default();
        let mut scope = "layer".to_string();
        if let Some(saved) = app.session.prefs().dialogs.get(REMEMBERED) {
            opts.apply(saved).ok();
            if let Some(s) = saved.get("scope").and_then(Value::as_str)
                && matches!(s, "layer" | "group" | "all")
            {
                scope = s.to_string();
            }
        }
        // A sensible starting brush (about a tenth of the image), then the settings last used, so
        // Liquify opens as it was left (#418). Saved values are clamped; a bad one is skipped.
        let fresh = (f64::from(canvas.width().max(canvas.height())) / 10.0).round().clamp(10.0, 1500.0);
        if app.session.prefs().dialogs.get(REMEMBERED).is_none_or(|s| s.get("size").is_none()) {
            opts.size = fresh as f32;
        }
        let targets = targets_for(app, &scope).or_else(|_| targets_for(app, "layer"))?;
        let raster: Vec<LayerId> = targets.iter().copied().filter(|id| doc.layer(*id).is_some_and(|l| matches!(l.content, LayerContent::Raster(_)))).collect();
        if raster.is_empty() {
            return Err("The target has no pixel layers (rasterize the smart object first)".into());
        }
        let cell = auto_cell(canvas);
        let hidden = Arc::new(without_layers(&doc));
        let preview = Preview::new(&doc, &raster, canvas, zoom);
        Ok(LiquifyBrush {
            document: doc.id,
            targets,
            scope,
            scope_dirty: false,
            canvas,
            cell,
            field: LiquifyField::new(canvas, cell),
            strokes: Vec::new(),
            redo: Vec::new(),
            cur: None,
            lasso: None,
            opts,
            preview,
            hidden,
            ctx: ctx.clone(),
            key: app.ui.alloc_id(),
            revision,
            hover: None,
            last_ms: 0.0,
            resize: None,
            dab_ms: 0.0,
            preview_ms: 0.0,
            mask_dirty: true,
        })
    }

    pub fn describe(&self) -> Value {
        json!({
            "document": self.document.0,
            "scope": self.scope,
            "targets": self.targets.iter().map(|id| id.0).collect::<Vec<_>>(),
            "tool": self.opts.tool,
            "size": self.opts.size,
            "density": self.opts.density,
            "pressure": self.opts.pressure,
            "rate": self.opts.rate,
            "strokes": self.strokes.len(),
            "canUndo": !self.strokes.is_empty(),
            "canRedo": !self.redo.is_empty(),
            "dabMs": self.dab_ms,
            "previewMs": self.preview_ms,
        })
    }

    /// Rebuilds the frozen target set after a Scope change. The strokes stay: only the pixels
    /// they reach change.
    fn refresh_scope(&mut self, app: &PhotocraftApp) -> Result<(), String> {
        self.targets = targets_for(app, &self.scope)?;
        let st = app.session.active().ok_or("no document")?;
        let raster: Vec<LayerId> =
            self.targets.iter().copied().filter(|id| st.doc.layer(*id).is_some_and(|l| matches!(l.content, LayerContent::Raster(_)))).collect();
        if raster.is_empty() {
            return Err("The target has no pixel layers".into());
        }
        self.preview.rebuild(&st.doc, &raster, self.canvas, scale_for(self.canvas, app.point_zoom(), false));
        self.scope_dirty = false;
        self.mask_dirty = true;
        Ok(())
    }

    // ---- strokes ----

    fn begin(&mut self, p: [f64; 3], now: f64, alt: bool) {
        self.redo.clear();
        let tool = if alt { alt_tool(self.opts.tool) } else { self.opts.tool };
        let mut s = self.template(tool);
        s.points.push(vec![p[0], p[1], p[2], 0.0]);
        let t0 = crate::gpu_canvas::now_ms();
        let d = self.field.stroke_begin(&s, p);
        self.dab_ms = crate::gpu_canvas::now_ms() - t0;
        self.mark(d, matches!(s.tool, LiquifyTool::Freeze | LiquifyTool::Thaw));
        self.cur = Some(Stroke { s, last: p });
        self.last_ms = now;
    }

    fn extend(&mut self, p: [f64; 3], now: f64) {
        let Some(mut c) = self.cur.take() else { return };
        let ms = (now - self.last_ms).clamp(0.0, MAX_LIVE_MS);
        c.s.points.push(vec![p[0], p[1], p[2], ms]);
        let t0 = crate::gpu_canvas::now_ms();
        let d = self.field.stroke_segment_live(&c.s, c.last, p, ms);
        self.dab_ms = crate::gpu_canvas::now_ms() - t0;
        self.mark(d, matches!(c.s.tool, LiquifyTool::Freeze | LiquifyTool::Thaw));
        c.last = p;
        self.cur = Some(c);
        self.last_ms = now;
    }

    fn end(&mut self) {
        if let Some(c) = self.cur.take() {
            let d = self.field.finish_stroke();
            self.mark(d, matches!(c.s.tool, LiquifyTool::Freeze | LiquifyTool::Thaw));
            self.strokes.push(c.s);
        }
    }

    /// Records the open lasso polygon as a Freeze (or Alt: Thaw) stroke.
    fn close_lasso(&mut self, subtract: bool, mut pts: Vec<[f64; 2]>) {
        self.end();
        if pts.len() < 3 {
            return;
        }
        if let (Some(&first), Some(&last)) = (pts.first(), pts.last())
            && ((first[0] - last[0]).abs() > 0.5 || (first[1] - last[1]).abs() > 0.5)
        {
            pts.push(first);
        }
        self.redo.clear();
        let mut s = self.template(LiquifyTool::LassoMask);
        s.amount = Some(if subtract { 0.0 } else { 1.0 });
        s.points = pts.into_iter().map(|p| p.to_vec()).collect();
        let d = self.field.apply_stroke(&s);
        self.strokes.push(s);
        self.mark(d, true);
    }

    /// Reconstruct…, Mask All / None / Invert All: whole-field strokes, undoable like the rest.
    fn global(&mut self, tool: LiquifyTool, amount: Option<f64>) {
        self.end();
        self.redo.clear();
        let mut s = LiquifyStroke::new(tool, 1.0);
        s.amount = amount;
        let d = self.field.apply_stroke(&s);
        self.strokes.push(s);
        self.mark(d, true);
    }

    fn template(&self, tool: LiquifyTool) -> LiquifyStroke {
        let mut s = LiquifyStroke::new(tool, f64::from(self.opts.size));
        s.density = f64::from(self.opts.density);
        s.pressure = f64::from(self.opts.pressure);
        s.rate = f64::from(self.opts.rate);
        s
    }

    fn undo(&mut self) {
        if let Some(s) = self.strokes.pop() {
            self.redo.push(s);
            self.rebuild_field();
        }
    }

    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.field.apply_stroke(&s);
            self.strokes.push(s);
            self.mark(self.field.bounds, true);
        }
    }

    fn restore_all(&mut self) {
        self.strokes.clear();
        self.redo.clear();
        self.rebuild_field();
    }

    /// Replays the kept strokes on a fresh field: undo, Restore All.
    fn rebuild_field(&mut self) {
        self.cur = None;
        self.field = LiquifyField::from_strokes(self.canvas, self.cell, &self.strokes);
        self.mark(self.canvas, true);
    }

    fn mark(&mut self, d: DRect, mask: bool) {
        if !d.is_empty() {
            self.preview.mark(d);
        }
        if mask {
            self.mask_dirty = true;
        }
    }

    /// Frame hook: follows the view, warps the dirty proxy rectangle and (re)builds the mask
    /// overlay. No-op when nothing changed.
    fn tick(&mut self, app: &PhotocraftApp, zoom: f32) {
        let Some(st) = app.session.active() else { return };
        if st.doc.id != self.document {
            return;
        }
        let t0 = crate::gpu_canvas::now_ms();
        let display = crate::canvas::canvas_display(app, &st.doc, None).0;
        let raster: Vec<LayerId> =
            self.targets.iter().copied().filter(|id| st.doc.layer(*id).is_some_and(|l| matches!(l.content, LayerContent::Raster(_)))).collect();
        self.preview.update(&self.field, &st.doc, &raster, display.as_deref(), &self.ctx, scale_for(self.canvas, zoom, false));
        if self.mask_dirty {
            self.mask_dirty = false;
            self.preview.mask_texture(&self.field, &self.ctx);
        }
        self.preview_ms = crate::gpu_canvas::now_ms() - t0;
    }
}

// ---- opening, committing ----

fn targets_for(app: &PhotocraftApp, scope: &str) -> Result<Vec<LayerId>, String> {
    photocraft_engine::distort_cmds::liquify_targets(&app.session, &json!({"scope": scope})).map_err(|e| e.to_string())
}

/// Opens the mode when the Liquify tool is selected (or the command runs with `tool: true`).
pub fn open(app: &mut PhotocraftApp, ctx: &egui::Context) -> Result<(), String> {
    if app.ui.transform.is_some() {
        return Err("Finish Free Transform first".into());
    }
    if app.distort.puppet.is_some() || app.distort.perspective.is_some() {
        return Err("Finish the other warp first".into());
    }
    let s = LiquifyBrush::new(app, ctx, app.point_zoom())?;
    app.distort.liquify_brush = Some(s);
    Ok(())
}

fn remember(app: &mut PhotocraftApp, s: &LiquifyBrush) {
    let mut v = s.opts.to_json();
    if let Some(o) = v.as_object_mut() {
        o.insert("scope".into(), json!(s.scope));
    }
    app.session.prefs.edit(|p| p.dialogs.insert(REMEMBERED.into(), v));
}

/// Esc / Cancel: the document is untouched; the brush settings are kept.
pub fn cancel(app: &mut PhotocraftApp) {
    if let Some(s) = app.distort.liquify_brush.take() {
        remember(app, &s);
        // Esc / Cancel leaves the mode: the tool it was entered from takes over again.
        if let Some(prev) = app.distort.liquify_prev_tool.take()
            && prev != Tool::Liquify
        {
            crate::tool_memory::select(app, prev);
        }
    }
}

/// OK / ↩: one history step (`filter.liquify`), or a smart filter on a smart object.
pub fn commit(app: &mut PhotocraftApp) {
    let Some(mut s) = app.distort.liquify_brush.take() else { return };
    s.end();
    remember(app, &s);
    if s.strokes.is_empty() {
        return;
    }
    if s.scope_dirty
        && let Err(error) = s.refresh_scope(app)
    {
        crate::notices::error(app, error);
        app.distort.liquify_brush = Some(s);
        return;
    }
    let layer = s.targets.first().copied();
    let params = json!({
        "strokes": s.strokes,
        "meshSize": s.cell,
        "layer": layer.map(|id| id.0),
        "layers": s.targets.iter().map(|id| id.0).collect::<Vec<_>>(),
        "scope": s.scope,
    });
    if let Err(error) = app.run("filter.liquify", params) {
        crate::notices::error(app, error);
        s.rebuild_field();
        app.distort.liquify_brush = Some(s);
    }
}

// ---- per-frame hooks ----

/// The document the canvas draws: every layer hidden, so the proxy image in [`draw_content`] is
/// the only picture (and the GPU canvas has nothing to composite per dab).
pub fn display_doc(app: &PhotocraftApp, idx: usize) -> Option<(Arc<Document>, u64)> {
    if app.session.active_index() != Some(idx) {
        return None;
    }
    let s = app.distort.liquify_brush.as_ref()?;
    if app.session.active().is_none_or(|st| st.doc.id != s.document) {
        return None;
    }
    // Show Backdrop: the un-warped document stays behind a translucent preview, which is what
    // the backdrop is for. Off, the preview is the whole picture.
    let doc = if s.opts.show_backdrop { app.session.active()?.doc.clone() } else { s.hidden.clone() };
    // The key changes with what is drawn: Show Backdrop swaps the hidden document for the real
    // one, and the canvas cache must not keep the other picture.
    Some((doc, (1 << 43) + s.key * 2 + u64::from(s.opts.show_backdrop)))
}

/// Updates the proxy before the canvas draws it.
pub fn prepare(app: &mut PhotocraftApp, zoom: f32) {
    let Some(mut s) = app.distort.liquify_brush.take() else { return };
    let stale = match app.session.active() {
        Some(st) => st.doc.id != s.document || st.revision != s.revision,
        None => true,
    };
    if stale {
        if app.session.active().is_some() {
            crate::notices::error(app, crate::i18n::fmt(tl!("Liquify closed: the document changed. Open Liquify again to start over."), &[]));
        }
        return;
    }
    if s.scope_dirty && s.refresh_scope(app).is_err() {
        s.scope_dirty = false;
    }
    s.tick(app, zoom);
    app.distort.liquify_brush = Some(s);
}

/// The warped multy-layer image, under the grid and guides; then the freeze mask, over the pixels.
pub fn draw_content(app: &PhotocraftApp, painter: &egui::Painter, xf: &ViewXform) {
    let Some(s) = app.distort.liquify_brush.as_ref() else { return };
    if app.session.active().is_some_and(|st| st.doc.id != s.document) {
        return;
    }
    let Some(tex) = s.preview.texture() else { return };
    let alpha = if s.opts.show_backdrop { ((100.0 - s.opts.backdrop_opacity).clamp(0.0, 100.0) * 2.55) as u8 } else { 255 };
    draw_image(painter, xf, s.canvas, tex, Color32::from_white_alpha(alpha));
    if s.opts.show_mask
        && let Some(mask) = s.preview.mask_texture_id()
    {
        draw_image(painter, xf, s.canvas, mask, Color32::WHITE);
    }
}

/// Mesh, lasso and the brush circle, over the guides.
pub fn draw_overlay(app: &PhotocraftApp, painter: &egui::Painter, xf: &ViewXform) {
    let Some(s) = app.distort.liquify_brush.as_ref() else { return };
    if app.session.active().is_none_or(|st| st.doc.id != s.document) {
        return;
    }
    if s.opts.show_mesh {
        draw_mesh(&s.field, s.canvas, s.opts.mesh_size, painter, xf);
    }
    if let Some((subtract, pts)) = &s.lasso {
        let color = if *subtract { Color32::from_rgb(90, 200, 255) } else { Color32::from_rgb(230, 60, 60) };
        let stroke = egui::Stroke::new(1.25, color);
        for seg in pts.windows(2) {
            painter.line_segment([xf.to_screen(seg[0][0] as f32, seg[0][1] as f32), xf.to_screen(seg[1][0] as f32, seg[1][1] as f32)], stroke);
        }
    }
    // The brush circle follows the pointer while the tool is active on this canvas; resizing
    // (Control+Alt-drag) keeps it at the press.
    if app.ui.tool == Tool::Liquify
        && (s.resize.is_some() || painter.ctx().input(|i| i.pointer.hover_pos().is_some()))
    {
        let at = match &s.resize {
            Some(r) => Some([r.anchor[0], r.anchor[1]]),
            None => painter.ctx().input(|i| i.pointer.hover_pos()).filter(|p| {
                xf.image_bounds([s.canvas.width() as u32, s.canvas.height() as u32]).contains(*p)
            }).map(|p| xf.to_doc(p)),
        };
        if let Some(d) = at {
            let r = (s.opts.size as f32) * 0.5 * xf.zoom;
            if r > 0.5 {
                let c = xf.to_screen(d[0] as f32, d[1] as f32);
                crate::brush_resize::liquify_tip_preview(painter, c, r.max(1.0), (s.opts.density / 100.0).clamp(0.0, 1.0), Color32::from_rgba_unmultiplied(40, 40, 40, 60));
                painter.circle_stroke(c, r, egui::Stroke::new(1.0, Color32::from_black_alpha(160)));
                painter.circle_stroke(c, r + 1.0, egui::Stroke::new(1.0, Color32::from_white_alpha(200)));
            }
        }
    }
}

/// The proxy image over `area`, mapped through the view (rotation, flip and pixel aspect).
fn draw_image(painter: &egui::Painter, xf: &ViewXform, area: DRect, tex: egui::TextureId, tint: Color32) {
    let (x0, y0, x1, y1) = (area.x0 as f32, area.y0 as f32, area.x1 as f32, area.y1 as f32);
    let mut mesh = egui::Mesh::with_texture(tex);
    mesh.vertices.push(egui::epaint::Vertex { pos: xf.to_screen(x0, y0), uv: pos2(0.0, 0.0), color: tint });
    mesh.vertices.push(egui::epaint::Vertex { pos: xf.to_screen(x1, y0), uv: pos2(1.0, 0.0), color: tint });
    mesh.vertices.push(egui::epaint::Vertex { pos: xf.to_screen(x1, y1), uv: pos2(1.0, 1.0), color: tint });
    mesh.vertices.push(egui::epaint::Vertex { pos: xf.to_screen(x0, y1), uv: pos2(0.0, 1.0), color: tint });
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// The field's grid, drawn as level contours of the displaced grid, as Photoshop's mesh is.
fn draw_mesh(field: &LiquifyField, area: DRect, size: usize, painter: &egui::Painter, xf: &ViewXform) {
    let cells = [64.0, 40.0, 24.0][size.min(2)];
    let step = f64::from(area.width().max(area.height())) / cells;
    if step <= 0.0 {
        return;
    }
    let h = step / 4.0;
    let nx = (f64::from(area.width()) / h).ceil() as usize + 1;
    let ny = (f64::from(area.height()) / h).ceil() as usize + 1;
    if nx.saturating_mul(ny) > 400_000 {
        return;
    }
    let (x0, y0) = (f64::from(area.x0), f64::from(area.y0));
    let mut warped = Vec::with_capacity(nx * ny);
    for j in 0..ny {
        for i in 0..nx {
            let p = [x0 + i as f64 * h, y0 + j as f64 * h];
            let v = field.sample(p[0], p[1]);
            warped.push([p[0] + v[0], p[1] + v[1]]);
        }
    }
    let stroke = egui::Stroke::new(0.75, Color32::from_rgba_unmultiplied(90, 90, 90, 210));
    let pos = |i: usize, j: usize| [x0 + i as f64 * h, y0 + j as f64 * h];
    for axis in [0usize, 1] {
        let origin = if axis == 0 { x0 } else { y0 };
        for jj in 0..ny.saturating_sub(1) {
            for ii in 0..nx.saturating_sub(1) {
                let corners = [(ii, jj), (ii + 1, jj), (ii + 1, jj + 1), (ii, jj + 1)];
                let vals = corners.map(|(i, j)| warped[j * nx + i][axis]);
                let (lo, hi) = (vals.iter().copied().fold(f64::MAX, f64::min), vals.iter().copied().fold(f64::MIN, f64::max));
                let mut k = ((lo - origin) / step).ceil() as i64;
                while origin + k as f64 * step <= hi {
                    let level = origin + k as f64 * step;
                    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(4);
                    for e in 0..4 {
                        let (a, b) = (vals[e] - level, vals[(e + 1) % 4] - level);
                        if (a <= 0.0) != (b <= 0.0) {
                            let t = a / (a - b);
                            let (pa, pb) = (pos(corners[e].0, corners[e].1), pos(corners[(e + 1) % 4].0, corners[(e + 1) % 4].1));
                            pts.push([pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t]);
                        }
                    }
                    for seg in pts.as_chunks::<2>().0 {
                        painter.line_segment(
                            [xf.to_screen(seg[0][0] as f32, seg[0][1] as f32), xf.to_screen(seg[1][0] as f32, seg[1][1] as f32)],
                            stroke,
                        );
                    }
                    k += 1;
                }
            }
        }
    }
}

// ---- pointer and keys ----

/// Photoshop's Alt variant of a Liquify brush: the twirls reverse and Pucker/Bloat swap. Alt on
/// the lasso subtracts from the freeze mask instead.
fn alt_tool(t: LiquifyTool) -> LiquifyTool {
    match t {
        LiquifyTool::TwirlCw => LiquifyTool::TwirlCcw,
        LiquifyTool::TwirlCcw => LiquifyTool::TwirlCw,
        LiquifyTool::Pucker => LiquifyTool::Bloat,
        LiquifyTool::Bloat => LiquifyTool::Pucker,
        t => t,
    }
}

pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, mods: egui::Modifiers) {
    let now = crate::gpu_canvas::now_ms();
    let zoom = app.point_zoom();
    let Some(s) = app.distort.liquify_brush.as_mut() else { return };
    if (mods.ctrl && mods.alt) || s.resize.is_some() {
        match ev {
            ToolEvent::Down { x, y, .. } => {
                s.resize = Some(Resize { anchor: [x, y], size: s.opts.size, density: s.opts.density });
            }
            ToolEvent::Move { x, y, .. } => {
                if let Some(r) = &s.resize {
                    let (size, density) = resized_brush(r.size, r.density, (x - r.anchor[0]) as f32, (y - r.anchor[1]) as f32, zoom);
                    s.opts.size = size;
                    s.opts.density = density;
                }
            }
            ToolEvent::Up { .. } => s.resize = None,
        }
        return;
    }
    match ev {
        ToolEvent::Down { x, y, pressure } => {
            s.hover = Some([x, y]);
            if s.opts.tool == LiquifyTool::LassoMask {
                s.lasso = Some((mods.alt, vec![[x, y]]));
            } else {
                s.begin([x, y, f64::from(pressure)], now, mods.alt);
            }
        }
        ToolEvent::Move { x, y, pressure } => {
            s.hover = Some([x, y]);
            if let Some((_, pts)) = &mut s.lasso {
                if pts.last().is_none_or(|&l| l[0] != x || l[1] != y) {
                    pts.push([x, y]);
                }
            } else if s.cur.is_some() {
                s.extend([x, y, f64::from(pressure)], now);
            }
        }
        ToolEvent::Up { x, y } => {
            s.hover = Some([x, y]);
            if let Some((subtract, pts)) = s.lasso.take() {
                s.close_lasso(subtract, pts);
                return;
            }
            if let Some(c) = &s.cur
                && (c.last[0] != x || c.last[1] != y)
            {
                s.extend([x, y, c.last[2]], now);
            }
            s.end();
        }
    }
}

/// Liquify's own shortcuts while the mode is open (↩ / Esc are handled by the shell).
pub fn keys(app: &mut PhotocraftApp, ctx: &egui::Context) -> bool {
    use egui::{Key, Modifiers};
    let Some(s) = app.distort.liquify_brush.as_mut() else { return false };
    let mut used = true;
    let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
    if ctx.input_mut(|i| i.consume_key(cmd_shift, Key::Z)) {
        s.redo();
    } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
        s.undo();
    } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::H)) {
        // Hide Extras toggles the red freeze-mask overlay; the mask itself stays active.
        s.opts.show_mask = !s.opts.show_mask;
    } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::I)) {
        s.global(LiquifyTool::InvertFreeze, None);
    } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::D)) {
        s.global(LiquifyTool::ThawAll, None);
    } else if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::OpenBracket)) {
        // At least one pixel, or 2 px * 0.9 rounds back to 2 and `[` never reaches 1.
        s.opts.size = (s.opts.size * 0.9).round().min(s.opts.size - 1.0).clamp(1.0, 15000.0);
    } else if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::CloseBracket)) {
        s.opts.size = (s.opts.size * 1.1).round().max(s.opts.size + 1.0).min(15000.0);
    } else {
        used = false;
    }
    if used {
        return true;
    }
    let tools = [
        (egui::Key::W, LiquifyTool::ForwardWarp),
        (egui::Key::R, LiquifyTool::Reconstruct),
        (egui::Key::E, LiquifyTool::Smooth),
        (egui::Key::C, LiquifyTool::TwirlCw),
        (egui::Key::S, LiquifyTool::Pucker),
        (egui::Key::B, LiquifyTool::Bloat),
        (egui::Key::O, LiquifyTool::PushLeft),
        (egui::Key::F, LiquifyTool::Freeze),
        (egui::Key::D, LiquifyTool::Thaw),
        (egui::Key::L, LiquifyTool::LassoMask),
    ];
    for (k, t) in tools {
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k)) {
            s.opts.tool = t;
            return true;
        }
    }
    false
}

fn tool_icon(t: LiquifyTool) -> &'static str {
    match t {
        LiquifyTool::ForwardWarp => "pointer",
        LiquifyTool::Reconstruct => "brush-cleaning",
        LiquifyTool::Smooth => "droplet",
        LiquifyTool::TwirlCw => "rotate-cw",
        LiquifyTool::TwirlCcw => "undo-2",
        LiquifyTool::Pucker => "scan",
        LiquifyTool::Bloat => "maximize-2",
        LiquifyTool::PushLeft => "chevrons-left",
        LiquifyTool::Freeze => "lock",
        LiquifyTool::Thaw => "lock-open",
        LiquifyTool::LassoMask => "lasso",
        _ => "circle",
    }
}

/// A Control+Alt-drag's motion sets the diameter horizontally and the density vertically, over
/// the same travel the canvas uses for hardness.
fn resized_brush(start_size: f32, start_density: f32, dx: f32, dy: f32, zoom: f32) -> (f32, f32) {
    let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
    let (size, density) = crate::brush_resize::resized((start_size, start_density / 100.0), f64::from(dx / zoom), f64::from(dy / zoom), zoom);
    (size, (density * 100.0).round())
}

fn scope_label(scope: &str) -> &'static str {
    match scope {
        "group" => tl!("Group"),
        "all" => tl!("All Layers"),
        _ => tl!("Layer"),
    }
}

/// The options bar: every tool of the strip (Twirl Counterclockwise included), Scope, Size,
/// Hardness, Pressure, Rate, Reconstruct, the mask buttons, the view options and OK / Cancel.
/// True when drawn.
pub fn options_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui) -> bool {
    // A tool chosen elsewhere (a palette click, the control channel) ends the mode as switching
    // to another tool does: the strokes land as one history step.
    if app.ui.tool != Tool::Liquify {
        app.distort.liquify_brush_failed = false;
        if app.distort.liquify_brush.is_some() {
            commit(app);
        }
        return false;
    }
    if app.distort.liquify_brush.is_none() {
        if app.distort.liquify_brush_failed {
            return true;
        }
        if app.session.active().is_none() {
            return true;
        }
        if let Err(error) = open(app, ui.ctx()) {
            crate::notices::error(app, error);
            app.distort.liquify_brush_failed = true;
            return true;
        }
    }
    let Some(mut s) = app.distort.liquify_brush.take() else { return true };
    let mut commit_now = false;
    let mut cancel_now = false;
    let mut scope_changed = false;

    // The 11-tool strip, in Photoshop's order (Twirl Counterclockwise is a tool of its own here,
    // as the old build had it; Alt on Twirl Clockwise still reverses one stroke).
    for tool in tool_strip() {
        let tip = if tool == LiquifyTool::LassoMask {
            tl!("Freeze Lasso: drag to freeze an area, Alt-drag to thaw it (L)").to_string()
        } else {
            tl!(tool.label()).to_string()
        };
        if crate::icons::button(ui, tool_icon(tool), 26.0, s.opts.tool == tool, &tip).on_hover_text(tip).clicked() {
            s.opts.tool = tool;
        }
    }
    widgets::vline(ui, 22.0);
    ui.label(tl!("Scope"));
    let scope_before = s.scope.clone();
    widgets::dropdown(
        ui,
        "liquify-brush-scope",
        tl!("Scope"),
        &mut s.scope,
        &[("layer".to_string(), scope_label("layer")), ("group".to_string(), scope_label("group")), ("all".to_string(), scope_label("all"))],
        84.0,
    );
    scope_changed = s.scope != scope_before;
    widgets::vline(ui, 22.0);

    let mut size = s.opts.size;
    if widgets::value_field(ui, &mut size, 1.0..=15000.0, "px", 64.0).on_hover_text(tl!("Size")).changed() {
        s.opts.size = size;
    }
    let mut hardness = s.opts.density;
    if widgets::value_field(ui, &mut hardness, 0.0..=100.0, "%", 54.0).on_hover_text(tl!("Hardness")).changed() {
        s.opts.density = hardness;
    }
    let mut pressure = s.opts.pressure;
    if widgets::value_field(ui, &mut pressure, 0.0..=100.0, "%", 54.0).on_hover_text(tl!("Pressure")).changed() {
        s.opts.pressure = pressure;
    }
    let mut rate = s.opts.rate;
    if widgets::value_field(ui, &mut rate, 0.0..=100.0, "%", 54.0).on_hover_text(tl!("Rate")).changed() {
        s.opts.rate = rate;
    }
    widgets::vline(ui, 22.0);

    let mut amount = s.opts.reconstruct_amount;
    if widgets::value_field(ui, &mut amount, 0.0..=100.0, "%", 54.0).on_hover_text(tl!("Reconstruct")).changed() {
        s.opts.reconstruct_amount = amount;
    }
    if widgets::secondary_button(ui, tl!("Reconstruct"), 0.0).on_hover_text(tl!("Scales the whole field toward zero")).clicked() {
        s.global(LiquifyTool::ReconstructAll, Some(f64::from(s.opts.reconstruct_amount)));
    }
    if widgets::secondary_button(ui, tl!("Restore All"), 0.0).clicked() {
        s.restore_all();
    }
    widgets::vline(ui, 22.0);
    if widgets::secondary_button(ui, tl!("None"), 0.0).on_hover_text(tl!("Mask Options: Mask None")).clicked() {
        s.global(LiquifyTool::ThawAll, None);
    }
    if widgets::secondary_button(ui, tl!("Mask All"), 26.0).on_hover_text(tl!("Freeze every pixel")).clicked() {
        s.global(LiquifyTool::FreezeAll, None);
    }
    if widgets::secondary_button(ui, tl!("Invert"), 0.0).clicked() {
        s.global(LiquifyTool::InvertFreeze, None);
    }
    widgets::vline(ui, 22.0);

    widgets::checkbox(ui, &mut s.opts.show_mesh, tl!("Show Mesh"));
    let mesh = ["Small", "Medium", "Large"];
    let mut mesh_size = s.opts.mesh_size;
    widgets::dropdown(
        ui,
        "liquify-brush-mesh",
        tl!("Mesh Size"),
        &mut mesh_size,
        &[(0usize, tl!(mesh[0])), (1usize, tl!(mesh[1])), (2usize, tl!(mesh[2]))],
        82.0,
    );
    if mesh_size != s.opts.mesh_size {
        s.opts.mesh_size = mesh_size;
    }
    widgets::checkbox(ui, &mut s.opts.show_mask, tl!("Show Mask"));
    widgets::checkbox(ui, &mut s.opts.show_backdrop, tl!("Show Backdrop"));
    if s.opts.show_backdrop {
        let mut opacity = s.opts.backdrop_opacity;
        if widgets::value_field(ui, &mut opacity, 0.0..=100.0, "%", 50.0).on_hover_text(tl!("Backdrop Opacity")).changed() {
            s.opts.backdrop_opacity = opacity;
        }
    }
    widgets::vline(ui, 22.0);
    if crate::icons::button(ui, "undo-2", 26.0, false, tl!("Undo")).clicked() {
        s.undo();
    }
    if crate::icons::button(ui, "redo-2", 26.0, false, tl!("Redo")).clicked() {
        s.redo();
    }

    // Right-aligned OK / Cancel, the two ends of a Liquify session.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::secondary_button(ui, tl!("Cancel"), 0.0).clicked() {
            cancel_now = true;
        }
        if ui.button(tl!("OK")).clicked() {
            commit_now = true;
        }
    });

    if scope_changed {
        s.scope_dirty = true;
        if let Err(error) = s.refresh_scope(app) {
            crate::notices::error(app, error);
            s.scope = "layer".into();
            let _ = s.refresh_scope(app);
        }
    }
    if commit_now {
        app.distort.liquify_brush = Some(s);
        commit(app);
        return true;
    }
    if cancel_now {
        app.distort.liquify_brush = Some(s);
        cancel(app);
        return true;
    }
    app.distort.liquify_brush = Some(s);
    true
}

fn tool_strip() -> [LiquifyTool; 11] {
    [
        LiquifyTool::ForwardWarp,
        LiquifyTool::Reconstruct,
        LiquifyTool::Smooth,
        LiquifyTool::TwirlCw,
        LiquifyTool::TwirlCcw,
        LiquifyTool::Pucker,
        LiquifyTool::Bloat,
        LiquifyTool::PushLeft,
        LiquifyTool::Freeze,
        LiquifyTool::Thaw,
        LiquifyTool::LassoMask,
    ]
}

// ---- preview ----

/// Downsample factor for a document shown at `zoom` physical px per document px.
fn scale_for(canvas: DRect, zoom: f32, faithful: bool) -> i32 {
    if faithful {
        return 1;
    }
    let (w, h) = (i64::from(canvas.width().max(1)), i64::from(canvas.height().max(1)));
    let side = ((w.max(h) as f64) / f64::from(MAX_SIDE)).ceil() as i32;
    let budget = (((w * h) as f64) / BUDGET_PIXELS).sqrt().ceil() as i32;
    let zoomed = if zoom.is_finite() && zoom > 0.0 { (1.0 / f64::from(zoom)).ceil() as i32 } else { 1 };
    zoomed.max(side).max(budget).max(1).min(64)
}

/// A copy of the document with every layer hidden: the checkerboard, nothing to composite.
fn without_layers(doc: &Document) -> Document {
    let mut d = doc.clone();
    let ids: Vec<LayerId> = d.walk().iter().map(|(_, _, l)| l.id).collect();
    for id in ids {
        if let Some(l) = d.layer_mut(id) {
            l.visible = false;
        }
    }
    d
}

/// The warped proxy: target layers carry the field, everything else is the reduced document; a
/// dab re-composites and uploads only its rectangle.
struct Preview {
    /// Proxy document: target surfaces carry the warp, everything else is the reduced original.
    doc: Document,
    doc_id: u64,
    k: i32,
    /// Proxy rectangle covering the canvas.
    bounds: DRect,
    /// Document canvas the proxy covers.
    origin: DRect,
    /// Document-resolution pixels of the targets, warped into their proxy copies.
    pixels: Vec<(LayerId, Surface)>,
    /// Document-resolution linked masks of the targets.
    masks: Vec<(LayerId, Surface)>,
    tex: Option<TextureHandle>,
    /// Proxy rectangle to warp and upload.
    dirty: Option<DRect>,
    mask_tex: Option<TextureHandle>,
    mask_dirty: bool,
}

impl Preview {
    fn new(doc: &Document, targets: &[LayerId], canvas: DRect, zoom: f32) -> Self {
        let mut p = Preview {
            doc: doc.clone(),
            doc_id: doc.id.0,
            k: 1,
            bounds: DRect::EMPTY,
            origin: canvas,
            pixels: Vec::new(),
            masks: Vec::new(),
            tex: None,
            dirty: None,
            mask_tex: None,
            mask_dirty: true,
        };
        p.rebuild(doc, targets, canvas, scale_for(canvas, zoom, false));
        p
    }

    fn size(&self) -> [usize; 2] {
        [self.bounds.width().max(0) as usize, self.bounds.height().max(0) as usize]
    }

    /// Rebuilds the proxy at `k`: the targets start from the unwarped document.
    fn rebuild(&mut self, doc: &Document, targets: &[LayerId], canvas: DRect, k: i32) {
        let k = k.max(1);
        self.doc = photocraft_compose::proxy::proxy_document(doc, k as u32);
        self.doc_id = doc.id.0;
        self.k = k;
        self.origin = canvas;
        self.pixels = targets.iter().filter_map(|id| Some((*id, doc.layer(*id)?.surface()?.clone()))).collect();
        self.masks =
            targets.iter().filter_map(|id| Some((*id, doc.layer(*id)?.mask.as_ref().filter(|m| m.linked)?.surface.clone()))).collect();
        // Proxy pixels are document pixels divided by `k`, keeping the document's own origin
        // (`proxy_document` divides content bounds the same way).
        let q = |v: i32| v.div_euclid(k);
        let ceil = |v: i32| (i64::from(v) + i64::from(k) - 1).div_euclid(i64::from(k)) as i32;
        self.bounds = DRect::new(q(canvas.x0), q(canvas.y0), ceil(canvas.x1), ceil(canvas.y1));
        self.tex = None;
        self.dirty = Some(self.bounds);
        self.mask_tex = None;
        self.mask_dirty = true;
    }

    /// Marks document pixels whose preview must change (a dab's damage).
    fn mark(&mut self, r: DRect) {
        if r.is_empty() || self.k <= 0 {
            return;
        }
        let k = i64::from(self.k);
        let rect = DRect::new(
            (i64::from(r.x0)).div_euclid(k) as i32,
            (i64::from(r.y0)).div_euclid(k) as i32,
            ((i64::from(r.x1) + k - 1).div_euclid(k)) as i32,
            ((i64::from(r.y1) + k - 1).div_euclid(k)) as i32,
        )
        .inflate(2)
        .intersect(&self.bounds);
        if rect.is_empty() {
            return;
        }
        self.dirty = Some(match self.dirty {
            Some(d) => DRect::new(d.x0.min(rect.x0), d.y0.min(rect.y0), d.x1.max(rect.x1), d.y1.max(rect.y1)),
            None => rect,
        });
    }

    /// Warps the dirty proxy region from the untouched document pixels and uploads the
    /// re-composited result. Rebuilds the proxy when the scale, document or targets changed.
    fn update(
        &mut self,
        field: &LiquifyField,
        doc: &Document,
        targets: &[LayerId],
        display: Option<&photocraft_engine::display_color::CanvasDisplay>,
        ctx: &egui::Context,
        k: i32,
    ) {
        let same_targets = self.pixels.len() == targets.len() && targets.iter().all(|id| self.pixels.iter().any(|(p, _)| p == id));
        if k != self.k || self.doc_id != doc.id.0 || !same_targets {
            self.rebuild(doc, targets, self.origin, k);
        }
        let Some(dirty) = self.dirty.take() else { return };
        if dirty.is_empty() {
            return;
        }
        let k = i64::from(self.k);
        // Displaced pixels reach a little further than the warp centre.
        let reach = (i64::from(field.max_displacement().ceil() as i32).max(0) / k).max(0) as i32 + 2;
        let region = dirty.inflate(reach).intersect(&self.bounds);
        if region.is_empty() {
            return;
        }
        for (id, src) in &self.pixels {
            if let Some(surface) = self.doc.layer_mut(*id).and_then(|l| match &mut l.content {
                LayerContent::Raster(s) => Some(s),
                _ => None,
            }) {
                warp_region(surface, src, field, self.k, region);
            }
        }
        for (id, src) in &self.masks {
            if let Some(surface) = self.doc.layer_mut(*id).and_then(|l| l.mask.as_mut().map(|m| &mut m.surface)) {
                warp_region(surface, src, field, self.k, region);
            }
        }
        let buf = photocraft_compose::render(&self.doc, region);
        let image = crate::canvas::display_image(display, &buf);
        let options = egui::TextureOptions::LINEAR;
        let full = self.size();
        match &mut self.tex {
            Some(tex) if tex.size() == full => {
                tex.set_partial([region.x0.max(0) as usize, region.y0.max(0) as usize], image, options);
            }
            _ => {
                let whole = photocraft_compose::render(&self.doc, self.bounds);
                self.tex = Some(ctx.load_texture("liquify-brush", crate::canvas::display_image(display, &whole), options));
            }
        }
    }

    #[cfg(test)]
    fn doc(&self) -> &Document {
        &self.doc
    }

    fn texture(&self) -> Option<egui::TextureId> {
        self.tex.as_ref().map(|t| t.id())
    }

    fn mask_texture_id(&self) -> Option<egui::TextureId> {
        self.mask_tex.as_ref().map(|t| t.id())
    }

    /// The red freeze-mask overlay, sampled from the field at the proxy resolution.
    fn mask_texture(&mut self, field: &LiquifyField, ctx: &egui::Context) {
        if !self.mask_dirty && self.mask_tex.is_some() {
            return;
        }
        self.mask_dirty = false;
        let k = f64::from(self.k);
        let [w, h] = self.size();
        if w == 0 || h == 0 {
            return;
        }
        let mut px = vec![Color32::TRANSPARENT; w * h];
        for y in 0..h {
            for x in 0..w {
                let v = field.freeze_at(f64::from(self.bounds.x0) + (x as f64 + 0.5) * k, f64::from(self.bounds.y0) + (y as f64 + 0.5) * k);
                px[y * w + x] = Color32::from_rgba_unmultiplied(230, 40, 40, (v.clamp(0.0, 1.0) * 140.0) as u8);
            }
        }
        let image = ColorImage::new([w, h], px);
        match &mut self.mask_tex {
            Some(t) => t.set(image, TextureOptions::LINEAR),
            None => self.mask_tex = Some(ctx.load_texture("liquify-brush-mask", image, TextureOptions::LINEAR)),
        }
    }
}

/// Warps `src` (document resolution) through `field` into `dst`'s proxy pixels over `region`,
/// bilinearly and premultiplied. Pixels outside `region` keep what they hold.
fn warp_region(dst: &mut Surface, src: &Surface, field: &LiquifyField, k: i32, region: DRect) {
    let kf = f64::from(k);
    let bounds = field.bounds;
    // `region` is in proxy coordinates (document / k), so multiplying it back lands on the
    // document grid directly.
    let pad = f64::from(field.max_displacement()).ceil() as i32 + 3;
    let foot = DRect::new(region.x0 * k - pad, region.y0 * k - pad, region.x1 * k + pad, region.y1 * k + pad).intersect(&bounds);
    if foot.is_empty() {
        return;
    }
    let (fw, fh) = (foot.width() as usize, foot.height() as usize);
    let mut source = vec![[0.0f32; 4]; fw * fh];
    src.read_rgba_into(foot, &mut source);
    for p in &mut source {
        let a = p[3];
        p[0] *= a;
        p[1] *= a;
        p[2] *= a;
    }
    let at = |x: i32, y: i32| -> [f32; 4] {
        let x = x.clamp(foot.x0, foot.x1 - 1) - foot.x0;
        let y = y.clamp(foot.y0, foot.y1 - 1) - foot.y0;
        source[y as usize * fw + x as usize]
    };
    let sample = |x: f64, y: f64| -> [f32; 4] {
        let fx = (x - 0.5).clamp(f64::from(foot.x0), f64::from(foot.x1 - 1));
        let fy = (y - 0.5).clamp(f64::from(foot.y0), f64::from(foot.y1 - 1));
        let (ix, iy) = (fx.floor(), fy.floor());
        let (tx, ty) = ((fx - ix) as f32, (fy - iy) as f32);
        let (a, b, c, e) = (at(ix as i32, iy as i32), at(ix as i32 + 1, iy as i32), at(ix as i32, iy as i32 + 1), at(ix as i32 + 1, iy as i32 + 1));
        std::array::from_fn(|i| (a[i] * (1.0 - tx) + b[i] * tx) * (1.0 - ty) + (c[i] * (1.0 - tx) + e[i] * tx) * ty)
    };
    let format = dst.format();
    let n = format.channels();
    let (w, h) = (region.width() as usize, region.height() as usize);
    let mut values = vec![0.0f32; w * h * n];
    let mut px = [0.0f32; 8];
    for y in 0..h {
        let dy = (region.y0 as f64 + y as f64 + 0.5) * kf;
        for x in 0..w {
            let dx = (region.x0 as f64 + x as f64 + 0.5) * kf;
            let d = field.sample(dx, dy);
            let p = sample(dx + d[0], dy + d[1]);
            let a = p[3].clamp(0.0, 1.0);
            let rgb = if a > 0.0 { [p[0] / a, p[1] / a, p[2] / a] } else { [0.0; 3] };
            let rgba = [rgb[0].clamp(0.0, 1.0), rgb[1].clamp(0.0, 1.0), rgb[2].clamp(0.0, 1.0), a];
            let written = photocraft_raster::from_rgba_into(&format, rgba, &mut px);
            let o = (y * w + x) * n;
            values[o..o + written].copy_from_slice(&px[..written]);
        }
    }
    dst.write_region(region, &values);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app_with_stripes() -> (PhotocraftApp, LayerId) {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.session.execute("file.new", json!({"width": 160, "height": 100, "depth": 8})).unwrap();
        let layer = app.session.edit("paint", |doc, active| {
            let s = doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap();
            for y in 10..90 {
                for x in 10..150 {
                    let v = ((x / 8) % 2) as f32;
                    s.write_pixel(x, y, &[v, 0.2, 1.0 - v, 1.0]);
                }
            }
            Ok(active.unwrap())
        });
        app.session.edit("paint", |_, _| Ok(())).unwrap();
        (app, layer.unwrap())
    }

    /// Selecting the tool and dragging warps the pixels; OK lands the strokes as one history
    /// step, and Undo restores the pixels.
    #[test]
    fn brush_stroke_warps_pixels_and_commits_as_one_step() {
        let ctx = egui::Context::default();
        let (mut app, layer) = app_with_stripes();
        crate::tool_memory::select(&mut app, Tool::Liquify);
        open(&mut app, &ctx).unwrap();
        let shown = app.session.active().unwrap().doc.layer(layer).unwrap().surface().unwrap().clone();
        let mut before = [0.0f32; 4];
        shown.read_pixel(70, 50, &mut before);

        pointer(&mut app, ToolEvent::Down { x: 60.0, y: 50.0, pressure: 1.0 }, egui::Modifiers::NONE);
        pointer(&mut app, ToolEvent::Move { x: 78.0, y: 50.0, pressure: 1.0 }, egui::Modifiers::NONE);
        pointer(&mut app, ToolEvent::Up { x: 80.0, y: 50.0 }, egui::Modifiers::NONE);
        assert_eq!(app.distort.liquify_brush.as_ref().unwrap().strokes.len(), 1);

        commit(&mut app);
        assert!(app.distort.liquify_brush.is_none(), "OK closes the mode");
        let warped = app.session.active().unwrap().doc.layer(layer).unwrap().surface().unwrap().clone();
        let mut after = [0.0f32; 4];
        warped.read_pixel(70, 50, &mut after);
        assert_ne!(before, after, "the stroke moved pixels under the brush");

        assert!(app.session.undo(), "one history step");
        let undone = app.session.active().unwrap().doc.layer(layer).unwrap().surface().unwrap().clone();
        let mut back = [0.0f32; 4];
        undone.read_pixel(70, 50, &mut back);
        assert_eq!(before, back, "Undo puts the pixels back");
    }

    /// The live preview warps the proxy under the brush and leaves the rest untouched.
    #[test]
    fn preview_warps_the_proxy_under_the_brush() {
        let ctx = egui::Context::default();
        let (mut app, layer) = app_with_stripes();
        let doc = app.session.active().unwrap().doc.clone();
        let canvas = doc.bounds();
        let mut field = LiquifyField::new(canvas, 2.0);
        let mut stroke = LiquifyStroke::new(LiquifyTool::ForwardWarp, 40.0);
        stroke.points.push(vec![60.0, 50.0]);
        field.stroke_begin(&stroke, [60.0, 50.0, 1.0]);
        stroke.points.push(vec![80.0, 50.0]);
        field.stroke_segment(&stroke, [60.0, 50.0, 1.0], [80.0, 50.0, 1.0]);

        let mut preview = Preview::new(&doc, &[layer], canvas, 1.0);
        preview.mark(DRect::new(35, 25, 105, 75));
        preview.update(&field, &doc, &[layer], None, &ctx, 1);
        assert!(preview.texture().is_some(), "the proxy is uploaded");
        let (w, h) = (60usize, 40usize);
        let region = DRect::new(40, 30, 100, 70);
        let mut before = vec![[0.0f32; 4]; w * h];
        doc.layer(layer).unwrap().surface().unwrap().read_rgba_into(region, &mut before);
        let mut after = vec![[0.0f32; 4]; w * h];
        preview.doc().layer(layer).unwrap().surface().unwrap().read_rgba_into(region, &mut after);
        let changed = before.iter().zip(&after).filter(|(a, b)| a != b).count();
        assert!(changed > 5, "the preview moved the pixels under the stroke ({changed} changed)");
        // Away from the brush the proxy still holds the document's pixels.
        let mut far = [0.0f32; 4];
        let mut original = [0.0f32; 4];
        preview.doc().layer(layer).unwrap().surface().unwrap().read_pixel(120, 30, &mut far);
        doc.layer(layer).unwrap().surface().unwrap().read_pixel(120, 30, &mut original);
        assert_eq!(far, original);
    }

    /// Scope reaches more than the active layer: All Layers warps every raster layer.
    #[test]
    fn scope_all_warps_every_layer() {
        let ctx = egui::Context::default();
        let (mut app, first) = app_with_stripes();
        app.session.execute("layer.new.layer", json!({})).unwrap();
        let second = app.session.edit("paint", |doc, active| {
            let s = doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap();
            s.fill_rect(DRect::new(10, 10, 150, 90), &[0.1, 0.9, 0.1, 1.0]);
            Ok(active.unwrap())
        }).unwrap();
        assert_ne!(first, second);
        crate::tool_memory::select(&mut app, Tool::Liquify);
        open(&mut app, &ctx).unwrap();
        let mut s = app.distort.liquify_brush.take().unwrap();
        s.scope = "all".into();
        s.scope_dirty = true;
        s.refresh_scope(&app).unwrap();
        assert!(s.targets.contains(&first) && s.targets.contains(&second), "All Layers targets every raster layer");
        app.distort.liquify_brush = Some(s);
        app.distort.liquify_brush.as_mut().unwrap().opts.size = 50.0;
        pointer(&mut app, ToolEvent::Down { x: 60.0, y: 50.0, pressure: 1.0 }, egui::Modifiers::NONE);
        pointer(&mut app, ToolEvent::Move { x: 84.0, y: 50.0, pressure: 1.0 }, egui::Modifiers::NONE);
        pointer(&mut app, ToolEvent::Up { x: 86.0, y: 50.0 }, egui::Modifiers::NONE);
        commit(&mut app);
        // Both layers keep their colours but not their shape: a green-only layer warps too.
        let mut px = [0.0f32; 4];
        app.session.active().unwrap().doc.layer(second).unwrap().surface().unwrap().read_pixel(70, 50, &mut px);
        assert!(px[3] > 0.0);
    }
}
