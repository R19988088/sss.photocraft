//! Line art's field: C3's `maxDiff` cache (`C2AdjustmentGPU.ensureLineArtField`).
//!
//! C3 computes the edge amount once per ROI into a texture keyed by
//! `(upstream, revision, origin, size, canvas, thickness, background)`, then every appearance
//! slider (intensity, sharpness, zones, colour, blend) is a pointwise read of that texture
//! (`webApplyLineArtField`). Two things follow, and both matter for the chunked compositor:
//!
//! - no chunk boundary can cut a pixel's neighbours, because the read never leaves the pixel;
//! - dragging the sliders costs one `textureLoad`, not a ring of up to 440 samples per pixel.
//!
//! The field only covers the drawn region (C3 does the same for its ROI fields), so a render of
//! one screen never pays for the whole canvas. A document too large for one field texture falls
//! back to the per-chunk ring pass (`fs_adjust_near`), which clamps to the canvas.

use photocraft_doc::Document;
use photocraft_geom::Rect;

use crate::consts::STRIDE;
use crate::convert::{W, words};
use crate::kit::{texture_group, uniform_group};
use crate::plan::FieldSpec;
use crate::textures::{CachedField, Tex};
use crate::{F, I, Kernel, Plan, U};

/// Most pixels one field texture may hold (16 MiB of R32F); a bigger drawn region keeps the
/// per-chunk ring pass instead of paging the field.
pub(crate) const MAX_FIELD_PIXELS: u64 = 4 << 20;

impl super::Compositor {
    /// Uploads the fields `plan` asks for, reusing the cached ones whose backdrop still holds.
    /// `changed` is the lowest root layer this render changed: a field describes the layers
    /// *below* its adjustment layer, so a change at the adjustment layer's own index (its
    /// sliders, its mask) or above it leaves the field valid, and only a change below it rebuilds
    /// the field. Returns how many fields were built, and how many of those the CPU had to
    /// compute because no field pipeline exists.
    pub(crate) fn sync_fields(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &Document,
        plan: &Plan<'_>,
        changed: Option<usize>,
    ) -> (usize, usize) {
        self.field_views.clear();
        self.field_views.resize(plan.fields.len(), None);
        if plan.fields.is_empty() {
            self.line_fields.clear();
            return (0, 0);
        }
        let mut cached: Vec<Option<CachedField>> = std::mem::take(&mut self.line_fields).into_iter().map(Some).collect();
        let mut out = Vec::new();
        let (mut built, mut on_cpu) = (0, 0);
        for (slot, spec) in plan.fields.iter().enumerate() {
            // `changed >= spec.layer_index`: the edit is at or above this layer, so everything the
            // field was computed from is untouched. (`changed < layer_index` was the old test, and
            // it rebuilt the field on every appearance-slider tick — a ring of hundreds of taps
            // per pixel each frame, which is what made the panel unusable.)
            let reusable = if changed.is_none_or(|ch| ch >= spec.layer_index) {
                cached.iter().position(|f| matches!(f, Some(f) if f.layer_index == spec.layer_index && f.thickness == spec.thickness && f.rect == spec.rect))
            } else {
                None
            };
            let fresh = reusable.and_then(|at| cached[at].take()).unwrap_or_else(|| {
                built += 1;
                let (field, cpu) = self.build_field(device, queue, doc, spec);
                on_cpu += usize::from(cpu);
                field
            });
            self.field_views[slot] = Some(fresh.texture.view.clone());
            out.push(fresh);
        }
        self.line_fields = out;
        (built, on_cpu)
    }

    /// The field for `spec`: `true` alongside it when the CPU had to compute it (no pipeline that
    /// renders to a float target).
    fn build_field(&self, device: &wgpu::Device, queue: &wgpu::Queue, doc: &Document, spec: &FieldSpec) -> (CachedField, bool) {
        // The backdrop: every root layer below the adjustment, composited for the field's rect
        // (which already covers the ring's reach around the drawn region).
        let mut below = doc.clone();
        below.layers.truncate(spec.layer_index);
        let buf = photocraft_compose::render(&below, spec.rect);
        let (w, h) = (spec.rect.width(), spec.rect.height());
        // C3's `c2LineArtFieldKernel` on the GPU (`fs_line_field`): one pass over the region
        // instead of a ring of up to 440 taps per pixel on the CPU, which cost close to half a
        // second at the default thickness (measured) — the whole canvas froze on every stroke
        // drawn below the adjustment. Adapters that can't render to a float target keep the CPU
        // field, uploaded as R32F (`case 29` reads `.r` either way).
        let gpu = self.kit.pipelines.contains_key(&(Kernel::LineField, self.acc_format));
        let tex = if gpu {
            let field = Tex::new(device, "pc_line_field", w, h, self.acc_format, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT);
            self.render_field(device, queue, &buf.px, w, h, &field, spec.thickness);
            field
        } else {
            let field = photocraft_compose::adjust::line_art_field(&buf, spec.thickness);
            let bytes: Vec<u8> = field.max_diff.iter().flat_map(|v| v.to_le_bytes()).collect();
            let tex =
                Tex::new(device, "pc_line_field", w, h, wgpu::TextureFormat::R32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &bytes,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            tex
        };
        let texel = if self.acc_format == wgpu::TextureFormat::Rgba16Float { 8 } else { 16 };
        let bytes = u64::from(w) * u64::from(h) * if gpu { texel } else { 4 };
        (CachedField { layer_index: spec.layer_index, thickness: spec.thickness, rect: spec.rect, texture: tex, bytes }, !gpu)
    }

    /// Draw `fs_line_field` over `field`: upload `px` (the region's composited artwork) as the
    /// pass's `tex_a`, and run the ring with `thickness` in `op.p0.z`.
    fn render_field(&self, device: &wgpu::Device, queue: &wgpu::Queue, px: &[[f32; 4]], w: u32, h: u32, field: &Tex, thickness: f32) {
        let src = Tex::new(device, "pc_line_src", w, h, wgpu::TextureFormat::Rgba32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        // Rgba32Float texels are exactly `[f32; 4]` little-endian, so the upload borrows the
        // composite instead of converting it byte by byte (that conversion alone cost ~110 ms on a
        // 1600×1000 region — more than the ring pass it feeds).
        let src_bytes: &[u8] = bytemuck::cast_slice(px);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &src.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            src_bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 16), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let mut data = vec![0u8; 2 * STRIDE as usize];
        let chunk = words(&[I(0), I(0), I(w as i32), I(h as i32)]);
        data[..chunk.len()].copy_from_slice(&chunk);
        let op = field_op(thickness);
        data[STRIDE as usize..][..op.len()].copy_from_slice(&op);
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pc_line_field_uniforms"),
            size: data.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ubuf, 0, &data);
        let bg0 = uniform_group(device, &self.kit.bgl0, &ubuf);
        let mut views: Vec<&wgpu::TextureView> = (0..9).map(|_| &self.kit.dummy).collect();
        views[0] = &src.view;
        let bg1 = texture_group(device, &self.kit.bgl1, &views);
        let Some(pipeline) = self.kit.pipelines.get(&(Kernel::LineField, self.acc_format)) else { return };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pc_line_field") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pc_line_field"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &field.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bg0, &[0, STRIDE as u32]);
            pass.set_bind_group(1, &bg1, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
    }
}

/// The `Op` record of the `fs_line_field` pass: `p0.z` is the thickness, `p0.w` the ring's
/// weight sum (see `ring_weight_sum`). Every other field is zero — the pass reads `tex_a` and
/// nothing else.
pub(crate) fn field_op(thickness: f32) -> Vec<u8> {
    let mut v: Vec<W> = vec![I(0), I(0), U(0), U(0), F(1.0), F(0.0), F(1.0), F(1.0)];
    v.extend((0..8).map(|_| I(0)));
    v.extend((0..4).map(|_| F(0.0)));
    v.extend([F(0.0), F(0.0), F(thickness), F(photocraft_compose::adjust::line_art_ring_weight_sum(thickness))]);
    v.extend((0..12).map(|_| F(0.0)));
    v.extend((0..4).map(|_| I(0)));
    v.extend((0..24).map(|_| F(0.0)));
    words(&v)
}

/// Whether a drawn region is small enough to build one field texture for.
pub(crate) fn field_fits(rect: Rect) -> bool {
    !rect.is_empty() && u64::from(rect.width()) * u64::from(rect.height()) <= MAX_FIELD_PIXELS
}
