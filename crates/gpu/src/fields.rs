//! Line art's field: C3's `maxDiff` cache (`C2AdjustmentGPU.ensureLineArtField`).
//!
//! C3 computes the edge amount once per ROI into a texture keyed by
//! `(upstream, revision, origin, size, canvas, thickness, background)`, then every appearance
//! slider (intensity, sharpness, zones, colour, blend) is a pointwise read of that texture
//! (`webApplyLineArtField`). Two things follow, and both matter for the chunked compositor:
//!
//! - no chunk boundary can cut a pixel's neighbours, because the read never leaves the pixel;
//! - dragging the sliders costs one `textureLoad`, not a ring of 59 samples per pixel.
//!
//! The field only covers the drawn region (C3 does the same for its ROI fields), so a render of
//! one screen never pays for the whole canvas. A document too large for one field texture falls
//! back to the per-chunk ring pass (`fs_adjust_near`), which clamps to the canvas.

use photocraft_geom::Rect;
use photocraft_doc::Document;

use crate::plan::FieldSpec;
use crate::Plan;
use crate::textures::{CachedField, Tex};

/// Most pixels one field texture may hold (16 MiB of R32F); a bigger drawn region keeps the
/// per-chunk ring pass instead of paging the field.
pub(crate) const MAX_FIELD_PIXELS: u64 = 4 << 20;

impl super::Compositor {
    /// Uploads the fields `plan` asks for, reusing the cached ones whose backdrop still holds.
    /// `changed` is the lowest root layer this render changed: a field describes the layers
    /// *below* its adjustment layer, so it survives changes at or above that layer (the sliders,
    /// its own mask, the layers above it).
    pub(crate) fn sync_fields(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, doc: &Document, plan: &Plan<'_>, changed: Option<usize>) {
        self.field_views.clear();
        self.field_views.resize(plan.fields.len(), None);
        if plan.fields.is_empty() {
            self.line_fields.clear();
            return;
        }
        let mut cached: Vec<Option<CachedField>> = std::mem::take(&mut self.line_fields).into_iter().map(Some).collect();
        let mut out = Vec::new();
        for (slot, spec) in plan.fields.iter().enumerate() {
            let reusable = if changed.is_none_or(|ch| ch < spec.layer_index) {
                cached.iter().position(|f| matches!(f, Some(f) if f.layer_index == spec.layer_index && f.thickness == spec.thickness && f.rect == spec.rect))
            } else {
                None
            };
            let fresh = reusable
                .and_then(|at| cached[at].take())
                .unwrap_or_else(|| self.build_field(device, queue, doc, spec));
            self.field_views[slot] = Some(fresh.texture.view.clone());
            out.push(fresh);
        }
        self.line_fields = out;
    }

    fn build_field(&self, device: &wgpu::Device, queue: &wgpu::Queue, doc: &Document, spec: &FieldSpec) -> CachedField {
        // The backdrop: every root layer below the adjustment, composited on the CPU for the
        // field's rect (which already covers the ring's reach around the drawn region).
        let mut below = doc.clone();
        below.layers.truncate(spec.layer_index);
        let buf = photocraft_compose::render(&below, spec.rect);
        let field = photocraft_compose::adjust::line_art_field(&buf, spec.thickness);
        let (w, h) = (spec.rect.width(), spec.rect.height());
        let bytes: Vec<u8> = field.max_diff.iter().flat_map(|v| v.to_le_bytes()).collect();
        let tex = Tex::new(device, "pc_line_field", w, h, wgpu::TextureFormat::R32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let bytes = u64::from(w) * u64::from(h) * 4;
        CachedField { layer_index: spec.layer_index, thickness: spec.thickness, rect: spec.rect, texture: tex, bytes }
    }
}

/// Whether a drawn region is small enough to build one field texture for.
pub(crate) fn field_fits(rect: Rect) -> bool {
    !rect.is_empty() && u64::from(rect.width()) * u64::from(rect.height()) <= MAX_FIELD_PIXELS
}
