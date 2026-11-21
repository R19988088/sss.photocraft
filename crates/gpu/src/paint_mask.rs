//! The batch mask backend: one frame's dabs laid into an atlas on the GPU, read back once
//! (`paint_surface.wgsl` does the tip maths and the pattern sampling).
//!
//! It plugs into `photocraft-paint`'s [`DabMaskBatch`] slot, so `StrokeRenderer::raster_pending`
//! stops running `rasterize_into`'s per-pixel loop per dab and takes the masks this backend laid
//! instead. Everything downstream — the coverage lanes, Wet Edges' depth, and above all the
//! pigment mixing (`accumulate_pigment` on the CPU, untouched) — keeps reading masks per dab, in
//! submission order.
//!
//! Each dab gets its own tile in the atlas (its rectangle, shelf-packed), so a batch never
//! accumulates dabs into each other: one tile, one dab, the CPU's own per-dab coverage. The
//! atlas is drawn by one instanced pass and read back once per batch, never per dab.

use std::sync::{Arc, Mutex};

use photocraft_geom::Rect;
use photocraft_paint::stamp::{DabMaskBatch, MaskBatch, MaskBatchSlot, MaskDab};

use crate::{DabStamp, PaintSurface, TextureOptions};

/// Below this many mask pixels a batch is cheaper to rasterise on the CPU than to submit, wait
/// for and read back (a small-brush stroke stays exactly as it was).
const MIN_BATCH_PIXELS: u64 = 100_000;

/// The biggest atlas a batch may ask for (coverage + paint as `Rgba16Float`, depth as `R16Float`:
/// ~18 bytes a pixel, so this is ~288 MB — well past any window the UI would let through).
const MAX_ATLAS_PIXELS: u64 = 4096 * 4096;
/// The atlas width a batch starts from before shelves stack up.
const ATLAS_START: i32 = 512;
/// The widest atlas a batch packs into.
const ATLAS_MAX_W: i32 = 4096;

/// `PC_MASK_DEBUG=1` explains why a batch fell back to the CPU.
fn dbg_note(why: &str) {
    if std::env::var_os("PC_MASK_DEBUG").is_some() {
        eprintln!("[mask] {why}");
    }
}

/// Lays a batch of dabs' masks on the GPU and hands them back to the brush engine.
pub struct GpuMasker {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The atlas and its size, rebuilt when a batch needs a different one.
    surface: Mutex<Option<(PaintSurface, (u32, u32))>>,
    /// The surface a folded frame (`frame`) is drawn on: one per frame size, reused.
    frame_surface: Mutex<Option<(PaintSurface, (u32, u32))>>,
    /// What the surface's stroke texture was last set to: the pattern is uploaded once per
    /// image, not once per frame.
    texture_key: Mutex<Option<(usize, u32, u32, u32, bool)>>,
    /// The tip the surface holds: keeping its `Arc` alive makes `ptr_eq` a sound "already
    /// uploaded" test (the address cannot be recycled under us).
    tip_key: Mutex<Option<Arc<photocraft_paint::stamp::MaskTip>>>,
}

/// The batch mask backend as the slot the brush engine takes (`set_process_mask_batch`): tilts
/// every stroke's tip maths and pattern sampling onto the window's GPU device.
pub fn masker_slot(device: wgpu::Device, queue: wgpu::Queue) -> MaskBatchSlot {
    MaskBatchSlot::new(Arc::new(GpuMasker::new(device, queue)))
}

impl GpuMasker {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self { device, queue, surface: Mutex::new(None), frame_surface: Mutex::new(None), texture_key: Mutex::new(None), tip_key: Mutex::new(None) }
    }

    /// The shelf packing of `rects`: where each tile lands, the atlas size (power-of-two buckets,
    /// so a walking stroke keeps one surface) and the extent the tiles actually fill.
    fn pack(rects: &[Rect]) -> Option<(Vec<[f32; 4]>, (u32, u32), (u32, u32))> {
        let total: u64 = rects.iter().map(|r| u64::from(r.width()) * u64::from(r.height())).sum();
        if total < MIN_BATCH_PIXELS || total > MAX_ATLAS_PIXELS {
            return None;
        }
        let widest = rects.iter().map(|r| r.width() as i32).max()?;
        let side = (total as f64).sqrt().ceil() as i32;
        let width = side.max(widest).max(1).min(ATLAS_MAX_W.max(widest));
        let mut at = Vec::with_capacity(rects.len());
        let (mut x, mut y, mut shelf) = (0i32, 0i32, 0i32);
        for r in rects {
            let (w, h) = (r.width() as i32, r.height() as i32);
            if x + w > width {
                y += shelf;
                x = 0;
                shelf = 0;
            }
            at.push([x as f32, y as f32, r.x0 as f32, r.y0 as f32]);
            x += w;
            shelf = shelf.max(h);
        }
        let height = y + shelf;
        if height <= 0 || height as u64 * width as u64 > MAX_ATLAS_PIXELS {
            return None;
        }
        // Bucket the size so a walking stroke keeps one atlas (re-creating textures per frame
        // would cost more than the masks save).
        let bucket = |v: i32| (v.max(64) as u32).next_power_of_two().min(MAX_ATLAS_PIXELS as u32);
        let (bw, bh) = (bucket(width), bucket(height));
        if u64::from(bw) * u64::from(bh) > MAX_ATLAS_PIXELS {
            return None;
        }
        Some((at, (bw, bh), (width.max(1) as u32, height.max(1) as u32)))
    }

    fn options(texture: &photocraft_paint::stamp::MaskTexture) -> TextureOptions {
        TextureOptions {
            scale: texture.scale,
            depth: texture.depth,
            mode: texture.mode,
            coordinate: texture.coordinate,
            c3_math: texture.c3_math,
            each_tip: texture.each_tip,
            origin: texture.origin,
        }
    }
}

impl DabMaskBatch for GpuMasker {
    /// A frame folded on one surface: the dabs accumulate over the frame's union rectangle and
    /// come back already folded, so a stroke of thousands of overlapping dabs costs one small
    /// readback a frame instead of one per dab. A Dual Brush's lane is drawn below the primary
    /// one on the same surface, so the frame still costs one read.
    fn frame(&self, f: &photocraft_paint::stamp::MaskFrame) -> Option<photocraft_paint::stamp::MaskFrameOut> {
        use photocraft_paint::stamp::{LaneMasks, MaskFrameOut};
        let lane = &f.lane;
        if lane.jobs.is_empty() || lane.jobs.len() != lane.rects.len() || lane.union.is_empty() {
            return None;
        }
        let pw = lane.union.width();
        let ph = lane.union.height();
        let (dw, dh) = f.dual.as_ref().map_or((0, 0), |d| (d.union.width(), d.union.height()));
        let w = pw.max(dw);
        let h = ph + dh;
        if u64::from(w) * u64::from(h) > MAX_ATLAS_PIXELS {
            dbg_note(&format!("frame union {w}×{h} is past the atlas"));
            return None;
        }
        let bucket = |v: u32| (v.max(16)).next_power_of_two().min(MAX_ATLAS_PIXELS as u32);
        let (bw, bh) = (bucket(w), bucket(h));
        let mut surface = self.frame_surface.lock().ok()?;
        if surface.as_ref().map(|(_, size)| *size) != Some((bw, bh)) {
            *surface = PaintSurface::new(&self.device, &self.queue, bw, bh).ok().map(|s| (s, (bw, bh)));
            *self.texture_key.lock().ok()? = None;
            *self.tip_key.lock().ok()? = None;
        }
        let (st, _) = surface.as_mut()?;
        // One lane's bindings (pattern and sampled tip) and its dabs, as a draw on the surface:
        // `y` is the surface row its union's top edge is drawn at.
        let upload = |st: &mut PaintSurface, lane: &photocraft_paint::stamp::FrameLane, y: u32| -> Vec<DabStamp> {
            let options = Self::options(&lane.texture);
            let key = lane.texture.image.as_ref().map(|i| (Arc::as_ptr(i) as usize, i.width as u32, i.height as u32));
            let want = key.map(|(p, iw, ih)| (p, iw, ih, options.mode, options.c3_math));
            {
                let mut uploaded = self.texture_key.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if *uploaded != want {
                    st.set_texture(&self.device, &self.queue, lane.texture.image.as_deref(), options);
                    *uploaded = want;
                }
            }
            {
                let mut tip_uploaded = self.tip_key.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let same = match (&*tip_uploaded, &lane.tip) {
                    (Some(have), Some(want)) => Arc::ptr_eq(have, want),
                    (None, None) => true,
                    _ => false,
                };
                if !same {
                    st.set_tip(&self.device, &self.queue, lane.tip.as_deref());
                    *tip_uploaded = lane.tip.clone();
                }
            }
            let origin = [lane.union.x0 as f32, lane.union.y0 as f32 - y as f32];
            lane.jobs
                .iter()
                .zip(&lane.rects)
                .map(|(job, rect)| {
                    let mut dab = DabStamp::of(job, *rect);
                    dab.atlas = [origin[0], origin[1], origin[0], origin[1]];
                    dab
                })
                .collect()
        };
        let primary = upload(st, lane, 0);
        let dual = f.dual.as_ref().map(|d| upload(st, d, ph));
        st.set_ceiling(&self.queue, lane.ceil.clamp(0.0, 1.0));
        st.clear(&self.device, &self.queue);
        st.stamp(&self.device, &self.queue, &primary).map_err(|e| dbg_note(&e.0)).ok()?;
        if let (Some(dual), Some(lane2)) = (dual, f.dual.as_ref()) {
            st.set_ceiling(&self.queue, lane2.ceil.clamp(0.0, 1.0));
            st.stamp(&self.device, &self.queue, &dual).map_err(|e| dbg_note(&e.0)).ok()?;
        }
        // One read, colour included when a lane has per-dab colours (colour dynamics).
        let want_color = lane.want_color || f.dual.as_ref().is_some_and(|d| d.want_color);
        let (coverage, depth, colors) = if want_color {
            let (c, d, rgb) = st.read_masks_color(&self.device, &self.queue, (w, h)).map_err(|e| dbg_note(&e.0)).ok()?;
            (c, d, Some(rgb))
        } else {
            let (c, d) = st.read_masks(&self.device, &self.queue, (w, h)).map_err(|e| dbg_note(&e.0)).ok()?;
            (c, d, None)
        };
        // Split the one read into each lane's own rectangle: rows 0..ph are the primary lane,
        // rows ph.. are the Dual Brush's, both starting at x 0.
        let lane_out = |x0: u32, y0: u32, lw: u32, lh: u32| {
            let (mut cov, mut dep) = (Vec::with_capacity((lw * lh) as usize), Vec::with_capacity((lw * lh) as usize));
            let mut col = colors.as_ref().map(|_| Vec::with_capacity((lw * lh) as usize * 3));
            for row in 0..lh {
                let at = ((y0 + row) * w + x0) as usize;
                let slice = &coverage[at..at + lw as usize];
                cov.extend_from_slice(slice);
                dep.extend_from_slice(&depth[at..at + lw as usize]);
                if let (Some(out), Some(rgb)) = (col.as_mut(), &colors) {
                    out.extend_from_slice(&rgb[at * 3..(at + lw as usize) * 3]);
                }
            }
            (cov, dep, col.unwrap_or_default())
        };
        let (cov, dep, col) = lane_out(0, 0, pw, ph);
        let dual_masks = f.dual.as_ref().map(|d| {
            let (cov, dep, col) = lane_out(0, ph, d.union.width(), d.union.height());
            LaneMasks { coverage: cov, depth: dep, color: col }
        });
        Some(MaskFrameOut { coverage: cov, depth: dep, color: col, dual: dual_masks })
    }

    fn masks(&self, batch: &MaskBatch) -> Option<Vec<MaskDab>> {
        if batch.jobs.is_empty() || batch.jobs.len() != batch.rects.len() {
            return None;
        }
        let Some(packed) = Self::pack(&batch.rects) else {
            dbg_note(&format!("pack refused {} dabs", batch.rects.len()));
            return None;
        };
        let (at, (width, height), used) = packed;
        let mut surface = self.surface.lock().ok()?;
        if surface.as_ref().map(|(_, size)| *size) != Some((width, height)) {
            *surface = PaintSurface::new(&self.device, &self.queue, width, height).ok().map(|s| (s, (width, height)));
            *self.texture_key.lock().ok()? = None;
            *self.tip_key.lock().ok()? = None;
        }
        let (st, _size) = surface.as_mut()?;
        let options = Self::options(&batch.texture);
        let key = batch.texture.image.as_ref().map(|i| (Arc::as_ptr(i) as usize, i.width as u32, i.height as u32));
        let mut uploaded = self.texture_key.lock().ok()?;
        let want = key.map(|(p, w, h)| (p, w, h, options.mode, options.c3_math));
        if *uploaded != want {
            st.set_texture(&self.device, &self.queue, batch.texture.image.as_deref(), options);
            *uploaded = want;
        }
        // The sampled tip: uploaded once per bitmap, and only when the batch actually carries one.
        let mut tip_uploaded = self.tip_key.lock().ok()?;
        let same = match (&*tip_uploaded, &batch.tip) {
            (Some(have), Some(want)) => Arc::ptr_eq(have, want),
            (None, None) => true,
            _ => false,
        };
        if !same {
            st.set_tip(&self.device, &self.queue, batch.tip.as_deref());
            *tip_uploaded = batch.tip.clone();
        }
        st.clear(&self.device, &self.queue);
        let dabs: Vec<DabStamp> = batch
            .jobs
            .iter()
            .zip(&batch.rects)
            .zip(&at)
            .map(|((job, rect), at)| {
                let mut dab = DabStamp::of(job, *rect);
                // The instance keeps its document rectangle; this is the shift that lands it on
                // its tile (and, read back in the fragment, back onto the document).
                let shift = [rect.x0 as f32 - at[0], rect.y0 as f32 - at[1]];
                dab.atlas = [shift[0], shift[1], shift[0], shift[1]];
                dab
            })
            .collect();
        st.stamp(&self.device, &self.queue, &dabs).map_err(|e| dbg_note(&e.0)).ok()?;
        // Only the packed extent comes back: the atlas is bucketed to powers of two and can be
        // several times the pixels the dabs actually fill.
        let (coverage, depth) = st.read_masks(&self.device, &self.queue, used).map_err(|e| dbg_note(&e.0)).ok()?;
        let atlas_w = used.0 as usize;
        let mut out = Vec::with_capacity(batch.rects.len());
        for (rect, at) in batch.rects.iter().zip(&at) {
            let (w, h) = (rect.width() as usize, rect.height() as usize);
            let (x0, y0) = (at[0] as usize, at[1] as usize);
            let mut cov = Vec::with_capacity(w * h);
            let mut dep = Vec::with_capacity(w * h);
            for row in 0..h {
                let src = (y0 + row) * atlas_w + x0;
                let Some(slice) = coverage.get(src..src + w) else { return None };
                cov.extend_from_slice(slice);
                let Some(slice) = depth.get(src..src + w) else { return None };
                dep.extend_from_slice(slice);
            }
            out.push(MaskDab { coverage: cov, depth: dep });
        }
        let _ = ATLAS_START;
        Some(out)
    }
}
