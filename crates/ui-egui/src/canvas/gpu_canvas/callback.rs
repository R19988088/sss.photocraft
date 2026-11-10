use super::resources::{CanvasStyle, Resources, ViewGpu, f32_bytes};
use super::{CallbackResources, CallbackTrait, ScreenDescriptor, VIEW_FLOATS, VIEW_UNIFORM_SIZE, ViewParams, wgpu};

// ---------------------------------------------------------------------------------------------
// Per-frame callback

pub(super) struct CanvasCallback {
    pub(super) rect: egui::Rect,
    pub(super) params: ViewParams,
}

/// The canvas drop shadow on its own, so a view drawn on the CPU still gets the same shadow as
/// one drawn by [`CanvasCallback`]. It needs nothing but the view uniform: C3's
/// `canvasShadowVertex`/`canvasShadowFragment` take the document rectangle from `view.b`.
pub(super) struct ShadowCallback {
    pub(super) rect: egui::Rect,
    pub(super) params: ViewParams,
}

/// Sampling mode: 0 trilinear, 1 bilinear, 2 anti-aliased nearest.
pub(super) fn filter_mode(scale: f32) -> (f32, f32) {
    if scale < 0.999 {
        (0.0, (-scale.log2()).max(0.0))
    } else if (scale - scale.round()).abs() < 1e-3 || scale >= 2.0 {
        (2.0, 0.0)
    } else {
        (1.0, 0.0)
    }
}

pub(super) fn map_doc_px(origin: [f32; 2], scale: f32, center: [f32; 2], cos: f32, sin: f32, x: f32, y: f32) -> [f32; 2] {
    let dx = x - center[0];
    let dy = y - center[1];
    let rx = dx * cos - dy * sin;
    let ry = dx * sin + dy * cos;
    [origin[0] + center[0] * scale + rx * scale, origin[1] + center[1] * scale + ry * scale]
}

pub(super) fn tile_screen_aabb(origin: [f32; 2], scale: f32, center: [f32; 2], cos: f32, sin: f32, tile: [f32; 4]) -> (f32, f32, f32, f32) {
    let [tx, ty, tw, th] = tile;
    if sin.abs() < 1e-8 && (cos - 1.0).abs() < 1e-8 {
        let x0 = origin[0] + tx * scale;
        let y0 = origin[1] + ty * scale;
        return (x0, y0, x0 + tw * scale, y0 + th * scale);
    }
    let pts = [
        map_doc_px(origin, scale, center, cos, sin, tx, ty),
        map_doc_px(origin, scale, center, cos, sin, tx + tw, ty),
        map_doc_px(origin, scale, center, cos, sin, tx + tw, ty + th),
        map_doc_px(origin, scale, center, cos, sin, tx, ty + th),
    ];
    let mut x0 = pts[0][0];
    let mut y0 = pts[0][1];
    let mut x1 = x0;
    let mut y1 = y0;
    for p in pts {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    (x0, y0, x1, y1)
}

impl CanvasCallback {
    /// Screen-pixel origin of document (0, 0) (snapped to whole pixels) and device px per doc px.
    fn placement(&self, ppp: f32) -> ([f32; 2], f32) {
        placement_of(&self.params, self.rect, ppp)
    }

    fn uniforms(&self, screen: [u32; 2], ppp: f32, out_linear: bool, style: &CanvasStyle) -> [f32; VIEW_FLOATS] {
        uniforms_of(&self.params, self.rect, screen, ppp, out_linear, style)
    }
}

impl ShadowCallback {
    fn uniforms(&self, screen: [u32; 2], ppp: f32, out_linear: bool, style: &CanvasStyle) -> [f32; VIEW_FLOATS] {
        uniforms_of(&self.params, self.rect, screen, ppp, out_linear, style)
    }
}

/// Screen-pixel origin of document (0, 0) (snapped to whole pixels) and device px per doc px.
fn placement_of(p: &ViewParams, rect: egui::Rect, ppp: f32) -> ([f32; 2], f32) {
    let scale = p.zoom * ppp;
    let c = rect.center();
    let ox = (c.x * ppp - p.center[0] * scale).round();
    let oy = (c.y * ppp - p.center[1] * scale).round();
    ([ox, oy], scale)
}

fn uniforms_of(p: &ViewParams, rect: egui::Rect, screen: [u32; 2], ppp: f32, out_linear: bool, style: &CanvasStyle) -> [f32; VIEW_FLOATS] {
    let (origin, scale) = placement_of(p, rect, ppp);
        let (mode, lod) = filter_mode(scale);
        // The pixel grid shows above 500% (the constants are shared with the CPU path,
        // pixel_grid.rs). `p.zoom` is screen points per document pixel; the shared threshold is
        // in device pixels, so scale by the display's pixels per point.
        let grid = if p.pixel_grid && crate::pixel_grid::shows_at(p.zoom * ppp) { crate::pixel_grid::STRENGTH } else { 0.0 };
        let square = if style.checker_square > 0.0 { (style.checker_square * ppp).round().max(1.0) } else { 0.0 };
        let (l, d, g) = (style.checker_light, style.checker_dark, style.gamut_color);
        // 32-bit preview: linear-light gain 2^exposure (0 = off) and 1 / gamma.
        let (gain, inv_gamma) = match p.hdr {
            Some([e, gm]) if e.is_finite() && gm.is_finite() && gm > 0.0 => (2f32.powf(e.clamp(-20.0, 20.0)), 1.0 / gm.max(0.01)),
            _ => (0.0, 1.0),
        };
        [
            screen[0] as f32,
            screen[1] as f32,
            scale,
            ppp,
            origin[0],
            origin[1],
            p.doc_size[0] as f32,
            p.doc_size[1] as f32,
            mode,
            lod,
            grid,
            square,
            if p.offset[0].is_finite() { p.offset[0] } else { 0.0 },
            if p.offset[1].is_finite() { p.offset[1] } else { 0.0 },
            p.display as f32,
            if out_linear { 1.0 } else { 0.0 },
            l[0],
            l[1],
            l[2],
            style.gamut_opacity,
            d[0],
            d[1],
            d[2],
            gain,
            g[0],
            g[1],
            g[2],
            inv_gamma,
            {
                let r = if p.rotation.is_finite() { p.rotation } else { 0.0 };
                if r.abs() < 1e-8 { 1.0 } else { r.cos() }
            },
            {
                let r = if p.rotation.is_finite() { p.rotation } else { 0.0 };
                if r.abs() < 1e-8 { 0.0 } else { r.sin() }
            },
            p.center[0],
            p.center[1],
        ]
}

/// Upload the view uniform this callback's key points at. Shared: the shadow-only callback
/// writes its own key so the two never fight over a buffer.
fn upload_view(
    params: &ViewParams,
    data: &[f32; VIEW_FLOATS],
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    resources: &mut CallbackResources,
) -> bool {
    let Some(res) = resources.get_mut::<Resources>() else { return false };
    if !res.health.is_ok() {
        return false;
    }
    let (bgl, sampler) = (&res.view_bgl, &res.sampler);
    let v = res.views.entry(params.view_key).or_insert_with(|| {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pc_canvas_view"),
            size: VIEW_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pc_canvas_view"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        });
        ViewGpu { buffer, bind_group, frame_data: [f32::NAN; VIEW_FLOATS] }
    });
    if v.frame_data != *data {
        queue.write_buffer(&v.buffer, 0, &f32_bytes(data));
        v.frame_data = *data;
    }
    true
}

/// Set up the pass for a callback drawn in whole-screen device pixels, with the scissor doing
/// the clipping. Returns `None` when the clip is empty.
fn set_screen_pass(info: &egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>) -> Option<([f32; 2], egui::Rect)> {
    let [sw, sh] = info.screen_size_px;
    let clip = info.clip_rect_in_pixels();
    if clip.width_px <= 0 || clip.height_px <= 0 || sw == 0 || sh == 0 {
        return None;
    }
    pass.set_viewport(0.0, 0.0, sw as f32, sh as f32, 0.0, 1.0);
    pass.set_scissor_rect(clip.left_px.max(0) as u32, clip.top_px.max(0) as u32, clip.width_px as u32, clip.height_px as u32);
    Some(([sw as f32, sh as f32], info.clip_rect))
}

impl CallbackTrait for CanvasCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get::<Resources>() else { return Vec::new() };
        let data = self.uniforms(screen.size_in_pixels, screen.pixels_per_point, res.out_linear, &res.style);
        upload_view(&self.params, &data, device, queue, resources);
        Vec::new()
    }

    fn paint(&self, info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &CallbackResources) {
        let Some(res) = resources.get::<Resources>() else { return };
        if !res.health.is_ok() {
            return;
        }
        let Some(view) = res.views.get(&self.params.view_key) else { return };
        if set_screen_pass(&info, pass).is_none() {
            return;
        }
        let clip = info.clip_rect_in_pixels();
        pass.set_bind_group(0, &view.bind_group, &[]);
        if self.params.shadow {
            pass.set_pipeline(&res.shadow_pipeline);
            pass.draw(0..6, 0..1);
        }
        let Some(doc) = res.docs.get(&self.params.doc) else { return };
        if doc.size != self.params.doc_size {
            return;
        }
        let (origin, scale) = self.placement(info.pixels_per_point);
        let (cx0, cy0) = (clip.left_px as f32, clip.top_px as f32);
        let (cx1, cy1) = (cx0 + clip.width_px as f32, cy0 + clip.height_px as f32);
        let rot = if self.params.rotation.is_finite() { self.params.rotation } else { 0.0 };
        let (cos, sin) = if rot.abs() < 1e-8 { (1.0, 0.0) } else { (rot.cos(), rot.sin()) };
        let cen = self.params.center;
        let [ox, oy] = self.params.offset.map(|v| if v.is_finite() { v } else { 0.0 });
        pass.set_pipeline(&res.tile_pipeline);
        pass.set_bind_group(2, res.luts.get(&(self.params.doc, self.params.output)).unwrap_or(&res.identity_lut), &[]);
        for t in &doc.tiles {
            let [tx, ty, tw, th] = t.rect.map(|v| v as f32);
            let (x0, y0, x1, y1) = tile_screen_aabb(origin, scale, cen, cos, sin, [tx + ox, ty + oy, tw, th]);
            if x1 < cx0 || y1 < cy0 || x0 > cx1 || y0 > cy1 {
                continue;
            }
            pass.set_bind_group(1, &t.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

impl CallbackTrait for ShadowCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get::<Resources>() else { return Vec::new() };
        let data = self.uniforms(screen.size_in_pixels, screen.pixels_per_point, res.out_linear, &res.style);
        upload_view(&self.params, &data, device, queue, resources);
        Vec::new()
    }

    fn paint(&self, info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &CallbackResources) {
        let Some(res) = resources.get::<Resources>() else { return };
        if !res.health.is_ok() {
            return;
        }
        let Some(view) = res.views.get(&self.params.view_key) else { return };
        if set_screen_pass(&info, pass).is_none() {
            return;
        }
        pass.set_bind_group(0, &view.bind_group, &[]);
        pass.set_pipeline(&res.shadow_pipeline);
        pass.draw(0..6, 0..1);
    }
}
