//! Dab rasterisation and stroke compositing.
//!
//! A stroke paints into a sparse, tiled *stroke buffer* (coverage plus, with Color Dynamics, a
//! premultiplied colour), exactly like a private stroke layer: dabs build up with flow up to their
//! opacity ceiling, stroke-level masks (Dual Brush, Texture) apply when the buffer is composited,
//! and the whole buffer composites onto the pre-stroke pixels at the stroke opacity. Because the
//! buffer is sparse and compositing always starts from the pre-stroke pixels, strokes can be fed in
//! chunks and re-composited incrementally for interactive painting (see [`StrokeRenderer`]).

use crate::material::{ResolvedWet, WetOutline, WetStyle};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use photocraft_color::{BlendMode, ColorMode, PixelFormat, SampleType};
use photocraft_geom::{Point, Rect};
use photocraft_raster::{Surface, from_rgba_into, to_rgba};

use crate::brush::{BrushSettings, MaskMode, Pattern, TipShape};
use crate::dynamics::DabGenerator;
use crate::retouch::{Footprint, alpha_index, over_native};
use crate::rng::hash2;
use crate::tile::{Mips, PatternImage};
use crate::{Dab, StrokePoint, dab_coverage, tip_falloff, tip_reach};

/// Salt of the 8-bit dither's per-pixel noise.
const DITHER_SALT: u64 = 0x0000_6469_7468_6572;

/// Before a stroke's result is quantised to 8 bits, offset every channel of the pixel by the same
/// deterministic noise of under half a level, so fractional values round up or down in proportion
/// (a soft, low-flow stroke's fine gradient survives as grain instead of bands). A value already on
/// an 8-bit level still rounds back to itself, and because the noise depends only on the pixel's
/// position, the live preview, the commit and a replay all agree.
#[inline]
fn dither8(px: &mut [f32], x: i32, y: i32) {
    let d = (hash2(x, y, DITHER_SALT) - 0.5) / 255.0;
    for v in px.iter_mut() {
        *v += d;
    }
}


// ---------------------------------------------------------------------------
// Parallelism (ported from the upstream renderer; pixel results are identical)
// ---------------------------------------------------------------------------

/// Independent pixels worth the thread pool. Below it the pool's overhead outweighs the work.
pub(crate) const DEFAULT_PARALLEL_MIN_PIXELS: usize = 256 * 256;

/// The general loop (sampled tips, noise, textures, wet edges) reaches the thread pool at a
/// quarter of the pixels: threads already halve a dab of 16,000 pixels or more.
#[cfg(not(target_arch = "wasm32"))]
const GENERAL_LOOP_COST: usize = 4;

#[cfg(not(target_arch = "wasm32"))]
static PARALLEL_MIN_PIXELS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(DEFAULT_PARALLEL_MIN_PIXELS);

/// Whether `pixels` of independent work are worth the thread pool (the web has none: there every
/// stage takes its serial path).
#[cfg(not(target_arch = "wasm32"))]
fn parallel(pixels: usize) -> bool {
    pixels >= PARALLEL_MIN_PIXELS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Tests lower the threshold to exercise both paths.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn set_parallel_min_pixels(n: usize) {
    PARALLEL_MIN_PIXELS.store(n, std::sync::atomic::Ordering::Relaxed);
}

fn for_each_item<T: Send>(items: &mut [T], pixels: usize, f: impl Fn(&mut T) + Sync) {
    #[cfg(not(target_arch = "wasm32"))]
    if parallel(pixels) {
        use rayon::prelude::*;
        items.par_iter_mut().for_each(&f);
        return;
    }
    #[cfg(target_arch = "wasm32")]
    let _ = pixels;
    items.iter_mut().for_each(&f);
}

/// Buffers a composite hands back for reuse, so a long stroke does not allocate per tile.
#[derive(Default, Debug)]
struct RegionPool(Vec<Vec<f32>>);

impl Clone for RegionPool {
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// Edge of a stroke-buffer tile.
pub const COV_TILE: i32 = 64;
/// Computed tips with a smaller radius (in pixels) are rasterised by area, not at pixel centres.
pub(crate) const SMALL_TIP_RADIUS: f32 = 3.0;
/// Sub-pixel sample offsets (a 4×4 grid) for the area coverage of small tips.
const SUBPIXEL: [f32; 4] = [-0.375, -0.125, 0.125, 0.375];
const NOISE_SALT: u64 = 0x006E_6F69_7365;

#[inline]
/// Where an aliased (Pencil) dab of `diameter` pixels centred near (`x`, `y`) lands on the pixel
/// grid: the centre of the pixel holding the point for odd diameters, the nearest pixel corner
/// for even ones. Its footprint is then a whole-pixel block around that point, the same square
/// the Pencil cursor shows ([`grid_square`]).
pub fn grid_center(x: f64, y: f64, diameter: f32) -> (f32, f32) {
    let n = if diameter.is_finite() { diameter.round().max(1.0) as i64 } else { 1 };
    let snap = |v: f64| if n % 2 == 1 { v.floor() + 0.5 } else { v.round() };
    (snap(x) as f32, snap(y) as f32)
}

/// The whole-pixel square `[x0, y0, x1, y1]` an aliased dab of `diameter` pixels at (`x`, `y`)
/// can touch (see [`grid_center`]): the Pencil's cursor.
pub fn grid_square(x: f64, y: f64, diameter: f32) -> [f64; 4] {
    let n = if diameter.is_finite() { f64::from(diameter.round().clamp(1.0, 100_000.0)) } else { 1.0 };
    let (cx, cy) = grid_center(x, y, n as f32);
    let h = n / 2.0;
    [f64::from(cx) - h, f64::from(cy) - h, f64::from(cx) + h, f64::from(cy) + h]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Wet Edges: how much paint stays at a pixel whose nearest dab reaches it at `rn` (0 at a dab's
/// centre, 1 at its rim). The interior keeps half the paint, the rim all of it.
#[inline]
fn wet_factor(rn: f32) -> f32 {
    0.5 + 0.5 * smoothstep(0.5, 1.0, rn)
}

/// Colour Divide on coverages: `p / s`, clipped. A black secondary (s = 0) divides by nothing and
/// gives full coverage, as ProCraft's divide does.
#[inline]
fn divide(p: f32, s: f32) -> f32 {
    if s <= 0.0 {
        1.0
    } else {
        (p / s).min(1.0)
    }
}

/// Combine brush coverage `v` with a mask value `t` (texture/dual tip) at `depth`.
/// Every mode maps `v = 0` to 0 (a mask never adds paint where the brush has none).
pub fn mask_combine(mode: MaskMode, v: f32, t: f32, d: f32) -> f32 {
    if v <= 0.0 {
        return 0.0;
    }
    let lerp = |a: f32, b: f32| a + (b - a) * d;
    let r = match mode {
        MaskMode::Normal => v + d * t - v * d * t,
        MaskMode::Multiply => v * (1.0 - d + d * t),
        MaskMode::Subtract => (v - d * (1.0 - t)).max(0.0),
        MaskMode::Lighten => lerp(v, v.max(t)),
        MaskMode::Difference => lerp(v, (v - t).abs()),
        MaskMode::Divide => lerp(v, divide(v, t)),
        MaskMode::Darken => v.min(1.0 - d + d * t),
        MaskMode::Overlay => {
            let o = if v < 0.5 { 2.0 * v * t } else { 1.0 - 2.0 * (1.0 - v) * (1.0 - t) };
            lerp(v, o)
        }
        MaskMode::ColorDodge => lerp(v, (v / (1.0 - t).max(1e-3)).min(1.0)),
        MaskMode::ColorBurn => lerp(v, 1.0 - ((1.0 - v) / t.max(1e-3)).min(1.0)),
        MaskMode::LinearBurn => lerp(v, (v + t - 1.0).max(0.0)),
        MaskMode::HardMix => lerp(v, if v + t > 1.0 { 1.0 } else { 0.0 }),
        MaskMode::LinearHeight => v * ((t - (1.0 - d * v)) * 4.0 + 0.5).clamp(0.0, 1.0),
        MaskMode::Height => {
            if t >= 1.0 - d * v {
                v
            } else {
                0.0
            }
        }
    };
    r.clamp(0.0, 1.0)
}

/// C3 combines completed lane coverages; lifting modes also show a secondary-only footprint.
pub fn dual_blend(mode: MaskMode, p: f32, s: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    let s = s.clamp(0.0, 1.0);
    let lifts = matches!(mode, MaskMode::Normal | MaskMode::Lighten | MaskMode::Darken | MaskMode::Overlay | MaskMode::LinearBurn | MaskMode::HardMix);
    if p <= 0.001 {
        return if lifts { s } else { 0.0 };
    }
    let alpha = match mode {
        MaskMode::Normal => p + s - p * s,
        MaskMode::Multiply => p * s,
        MaskMode::Darken => p.min(s),
        MaskMode::Lighten => p.max(s),
        MaskMode::Difference => (p - s).abs(),
        MaskMode::Divide => divide(p, s),
        MaskMode::Overlay => {
            if p < 0.5 {
                2.0 * p * s
            } else {
                1.0 - 2.0 * (1.0 - p) * (1.0 - s)
            }
        }
        MaskMode::ColorDodge => {
            if s >= 1.0 {
                1.0
            } else {
                (p / (1.0 - s).max(0.0001)).min(1.0)
            }
        }
        MaskMode::ColorBurn => {
            if s <= 0.0 {
                0.0
            } else {
                (1.0 - (1.0 - p) / s.max(0.0001)).max(0.0)
            }
        }
        MaskMode::LinearBurn => (p + s - 1.0).max(0.0),
        MaskMode::HardMix => {
            if p + s >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        MaskMode::LinearHeight => p * (0.5 + 0.5 * s),
        MaskMode::Subtract => (p - s).max(0.0),
        MaskMode::Height => mask_combine(mode, p, s, 1.0),
    };
    if lifts { alpha } else { alpha.min(p) }
}

const HEIGHT_REACH: f32 = 12.0;

/// Combine brush coverage `v` with a Texture pattern value `t` at `depth` (Brush Settings ›
/// Texture). Where it differs from [`mask_combine`] (still used for the Dual Brush) it follows
/// Photoshop as measured with single dabs over a grey ramp (#3340):
/// - Subtract takes `depth × t` away, so a white pattern removes the most paint.
/// - Color Dodge and Color Burn move the pattern toward their neutral colour as depth falls.
/// - Height treats the pattern as a height field the brush presses into: the paint reaches
///   `12 × depth × v` and fills whatever lies below that, so a low depth paints only the dark
///   valleys, from about 17 % the whole pattern is under the paint, and a brighter pattern
///   takes paint away.
/// - Linear Height is Height, but where the reach stays below the top of the pattern the paint
///   fades with the pattern instead of being cut off.
pub fn texture_combine(mode: MaskMode, v: f32, t: f32, d: f32) -> f32 {
    if v <= 0.0 {
        return 0.0;
    }
    let r = match mode {
        MaskMode::Subtract => v - d * t,
        MaskMode::ColorDodge => v / (1.0 - d * t).max(1e-3),
        MaskMode::ColorBurn => 1.0 - (1.0 - v) / (1.0 - d + d * t).max(1e-3),
        // Photoshop steps the depth in 1/255ths here, which shows at the lowest depths.
        MaskMode::Height => HEIGHT_REACH * (d * 255.0).round() / 255.0 * v - t,
        MaskMode::LinearHeight => {
            let reach = HEIGHT_REACH * (d * 255.0).round() / 255.0 * v;
            if reach < 1.0 { reach * (1.0 - t) } else { reach - t }
        }
        _ => mask_combine(mode, v, t, d),
    };
    if r.is_finite() { r.clamp(0.0, 1.0) } else { 0.0 }
}

/// A stroke-ready brush: the settings plus prepared tip mips and texture.
#[derive(Clone, Debug)]
pub struct BrushContext {
    pub brush: BrushSettings,
    tip: Option<Arc<Mips>>,
    dual_tip: Option<Arc<Mips>>,
    /// The tip's level 0 as an 8-bit bitmap for a batch mask backend (built once, with the tip).
    tip_mask: Option<Arc<crate::stamp::MaskTip>>,
    texture: Option<Arc<PatternImage>>,
    secondary: Option<Box<BrushContext>>,
    texture_origin: Option<Point>,
    texture_tip: Option<Dab>,
    /// The injected dab-stamping backend (the GPU kernel); empty = paint everything on the CPU.
    stamper: crate::stamp::StamperSlot,
    /// The injected batch mask backend (the GPU paint surface); empty = rasterise every dab's
    /// mask on the CPU, as paint alone does.
    pub(crate) masker: crate::stamp::MaskBatchSlot,
    /// The injected transport backend (the GPU compute pass); empty = smear on the CPU.
    pub(crate) transport: crate::stamp::TransportSlot,
}

/// The tip's level 0 as an 8-bit bitmap, for a backend that samples the pyramid itself. The
/// `Shape` bitmaps brushes ship are 8-bit, so this round trip is exact.
fn mask_tip_of(m: &Mips) -> Option<Arc<crate::stamp::MaskTip>> {
    let (w, h, values) = m.levels.first()?;
    let data: Vec<u8> = values.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    Some(Arc::new(crate::stamp::MaskTip { width: *w as u32, height: *h as u32, data }))
}

fn prepare_pattern(b: &BrushSettings) -> Option<Arc<PatternImage>> {
    let t = &b.texture;
    if !t.enabled {
        return None;
    }
    let mut img = match &t.pattern {
        Pattern::Procedural { style, size, seed } => crate::procedural::pattern(*style, *size, *seed),
        p => match p.bitmap() {
            Some(g) if g.is_valid() => PatternImage { width: g.width as usize, height: g.height as usize, data: g.to_f32() },
            _ => return None,
        },
    };
    for v in &mut img.data {
        *v = t.level(*v);
    }
    Some(Arc::new(img))
}

/// The pixel rectangle `reach` (plus 1 px of anti-aliasing slack) around a dab's centre. The casts
/// saturate and the arithmetic saturates, so a far or non-finite centre or reach from a direct
/// caller gives a (possibly empty) rectangle instead of an `i32` overflow (#977).
pub(crate) fn rect_around(d: &Dab, reach: f32) -> Rect {
    let rr = (reach.ceil() as i32).saturating_add(1);
    let (cx, cy) = (d.center.x.floor() as i32, d.center.y.floor() as i32);
    Rect::new(cx.saturating_sub(rr), cy.saturating_sub(rr), cx.saturating_add(rr).saturating_add(1), cy.saturating_add(rr).saturating_add(1))
}

impl BrushContext {
    /// Route dabs through `stamper` when it handles them (a GPU backend), for this context and,
    /// when the brush has a Dual Brush, its secondary context too.
    pub fn set_stamper(&mut self, stamper: crate::stamp::StamperSlot) {
        if let Some(secondary) = self.secondary.as_deref_mut() {
            secondary.set_stamper(stamper.clone());
        }
        self.stamper = stamper;
    }

    /// The backend injected by [`set_stamper`](Self::set_stamper), if any.
    pub fn stamper(&self) -> Option<&dyn crate::stamp::DabStamper> {
        self.stamper.stamper()
    }

    /// Route whole-frame masks through `masker` when it handles them (the GPU paint surface),
    /// for this context and, when the brush has a Dual Brush, its secondary context too.
    pub fn set_masker(&mut self, masker: crate::stamp::MaskBatchSlot) {
        if let Some(secondary) = self.secondary.as_deref_mut() {
            secondary.set_masker(masker.clone());
        }
        self.masker = masker;
    }

    /// The batch mask backend this context was given, if any.
    pub fn masker(&self) -> Option<&dyn crate::stamp::DabMaskBatch> {
        self.masker.batch()
    }

    /// Route a stroke's colour transport through `transport` when it handles it (the GPU compute
    /// pass), and through the CPU's own per-dab carrier otherwise.
    pub fn set_transport(&mut self, transport: crate::stamp::TransportSlot) {
        if let Some(secondary) = self.secondary.as_deref_mut() {
            secondary.set_transport(transport.clone());
        }
        self.transport = transport;
    }

    /// The transport backend this context was given, if any.
    pub fn transport(&self) -> Option<&dyn crate::stamp::DabTransport> {
        self.transport.transport()
    }

    /// The stroke's texture as a batch backend takes it.
    pub fn mask_texture(&self) -> crate::stamp::MaskTexture {
        let t = &self.brush.texture;
        crate::stamp::MaskTexture {
            image: self.texture.clone(),
            mode: t.mode.code(),
            coordinate: t.coordinate_mode.code(),
            scale: t.scale.max(0.01),
            depth: t.depth.clamp(0.0, 1.0),
            c3_math: t.c3_math,
            each_tip: t.enabled && t.each_tip,
            origin: self.texture_origin.map_or([0.0, 0.0], |p| [p.x as f32, p.y as f32]),
        }
    }

    /// The job a stamper would take for this dab, `None` for a dab only the CPU rasterises
    /// (sampled tip, per-tip texture, Noise, Grain, single-dab Wet Edges, non-finite fields).
    pub fn stamp_job(&self, d: &Dab, with_depth: bool) -> Option<crate::stamp::StampJob> {
        crate::stamp::StampJob::of(d, self, with_depth)
    }

    /// The job a batch mask backend takes for this dab: as [`stamp_job`](Self::stamp_job), but a
    /// per-tip textured or sampled-tip dab is allowed (the backend samples both itself).
    pub fn mask_job(&self, d: &Dab, wet: bool) -> Option<crate::stamp::StampJob> {
        crate::stamp::StampJob::of_with(d, self, wet, true, true)
    }

    /// The tip bitmap's level-0 scale (`big / (2 r)`) and the mip level a dab samples
    /// (`level_for(2 r ro)`), as `rasterize_into` takes them; `(0, 0)` for a computed tip.
    pub(crate) fn tip_scale_and_level(&self, radius: f32, across: f32) -> (f32, u32) {
        let Some(m) = self.tip.as_deref() else { return (0.0, 0) };
        let (w, h, _) = &m.levels[0];
        let big = *w.max(h) as f32;
        (big / (2.0 * radius).max(0.001), m.level_for(across) as u32)
    }

    /// The brush's sampled tip as a mask backend takes it (level 0, 8-bit); `None` for a
    /// computed tip.
    pub fn mask_tip(&self) -> Option<std::sync::Arc<crate::stamp::MaskTip>> {
        self.tip_mask.clone()
    }

    /// Whether the tip is a sampled bitmap (which a computed-tip stamper can't draw).
    pub(crate) fn sampled_tip(&self) -> bool {
        self.tip.is_some()
    }

    /// Whether the dab takes a per-tip texture sample.
    pub(crate) fn per_tip_texture(&self) -> bool {
        self.brush.texture.enabled && self.brush.texture.each_tip && self.texture.is_some()
    }

    /// Whether Wet Edges runs as a single-dab outline (a stroke-level feature).
    pub(crate) fn single_wet_outline(&self) -> bool {
        let b = &self.brush;
        b.wet_edges && !b.erase && b.wet_edge.as_ref().is_some_and(|w| w.outline == WetOutline::Single)
    }

    pub fn new(brush: &BrushSettings) -> Self {
        let brush = brush.bounded_for_render();
        // Sampled tips go through the prepared-tip cache (upstream's `mip_cache`): a repeated
        // brush reuses its pyramid, and Edit › Purge › All can report and release it.
        let mips = |t: &TipShape| match t.bitmap() {
            Some(g) if g.is_valid() => Some(crate::mip_cache::tip_mips(g)),
            _ => None,
        };
        let tip = mips(&brush.tip);
        let tip_mask = tip.as_deref().and_then(mask_tip_of);
        Self {
            texture_origin: None,
            texture_tip: None,
            stamper: crate::stamp::process_stamper(),
            masker: crate::stamp::process_mask_batch(),
            transport: crate::stamp::process_transport(),
            tip_mask,
            tip,
            dual_tip: if brush.dual_brush.enabled { mips(&brush.dual_brush.tip) } else { None },
            texture: prepare_pattern(&brush),
            secondary: brush.dual_brush.enabled.then(|| brush.dual_brush.resolved_brush(&brush)).flatten().as_ref().map(|b| Box::new(Self::new(b))),
            brush,
        }
    }

    /// Texture value at a document pixel (bilinear, tiled).
    #[inline]
    pub fn texture_at(&self, x: i32, y: i32) -> f32 {
        self.texture_at_point(x as f32 + 0.5, y as f32 + 0.5, self.texture_tip.as_ref())
    }

    fn texture_at_point(&self, mut x: f32, mut y: f32, dab: Option<&Dab>) -> f32 {
        use crate::brush::TextureCoordinateMode;
        let Some(p) = &self.texture else { return 1.0 };
        match self.brush.texture.coordinate_mode {
            TextureCoordinateMode::CanvasLocked => {}
            TextureCoordinateMode::StrokeLocked => {
                if let Some(origin) = self.texture_origin {
                    x -= origin.x as f32;
                    y -= origin.y as f32;
                }
            }
            TextureCoordinateMode::TipLocal => {
                if let Some(d) = dab {
                    let dx = x - d.center.x as f32;
                    let dy = -(y - d.center.y as f32);
                    let (sn, cs) = (if self.brush.c3_tip_math { -d.angle } else { d.angle }).sin_cos();
                    let u = (dx * cs + dy * sn) * if d.flip_x { -1.0 } else { 1.0 };
                    let v = (-dx * sn + dy * cs) * if d.flip_y { -1.0 } else { 1.0 };
                    x = u + d.radius;
                    y = (if self.brush.c3_tip_math { -v } else { v }) / d.roundness.max(0.001) + d.radius;
                }
            }
        }
        let scale = self.brush.texture.scale.max(0.01);
        // Bilinear, as the reference application samples its pattern (a nearest-neighbour read
        // makes a scaled texture blocky and aliased).
        let (u, v) = (x / scale - 0.5, y / scale - 0.5);
        let (x0, y0) = (u.floor(), v.floor());
        p.sample_bilinear_wrap(x0, y0, u - x0, v - y0)
    }

    /// Bounding rectangle of a dab (rotated sampled tips need the corner reach).
    pub fn dab_rect(&self, d: &Dab, dual: bool) -> Rect {
        if dual && let Some(ctx) = &self.secondary {
            return ctx.dab_rect(d, false);
        }
        let sampled = if dual { self.dual_tip.is_some() } else { self.tip.is_some() };
        let hardness = if dual {
            if self.brush.dual_brush.hardness_dynamic.is_active() { d.hardness } else { self.brush.dual_brush.hardness }
        } else if self.brush.hardness_dynamic.is_active() || self.brush.pigment.is_some() || self.brush.contour.is_some() {
            d.hardness
        } else {
            self.brush.hardness
        };
        // A computed soft tip paints past its nominal radius (see `tip_shape`).
        let aliased = self.brush.aliased && !dual;
        let reach = if sampled || self.brush.triangle {
            d.radius * std::f32::consts::SQRT_2
        } else if aliased || self.brush.c3_tip_math {
            d.radius + 1.0
        } else {
            crate::tip_reach(d.radius, hardness).max(d.radius)
        };
        rect_around(d, reach)
    }

    /// Rasterise a dab over `rect` into `out` (tip shape × noise × per-tip texture × wet edges × flow).
    /// Wet Edges shapes this one dab; a stroke applies it to its accumulated coverage instead.
    pub fn rasterize(&self, d: &Dab, dual: bool, rect: Rect, out: &mut Vec<f32>) {
        self.rasterize_into(d, dual, rect, out, None);
    }

    /// Hands the dab to the injected stamper (the GPU), with `out` and `depth` sized for `rect`.
    /// `true` when it stamped the dab; `false` — no stamper, or a dab it doesn't draw — leaves the
    /// CPU path to do it, exactly as without a stamper.
    fn stamp_with_stamper(&self, d: &Dab, rect: Rect, out: &mut Vec<f32>, mut depth: Option<&mut Vec<f32>>) -> bool {
        let (Some(stamper), Some(job)) = (self.stamper.stamper(), self.stamp_job(d, depth.is_some())) else {
            return false;
        };
        let (w, h) = (rect.width() as usize, rect.height() as usize);
        out.clear();
        out.resize(w * h, 0.0);
        if let Some(dp) = depth.as_deref_mut() {
            dp.clear();
            dp.resize(w * h, 0.0);
        }
        stamper.stamp(&job, rect, out, depth.as_deref_mut())
    }

    /// [`rasterize`](Self::rasterize); with `depth`, Wet Edges is left out of `out` and each
    /// painted pixel's depth inside the dab (`1 − rn`) goes to `depth`, for the stroke to apply it.
    pub fn rasterize_into(&self, d: &Dab, dual: bool, rect: Rect, out: &mut Vec<f32>, mut depth: Option<&mut Vec<f32>>) {
        if dual && let Some(ctx) = &self.secondary {
            ctx.rasterize_into(d, false, rect, out, depth);
            return;
        }
        // An injected stamper (the GPU) takes the dab when it handles it; otherwise, and for
        // every dab it refuses, the CPU path below runs unchanged.
        if self.stamp_with_stamper(d, rect, out, depth.as_deref_mut()) {
            return;
        }
        let (w, h) = (rect.width() as usize, rect.height() as usize);
        out.clear();
        out.resize(w * h, 0.0);
        if let Some(dp) = depth.as_deref_mut() {
            dp.clear();
            dp.resize(w * h, 0.0);
        }
        let b = &self.brush;
        let hardness = if dual {
            if b.dual_brush.hardness_dynamic.is_active() { d.hardness } else { b.dual_brush.hardness }
        } else if b.hardness_dynamic.is_active() || b.pigment.is_some() || b.contour.is_some() {
            d.hardness
        } else {
            b.hardness
        };
        let mips = if dual { self.dual_tip.as_deref() } else { self.tip.as_deref() };
        let aliased = b.aliased && !dual;
        let wet_settings = if dual {
            b.dual_brush.wet_edges.then_some(b.dual_brush.wet_edge.as_ref()).flatten()
        } else {
            b.wet_edges.then_some(b.wet_edge.as_ref()).flatten()
        };
        let single_wet = wet_settings.filter(|w| !b.erase && w.outline == WetOutline::Single);
        let wet = b.wet_edges && b.wet_edge.is_none() && !dual && depth.is_none();
        let grain = if dual { b.dual_brush.grain.as_ref() } else { b.noise.then_some(b.grain.as_ref()).flatten() };
        let noise = b.noise && b.grain.is_none() && !dual;
        let tex_tip = !dual && b.texture.enabled && b.texture.each_tip && self.texture.is_some();
        let (cx, cy) = if aliased { grid_center(d.center.x, d.center.y, 2.0 * d.radius) } else { (d.center.x as f32, d.center.y as f32) };
        let (sn, cs) = (if b.c3_tip_math { -d.angle } else { d.angle }).sin_cos();
        let (fx, fy) = (if d.flip_x { -1.0 } else { 1.0 }, if d.flip_y { -1.0 } else { 1.0 });
        // Brush Projection: stretch the sampling coordinates along the tilt direction, which
        // foreshortens the tip there by `proj_scale`.
        let proj = (!dual && d.proj_scale < 0.999).then(|| {
            let (ps, pc) = d.proj_angle.sin_cos();
            (pc, ps, 1.0 / d.proj_scale.max(0.05) - 1.0)
        });
        let r = d.radius;
        let ro = if b.c3_tip_math { d.roundness.max(0.01) } else { d.roundness.max(0.5 / r).min(1.0) };
        let rm = (r * ro).max(0.5);
        let reach = if b.c3_tip_math {
            rm + 1.0
        } else if aliased || mips.is_some() {
            rm
        } else {
            crate::tip_reach(rm, hardness).max(rm)
        };
        let reach2 = (reach + 0.5) * (reach + 0.5);
        // Sampled-tip mapping.
        let (level, inv_scale, tw, th) = match mips {
            Some(m) => {
                let (w0, h0, _) = &m.levels[0];
                let big = *w0.max(h0) as f32;
                (m.level_for(2.0 * r * ro.max(0.25)), big / (2.0 * r), *w0 as f32, *h0 as f32)
            }
            None => (0, 1.0, 1.0, 1.0),
        };
        // Keep local Y up for bitmap sampling; C3 angles turn clockwise on the screen.
        let to_tip = |dx: f32, dy: f32| {
            let (mut ux, mut uy) = (dx, -dy);
            if let Some((ax, ay, k)) = proj {
                let t = (ux * ax + uy * ay) * k;
                ux += t * ax;
                uy += t * ay;
            }
            ((ux * cs + uy * sn) * fx, (-ux * sn + uy * cs) * fy)
        };
        // A tip a few pixels across takes each pixel's covered area (a 4×4 grid inside it):
        // sampling only the pixel centre made a tiny dab's total coverage swing with its sub-pixel
        // position, and thin lines beaded.
        let small = mips.is_none() && !aliased && if b.c3_tip_math { r * 2.0 < 5.0 } else { rm < SMALL_TIP_RADIUS };
        let small_reach2 = (reach + 0.75) * (reach + 0.75);
        // Keep ordinary computed tips separate from the feature loop, so their dense rows
        // can be vectorised without the per-pixel sampled-mask and stroke-depth branches.
        if !(b.c3_tip_math
            || mips.is_some()
            || b.triangle
            || small
            || (aliased && b.square)
            || noise
            || grain.is_some()
            || tex_tip
            || wet
            || single_wet.is_some())
            && depth.is_none()
            && proj.is_none()
            && d.angle == 0.0
            && d.radius.is_finite()
            && d.roundness.is_finite()
            && rm.is_finite()
            && cx.is_finite()
            && cy.is_finite()
            && hardness.is_finite()
            && d.alpha.is_finite()
            && w > 0
        {
            rasterize_round(d, rect, out, hardness, aliased);
            return;
        }
        for yy in 0..h {
            let y = rect.y0 + yy as i32;
            let dy = y as f32 + 0.5 - cy;
            for xx in 0..w {
                let x = rect.x0 + xx as i32;
                let dx = x as f32 + 0.5 - cx;
                let (u, v) = to_tip(dx, dy);
                let (mut val, rn) = match mips {
                    _ if b.triangle => {
                        let value = if small {
                            let mut sum = 0.0;
                            for sy in SUBPIXEL {
                                for sx in SUBPIXEL {
                                    let (su, sv) = to_tip(dx + sx, dy + sy);
                                    sum += triangle_mask(su, sv, r, ro, hardness, true);
                                }
                            }
                            sum / (SUBPIXEL.len() * SUBPIXEL.len()) as f32
                        } else {
                            triangle_mask(u, v, r, ro, hardness, false)
                        };
                        (if aliased { if value >= 0.5 { 1.0 } else { 0.0 } } else { value }, 1.0 - value)
                    }
                    // Block: the whole-pixel square, hard (the Eraser's Block mode).
                    _ if aliased && b.square => {
                        if dx.abs() >= r || dy.abs() >= r {
                            continue;
                        }
                        (1.0, 0.0)
                    }
                    None if small => {
                        let d2 = (u * ro).powi(2) + v * v;
                        if d2 >= small_reach2 {
                            continue;
                        }
                        let mut sum = 0.0;
                        for sy in SUBPIXEL {
                            for sx in SUBPIXEL {
                                let (su, sv) = to_tip(dx + sx, dy + sy);
                                let sd = ((su * ro).powi(2) + sv * sv).sqrt();
                                // Each sample takes the tip's own edge, so the covered area
                                // changes smoothly with the dab's position.
                                sum += if b.c3_tip_math { c3_tip_falloff(sd / ro, r, hardness, true) } else { tip_falloff(sd, rm, hardness) };
                            }
                        }
                        (sum / (SUBPIXEL.len() * SUBPIXEL.len()) as f32, d2.sqrt() / rm)
                    }
                    None => {
                        let d2 = (u * ro).powi(2) + v * v;
                        if d2 >= reach2 {
                            continue;
                        }
                        let dd = d2.sqrt();
                        let val = tip_coverage(b.c3_tip_math, aliased, dd, rm, ro, r, hardness);
                        (val, dd / rm)
                    }
                    Some(m) => {
                        let (px, py) = (u * inv_scale + tw / 2.0, -(v / ro) * inv_scale + th / 2.0);
                        if px < -1.0 || py < -1.0 || px > tw + 1.0 || py > th + 1.0 {
                            continue;
                        }
                        let s = if b.c3_tip_math { c3_sampled_mask(m, px / tw, py / th, hardness) } else { m.sample_clamped(level, px / tw, py / th) };
                        let val = if aliased { if s >= 0.5 { 1.0 } else { 0.0 } } else { s };
                        (val, 1.0 - s)
                    }
                };
                if val <= 0.0 {
                    continue;
                }
                if noise && val < 1.0 {
                    let n = hash2(x, y, NOISE_SALT);
                    let hard = if n < val { 1.0 } else { 0.0 };
                    val += (hard - val) * 0.7;
                }
                if let Some(g) = grain {
                    val = g.alpha(val, d.grain_density, x, y);
                }
                let single_edge = single_wet.map(|ws| ws.dab_edge(val, rn, d.wet_edge));
                if let (Some(ws), Some(edge)) = (single_wet, single_edge) {
                    val = ws.alpha(val, edge, d.wet_edge);
                }
                if tex_tip {
                    val = b.texture.combine(val, self.texture_at_point(x as f32 + 0.5, y as f32 + 0.5, Some(d)), d.depth);
                }
                if wet {
                    val *= wet_factor(rn);
                }
                if let Some(slot) = depth.as_deref_mut().and_then(|dp| dp.get_mut(yy * w + xx)) {
                    *slot = single_edge.unwrap_or_else(|| (1.0 - rn).clamp(0.0, 1.0));
                }
                out[yy * w + xx] = val * d.alpha;
            }
        }
    }

    /// One dab's dense footprint (for the sequential retouch tools).
    pub fn footprint(&self, d: &Dab, index: usize) -> Footprint {
        let rect = self.dab_rect(d, false);
        let mut cov = Vec::new();
        self.rasterize(d, false, rect, &mut cov);
        Footprint { dab: *d, index, rect, cov }
    }

    /// Stroke-level masks applied to accumulated coverage `c` at a pixel.
    #[inline]
    pub fn stroke_mask(&self, c: f32, dual: Option<f32>, x: i32, y: i32) -> f32 {
        self.stroke_mask_wet(c, None, dual, x, y)
    }

    /// [`stroke_mask`](Self::stroke_mask) with Wet Edges from the stroke's depth at the pixel
    /// (the deepest any dab reaches it, see [`CoverageMap::depth`]).
    #[inline]
    pub fn stroke_mask_wet(&self, c: f32, depth: Option<f32>, dual: Option<f32>, x: i32, y: i32) -> f32 {
        let b = &self.brush;
        if let Some(ctx) = &self.secondary {
            let p = c * b.opacity * b.dual_brush.primary_strength;
            let mut secondary = dual.unwrap_or(0.0) * b.dual_brush.secondary_strength;
            if ctx.texture.is_some() && !ctx.brush.texture.each_tip {
                secondary = ctx.brush.texture.combine(secondary, ctx.texture_at(x, y), ctx.brush.texture.depth.clamp(0.0, 1.0));
            }
            let mut result = dual_blend(b.dual_brush.mode, p, secondary);
            if self.texture.is_some() && !b.texture.each_tip {
                result = b.texture.combine(result, self.texture_at(x, y), b.texture.depth.clamp(0.0, 1.0));
            }
            return result;
        }
        let c = c * b.dual_brush.primary_strength.clamp(0.0, 1.0);
        let mut m = match depth {
            Some(dp) if b.wet_edge.is_none() => c * wet_factor(1.0 - dp),
            Some(_) => c,
            None => c,
        };
        if let Some(dv) = dual {
            m = mask_combine(b.dual_brush.mode, m, dv, (b.dual_brush.opacity * b.dual_brush.secondary_strength).clamp(0.0, 1.0));
        }
        if self.texture.is_some() && !b.texture.each_tip {
            m = b.texture.combine(m, self.texture_at(x, y), b.texture.depth.clamp(0.0, 1.0));
        }
        m
    }
}

// ---------------------------------------------------------------------------
// Sparse stroke buffer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct CovTile {
    cov: Vec<f32>,
    /// Wet Edges: the deepest any dab reaches each pixel (`1 − rn` of the nearest dab), empty
    /// without Wet Edges. The stroke's interior is deep, its rim shallow, whatever the dabs' overlap.
    depth: Vec<f32>,
    /// Premultiplied native colour channels (`nc` per pixel), empty without per-dab colour.
    col: Vec<f32>,
    /// Per-pixel resolved wet parameters and single-dab edge weight.
    material: Vec<[f32; 4]>,
    pigment: Vec<[f32; 4]>,
    pigment_settings: Vec<crate::pigment_material::ResolvedPigment>,
    pigment_motion: Vec<[f32; 2]>,
    saturation: Vec<[f32; 2]>,
    /// Per-pixel strength of the mixing pipeline (V1/V2/V3) where a dab touched, empty when the
    /// lane has no transport. Kept apart from the blur strength so the two pipelines stay
    /// independent instead of one overwriting the other's result.
    mix_weight: Vec<f32>,
    /// Per-pixel strength of the blur pipeline, empty when the lane does not blur.
    blur_weight: Vec<f32>,
}

/// A sparse tiled coverage (+ optional colour) buffer.
#[derive(Clone, Debug, Default)]
pub struct CoverageMap {
    tiles: HashMap<(i32, i32), CovTile>,
    nc: usize,
    bounds: Rect,
    dirty: HashSet<(i32, i32)>,
}

impl CoverageMap {
    pub fn new(color_channels: usize) -> Self {
        Self { nc: color_channels, ..Default::default() }
    }
    pub fn bounds(&self) -> Rect {
        self.bounds
    }
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> f32 {
        let (tx, ty) = (x.div_euclid(COV_TILE), y.div_euclid(COV_TILE));
        self.tiles.get(&(tx, ty)).map_or(0.0, |t| t.cov[((y - ty * COV_TILE) * COV_TILE + (x - tx * COV_TILE)) as usize])
    }

    /// Wet Edges depth at a pixel (see [`CovTile::depth`]); `None` when the stroke has none.
    #[inline]
    pub fn depth(&self, x: i32, y: i32) -> Option<f32> {
        let (tx, ty) = (x.div_euclid(COV_TILE), y.div_euclid(COV_TILE));
        self.tiles.get(&(tx, ty)).and_then(|t| t.depth.get(((y - ty * COV_TILE) * COV_TILE + (x - tx * COV_TILE)) as usize).copied())
    }

    fn material_at(&self, x: i32, y: i32) -> (ResolvedWet, f32) {
        let (tx, ty) = (x.div_euclid(COV_TILE), y.div_euclid(COV_TILE));
        let i = ((y - ty * COV_TILE) * COV_TILE + (x - tx * COV_TILE)) as usize;
        let m = self.tiles.get(&(tx, ty)).and_then(|t| t.material.get(i)).copied().unwrap_or([0.0; 4]);
        (ResolvedWet { strength: m[0], width: m[1], stamp_width: m[2] }, m[3])
    }

    fn pigment_at(&self, x: i32, y: i32) -> [f32; 4] {
        let (tx, ty) = (x.div_euclid(COV_TILE), y.div_euclid(COV_TILE));
        let i = ((y - ty * COV_TILE) * COV_TILE + x - tx * COV_TILE) as usize;
        self.tiles.get(&(tx, ty)).and_then(|t| t.pigment.get(i)).copied().unwrap_or([0.0; 4])
    }
    #[cfg(test)]
    fn pigment_sample(&self, x: f32, y: f32) -> [f32; 4] {
        let (ix, iy) = (x.floor() as i32, y.floor() as i32);
        let (fx, fy) = (x - x.floor(), y - y.floor());
        let (a, b, c, d) = (self.pigment_at(ix, iy), self.pigment_at(ix + 1, iy), self.pigment_at(ix, iy + 1), self.pigment_at(ix + 1, iy + 1));
        std::array::from_fn(|i| (a[i] + (b[i] - a[i]) * fx) * (1.0 - fy) + (c[i] + (d[i] - c[i]) * fx) * fy)
    }
    /// Record how strongly each transport pipeline touched a pixel of `rect`. Only the strength is
    /// stored — the pipelines keep their own colour surfaces — so mixing and blur never read each
    /// other's output and the composite can weigh both independently.
    fn accumulate_transport(&mut self, rect: Rect, vals: &[f32], mix: f32, blur: f32, alpha: f32) {
        if (mix <= 0.0 && blur <= 0.0) || rect.is_empty() {
            return;
        }
        let width = rect.width() as usize;
        let len = (COV_TILE * COV_TILE) as usize;
        let inv_alpha = 1.0 / alpha.max(1e-6);
        let (t0x, t0y) = (rect.x0.div_euclid(COV_TILE), rect.y0.div_euclid(COV_TILE));
        let (t1x, t1y) = ((rect.x1 - 1).div_euclid(COV_TILE), (rect.y1 - 1).div_euclid(COV_TILE));
        // Tile by tile, one map lookup a tile: a 900 px dab covers ~250 tiles, not 810k pixels, and
        // the pixels of a tile are contiguous in both buffers.
        for ty in t0y..=t1y {
            for tx in t0x..=t1x {
                let tr = Rect::new(tx * COV_TILE, ty * COV_TILE, (tx + 1) * COV_TILE, (ty + 1) * COV_TILE).intersect(&rect);
                if tr.is_empty() {
                    continue;
                }
                let any = (tr.y0..tr.y1).any(|y| {
                    let row = (y - rect.y0) as usize * width;
                    vals[row + (tr.x0 - rect.x0) as usize..row + (tr.x1 - rect.x0) as usize].iter().any(|&v| v > 0.0)
                });
                let Some(tile) = self.tiles.get_mut(&(tx, ty)).filter(|_| any) else { continue };
                if mix > 0.0 && tile.mix_weight.is_empty() {
                    tile.mix_weight = vec![0.0; len];
                }
                if blur > 0.0 && tile.blur_weight.is_empty() {
                    tile.blur_weight = vec![0.0; len];
                }
                for y in tr.y0..tr.y1 {
                    let row = (y - rect.y0) as usize * width;
                    let trow = ((y - ty * COV_TILE) * COV_TILE) as usize;
                    for x in tr.x0..tr.x1 {
                        let v = vals[row + (x - rect.x0) as usize] * inv_alpha;
                        if v <= 0.0 {
                            continue;
                        }
                        let i = trow + (x - tx * COV_TILE) as usize;
                        // The drag follows the tip's real shape: its strength is the dab's own
                        // coverage (the same alpha the tip rasterises), never a flat ratio.
                        if mix > 0.0 {
                            tile.mix_weight[i] = tile.mix_weight[i].max((mix * v).min(1.0));
                        }
                        if blur > 0.0 {
                            tile.blur_weight[i] = tile.blur_weight[i].max(blur * v);
                        }
                    }
                }
            }
        }
    }

    /// (mixing, blur) strength recorded at this pixel; zero without a transport.
    #[inline]
    fn transport_weights(&self, x: i32, y: i32) -> [f32; 2] {
        let (tx, ty) = (x.div_euclid(COV_TILE), y.div_euclid(COV_TILE));
        let i = ((y - ty * COV_TILE) * COV_TILE + x - tx * COV_TILE) as usize;
        self.tiles
            .get(&(tx, ty))
            .map_or([0.0; 2], |t| [t.mix_weight.get(i).copied().unwrap_or(0.0), t.blur_weight.get(i).copied().unwrap_or(0.0)])
    }

    fn accumulate_pigment(&mut self, ctx: &BrushContext, d: &Dab, rect: Rect, vals: &[f32], motion: [f32; 2]) {
        if d.pigment.is_inert() || ctx.brush.erase || d.alpha <= 0.0 || rect.is_empty() {
            return;
        }
        let width = rect.width() as usize;
        let mask = |ix: i32, iy: i32| {
            if ix < rect.x0 || ix >= rect.x1 || iy < rect.y0 || iy >= rect.y1 {
                return 0.0;
            }
            vals.get((iy - rect.y0) as usize * width + (ix - rect.x0) as usize).copied().unwrap_or(0.0) / d.alpha.max(1e-6)
        };
        // Emboss samples the hard tip at fractional positions, before grain, wet edges or flow.
        // The rotation is a dab constant, so it is computed once rather than per sample.
        let (sn, cs) = (if ctx.brush.c3_tip_math { -d.angle } else { d.angle }).sin_cos();
        let hard_tip = |x: f32, y: f32| {
            let dx = x + 0.5 - d.center.x as f32;
            let dy = -(y + 0.5 - d.center.y as f32);
            let (u, v) = (dx * cs + dy * sn, -dx * sn + dy * cs);
            let roundness = d.roundness.max(0.01);
            if ctx.brush.triangle {
                return triangle_mask(u, v, d.radius.max(0.1), roundness, 1.0, false);
            }
            if let Some(m) = ctx.tip.as_ref() {
                let (w, h, _) = &m.levels[0];
                let big = *w.max(h) as f32;
                let u = u * if d.flip_x { -1.0 } else { 1.0 };
                let v = v * if d.flip_y { -1.0 } else { 1.0 };
                m.sample_clamped(0, u * big / (d.radius * 2.0).max(0.2) / *w as f32 + 0.5, -v / roundness * big / (d.radius * 2.0).max(0.2) / *h as f32 + 0.5)
            } else {
                dab_coverage(u.hypot(v / roundness), d.radius.max(0.1), 1.0)
            }
        };
        let step = (d.radius * 0.075).clamp(0.65, 3.0);
        let dirs = [
            [1.0, 0.0],
            [-1.0, 0.0],
            [0.0, 1.0],
            [0.0, -1.0],
            [std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2],
            [-std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2],
            [std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2],
            [-std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2],
        ];
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let (tx, ty) = (x.div_euclid(COV_TILE), y.div_euclid(COV_TILE));
                let Some(tile) = self.tiles.get_mut(&(tx, ty)) else { continue };
                let i = ((y - ty * COV_TILE) * COV_TILE + x - tx * COV_TILE) as usize;
                let coverage = mask(x, y);
                if coverage <= 0.0 {
                    continue;
                }
                // The emboss only reaches the output through `deposit`'s visual-thickness term, so a
                // zero visual thickness makes the sixteen hard-tip samples provably invisible: skip them.
                let emboss = if d.pigment.visual_thickness > 0.0 {
                    let near = dirs.map(|o| hard_tip(x as f32 + o[0] * step, y as f32 + o[1] * step));
                    let far = dirs.map(|o| hard_tip(x as f32 + o[0] * step * 2.15, y as f32 + o[1] * step * 2.15));
                    let dx = x as f32 + 0.5 - d.center.x as f32;
                    let dy = -(y as f32 + 0.5 - d.center.y as f32);
                    let aspect = ctx.tip.as_ref().map_or([1.0, 1.0], |m| {
                        let (w, h, _) = &m.levels[0];
                        let big = *w.max(h) as f32;
                        [*w as f32 / big, *h as f32 / big]
                    });
                    let local = [
                        (dx * cs + dy * sn) / (d.radius * aspect[0]).max(0.1),
                        (if ctx.brush.c3_tip_math { dx * sn - dy * cs } else { -dx * sn + dy * cs }) / (d.radius * d.roundness * aspect[1]).max(0.1),
                    ];
                    let pattern = if ctx.texture.is_some() && ctx.brush.texture.each_tip {
                        ctx.texture_at_point(x as f32 + 0.5, y as f32 + 0.5, Some(d))
                    } else if ctx.tip.is_some() {
                        hard_tip(x as f32, y as f32)
                    } else {
                        1.0
                    };
                    crate::pigment_material::emboss_height(coverage, local, pattern, near, far)
                } else {
                    0.0
                };
                let deposit = crate::pigment_material::deposit(d.pigment, coverage, d.alpha, d.opacity * ctx.brush.opacity, emboss);
                if tile.pigment.is_empty() {
                    let len = (COV_TILE * COV_TILE) as usize;
                    tile.pigment = vec![[0.0; 4]; len];
                    tile.pigment_settings = vec![Default::default(); len];
                }
                tile.pigment[i] = crate::pigment_material::over_material(tile.pigment[i], deposit);
                tile.pigment_settings[i] = d.pigment;
                // `shade` only reads the motion through `liquidity`; a dry material stores nothing,
                // which spares one write per pixel of every dab.
                if d.pigment.liquidity > 0.0 {
                    if tile.pigment_motion.is_empty() {
                        tile.pigment_motion = vec![[0.0; 2]; (COV_TILE * COV_TILE) as usize];
                    }
                    tile.pigment_motion[i] = motion;
                }
            }
        }
    }

    /// Accumulate dab values over `rect`: flow builds up towards the dab's opacity ceiling
    /// (`c ← c + v·(ceil − c)`). `color` = native colour for per-dab colour. `depth` (Wet Edges, one
    /// value per pixel of `rect`) keeps the deepest any dab reaches each pixel.
    pub fn accumulate(&mut self, rect: Rect, vals: &[f32], ceil: f32, color: Option<&[f32]>, depth: Option<&[f32]>) {
        self.accumulate_wet(rect, vals, ceil, color.map(|c| (c, false)), depth, None, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn accumulate_wet(
        &mut self,
        rect: Rect,
        vals: &[f32],
        ceil: f32,
        color: Option<(&[f32], bool)>,
        depth: Option<&[f32]>,
        wet: Option<(ResolvedWet, Option<f32>)>,
        saturation: Option<(f32, f32, f32, PixelFormat)>,
    ) {
        if rect.is_empty() {
            return;
        }
        self.bounds = self.bounds.union(&rect);
        let w = rect.width() as usize;
        let nc = self.nc;
        let (t0x, t0y) = (rect.x0.div_euclid(COV_TILE), rect.y0.div_euclid(COV_TILE));
        let (t1x, t1y) = ((rect.x1 - 1).div_euclid(COV_TILE), (rect.y1 - 1).div_euclid(COV_TILE));
        let want_col = color.is_some() && nc > 0;
        for ty in t0y..=t1y {
            for tx in t0x..=t1x {
                let tr = Rect::new(tx * COV_TILE, ty * COV_TILE, (tx + 1) * COV_TILE, (ty + 1) * COV_TILE).intersect(&rect);
                if tr.is_empty() {
                    continue;
                }
                // Skip tiles this dab doesn't touch.
                let any = (tr.y0..tr.y1).any(|y| {
                    let row = (y - rect.y0) as usize * w;
                    vals[row + (tr.x0 - rect.x0) as usize..row + (tr.x1 - rect.x0) as usize].iter().any(|&v| v > 0.0)
                });
                if !any {
                    continue;
                }
                let tile = self.tiles.entry((tx, ty)).or_insert_with(|| CovTile {
                    cov: vec![0.0; (COV_TILE * COV_TILE) as usize],
                    depth: Vec::new(),
                    material: Vec::new(),
                    pigment: Vec::new(),
                    pigment_settings: Vec::new(),
                    pigment_motion: Vec::new(),
                    saturation: Vec::new(),
                    mix_weight: Vec::new(),
                    blur_weight: Vec::new(),
                    col: if want_col { vec![0.0; (COV_TILE * COV_TILE) as usize * nc] } else { Vec::new() },
                });
                if want_col && tile.col.is_empty() {
                    tile.col = vec![0.0; (COV_TILE * COV_TILE) as usize * nc];
                }
                if depth.is_some() && tile.depth.is_empty() {
                    tile.depth = vec![0.0; (COV_TILE * COV_TILE) as usize];
                }
                if wet.is_some() && tile.material.is_empty() {
                    tile.material = vec![[0.0; 4]; (COV_TILE * COV_TILE) as usize];
                }
                if saturation.is_some() && tile.saturation.is_empty() {
                    tile.saturation = vec![[0.0; 2]; (COV_TILE * COV_TILE) as usize];
                }
                self.dirty.insert((tx, ty));
                for y in tr.y0..tr.y1 {
                    let row = (y - rect.y0) as usize * w;
                    let trow = ((y - ty * COV_TILE) * COV_TILE) as usize;
                    for x in tr.x0..tr.x1 {
                        let v = vals[row + (x - rect.x0) as usize];
                        if v <= 0.0 {
                            continue;
                        }
                        let ti = trow + (x - tx * COV_TILE) as usize;
                        if let (Some(dv), Some(slot)) = (depth.and_then(|dp| dp.get(row + (x - rect.x0) as usize)), tile.depth.get_mut(ti)) {
                            *slot = slot.max(*dv);
                        }
                        let c = tile.cov[ti];
                        let nv = if ceil > c { c + v * (ceil - c) } else { c };
                        if let (Some((wet, single)), Some(slot)) = (wet, tile.material.get_mut(ti)) {
                            let edge =
                                single.map_or(0.0, |strength| strength * depth.and_then(|v| v.get(row + (x - rect.x0) as usize)).copied().unwrap_or(0.0));
                            let incoming = [wet.strength, wet.width, wet.stamp_width, edge];
                            let weight = if nv > 0.0 { (nv - c) / nv } else { 0.0 };
                            for (dst, src) in slot.iter_mut().zip(incoming) {
                                *dst += (src - *dst) * weight;
                            }
                        }
                        if let (true, Some((colors, per_pixel))) = (want_col, color) {
                            let offset = if per_pixel { (row + (x - rect.x0) as usize) * nc } else { 0 };
                            let col = &colors[offset..offset + nc];
                            let incoming = nv.min(ceil * v);
                            let retained = (nv - incoming).max(0.0);
                            let p = &mut tile.col[ti * nc..(ti + 1) * nc];
                            for i in 0..nc {
                                p[i] = col[i] * incoming + if c > 0.0001 { p[i] / c * retained } else { 0.0 };
                            }
                            if let Some((record, preserve, opacity, fmt)) = saturation {
                                let mut native = [0.0; 8];
                                native[..nc].copy_from_slice(p);
                                if fmt.alpha {
                                    native[nc] = 1.0;
                                }
                                let mut rgba = to_rgba(&fmt, &native[..fmt.channels()]);
                                for channel in &mut rgba[..3] {
                                    *channel *= opacity;
                                }
                                let memory = &mut tile.saturation[ti];
                                let previous = if memory[1] > 0.0001 { memory[0] / memory[1] } else { 0.0 };
                                let rgb = crate::attributes::preserve_saturation([rgba[0], rgba[1], rgba[2]], previous, preserve);
                                rgba[..3].copy_from_slice(&rgb.map(|v| v / opacity.max(0.000001)));
                                from_rgba_into(&fmt, rgba, &mut native);
                                p.copy_from_slice(&native[..nc]);
                                native[..nc].copy_from_slice(col);
                                if fmt.alpha {
                                    native[nc] = 1.0;
                                }
                                let source = to_rgba(&fmt, &native[..fmt.channels()]);
                                let sat = crate::attributes::saturation([source[0], source[1], source[2]]);
                                let weight = (incoming * opacity * record).clamp(0.0, 1.0);
                                *memory = [memory[0] * (1.0 - weight) + sat * weight, memory[1] * (1.0 - weight) + weight];
                            }
                        }
                        tile.cov[ti] = nv;
                    }
                }
            }
        }
    }

    fn take_dirty(&mut self) -> Vec<(i32, i32)> {
        let mut v: Vec<_> = self.dirty.drain().collect();
        v.sort_unstable();
        v
    }

    /// The finished coverage over the tiles `keys`: `tail`'s copy of a tile where finishing the
    /// stroke changes it (see [`StrokeRenderer::tail_preview`]), else this map's own.
    fn finished_subset(&self, tail: Option<&Self>, keys: &HashSet<(i32, i32)>) -> Self {
        let tile = |k: &(i32, i32)| tail.and_then(|t| t.tiles.get(k)).or_else(|| self.tiles.get(k)).map(|t| (*k, t.clone()));
        Self { tiles: keys.iter().filter_map(tile).collect(), nc: self.nc, bounds: tail.map_or(self.bounds, |t| t.bounds), dirty: HashSet::new() }
    }

    /// Union two passes of the same brush stroke without applying opacity twice where they
    /// overlap. The stronger coverage wins, including its per-dab colour when present.
    fn union_max(&mut self, other: &Self) {
        self.bounds = self.bounds.union(&other.bounds);
        for (&key, source) in &other.tiles {
            let Some(target) = self.tiles.get_mut(&key) else {
                self.tiles.insert(key, source.clone());
                self.dirty.insert(key);
                continue;
            };
            if target.depth.is_empty() {
                target.depth.clone_from(&source.depth);
            } else {
                for (dst, src) in target.depth.iter_mut().zip(&source.depth) {
                    *dst = dst.max(*src);
                }
            }
            for (index, (dst, src)) in target.cov.iter_mut().zip(&source.cov).enumerate() {
                if *src > *dst {
                    *dst = *src;
                    if let Some(material) = source.material.get(index) {
                        if target.material.is_empty() {
                            target.material = vec![[0.0; 4]; source.cov.len()];
                        }
                        if let Some(slot) = target.material.get_mut(index) {
                            *slot = *material;
                        }
                    }
                    if let Some(value) = source.pigment.get(index) {
                        if target.pigment.is_empty() {
                            target.pigment = vec![[0.0; 4]; source.cov.len()];
                            target.pigment_settings = vec![Default::default(); source.cov.len()];
                            target.pigment_motion = vec![[0.0; 2]; source.cov.len()];
                        }
                        target.pigment[index] = *value;
                        if let (Some(settings), Some(value)) = (target.pigment_settings.get_mut(index), source.pigment_settings.get(index)) {
                            *settings = *value;
                        }
                        if let (Some(motion), Some(value)) = (target.pigment_motion.get_mut(index), source.pigment_motion.get(index)) {
                            *motion = *value;
                        }
                    }
                    if !source.mix_weight.is_empty() && target.mix_weight.is_empty() {
                        target.mix_weight = vec![0.0; source.cov.len()];
                    }
                    if !source.blur_weight.is_empty() && target.blur_weight.is_empty() {
                        target.blur_weight = vec![0.0; source.cov.len()];
                    }
                    for (slot, fresh) in [(target.mix_weight.get_mut(index), source.mix_weight.get(index)), (target.blur_weight.get_mut(index), source.blur_weight.get(index))] {
                        if let (Some(slot), Some(fresh)) = (slot, fresh) {
                            *slot = *fresh;
                        }
                    }
                    let start = index.saturating_mul(self.nc);
                    let end = start.saturating_add(self.nc);
                    if let (Some(dst_color), Some(src_color)) = (target.col.get_mut(start..end), source.col.get(start..end)) {
                        dst_color.copy_from_slice(src_color);
                    }
                }
            }
            self.dirty.insert(key);
        }
    }
}

// ---------------------------------------------------------------------------
// Stroke renderer
// ---------------------------------------------------------------------------

/// What [`StrokeRenderer::composite_union_live`] drew last: the coverage tiles of the passes'
/// smoothing tails (redrawn without them next time) and the merged bounds.
#[derive(Clone, Debug, Default)]
pub struct UnionLive {
    tail_keys: HashSet<(i32, i32)>,
    bounds: Rect,
}

impl UnionLive {
    /// The merged bounds of the finished passes, as last drawn.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// Each colour-transport lane owns its carrier and sparse working pixels.
#[derive(Clone, Debug)]
struct LaneTransport {
    /// The V1/V2/V3 transport's own copy of the layer. It never receives the brush's colour and
    /// never reads the blur pipeline, so what it drags is exactly the paint that was there.
    mix: Surface,
    /// The blur pipeline's own copy, allocated only for a lane that actually blurs.
    blur: Option<Surface>,
    mix_state: crate::transport::Transport,
    blur_state: crate::transport::Transport,
    /// The lane's working pixels, grown to cover the stroke as it moves: the surface is read only
    /// for the strips the buffer does not have yet, so a stroke of overlapping dabs reads each
    /// pixel of its area once instead of once per dab.
    region: crate::retouch::Region,
    /// The blur pipeline's working pixels (the pixels it reads differ from the mixing ones).
    blur_region: crate::retouch::Region,
    /// Pixels of `region` / `blur_region` changed since they were written back to their surfaces.
    changed: Rect,
    blur_changed: Rect,
    /// Dabs this frame queued for the transport backend, with the footprints the CPU falls back to
    /// if the backend refuses them.
    pending: Vec<Pending>,
    /// The area the queued dabs cover: a frame is handed to the backend in chunks that fit the
    /// working-pixel budget, so a long stroke never pins a whole document on the GPU at once.
    pending_area: Rect,
    out: Vec<f32>,
}

/// One dab waiting for the transport backend, with the dab the CPU falls back to if it refuses.
#[derive(Clone, Debug)]
struct Pending {
    dab: Dab,
    td: crate::stamp::TransportDab,
}
impl LaneTransport {
    fn new(brush: &BrushSettings, pre: &Surface) -> Option<Self> {
        let m = &brush.mix_settings;
        // Each lane owns one transport per pipeline: mixing (V1/V2/V3) and blur are separate
        // surfaces and separate states, so blurring can never eat the dragged colour and mixing can
        // never sample a blurred layer. Colour attributes stay with the paint pipeline.
        m.enabled.then(|| Self {
            mix: pre.clone(),
            blur: (m.blur_strength > 0.0).then(|| pre.clone()),
            mix_state: crate::transport::Transport::new(brush, alpha_index(&pre.format())),
            blur_state: crate::transport::Transport::new(brush, alpha_index(&pre.format())),
            region: crate::retouch::Region::empty(),
            blur_region: crate::retouch::Region::empty(),
            changed: Rect::EMPTY,
            blur_changed: Rect::EMPTY,
            pending: Vec::new(),
            pending_area: Rect::EMPTY,
            out: Vec::new(),
        })
    }
    fn dab(&mut self, ctx: &BrushContext, d: &Dab, rect: Rect, mask: &[f32], _color: Option<(&[f32], bool)>) {
        if rect.is_empty() || mask.is_empty() {
            return;
        }
        // Both pipelines read only their own dab's square plus where the previous carrier landed,
        // which is inside the neighbouring dabs' squares: a stroke's dabs sit a spacing step apart,
        // so the union of their rectangles is the whole area the working pixels ever need. A
        // generous halo here used to push a 900 px dab's union past the region budget, which
        // re-read the layer on every dab.
        let needed = rect.inflate(2);
        if d.mix_strength > 0.0 {
            if self.takes_backend(ctx) {
                // 64 MB of working pixels each way is the most one backend frame may pin on the
                // GPU, so the queued dabs are handed over in chunks that fit.
                let area = self.pending_area.union(&rect.inflate(2));
                if i64::from(area.width()) * i64::from(area.height()) * 16 > 64 * 1024 * 1024 {
                    self.flush_pending(ctx);
                }
                // The backend carries the coverage itself, so the CPU footprint stays empty here: a
                // copy of the dab's mask is 13 MB at 900 px, and the backend already needs one.
                let plan = Footprint { dab: *d, index: 0, rect, cov: Vec::new() };
                let (keep, scale, has_previous) = self.mix_state.feedback_plan(&plan);
                let m = &ctx.brush.mix_settings;
                self.pending.push(Pending {
                    dab: *d,
                    td: crate::stamp::TransportDab {
                        rect,
                        center: [d.center.x, d.center.y],
                        radius: d.radius,
                        mix_strength: d.mix_strength,
                        concentration: m.concentration,
                        output_strength: m.output_strength,
                        keep,
                        scale,
                        has_previous,
                        coverage: mask.to_vec(),
                    },
                });
                self.pending_area = self.pending_area.union(&rect.inflate(2));
            } else {
                // A CPU dab has to see what the dabs before it left: the backend's frame runs first.
                self.flush_pending(ctx);
                self.fit(ctx, &needed);
                self.region.grow_to(&self.mix, needed);
                let footprint = Footprint { dab: *d, index: 0, rect, cov: mask.to_vec() };
                self.mix_state.smudge_dab(&mut self.region, &footprint);
            }
            self.changed = self.changed.union(&rect);
        }
        if d.blur_strength > 0.0 && self.blur.is_some() {
            self.fit(ctx, &needed);
            if let Some(blur) = self.blur.as_ref() {
                self.blur_region.grow_to(blur, needed);
                let footprint = Footprint { dab: *d, index: 0, rect, cov: mask.to_vec() };
                self.blur_state.blur_dab(&mut self.blur_region, &footprint);
                self.blur_changed = self.blur_changed.union(&rect);
            }
        }
    }

    /// Whether the transport backend can take this lane's dabs at all: V1 only (the backend
    /// reproduces the reference's carrier feedback), no pigment branch, and pixels wide enough.
    fn takes_backend(&self, ctx: &BrushContext) -> bool {
        if ctx.transport().is_none() {
            return false;
        }
        let m = &ctx.brush.mix_settings;
        if !(m.smudge_enabled && m.algorithm == crate::transport::MixAlgorithm::V1) || m.blur_color_target.is_some() {
            return false;
        }
        // `over_carried`'s pigment branch and the premultiplied-alpha maths are CPU-only for now.
        if ctx.brush.pigment_mix_strength > 0.0 || self.mix.format().channels() != 4 || alpha_index(&self.mix.format()) != Some(3) {
            return false;
        }
        true
    }

    /// Run this frame's queued dabs on the transport backend (or, when it refuses, on the CPU),
    /// rewriting `region` in place. Nothing is written back to the surfaces yet.
    fn flush_pending(&mut self, ctx: &BrushContext) {
        if self.pending.is_empty() {
            return;
        }
        self.pending_area = Rect::EMPTY;
        let Some(backend) = ctx.transport() else {
            for p in std::mem::take(&mut self.pending) {
                let mut td = p.td;
                let cov = std::mem::take(&mut td.coverage);
                let footprint = Footprint { dab: p.dab, index: 0, rect: td.rect, cov };
                self.fit(ctx, &td.rect.inflate(2));
                self.region.grow_to(&self.mix, td.rect.inflate(2));
                self.mix_state.feedback_dab_planned(&mut self.region, &footprint, td.keep, td.scale, td.has_previous);
            }
            return;
        };
        let pending = std::mem::take(&mut self.pending);
        self.pending_area = Rect::EMPTY;
        // The backend reads the whole frame's area at once: the working pixels must hold every dab's
        // square (and the previous carrier's, which sits inside them).
        let mut area = Rect::EMPTY;
        for p in &pending {
            area = area.union(&p.td.rect.inflate(2));
        }
        self.fit(ctx, &area);
        self.region.grow_to(&self.mix, area);
        let region = self.region.rect;
        let channels = self.region.ch;
        let alpha = alpha_index(&self.mix.format());
        let dabs: Vec<crate::stamp::TransportDab> = pending.iter().map(|p| p.td.clone()).collect();
        let mut pixels = std::mem::take(&mut self.region.data);
        let done = {
            let mut frame = crate::stamp::TransportFrame { region, channels, alpha, pixels: &mut pixels, dabs: &dabs };
            backend.transport(&mut frame)
        };
        self.region.data = pixels;
        if !done {
            for p in pending {
                let mut td = p.td;
                let cov = std::mem::take(&mut td.coverage);
                let footprint = Footprint { dab: p.dab, index: 0, rect: td.rect, cov };
                self.mix_state.feedback_dab_planned(&mut self.region, &footprint, td.keep, td.scale, td.has_previous);
            }
        }
    }

    /// Drop the working pixels when a stroke's area outgrows the budget: the write-back has left
    /// the surfaces current, so re-reading the next dab's rectangle is exact, and a stroke across a
    /// huge document must not hold the whole of it twice at 4 bytes a channel.
    fn fit(&mut self, ctx: &BrushContext, needed: &Rect) {
        const MAX_PIXELS: i64 = 4_000_000;
        let grown = self.region.rect.union(needed);
        if i64::from(grown.width()) * i64::from(grown.height()) > MAX_PIXELS {
            self.flush(ctx);
            self.region = crate::retouch::Region::empty();
            self.blur_region = crate::retouch::Region::empty();
        }
    }

    /// Write this frame's changed pixels back into the lane's surfaces, once: the composite reads
    /// them, and the next frame must see the layer this frame left.
    fn flush(&mut self, ctx: &BrushContext) {
        self.flush_pending(ctx);
        if !self.changed.is_empty() {
            let r = self.changed;
            let region = &self.region;
            region.write_sub_into(&mut self.mix, r, &mut self.out);
            self.changed = Rect::EMPTY;
        }
        if !self.blur_changed.is_empty() {
            let r = self.blur_changed;
            let region = &self.blur_region;
            if let Some(blur) = self.blur.as_mut() {
                region.write_sub_into(blur, r, &mut self.out);
            }
            self.blur_changed = Rect::EMPTY;
        }
    }
}

/// Incremental stroke rendering: feed points, composite dirty tiles from the pre-stroke pixels.
#[derive(Clone, Debug)]
pub struct StrokeRenderer {
    pub ctx: BrushContext,
    generator: DabGenerator,
    /// ProCraft's stroke, when this app draws with its brush system (see
    /// [`crate::procraft::enabled`]): it turns the pointer into stamps, the generator steps aside.
    /// Behind a lock because ProCraft's builder holds a slot for a plugin engine that is `Send` but
    /// not `Sync`, and this renderer is shared by the parallel tile composite (`Sync`); nothing else
    /// ever touches it, and a stroke's own dabs are laid under `&mut self`.
    pub(crate) procraft: Option<std::sync::Arc<std::sync::Mutex<ProCraftStroke>>>,
    cov: CoverageMap,
    dual: Option<CoverageMap>,
    fmt: Option<PixelFormat>,
    per_dab_color: bool,
    material_base: Option<Arc<Surface>>,
    material_candidates: HashMap<(i32, i32), Arc<[[f32; 4]]>>,
    previous_centers: [Option<Point>; 2],
    transports: [Option<LaneTransport>; 2],
    dabs_done: usize,
    scratch: Vec<f32>,
    /// Wet Edges depth of the dab being accumulated (see [`CovTile::depth`]).
    depth_scratch: Vec<f32>,
    /// Per-pixel pattern alpha of the dab being rastered, so the sampled-pattern loop can run its
    /// rows in parallel and still scale the coverage in one serial pass afterwards.
    pattern_alpha: Vec<f32>,
    /// The folded frame's per-pixel lane colours, reused between frames.
    colour_scratch: Vec<f32>,
    dab_buf: Vec<Dab>,
    dual_buf: Vec<Dab>,
    all_dabs: Option<Vec<Dab>>,
    /// Buffers the last composite handed back, reused by the next one.
    regions: RegionPool,
}

/// Pixels of dab rectangles a batch mask backend gets in one submit. A backend packs them into
/// one texture, so this is an upper bound rather than a target: past it, dabs are submitted in
/// chunks.
const MASK_CHUNK_PIXELS: u64 = 8_000_000;

/// This app's stroke, drawn by ProCraft's brush: its builder turns pointer samples into stamps, and
/// the stamps are laid as this renderer's own dabs.
#[derive(Clone)]
struct ProCraftStroke {
    builder: procraft_engine::stroke::StrokeBuilder,
    settings: BrushSettings,
    color: [f32; 4],
}

impl std::fmt::Debug for ProCraftStroke {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProCraftStroke").field("size", &self.settings.size).field("spacing", &self.settings.spacing).finish_non_exhaustive()
    }
}

impl ProCraftStroke {
    fn new(brush: &BrushSettings) -> Self {
        Self { builder: crate::procraft::stroke(brush), settings: brush.clone(), color: brush.color }
    }

    /// The dabs the stamps just made are drawn as.
    fn dabs(&self, stamps: &[procraft_engine::stroke::Stamp]) -> Vec<Dab> {
        crate::procraft::dabs(stamps, &self.settings, self.color)
    }
}

/// `PHOTOCRAFT_PAINT_PROFILE=1` prints what a frame's dab batch costs, stage by stage.
fn profiling() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PHOTOCRAFT_PAINT_PROFILE").is_some())
}

/// Say why a frame went per-dab instead of folding, when `PC_FOLD_DEBUG` is set.
fn fold_debug(why: &str, n: usize) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("PC_FOLD_DEBUG").is_some()) {
        eprintln!("[paint] frame of {n} dabs kept per-dab: {why}");
    }
    false
}

/// Hand one chunk of dabs to `masker` and copy back the masks whose size matches their rectangle.
fn fill(masker: &dyn crate::stamp::DabMaskBatch, batch: &mut crate::stamp::MaskBatch, at: &[usize], out: &mut [Option<crate::stamp::MaskDab>]) {
    let Some(masks) = masker.masks(batch) else { return };
    if masks.len() != at.len() {
        return;
    }
    for (k, m) in masks.into_iter().enumerate() {
        let rect = batch.rects[k];
        let sized = m.coverage.len() == (rect.width() * rect.height()) as usize;
        // The depth lane only comes back for a wet stroke (`MaskBatch::want_depth`).
        if sized && (!batch.want_depth || m.depth.len() == m.coverage.len()) {
            out[at[k]] = Some(m);
        }
    }
}

/// The per-pixel colours a folded lane picks, lifted from the backend's straight rgb read (the
/// coverage-weighted average of the dabs' own colours) into the surface's channels.
fn lift_rgb(format: &PixelFormat, rgb: &[f32], out: &mut Vec<f32>) {
    let nc = format.mode.color_channels();
    let mut native = [0.0f32; 8];
    out.clear();
    out.reserve(rgb.len() / 3 * nc);
    for p in rgb.chunks_exact(3) {
        from_rgba_into(format, [p[0], p[1], p[2], 1.0], &mut native);
        out.extend_from_slice(&native[..nc]);
    }
}

/// The colours a patterned lane's dabs pick, filled over the whole `rect` of a folded frame.
/// The pattern is sampled in each dab's own frame (`scale · (pixel − centre)`), and the dabs of
/// one frame sit a pixel or two apart, so one reference dab stands in for the frame: the pattern
/// keeps its size and phase, and the colour drifts by the fraction of a texel between dabs.
fn lane_colors(pattern: &crate::brush::ColorPattern, format: &PixelFormat, rect: Rect, reference: &Dab, out: &mut Vec<f32>) {
    let width = rect.width() as usize;
    let nc = format.mode.color_channels();
    let scale = pattern.width.max(pattern.height) as f32 / (2.0 * reference.radius).max(1.0);
    out.clear();
    out.resize(width * rect.height() as usize * nc, 0.0);
    let row_at = |row: usize, line: &mut [f32]| {
        let mut native = [0.0f32; 8];
        let sy = scale * (rect.y0 as f32 + row as f32 + 0.5 - reference.center.y as f32);
        // A row walks +x, so the pattern's 4×4 texel neighbourhood is carried across pixels
        // instead of being rebuilt for every one of them.
        let mut walk = pattern.pigment_walk();
        for c in 0..width {
            let sx = scale * (rect.x0 as f32 + c as f32 + 0.5 - reference.center.x as f32);
            let rgba = walk.sample(sx, sy);
            from_rgba_into(format, rgba, &mut native);
            line[c * nc..(c + 1) * nc].copy_from_slice(&native[..nc]);
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    if parallel(width * rect.height() as usize * GENERAL_LOOP_COST) {
        use rayon::prelude::*;
        out.par_chunks_mut(width * nc).enumerate().for_each(|(row, line)| row_at(row, line));
        return;
    }
    for (row, line) in out.chunks_mut(width * nc).enumerate() {
        row_at(row, line);
    }
}

impl StrokeRenderer {
    /// Composite the union of this stroke and a mirrored pass as one stroke. This keeps
    /// overlapping dabs on a symmetry axis under one opacity ceiling at every bit depth.
    pub fn composite_union(&self, other: &Self, pre: &Surface, target: &mut Surface, selection: Option<&Surface>, lock_transparency: bool) -> Rect {
        self.composite_union_many(std::iter::once(other), pre, target, selection, lock_transparency)
    }

    fn union_transport_work(to: &mut Option<LaneTransport>, from: &Option<LaneTransport>, coverage: &CoverageMap, incoming: &CoverageMap) {
        let Some(source) = from else {
            return;
        };
        let Some(target) = to else {
            *to = Some(source.clone());
            return;
        };
        let channels = target.mix.format().channels();
        for (&(tx, ty), tile) in &incoming.tiles {
            let rect = Rect::new(tx * COV_TILE, ty * COV_TILE, (tx + 1) * COV_TILE, (ty + 1) * COV_TILE);
            let previous = coverage.tiles.get(&(tx, ty));
            let owns = |i: usize| tile.cov[i] > previous.map_or(0.0, |t| t.cov[i]);
            if !(0..tile.cov.len()).any(owns) {
                continue;
            }
            // Both pipelines own a surface per lane, so a mirrored pass has to merge each of them.
            let mut merge = |into: &mut Surface, from: &Surface| {
                let mut pixels = into.read_region(rect);
                let values = from.read_region(rect);
                for i in 0..tile.cov.len() {
                    if owns(i) {
                        let start = i * channels;
                        pixels[start..start + channels].copy_from_slice(&values[start..start + channels]);
                    }
                }
                into.write_region(rect, &pixels);
            };
            {
                let (into, from) = (&mut target.mix, &source.mix);
                merge(into, from);
            }
            if let (Some(into), Some(from)) = (target.blur.as_mut(), source.blur.as_ref()) {
                merge(into, from);
            }
        }
    }

    /// Combine all symmetry passes before applying brush opacity and selection once.
    pub fn composite_union_many<'a>(
        &self,
        others: impl IntoIterator<Item = &'a Self>,
        pre: &Surface,
        target: &mut Surface,
        selection: Option<&Surface>,
        lock_transparency: bool,
    ) -> Rect {
        let mut merged = self.clone();
        for other in others {
            Self::union_transport_work(&mut merged.transports[0], &other.transports[0], &merged.cov, &other.cov);
            if let (Some(to), Some(from)) = (&merged.dual, &other.dual) {
                Self::union_transport_work(&mut merged.transports[1], &other.transports[1], to, from);
            }
            merged.cov.union_max(&other.cov);
            if let (Some(to), Some(from)) = (&mut merged.dual, &other.dual) {
                to.union_max(from);
            }
        }
        merged.composite(pre, target, selection, lock_transparency, true)
    }

    /// Coverage tiles (stroke and Dual Brush) changed since the last call or composite, into
    /// `keys`; forgets them.
    fn drain_dirty_into(&mut self, keys: &mut HashSet<(i32, i32)>) {
        keys.extend(self.cov.dirty.drain());
        if let Some(d) = self.dual.as_mut() {
            keys.extend(d.dirty.drain());
        }
    }

    /// Every coverage tile (stroke and Dual Brush), into `keys`.
    fn keys_into(&self, keys: &mut HashSet<(i32, i32)>) {
        keys.extend(self.cov.tiles.keys());
        if let Some(d) = &self.dual {
            keys.extend(d.tiles.keys());
        }
    }

    /// Live preview of a stroke painted as several passes (the stroke, then its symmetry
    /// mirrors): leaves `target` as finishing every pass now and compositing them with
    /// [`composite_union_many`](Self::composite_union_many) over `pre` would, but redraws only the
    /// coverage tiles that changed since the previous call (new dabs, and the smoothing tail now
    /// and before). `full` redraws every tile: on the first call, or when a pass joins. Returns
    /// the damaged rectangle.
    pub fn composite_union_live(
        passes: &mut [&mut Self],
        live: &mut UnionLive,
        full: bool,
        pre: &Surface,
        target: &mut Surface,
        selection: Option<&Surface>,
        lock_transparency: bool,
    ) -> Rect {
        let mut keys = std::mem::take(&mut live.tail_keys);
        for p in passes.iter_mut() {
            p.drain_dirty_into(&mut keys);
            if full {
                p.keys_into(&mut keys);
            }
        }
        let spatial = passes.iter().any(|p| p.spatial_wet());
        let tails: Vec<_> = passes.iter().map(|p| p.tail_preview()).collect();
        let mut tail_keys = HashSet::new();
        for t in tails.iter().flatten() {
            t.keys_into(&mut tail_keys);
        }
        keys.extend(&tail_keys);
        if spatial {
            Self::expand_keys(&mut keys);
        }
        let output_keys = keys.clone();
        if spatial {
            Self::expand_keys(&mut keys);
        }
        let mut merged: Option<Self> = None;
        for (p, t) in passes.iter().zip(&tails) {
            let cov = p.cov.finished_subset(t.as_ref().map(|t| &t.cov), &keys);
            let dual = p.dual.as_ref().map(|d| d.finished_subset(t.as_ref().and_then(|t| t.dual.as_ref()), &keys));
            match merged.as_mut() {
                None => {
                    merged = Some(Self {
                        ctx: p.ctx.clone(),
                        generator: p.generator.clone(),
                        // A merge draws what the passes already made; no stroke builder is needed.
                        procraft: None,
                        cov,
                        dual,
                        fmt: p.fmt,
                        per_dab_color: p.per_dab_color,
                        material_base: p.material_base.clone(),
                        material_candidates: p.material_candidates.clone(),
                        previous_centers: p.previous_centers,
                        transports: t.as_ref().unwrap_or(p).transports.clone(),
                        dabs_done: p.dabs_done,
                        scratch: Vec::new(),
                        depth_scratch: Vec::new(),
                        pattern_alpha: Vec::new(),
                        colour_scratch: Vec::new(),
                        dab_buf: Vec::new(),
                        dual_buf: Vec::new(),
                        all_dabs: None,
            regions: RegionPool::default(),
                    })
                }
                Some(m) => {
                    let source = t.as_ref().unwrap_or(p);
                    Self::union_transport_work(&mut m.transports[0], &source.transports[0], &m.cov, &cov);
                    if let (Some(to), Some(from)) = (&m.dual, &dual) {
                        Self::union_transport_work(&mut m.transports[1], &source.transports[1], to, from);
                    }
                    m.cov.union_max(&cov);
                    if let (Some(to), Some(from)) = (&mut m.dual, &dual) {
                        to.union_max(from);
                    }
                }
            }
        }
        let Some(mut merged) = merged else { return Rect::EMPTY };
        let (before, after) = (live.bounds, merged.bounds());
        // Back to the pre-stroke pixels where the previous call drew and nothing covers now.
        let mut keys: Vec<_> = output_keys.iter().copied().collect();
        keys.sort_unstable();
        let mut dmg = Rect::EMPTY;
        let mut region = Vec::new();
        for &(tx, ty) in &keys {
            let tile = Rect::new(tx * COV_TILE, ty * COV_TILE, (tx + 1) * COV_TILE, (ty + 1) * COV_TILE);
            let old = tile.intersect(&before);
            if !old.is_empty() {
                pre.read_region_into(old, &mut region);
                target.write_region(old, &region);
                dmg = dmg.union(&old);
            }
            if merged.cov.tiles.contains_key(&(tx, ty)) {
                dmg = dmg.union(&tile.intersect(&after));
            }
        }
        merged.cov.dirty = output_keys;
        if let Some(d) = merged.dual.as_mut() {
            d.dirty.clear();
        }
        merged.composite_tiles(pre, target, selection, lock_transparency, false, false);
        live.tail_keys = tail_keys;
        live.bounds = after;
        dmg
    }
    /// `fmt` = target pixel format (needed for per-dab colour); `zoom` for smoothing.
    pub fn new(brush: &BrushSettings, fmt: Option<PixelFormat>, zoom: f32) -> Self {
        // Under ProCraft's brush the pattern is grain, not a second colour lane: one pipeline for
        // it, and a textured stroke then costs what a solid one costs (see `procraft::effective`).
        let effective = crate::procraft::enabled().then(|| crate::procraft::effective(brush));
        let brush = effective.as_ref().unwrap_or(brush);
        let single = brush.wet_edges
            && brush.wet_edge.as_ref().is_some_and(|w| w.outline == WetOutline::Single && w.color_mode != crate::material::WetColorMode::Normal);
        let per_dab_color = (brush.saturation_record_strength > 0.0
            || brush.saturation_preserve_output > 0.0
            || brush.color_dynamics.enabled
            || brush.pattern_enabled
            || brush.canvas_material.is_some()
            || brush.pigment.as_ref().is_some_and(|p| p.may_be_visible())
            || single)
            && !brush.erase
            && fmt.is_some();
        let nc = if per_dab_color { fmt.map_or(0, |f| f.mode.color_channels()) } else { 0 };
        Self {
            ctx: BrushContext::new(brush),
            generator: DabGenerator::new(brush, zoom),
            procraft: crate::procraft::enabled().then(|| std::sync::Arc::new(std::sync::Mutex::new(ProCraftStroke::new(brush)))),
            cov: CoverageMap::new(nc),
            dual: brush
                .dual_brush
                .enabled
                .then(|| CoverageMap::new(if brush.dual_brush.full_parameters.is_some() { fmt.map_or(0, |f| f.mode.color_channels()) } else { 0 })),
            fmt,
            per_dab_color,
            material_base: None,
            material_candidates: HashMap::new(),
            previous_centers: [None; 2],
            transports: [None, None],
            dabs_done: 0,
            scratch: Vec::new(),

            depth_scratch: Vec::new(),
            pattern_alpha: Vec::new(),
            colour_scratch: Vec::new(),
            dab_buf: Vec::new(),
            dual_buf: Vec::new(),
            all_dabs: None,
            regions: RegionPool::default(),
        }
    }

    /// Prepare colour filtering against the immutable pre-stroke pixels.
    pub fn on_surface(brush: &BrushSettings, pre: &Surface, zoom: f32) -> Self {
        let mut renderer = Self::new(brush, Some(pre.format()), zoom);
        renderer.transports[0] = LaneTransport::new(&renderer.ctx.brush, pre);
        renderer.transports[1] = renderer.ctx.secondary.as_ref().and_then(|c| LaneTransport::new(&c.brush, pre));
        let needs_base = |b: &BrushSettings| {
            b.wet_edges && b.wet_edge.as_ref().is_some_and(|w| w.outline == WetOutline::Single && w.color_mode != crate::material::WetColorMode::Normal)
        };
        if needs_base(brush) || renderer.ctx.secondary.as_ref().is_some_and(|c| needs_base(&c.brush)) {
            renderer.material_base = Some(Arc::new(pre.clone()));
        }
        renderer
    }

    /// Attach the stroke's starting pixels to every lane's transport surfaces. [`Self::new`] has
    /// no surface, so a renderer built that way has no mixing or blur pipeline at all — the live
    /// painting path builds its renderers with `new`, and must call this once `pre` exists or the
    /// transport never runs.
    pub fn attach_pre(&mut self, pre: &Surface) {
        self.transports[0] = LaneTransport::new(&self.ctx.brush, pre);
        self.transports[1] = self.ctx.secondary.as_ref().and_then(|c| LaneTransport::new(&c.brush, pre));
        let needs_base = |b: &BrushSettings| {
            b.wet_edges && b.wet_edge.as_ref().is_some_and(|w| w.outline == WetOutline::Single && w.color_mode != crate::material::WetColorMode::Normal)
        };
        if (needs_base(&self.ctx.brush) || self.ctx.secondary.as_ref().is_some_and(|c| needs_base(&c.brush))) && self.material_base.is_none() {
            self.material_base = Some(Arc::new(pre.clone()));
        }
    }

    /// Keep a copy of every primary dab (for tests and sequential tools).
    pub fn record_dabs(mut self) -> Self {
        self.all_dabs = Some(Vec::new());
        self
    }
    pub fn dabs(&self) -> &[Dab] {
        self.all_dabs.as_deref().unwrap_or(&[])
    }
    pub fn dab_count(&self) -> usize {
        self.dabs_done
    }
    /// Union of the dab rectangles so far.
    pub fn bounds(&self) -> Rect {
        self.dual.as_ref().filter(|_| self.ctx.secondary.is_some()).map_or(self.cov.bounds, |d| self.cov.bounds.union(&d.bounds))
    }

    /// The frame's masks from the batch backend: `None` for a dab it can't take (its own CPU
    /// raster still runs for that one, in order).
    fn batch_masks(&mut self, dabs: &[Dab], wet: bool) -> Vec<Option<crate::stamp::MaskDab>> {
        let mut out: Vec<Option<crate::stamp::MaskDab>> = dabs.iter().map(|_| None).collect();
        if self.ctx.masker.is_empty() {
            return out;
        }
        if let Some(first) = dabs.first() {
            self.ctx.texture_origin.get_or_insert(first.center);
        }
        let slot = self.ctx.masker.clone();
        let Some(masker) = slot.batch() else { return out };
        let mut batch = crate::stamp::MaskBatch {
            jobs: Vec::new(),
            rects: Vec::new(),
            texture: self.ctx.mask_texture(),
            tip: self.ctx.mask_tip(),
            want_depth: wet,
        };
        let mut at = Vec::with_capacity(dabs.len());
        let mut chunk_px = 0u64;
        for (i, d) in dabs.iter().enumerate() {
            let Some(job) = self.ctx.mask_job(d, wet) else { continue };
            let rect = self.ctx.dab_rect(d, false);
            let px = u64::from(rect.width()) * u64::from(rect.height());
            // A frame's dabs can be far more pixels than one atlas holds (a 1%-spacing stroke of
            // big dabs): hand the backend as many chunks as it takes.
            if !batch.jobs.is_empty() && chunk_px + px > MASK_CHUNK_PIXELS {
                fill(masker, &mut batch, &mut at, &mut out);
                batch.jobs.clear();
                batch.rects.clear();
                at.clear();
                chunk_px = 0;
            }
            batch.jobs.push(job);
            batch.rects.push(rect);
            at.push(i);
            chunk_px += px;
        }
        if !batch.jobs.is_empty() {
            fill(masker, &mut batch, &mut at, &mut out);
        }
        out
    }

    /// The frame folded by the batch backend as one rectangle, for a brush whose dabs all share
    /// one ceiling and carry nothing per-dab: the union is drawn on the GPU and comes back folded,
    /// so a frame costs one small readback instead of one per dab. Returns `false` (and leaves
    /// everything alone) when the frame doesn't qualify, so the per-dab path runs exactly as before.
    fn aggregate_frame(&mut self, dabs: &[Dab], duals: &[Dab], wet: bool, prof: bool, t_start: std::time::Instant) -> bool {
        if dabs.is_empty() || self.ctx.masker.is_empty() {
            return fold_debug("no dabs or no masker", dabs.len());
        }
        let b = &self.ctx.brush;
        // An eraser folds like any other stroke: the frame accumulates coverage into the map, and
        // the composite takes it away from the canvas (`erase` is read there, not here).
        if b.erase && b.mix_settings.enabled {
            return fold_debug("erase with mixing on", dabs.len());
        }
        if b.pigment.is_some() || b.canvas_material.is_some() {
            return fold_debug("pigment or material", dabs.len());
        }
        // Colour that follows the canvas beneath each dab (a material, pigment) has no folded
        // form. Colour dynamics does: every dab keeps its own colour, and the backend's colour
        // lane averages them under the same coverage the frame folds with.
        if self.per_dab_color && (b.pigment.as_ref().is_some_and(|p| p.may_be_visible()) || b.canvas_material.is_some()) {
            return fold_debug("per-dab colour", dabs.len());
        }
        if b.saturation_record_strength > 0.0 || b.saturation_preserve_output > 0.0 || self.transports[0].is_some() || self.transports[1].is_some() {
            return fold_debug("saturation or transport", dabs.len());
        }
        // A single-dab Wet Edges outline tints each dab's own rim against the canvas beneath it;
        // only the per-dab path has that base to read.
        let single = |wb: &BrushSettings| wb.wet_edges && wb.wet_edge.as_ref().is_some_and(|w| w.outline == WetOutline::Single && w.color_mode != crate::material::WetColorMode::Normal);
        if single(b) {
            return fold_debug("single-dab wet outline", dabs.len());
        }
        // The Dual Brush's lane has to qualify too: its dabs are folded with their own context.
        if let Some(ctx) = self.ctx.secondary.as_deref() {
            let s = &ctx.brush;
            if s.pigment.is_some() || s.canvas_material.is_some() || s.saturation_record_strength > 0.0 || s.saturation_preserve_output > 0.0 || single(s) {
                return fold_debug("secondary pigment, material, saturation or wet outline", dabs.len());
            }
        }
        // A single-dab Wet Edges outline tints its own rim; folded, the frame's dabs share one
        // rim depth, which is the same approximation the whole-stroke Wet Edges already makes.
        let Some(first) = dabs.first() else { return false };
        let ceil = first.opacity;
        let color = first.color;
        // A pattern's colour is sampled per pixel (dab-local), so it is recomputed over the
        // union; colour dynamics instead needs the backend's colour lane (every dab keeps its
        // own colour, and the lane averages them under the folded coverage).
        let wants_pattern = |wb: &BrushSettings| wb.pattern_enabled && wb.sampled_pattern.is_some();
        let want_color = b.color_dynamics.enabled && !wants_pattern(b);
        let mut union: Option<Rect> = None;
        let mut jobs = Vec::with_capacity(dabs.len());
        let mut rects = Vec::with_capacity(dabs.len());
        for d in dabs {
            if (d.opacity - ceil).abs() > 1e-4 || d.mix_strength > 0.0 || d.blur_strength > 0.0 || !d.pigment.is_inert() {
                return fold_debug("per-dab opacity, mix or pigment", dabs.len());
            }
            if !want_color && (0..4).any(|i| (d.color[i] - color[i]).abs() > 1e-3) {
                return fold_debug("per-dab colour", dabs.len());
            }
            let Some(job) = self.ctx.mask_job(d, wet) else { return fold_debug("mask job refused", dabs.len()) };
            let rect = self.ctx.dab_rect(d, false);
            if rect.is_empty() {
                return fold_debug("empty dab rect", dabs.len());
            }
            union = Some(union.map_or(rect, |u: Rect| u.union(&rect)));
            jobs.push(job);
            rects.push(rect);
        }
        let Some(union) = union else { return fold_debug("no union", dabs.len()) };
        let lane = crate::stamp::FrameLane { want_color, jobs, rects, union, ceil, texture: self.ctx.mask_texture(), tip: self.ctx.mask_tip() };
        // The Dual Brush's lane: its own context, its own union and ceiling.
        let dual_lane = if duals.is_empty() {
            None
        } else if let Some(ctx) = self.ctx.secondary.as_deref() {
            if duals.len() != dabs.len() {
                return fold_debug("dual dabs out of step", dabs.len());
            }
            let wet2 = ctx.brush.wet_edges && !b.erase;
            let Some(first2) = duals.first() else { return false };
            let ceil2 = ctx.brush.opacity * first2.opacity;
            let mut union2: Option<Rect> = None;
            let mut jobs2 = Vec::with_capacity(duals.len());
            let mut rects2 = Vec::with_capacity(duals.len());
            for d in duals {
                if d.mix_strength > 0.0 || d.blur_strength > 0.0 || !d.pigment.is_inert() || (d.opacity - first2.opacity).abs() > 1e-4 {
                    return fold_debug("dual per-dab opacity or mix", dabs.len());
                }
                let Some(job) = ctx.mask_job(d, wet2) else { return fold_debug("dual mask job refused", dabs.len()) };
                let rect = self.ctx.dab_rect(d, true);
                if rect.is_empty() {
                    return fold_debug("empty dual dab rect", dabs.len());
                }
                union2 = Some(union2.map_or(rect, |u: Rect| u.union(&rect)));
                jobs2.push(job);
                rects2.push(rect);
            }
            union2.map(|union| crate::stamp::FrameLane {
                want_color: ctx.brush.color_dynamics.enabled && !wants_pattern(&ctx.brush),
                jobs: jobs2,                rects: rects2,
                union,
                ceil: ceil2,
                texture: ctx.mask_texture(),
                tip: ctx.mask_tip(),
            })
        } else {
            // The generator always emits paired dual dabs, but with no Dual Brush they are
            // discarded by the per-dab path too, so the folded frame simply ignores them.
            None
        };
        let Some(masker) = self.ctx.masker.batch() else { return fold_debug("no batch backend", dabs.len()) };
        let frame = crate::stamp::MaskFrame { lane, dual: dual_lane };
        let t = profiling().then(std::time::Instant::now);
        let Some(out) = masker.frame(&frame) else { return fold_debug("backend refused the frame", dabs.len()) };
        let t_masks = t.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
        let px = |r: Rect| (r.width() * r.height()) as usize;
        if out.coverage.len() != px(frame.lane.union) || out.depth.len() != out.coverage.len() {
            return fold_debug("mask size mismatch", dabs.len());
        }
        if let (Some(lane2), Some(back)) = (&frame.dual, &out.dual)
            && (back.coverage.len() != px(lane2.union) || back.depth.len() != back.coverage.len())
        {
            return fold_debug("dual mask size mismatch", dabs.len());
        }
        if !out.color.is_empty() && out.color.len() != px(frame.lane.union) * 3 {
            return fold_debug("colour size mismatch", dabs.len());
        }
        let mut native = [0.0f32; 8];
        // The lane's per-pixel colour: the pattern's own sample (in the dab's own frame, so it is
        // recomputed once over the union from the frame's first dab), the backend's colour lane
        // when every dab brought its own, or else the stroke's one colour.
        let mut lane_col = |pattern: Option<&crate::brush::ColorPattern>, rect: Rect, reference: &Dab, rgb: &[f32], colors: &mut Vec<f32>| -> Option<bool> {
            let f = self.fmt?;
            match (pattern, rgb.is_empty()) {
                (Some(p), _) => {
                    lane_colors(p, &f, rect, reference, colors);
                    Some(true)
                }
                (None, false) => {
                    lift_rgb(&f, rgb, colors);
                    Some(true)
                }
                (None, true) => {
                    from_rgba_into(&f, color, &mut native);
                    colors.clear();
                    colors.extend_from_slice(&native[..f.mode.color_channels()]);
                    Some(false)
                }
            }
        };
        // One buffer for the lane's colours, kept between frames: a pattern frame writes 3–4
        // channels per pixel (10 MB at this size), and reallocating + zeroing that every frame
        // costs more than the sampling itself.
        let mut colors = std::mem::take(&mut self.colour_scratch);
        let pattern = b.pattern_enabled.then(|| b.sampled_pattern.as_ref()).flatten();
        let rgb: &[f32] = if pattern.is_none() { &out.color } else { &[] };
        let t_col = profiling().then(std::time::Instant::now);
        let col = lane_col(pattern, frame.lane.union, first, rgb, &mut colors);
        let t_col = t_col.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
        // The backend hands back what the frame's dabs leave over an empty pixel, `ceil·V`;
        // dividing by the ceiling gives the `v` the map's own `c + v·(ceil − c)` wants. Divides
        // the buffer it was handed (the backend's own Vec) so no second copy is allocated.
        fn divide(vals: &mut [f32], ceil: f32) {
            if ceil > 0.0 {
                for v in vals {
                    *v = (*v / ceil).clamp(0.0, 1.0);
                }
            } else {
                vals.iter_mut().for_each(|v| *v = 0.0);
            }
        }
        let mut vals = out.coverage;
        divide(&mut vals, frame.lane.ceil);
        let material = if wet {
            b.wet_edge.as_ref().map(|w| (first.wet_edge, (w.outline == WetOutline::Single).then(|| w.color_strength(first.wet_edge.strength))))
        } else {
            None
        };
        let t_acc = profiling().then(std::time::Instant::now);
        self.cov.accumulate_wet(frame.lane.union, &vals, frame.lane.ceil, col.map(|p| (colors.as_slice(), p)), wet.then_some(&out.depth[..]), material, None);
        let t_acc = t_acc.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
        self.cov.bounds = self.cov.bounds.union(&frame.lane.union);
        // The Dual Brush's lane, folded into its own map with its own ceiling.
        if let (Some(lane2), Some(back), Some(dm)) = (&frame.dual, out.dual, self.dual.as_mut()) {
            let mut vals2 = back.coverage;
            divide(&mut vals2, lane2.ceil);
            let ctx = self.ctx.secondary.as_deref();
            let wet2 = ctx.is_some_and(|c| c.brush.wet_edges) && !b.erase;
            let material2 = if wet2 {
                ctx.and_then(|c| c.brush.wet_edge.as_ref()).map(|w| (first.wet_edge, (w.outline == WetOutline::Single).then(|| w.color_strength(first.wet_edge.strength))))
            } else {
                None
            };
            let mut colors2 = Vec::new();
            let reference2 = duals.first().copied().unwrap_or(*first);
            let col2 = ctx.and_then(|c| {
                let pattern2 = c.brush.pattern_enabled.then(|| c.brush.sampled_pattern.as_ref()).flatten();
                let rgb2: &[f32] = if pattern2.is_none() { &back.color } else { &[] };
                lane_col(pattern2, lane2.union, &reference2, rgb2, &mut colors2)
            });
            dm.accumulate_wet(lane2.union, &vals2, lane2.ceil, col2.map(|p| (colors2.as_slice(), p)), wet2.then_some(&back.depth[..]), material2, None);
            dm.bounds = dm.bounds.union(&lane2.union);
        }
        self.colour_scratch = colors;
        self.dabs_done += dabs.len();
        if let Some(all) = self.all_dabs.as_mut() {
            all.extend_from_slice(dabs);
        }
        if prof {
            let total = t_start.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "[paint] {total:8.1} ms: one folded frame of {} dabs ({}×{}{}), masks {t_masks:8.1}, colour {t_col:8.1}, accumulate {t_acc:8.1}, rest {:8.1}",
                dabs.len(),
                frame.lane.union.width(),
                frame.lane.union.height(),
                frame.dual.as_ref().map_or(String::new(), |l| format!(" + dual {}×{}", l.union.width(), l.union.height())),
                total - t_masks
            );
        }
        true
    }

    fn raster_pending(&mut self) {
        let prof = profiling();
        let t_start = std::time::Instant::now();
        let mut t_masks = 0.0f64;
        let mut t_cpu_raster = 0.0f64;
        let mut t_col = 0.0f64;
        let mut t_transport = 0.0f64;
        let mut t_acc = 0.0f64;
        let mut t_atr = 0.0f64;
        let mut dabs = std::mem::take(&mut self.dab_buf);
        let mut duals = std::mem::take(&mut self.dual_buf);
        let wet = self.ctx.brush.wet_edges && !self.ctx.brush.erase;
        if self.aggregate_frame(&dabs, &duals, wet, prof, t_start) {
            dabs.clear();
            duals.clear();
            self.dab_buf = dabs;
            self.dual_buf = duals;
            return;
        }
        let masks = if prof {
            let t = std::time::Instant::now();
            let m = self.batch_masks(&dabs, wet);
            t_masks += t.elapsed().as_secs_f64() * 1000.0;
            m
        } else {
            self.batch_masks(&dabs, wet)
        };
        let cpu_fallback = masks.iter().filter(|m| m.is_none()).count() as u64;
        let mut native = [0.0f32; 8];
        let mut colors = Vec::new();
        let mut base_pixels = Vec::new();
        let mut canvas_colors = Vec::new();
        for (i, d) in dabs.iter().enumerate() {
            let rect = self.ctx.dab_rect(d, false);
            if let Some(m) = masks.get(i).and_then(Option::as_ref) {
                // The batch backend laid this dab's mask on the GPU: the tip maths and the
                // pattern sample are done, and everything below reads it as the CPU would.
                self.ctx.texture_tip = Some(*d);
                self.scratch.clear();
                self.scratch.extend_from_slice(&m.coverage);
                if wet {
                    self.depth_scratch.clear();
                    self.depth_scratch.extend_from_slice(&m.depth);
                }
            } else {
                let t = std::time::Instant::now();
                self.ctx.texture_origin.get_or_insert(d.center);
                self.ctx.texture_tip = Some(*d);
                // Wet Edges acts on the whole stroke (#2088): the dab records how deep it reaches each
                // pixel and the stroke darkens its own rim when composited, not every dab's.
                self.ctx.rasterize_into(d, false, rect, &mut self.scratch, wet.then_some(&mut self.depth_scratch));
                t_cpu_raster += t.elapsed().as_secs_f64() * 1000.0;
            }
            let single =
                self.ctx.brush.wet_edge.as_ref().filter(|w| wet && w.outline == WetOutline::Single && w.color_mode != crate::material::WetColorMode::Normal);
            let pattern = self.ctx.brush.pattern_enabled.then_some(self.ctx.brush.sampled_pattern.as_ref()).flatten();
            let t_c = profiling().then(std::time::Instant::now);
            let col = if let (Some(pattern), Some(f)) = (pattern, self.fmt) {
                let width = rect.width() as usize;
                let height = rect.height() as usize;
                let nc = f.mode.color_channels();
                let scale = pattern.width.max(pattern.height) as f32 / (2.0 * d.radius).max(1.0);
                colors.clear();
                colors.resize(width * height * nc, 0.0);
                if single.is_some()
                    && let Some(base) = &self.material_base
                {
                    base.read_region_into(rect, &mut base_pixels);
                }
                // The sampled-pattern loop is the hot path of a textured brush: every covered pixel
                // pays a five-tap alpha-weighted pattern sample. Its rows are independent, so they
                // go to the thread pool above the general-loop threshold and the coverage scaling
                // (which must touch `scratch` in order) happens in one serial pass below.
                self.pattern_alpha.clear();
                self.pattern_alpha.resize(width * height, 0.0);
                let scratch = &self.scratch;
                let depth = &self.depth_scratch;
                let base_view = &base_pixels[..];
                let sample_row = |row: usize, out: &mut [f32], alpha: &mut [f32]| {
                    let mut native = [0.0f32; 8];
                    let sy = scale * (rect.y0 as f32 + row as f32 + 0.5 - d.center.y as f32);
                    for col_i in 0..width {
                        let i = row * width + col_i;
                        let Some(&coverage) = scratch.get(i) else { break };
                        if coverage <= 0.0 {
                            continue;
                        }
                        let sx = scale * (rect.x0 as f32 + col_i as f32 + 0.5 - d.center.x as f32);
                        let mut rgba = pattern.sample_pigment(sx, sy);
                        alpha[col_i] = rgba[3];
                        if let Some(w) = single {
                            let channels = f.channels();
                            let base = base_view.get(i * channels..(i + 1) * channels).map_or([0.0; 4], |p| to_rgba(&f, p));
                            rgba = w.color(rgba, base, depth[i], d.wet_edge);
                        }
                        from_rgba_into(&f, rgba, &mut native);
                        out[col_i * nc..(col_i + 1) * nc].copy_from_slice(&native[..nc]);
                    }
                };
                let mut serial = |row: usize, out: &mut [f32], alpha: &mut [f32]| sample_row(row, out, alpha);
                #[cfg(not(target_arch = "wasm32"))]
                let mut pooled = |row: usize, out: &mut [f32], alpha: &mut [f32]| sample_row(row, out, alpha);
                #[cfg(not(target_arch = "wasm32"))]
                let use_pool = parallel(width * height * GENERAL_LOOP_COST);
                #[cfg(target_arch = "wasm32")]
                let use_pool = false;
                let _ = &mut serial;
                if use_pool {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        use rayon::prelude::*;
                        colors
                            .par_chunks_mut(width * nc)
                            .zip(self.pattern_alpha.par_chunks_mut(width))
                            .enumerate()
                            .for_each(|(row, (out, alpha))| pooled(row, out, alpha));
                    }
                } else {
                    for (row, (out, alpha)) in colors.chunks_mut(width * nc).zip(self.pattern_alpha.chunks_mut(width)).enumerate() {
                        serial(row, out, alpha);
                    }
                }
                for (coverage, alpha) in self.scratch.iter_mut().zip(&self.pattern_alpha) {
                    *coverage *= *alpha;
                }
                Some((colors.as_slice(), true))
            } else {
                match (self.per_dab_color, self.fmt, single) {
                    (true, Some(f), Some(w)) => {
                        let nc = f.mode.color_channels();
                        let n = f.channels();
                        colors.clear();
                        if let Some(base) = &self.material_base {
                            base.read_region_into(rect, &mut base_pixels);
                        } else {
                            base_pixels.clear();
                        }
                        for (i, edge) in self.depth_scratch.iter().enumerate() {
                            let base = base_pixels.get(i * n..(i + 1) * n).map_or([0.0; 4], |p| to_rgba(&f, p));
                            let rgba = w.color(d.color, base, *edge, d.wet_edge);
                            from_rgba_into(&f, rgba, &mut native);
                            colors.extend_from_slice(&native[..nc]);
                        }
                        Some((colors.as_slice(), true))
                    }
                    (true, Some(f), None) => {
                        from_rgba_into(&f, d.color, &mut native);
                        Some((&native[..f.mode.color_channels()], false))
                    }
                    _ => None,
                }
            };
            let col = if let (Some(material), Some(f)) = (&self.ctx.brush.canvas_material, self.fmt) {
                canvas_colors.clear();
                let nc = f.mode.color_channels();
                let width = rect.width() as usize;
                for i in 0..self.scratch.len() {
                    let mut rgba = d.color;
                    if let Some((values, per_pixel)) = col {
                        let off = if per_pixel { i * nc } else { 0 };
                        let mut value = [0.0; 8];
                        value[..nc].copy_from_slice(&values[off..off + nc]);
                        if let Some(ai) = alpha_index(&f) {
                            value[ai] = d.color[3];
                        }
                        rgba = to_rgba(&f, &value[..f.channels()]);
                    }
                    let rgb = material.pigment_response(
                        [rgba[0], rgba[1], rgba[2]],
                        [rect.x0 as f32 + (i % width) as f32 + 0.5, rect.y0 as f32 + (i / width) as f32 + 0.5],
                        self.ctx.brush.opacity,
                        d.alpha,
                    );
                    rgba[..3].copy_from_slice(&rgb);
                    let mut encoded = [0.0; 8];
                    from_rgba_into(&f, rgba, &mut encoded);
                    canvas_colors.extend_from_slice(&encoded[..nc]);
                }
                Some((canvas_colors.as_slice(), true))
            } else {
                col
            };
            t_col += t_c.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
            let material = if wet {
                self.ctx.brush.wet_edge.as_ref().map(|w| (d.wet_edge, (w.outline == WetOutline::Single).then(|| w.color_strength(d.wet_edge.strength))))
            } else {
                None
            };
            let t = profiling().then(std::time::Instant::now);
            if let Some(transport) = self.transports[0].as_mut() {
                transport.dab(&self.ctx, d, rect, &self.scratch, col);
            }
            t_transport += t.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
            let t = profiling().then(std::time::Instant::now);
            self.cov.accumulate_transport(rect, &self.scratch, d.mix_strength, d.blur_strength, d.alpha);
            t_atr += t.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
            let t = profiling().then(std::time::Instant::now);
            self.cov.accumulate_wet(
                rect,
                &self.scratch,
                d.opacity,
                col,
                wet.then_some(&self.depth_scratch[..]),
                material,
                self.fmt
                    .filter(|_| self.ctx.brush.saturation_record_strength > 0.0 || self.ctx.brush.saturation_preserve_output > 0.0)
                    .map(|f| (self.ctx.brush.saturation_record_strength, self.ctx.brush.saturation_preserve_output, self.ctx.brush.opacity, f)),
            );
            let motion = dab_motion(&mut self.previous_centers[0], d);
            self.cov.accumulate_pigment(&self.ctx, d, rect, &self.scratch, motion);
            t_acc += t.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
            // Bounds track the full dab rectangle (even fully transparent parts), like the damage.
            self.cov.bounds = self.cov.bounds.union(&rect);
        }
        if let Some(dm) = self.dual.as_mut() {
            for d in &duals {
                if let Some(ctx) = self.ctx.secondary.as_mut() {
                    ctx.texture_origin.get_or_insert(d.center);
                    ctx.texture_tip = Some(*d);
                }
                let rect = self.ctx.dab_rect(d, true);
                let secondary = self.ctx.secondary.as_ref();
                let secondary_brush = secondary.map(|ctx| &ctx.brush);
                let wet_settings = secondary_brush.map_or(self.ctx.brush.dual_brush.wet_edge.as_ref(), |b| b.wet_edge.as_ref());
                let wet = secondary_brush.map_or(self.ctx.brush.dual_brush.wet_edges, |b| b.wet_edges) && !self.ctx.brush.erase;
                self.ctx.rasterize_into(d, true, rect, &mut self.scratch, wet.then_some(&mut self.depth_scratch));
                let material = if wet {
                    wet_settings.map(|w| (d.wet_edge, (w.outline == WetOutline::Single).then(|| w.color_strength(d.wet_edge.strength))))
                } else {
                    None
                };
                let single =
                    wet.then_some(wet_settings).flatten().filter(|w| w.outline == WetOutline::Single && w.color_mode != crate::material::WetColorMode::Normal);
                if single.is_some()
                    && let (Some(base), Some(_)) = (&self.material_base, self.fmt)
                {
                    base.read_region_into(rect, &mut base_pixels);
                }
                let col = if let (Some(ctx), Some(f)) = (secondary, self.fmt) {
                    colors.clear();
                    let width = rect.width() as usize;
                    for (i, coverage) in self.scratch.iter_mut().enumerate() {
                        let mut rgba = if ctx.brush.pattern_enabled {
                            if let Some(p) = &ctx.brush.sampled_pattern {
                                let rgba = p.sample_pigment(
                                    rect.x0 as f32 + (i % width) as f32 + 0.5 - d.center.x as f32,
                                    rect.y0 as f32 + (i / width) as f32 + 0.5 - d.center.y as f32,
                                );
                                *coverage *= rgba[3];
                                rgba
                            } else {
                                d.color
                            }
                        } else {
                            d.color
                        };
                        if let Some(material) = &ctx.brush.canvas_material {
                            let rgb = material.pigment_response(
                                [rgba[0], rgba[1], rgba[2]],
                                [rect.x0 as f32 + (i % width) as f32 + 0.5, rect.y0 as f32 + (i / width) as f32 + 0.5],
                                ctx.brush.opacity,
                                d.alpha,
                            );
                            rgba[..3].copy_from_slice(&rgb);
                        }
                        if let Some(ws) = single {
                            let n = f.channels();
                            let base = base_pixels.get(i * n..(i + 1) * n).map_or([0.0; 4], |p| to_rgba(&f, p));
                            rgba = ws.color(rgba, base, self.depth_scratch[i], d.wet_edge);
                        }
                        from_rgba_into(&f, rgba, &mut native);
                        colors.extend_from_slice(&native[..f.mode.color_channels()]);
                    }
                    Some((colors.as_slice(), true))
                } else {
                    None
                };
                let ceiling = secondary.map_or(1.0, |ctx| ctx.brush.opacity) * d.opacity;
                if let (Some(transport), Some(ctx)) = (self.transports[1].as_mut(), secondary) {
                    transport.dab(ctx, d, rect, &self.scratch, col);
                }
                dm.accumulate_transport(rect, &self.scratch, d.mix_strength, d.blur_strength, d.alpha);
                dm.accumulate_wet(
                    rect,
                    &self.scratch,
                    ceiling,
                    col,
                    wet.then_some(&self.depth_scratch[..]),
                    material,
                    self.fmt.and_then(|f| {
                        secondary_brush
                            .filter(|b| b.saturation_record_strength > 0.0 || b.saturation_preserve_output > 0.0)
                            .map(|b| (b.saturation_record_strength, b.saturation_preserve_output, b.opacity, f))
                    }),
                );
                let motion = dab_motion(&mut self.previous_centers[1], d);
                if let Some(ctx) = secondary {
                    dm.accumulate_pigment(ctx, d, rect, &self.scratch, motion);
                }
            }
        }
        self.dabs_done += dabs.len();
        if let Some(all) = self.all_dabs.as_mut() {
            all.extend_from_slice(&dabs);
        }
        // The lanes' smeared pixels go back into their own surfaces once a frame, not once a dab.
        let Self { transports, ctx, .. } = self;
        for t in transports.iter_mut().flatten() {
            t.flush(ctx);
        }
        dabs.clear();
        duals.clear();
        self.dab_buf = dabs;
        self.dual_buf = duals;
        if prof {
            let total = t_start.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "[paint] {total:8.1} ms: masks {t_masks:8.1}, cpu raster {t_cpu_raster:8.1}, colour {t_col:8.1}, transport {t_transport:8.1}, tr-weights {t_atr:8.1}, accumulate {t_acc:8.1}, rest {:8.1} ({} dabs, {cpu_fallback} on the CPU)",
                total - t_masks - t_cpu_raster - t_col - t_transport - t_acc - t_atr,
                self.dabs_done.min(usize::MAX)
            );
        }
    }

    /// See [`DabGenerator::wants_time`].
    pub fn wants_time(&self) -> bool {
        self.generator.wants_time()
    }

    /// The string between the brush and the pointer while smoothing lags (see
    /// [`Smoother::string`](crate::dynamics::Smoother::string)).
    pub fn smoothing_string(&self) -> Option<([f64; 2], [f64; 2])> {
        self.generator.smoothing_string()
    }

    /// Draw this stroke with ProCraft's brush from here on (see [`crate::procraft::enabled`]).
    ///
    /// The switch is read from the environment when a stroke starts; this lets a caller — a test,
    /// or a panel that offers the choice per brush — take the stroke either way.
    pub fn use_procraft(&mut self, on: bool) {
        self.procraft = on.then(|| std::sync::Arc::new(std::sync::Mutex::new(ProCraftStroke::new(&self.ctx.brush))));
    }

    /// Whether this stroke is drawn by ProCraft's brush.
    pub fn draws_with_procraft(&self) -> bool {
        self.procraft.is_some()
    }

    /// Feed input points (any chunking gives the same result).
    pub fn push(&mut self, pts: &[StrokePoint]) {
        if let Some(shared) = &self.procraft {
            let mut p = shared.lock().unwrap_or_else(|e| e.into_inner());
            let mut dabs = Vec::new();
            for pt in pts {
                let sample = crate::procraft::sample(pt);
                // ProCraft's canvas adds with the tip shown: what the tapers still hold back comes
                // back as the tip (`add_tipped`), and the tip is the canvas's own to draw.
                let stamps = p.builder.add_tipped(&[sample]).0;
                dabs.extend(p.dabs(&stamps));
            }
            drop(p);
            dabs.sort_by_key(|d| d.index);
            self.push_dabs(&dabs);
            return;
        }
        self.generator.push(pts, &mut self.dab_buf, &mut self.dual_buf);
        self.raster_pending();
    }

    /// Lay dabs worked out elsewhere, as this stroke's own.
    ///
    /// The brush system is ProCraft's: its `StrokeBuilder` turns the pointer into `Stamp`s, and
    /// those come here as dabs. Everything after the dab — coverage, the GPU frame, the composite
    /// onto the layer — is unchanged, so only where the dabs fall and how they are shaped differs
    /// from the generator's own (which stays for presets the replacement does not reach yet).
    pub fn push_dabs(&mut self, dabs: &[Dab]) {
        // A dab's index is its place in the stroke: the stroke's own count so far keeps them in
        // order whatever produced them.
        let first = self.dabs_done as u64;
        self.dab_buf.extend(dabs.iter().enumerate().map(|(i, d)| Dab { index: first + i as u64, ..*d }));
        self.raster_pending();
    }

    /// End of stroke (smoothing catch-up, a lone first point).
    pub fn finish(&mut self) {
        if let Some(shared) = &self.procraft {
            // Pen up: the tapers' held-back stretch, drawn for good.
            let dabs = {
                let mut p = shared.lock().unwrap_or_else(|e| e.into_inner());
                let stamps = p.builder.add_tipped(&[]).0;
                p.dabs(&stamps)
            };
            self.push_dabs(&dabs);
            return;
        }
        self.generator.finish(&mut self.dab_buf, &mut self.dual_buf);
        self.raster_pending();
    }

    /// The primary dabs finishing the stroke now would add, without finishing it: for live
    /// previews of sequential-dab tools, which run each dab themselves (see [`tail_preview`](Self::tail_preview)).
    pub fn tail_dabs(&self) -> Vec<Dab> {
        let mut generator = self.generator.clone();
        let (mut dabs, mut duals) = (Vec::new(), Vec::new());
        generator.finish(&mut dabs, &mut duals);
        dabs
    }

    /// What finishing the stroke now would add (the smoothing catch-up tail to the last point, or
    /// a lone first dab), for live previews: a renderer holding copies of the coverage tiles the
    /// tail touches with the tail rendered in, so its `composite` draws exactly those tiles as
    /// [`finish`](Self::finish) would leave them. `None` when finishing adds nothing.
    pub fn tail_preview(&self) -> Option<StrokeRenderer> {
        let mut generator = self.generator.clone();
        let (mut dabs, mut duals) = (Vec::new(), Vec::new());
        generator.finish(&mut dabs, &mut duals);
        if dabs.is_empty() && duals.is_empty() {
            return None;
        }
        let mut keys = HashSet::new();
        for r in dabs.iter().map(|d| self.ctx.dab_rect(d, false)).chain(duals.iter().map(|d| self.ctx.dab_rect(d, true))) {
            if r.is_empty() {
                continue;
            }
            for ty in r.y0.div_euclid(COV_TILE)..=(r.y1 - 1).div_euclid(COV_TILE) {
                for tx in r.x0.div_euclid(COV_TILE)..=(r.x1 - 1).div_euclid(COV_TILE) {
                    keys.insert((tx, ty));
                }
            }
        }
        let subset = |m: &CoverageMap| CoverageMap {
            tiles: keys.iter().filter_map(|k| m.tiles.get(k).map(|t| (*k, t.clone()))).collect(),
            nc: m.nc,
            bounds: m.bounds,
            dirty: HashSet::new(),
        };
        let mut t = StrokeRenderer {
            ctx: self.ctx.clone(),
            generator,
            // The tail preview draws its own dabs; a stroke builder of its own is not needed.
            procraft: None,
            cov: subset(&self.cov),
            dual: self.dual.as_ref().map(subset),
            fmt: self.fmt,
            per_dab_color: self.per_dab_color,
            material_base: self.material_base.clone(),
            material_candidates: self.material_candidates.clone(),
            previous_centers: self.previous_centers,
            transports: self.transports.clone(),
            dabs_done: self.dabs_done,
            scratch: Vec::new(),

            depth_scratch: Vec::new(),
            pattern_alpha: Vec::new(),
            colour_scratch: Vec::new(),
            dab_buf: dabs,
            dual_buf: duals,
            all_dabs: None,
            regions: RegionPool::default(),
        };
        t.raster_pending();
        // Tiles whose coverage the tail doesn't change still composite the same: redraw them all.
        t.cov.dirty.extend(t.cov.tiles.keys().copied());
        Some(t)
    }

    /// Mark the coverage tiles over `r` for the next `composite` (e.g. to redraw over a preview).
    pub fn mark_dirty(&mut self, r: Rect) {
        if r.is_empty() {
            return;
        }
        for ty in r.y0.div_euclid(COV_TILE)..=(r.y1 - 1).div_euclid(COV_TILE) {
            for tx in r.x0.div_euclid(COV_TILE)..=(r.x1 - 1).div_euclid(COV_TILE) {
                if self.cov.tiles.contains_key(&(tx, ty)) {
                    self.cov.dirty.insert((tx, ty));
                }
            }
        }
    }

    /// The coverage tiles touched since the last call (or `composite`), as one rectangle clipped to
    /// the stroke bounds, and forget them: what a live preview that composites its own paint has
    /// to redraw.
    pub fn take_dirty_rect(&mut self) -> Rect {
        let r = self
            .cov
            .take_dirty()
            .into_iter()
            .fold(Rect::EMPTY, |acc, (tx, ty)| acc.union(&Rect::new(tx * COV_TILE, ty * COV_TILE, (tx + 1) * COV_TILE, (ty + 1) * COV_TILE)));
        if let Some(d) = self.dual.as_mut() {
            d.dirty.clear();
        }
        r.intersect(&self.bounds())
    }

    /// Final coverage over `r`, one value per pixel in rows (as [`dense_coverage`](Self::dense_coverage)
    /// over its bounds).
    pub fn coverage_in(&self, r: Rect) -> Vec<f32> {
        (r.y0..r.y1).flat_map(|y| (r.x0..r.x1).map(move |x| (x, y))).map(|(x, y)| self.coverage_at(x, y)).collect()
    }

    fn expand_keys(keys: &mut HashSet<(i32, i32)>) {
        let changed: Vec<_> = keys.iter().copied().collect();
        for (x, y) in changed {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    keys.insert((x + dx, y + dy));
                }
            }
        }
    }

    fn spatial_wet(&self) -> bool {
        let b = &self.ctx.brush;
        let spatial = |w: &crate::material::WetEdgeSettings| w.style == WetStyle::Watercolor2 && w.outline == WetOutline::Stroke;
        (b.wet_edges && b.wet_edge.as_ref().is_some_and(spatial))
            || self.ctx.secondary.as_ref().is_some_and(|c| c.brush.wet_edges && c.brush.wet_edge.as_ref().is_some_and(spatial))
            || (self.ctx.secondary.is_none() && b.dual_brush.enabled && b.dual_brush.wet_edges && b.dual_brush.wet_edge.as_ref().is_some_and(spatial))
    }

    fn wet_pixel(&self, dual: bool, x: i32, y: i32) -> (f32, f32, ResolvedWet) {
        let b = &self.ctx.brush;
        let map = if dual { self.dual.as_ref().unwrap_or(&self.cov) } else { &self.cov };
        let c = map.get(x, y);
        let ws = if dual {
            if let Some(ctx) = &self.ctx.secondary {
                ctx.brush.wet_edges.then_some(ctx.brush.wet_edge.as_ref()).flatten()
            } else {
                b.dual_brush.wet_edges.then_some(b.dual_brush.wet_edge.as_ref()).flatten()
            }
        } else {
            b.wet_edges.then_some(b.wet_edge.as_ref()).flatten()
        };
        let (wet, single_edge) = map.material_at(x, y);
        let Some(ws) = ws.filter(|_| !b.erase) else { return (c, 0.0, wet) };
        if c <= 0.0 {
            return (0.0, 0.0, wet);
        }
        if ws.outline == WetOutline::Single {
            return (c, single_edge, wet);
        }
        let edge = if ws.style == WetStyle::Watercolor2 { ws.spatial_edge(c, wet.width, x, y, |px, py| map.get(px, py)) } else { ws.color_mask(c, wet.width) };
        (ws.alpha(c, edge, wet), edge, wet)
    }

    fn material_color(&self, mut src: [f32; 4], base: [f32; 4], x: i32, y: i32) -> [f32; 4] {
        let b = &self.ctx.brush;
        if b.erase {
            return src;
        }
        if b.wet_edges
            && let Some(ws) = b.wet_edge.as_ref().filter(|w| w.outline != WetOutline::Single)
        {
            let (_, edge, wet) = self.wet_pixel(false, x, y);
            src = ws.color(src, base, edge, wet);
        }
        if self.ctx.secondary.is_none()
            && self.dual.is_some()
            && b.dual_brush.wet_edges
            && let Some(ws) = &b.dual_brush.wet_edge
        {
            let (_, edge, wet) = self.wet_pixel(true, x, y);
            src = if ws.outline == WetOutline::Single { ws.color_with_effect(src, base, edge) } else { ws.color(src, base, edge, wet) };
        }
        src
    }

    fn transported_output(&self, destination: [f32; 4], painted: [f32; 4], x: i32, y: i32, selection: f32) -> [f32; 4] {
        if self.transports.iter().all(Option::is_none) {
            return painted;
        }
        let brush = &self.ctx.brush;
        let lane_source = |map: &CoverageMap, b: &BrushSettings| {
            let c = map.get(x, y);
            let mut source = b.color;
            let (tx, ty) = (x.div_euclid(COV_TILE), y.div_euclid(COV_TILE));
            let i = ((y - ty * COV_TILE) * COV_TILE + x - tx * COV_TILE) as usize;
            if let (Some(tile), Some(fmt)) = (map.tiles.get(&(tx, ty)), self.fmt)
                && c > 0.0
                && !tile.col.is_empty()
            {
                let mut px = [0.0; 8];
                from_rgba_into(&fmt, source, &mut px);
                for (ch, v) in px.iter_mut().take(map.nc).enumerate() {
                    *v = tile.col[i * map.nc + ch] / c;
                }
                source = to_rgba(&fmt, &px[..fmt.channels()]);
            }
            source
        };
        let lane_wet_color = |secondary: bool, b: &BrushSettings, value: [f32; 4]| {
            if !secondary {
                return self.material_color(value, destination, x, y);
            }
            if b.wet_edges
                && let Some(w) = b.wet_edge.as_ref().filter(|w| w.outline == WetOutline::Stroke)
            {
                let (_, edge, wet) = self.wet_pixel(true, x, y);
                w.color(value, destination, edge, wet)
            } else {
                value
            }
        };
        // Each lane blends its *own* blurred colour and its *own* dragged colour into the lane's
        // paint colour, weighted by what that pipeline recorded for this pixel. The weights come
        // from the tip's own rasterised coverage, so the drag's shape and strength follow the nib
        // exactly; the two pipelines live on separate surfaces, so blurring never eats the drag and
        // the drag never samples a blurred layer.
        //
        // The mixed colour lies *under* the stroke's own: what the nib picked up and carried sits
        // beneath the colour it lays, and the stroke's colour lets it through by the weight the
        // pipeline recorded — C3's order, the mix below and the tip above. At a light mix the
        // stroke's colour is nearly all of the pixel; at a strong one the carried colour shows
        // through it, and the stroke's colour is never dropped from the pixel.
        let under = |out: [f32; 4], value: [f32; 4], k: f32| {
            let k = k.clamp(0.0, 1.0);
            let carried = (value[3] * k).clamp(0.0, 1.0);
            if carried <= 0.0 {
                return out;
            }
            // The carried colour lies within the painted pixel, so the pixel keeps the alpha the
            // stroke laid; only its colour is shared between the two.
            let above = (out[3] * (1.0 - k)).clamp(0.0, 1.0);
            let total = above + carried * (1.0 - above);
            if total <= 0.000001 {
                return out;
            }
            std::array::from_fn(|i| {
                if i < 3 {
                    (out[i] * above + value[i] * carried * (1.0 - above)) / total
                } else {
                    out[3]
                }
            })
        };
        let pipelines = |map: &CoverageMap, t: &LaneTransport, base: [f32; 4]| {
            let weights = map.transport_weights(x, y);
            let mut out = base;
            if weights[1] > 0.0
                && let Some(blur) = t.blur.as_ref()
            {
                let value = blur.rgba(x, y);
                if value[3] > 0.0 {
                    out = under(out, value, weights[1]);
                }
            }
            if weights[0] > 0.0 {
                let value = t.mix.rgba(x, y);
                if value[3] > 0.0 {
                    out = under(out, value, weights[0]);
                }
            }
            out
        };
        // Two pipelines per lane that both output, neither one rewriting the other's settings:
        //   1. the colour-output pipeline deposits the brush colour straight onto the destination with
        //      the stroke's own coverage, opacity, blend mode, colour attributes and pressure response;
        //   2. the smudge/blur pipeline then reads that result and moves it, weighted by the tip's own
        //      coverage and its own curves/surfaces.
        // Because the drag reads the already deposited colour, an opaque brush still shows the drag,
        // and because the deposit is composited first, no drag ratio can steal its coverage.
        let paint = |secondary: bool, b: &BrushSettings, map: &CoverageMap| {
            if !(b.mix_settings.color_output && !brush.erase) {
                return None;
            }
            let source = lane_wet_color(secondary, b, lane_source(map, b));
            Some(if b.color_attributes.enabled {
                b.color_attributes.composite(source, destination, source, map.get(x, y) * source[3])
            } else {
                source
            })
        };
        let deposit = |secondary: bool, b: &BrushSettings, map: &CoverageMap| -> [f32; 4] {
            match paint(secondary, b, map) {
                Some(colour) => {
                    let coverage = (self.wet_pixel(secondary, x, y).0 * b.opacity).clamp(0.0, 1.0);
                    self.fmt.map_or_else(
                        || photocraft_color::blend::composite(b.mode, destination, colour, coverage),
                        |fmt| lane_composite(&fmt, b, destination, colour, coverage, false),
                    )
                }
                None => destination,
            }
        };
        let primary = deposit(false, brush, &self.cov);
        // The dual lane needs the lanes' *uncomposited* straight colours, so it takes the paint
        // pipeline's own output rather than the deposited pixel.
        let primary_straight = paint(false, brush, &self.cov).unwrap_or(destination);
        let output: [f32; 4] = if let (Some(ctx), Some(dm)) = (&self.ctx.secondary, &self.dual) {
            let secondary = paint(true, &ctx.brush, dm).unwrap_or(destination);
            // The second half's own smudge, composited over its own colour before the two lanes are
            // blended: what it carried has to reach the pixels, not stay in its transport surface.
            let secondary = match self.transports[1].as_ref() {
                Some(t) => pipelines(dm, t, secondary),
                None => secondary,
            };
            let p = self.cov.get(x, y) * brush.opacity * brush.dual_brush.primary_strength;
            let d = dm.get(x, y) * brush.dual_brush.secondary_strength;
            let weight = (d / (p + d).max(0.0001)).clamp(0.0, 1.0);
            // The reference blends the two lanes' *straight* colours by that weight and applies
            // the combined mask once, rather than compositing each lane over the destination and
            // then lerping the results: the latter lets the base image show through at heavy
            // overlap.
            let straight = |c: [f32; 4]| {
                if c[3] > 0.0001 {
                    [c[0] / c[3], c[1] / c[3], c[2] / c[3]]
                } else {
                    [0.0; 3]
                }
            };
            let (pc, sc) = (straight(primary_straight), straight(secondary));
            let mixed = std::array::from_fn::<f32, 3, _>(|i| (pc[i] + (sc[i] - pc[i]) * weight).clamp(0.0, 1.0));
            // The combined alpha is the dual-mode blend of the two lanes' coverages.
            let a = dual_blend(brush.dual_brush.mode, p.min(1.0), d.min(1.0)).clamp(0.0, 1.0);
            // The combined mask already carries the stroke-level options, so the lane is applied
            // once over the destination (no extra coverage ratio).
            let lane: [f32; 4] = std::array::from_fn(|i| {
                let lane = if i < 3 { mixed[i] * a } else { a };
                destination[i] + (lane - destination[i]) * a
            });
            // The smudge/blur pipelines run last, over the lane's own output.
            match self.transports[0].as_ref() {
                Some(t) => pipelines(&self.cov, t, lane),
                None => lane,
            }
        } else {
            match self.transports[0].as_ref() {
                Some(t) => pipelines(&self.cov, t, primary),
                None => primary,
            }
        };
        let mut result = std::array::from_fn(|i| destination[i] + (output[i] - destination[i]) * selection);
        if brush.erase {
            result = painted;
        }
        result
    }

    /// The material field a composite pass shades: `pigment` normalised by the pixel's coverage,
    /// which is what the per-sample lookup used to recompute from scratch for every tap.
    fn material_field(&self, map: &CoverageMap, rect: Rect, selection: Option<&Surface>) -> Vec<[f32; 4]> {
        let w = rect.width().max(0) as usize;
        let opacity = if self.ctx.secondary.is_some() { 1.0 } else { self.ctx.brush.opacity };
        let mut out = Vec::with_capacity(w * rect.height().max(0) as usize);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let raw = map.pigment_at(x, y);
                let mut selected = [0.0; 8];
                let selected = selection.map_or(1.0, |s| {
                    s.read_pixel(x, y, &mut selected);
                    selected[0]
                });
                let coverage = (self.coverage_at(x, y) * opacity * selected).clamp(0.0, 1.0);
                out.push(if raw[3] > 0.0 { raw.map(|v| v * coverage / raw[3]) } else { [0.0, 0.0, 0.0, coverage] });
            }
        }
        out
    }

    fn shade_pigment(
        &self,
        normal: [f32; 4],
        x: i32,
        y: i32,
        pre: &Surface,
        candidates: &HashMap<(i32, i32), Arc<[[f32; 4]]>>,
        selection: Option<&Surface>,
        field: Option<(&[[f32; 4]], Rect)>,
    ) -> [f32; 4] {
        let mut selected = [0.0; 8];
        let selection_value = selection.map_or(1.0, |s| {
            s.read_pixel(x, y, &mut selected);
            selected[0]
        });
        if self.coverage_at(x, y) * selection_value <= 0.0 {
            return normal;
        }
        let bilinear = |point: [f32; 2], read: &dyn Fn(i32, i32) -> [f32; 4]| {
            let (ix, iy) = (point[0].floor() as i32, point[1].floor() as i32);
            let (fx, fy) = (point[0] - point[0].floor(), point[1] - point[1].floor());
            let (a, b, c, d) = (read(ix, iy), read(ix + 1, iy), read(ix, iy + 1), read(ix + 1, iy + 1));
            std::array::from_fn(|i| (a[i] + (b[i] - a[i]) * fx) * (1.0 - fy) + (c[i] + (d[i] - c[i]) * fx) * fy)
        };
        let source = |xx: i32, yy: i32| {
            let (tx, ty) = (xx.div_euclid(COV_TILE), yy.div_euclid(COV_TILE));
            let i = ((yy - ty * COV_TILE) * COV_TILE + xx - tx * COV_TILE) as usize;
            candidates.get(&(tx, ty)).and_then(|v| v.get(i)).copied().unwrap_or_else(|| pre.rgba(xx, yy))
        };
        // `shade` asks for the same handful of neighbouring pixels dozens of times per output pixel
        // (eight height samples, their bilinear taps, then texture / liquid samples all overlap). Every
        // miss costs a tile lookup plus a stroke-mask evaluation, so memoise them for this pixel.
        struct MaterialCache {
            keys: [[i32; 2]; 32],
            values: [[f32; 4]; 32],
            len: usize,
        }
        impl MaterialCache {
            fn find(&self, xx: i32, yy: i32) -> Option<[f32; 4]> {
                self.keys[..self.len].iter().position(|k| k == &[xx, yy]).map(|i| self.values[i])
            }
            fn insert(&mut self, xx: i32, yy: i32, value: [f32; 4]) {
                if self.len == self.keys.len() {
                    return;
                }
                self.keys[self.len] = [xx, yy];
                self.values[self.len] = value;
                self.len += 1;
            }
        }
        let shade_lane = |map: &CoverageMap, fast: Option<(&[[f32; 4]], Rect)>| {
            let (tx, ty) = (x.div_euclid(COV_TILE), y.div_euclid(COV_TILE));
            let i = ((y - ty * COV_TILE) * COV_TILE + x - tx * COV_TILE) as usize;
            let tile = map.tiles.get(&(tx, ty));
            let settings = tile.and_then(|t| t.pigment_settings.get(i)).copied().unwrap_or_default();
            // The material at this pixel is a field of zeros: `shade` would return `normal` untouched.
            if settings.is_inert() {
                return normal;
            }
            let motion = tile.and_then(|t| t.pigment_motion.get(i)).copied().unwrap_or([0.0; 2]);
            let cache = std::cell::RefCell::new(MaterialCache { keys: [[0; 2]; 32], values: [[0.0; 4]; 32], len: 0 });
            let material = |xx: i32, yy: i32| {
                if let Some((data, rect)) = fast
                    && xx >= rect.x0
                    && xx < rect.x1
                    && yy >= rect.y0
                    && yy < rect.y1
                {
                    return data[(yy - rect.y0) as usize * rect.width() as usize + (xx - rect.x0) as usize];
                }
                if let Some(value) = cache.borrow().find(xx, yy) {
                    return value;
                }
                let raw = map.pigment_at(xx, yy);
                let mut selected = [0.0; 8];
                let selection = selection.map_or(1.0, |s| {
                    s.read_pixel(xx, yy, &mut selected);
                    selected[0]
                });
                let opacity = if self.ctx.secondary.is_some() { 1.0 } else { self.ctx.brush.opacity };
                let coverage = (self.coverage_at(xx, yy) * opacity * selection).clamp(0.0, 1.0);
                let value = if raw[3] > 0.0 { raw.map(|v| v * coverage / raw[3]) } else { [0.0, 0.0, 0.0, coverage] };
                cache.borrow_mut().insert(xx, yy, value);
                value
            };
            crate::pigment_material::shade(
                normal,
                settings,
                settings.pressure,
                motion,
                |o| bilinear([x as f32 + o[0], y as f32 + o[1]], &material),
                |o| bilinear([x as f32 + o[0], y as f32 + o[1]], &source),
            )
        };
        let primary = shade_lane(&self.cov, field);
        if let (Some(_), Some(dm)) = (&self.ctx.secondary, &self.dual) {
            let secondary = shade_lane(dm, None);
            let p = self.cov.get(x, y) * self.ctx.brush.opacity * self.ctx.brush.dual_brush.primary_strength;
            let d = dm.get(x, y) * self.ctx.brush.dual_brush.secondary_strength;
            let w = (d / (p + d).max(0.0001)).clamp(0.0, 1.0);
            std::array::from_fn(|i| primary[i] + (secondary[i] - primary[i]) * w)
        } else {
            primary
        }
    }

    /// Does any pixel of `r` carry stroke coverage? The stroke trail skips a restamp of a region
    /// the stroke never touched.
    pub fn touches(&self, r: Rect) -> bool {
        if r.is_empty() {
            return false;
        }
        // Tile granularity first: an untouched region costs one map lookup per tile.
        let (t0x, t0y) = (r.x0.div_euclid(COV_TILE), r.y0.div_euclid(COV_TILE));
        let (t1x, t1y) = ((r.x1 - 1).div_euclid(COV_TILE), (r.y1 - 1).div_euclid(COV_TILE));
        (t0y..=t1y).any(|ty| (t0x..=t1x).any(|tx| self.cov.tiles.contains_key(&(tx, ty))))
    }

    /// Final stroke coverage at a pixel (stroke-level masks applied, before opacity/selection).
    pub fn coverage_at(&self, x: i32, y: i32) -> f32 {
        let c = self.cov.get(x, y);
        if c <= 0.0 && self.ctx.secondary.is_none() {
            return 0.0;
        }
        let b = &self.ctx.brush;
        if b.wet_edge.is_none() && b.dual_brush.wet_edge.is_none() && self.ctx.secondary.as_ref().is_none_or(|c| c.brush.wet_edge.is_none()) {
            return self.ctx.stroke_mask_wet(c, self.cov.depth(x, y), self.dual.as_ref().map(|d| d.get(x, y)), x, y);
        }
        let c = self.wet_pixel(false, x, y).0;
        let dual = self.dual.as_ref().map(|_| self.wet_pixel(true, x, y).0);
        self.ctx.stroke_mask_wet(c, self.cov.depth(x, y), dual, x, y)
    }

    /// Dense final coverage over the stroke bounds.
    pub fn dense_coverage(&self) -> (Rect, Vec<f32>) {
        let b = self.bounds();
        let out = (b.y0..b.y1).flat_map(|y| (b.x0..b.x1).map(move |x| (x, y))).map(|(x, y)| self.coverage_at(x, y)).collect();
        (b, out)
    }

    /// Composite the stroke buffer onto `target`, reading original pixels from `pre` (the
    /// pre-stroke surface; pass a clone of the target taken before the first `push`). With
    /// `all = false` only tiles touched since the last composite are processed (interactive use).
    /// Returns the damaged rectangle.
    pub fn composite(&mut self, pre: &Surface, target: &mut Surface, selection: Option<&Surface>, lock_transparency: bool, all: bool) -> Rect {
        self.composite_tiles(pre, target, selection, lock_transparency, all, true)
    }

    fn composite_tiles(&mut self, pre: &Surface, target: &mut Surface, selection: Option<&Surface>, lock_transparency: bool, all: bool, halo: bool) -> Rect {
        let pigment_active = !self.ctx.brush.erase
            && (self.ctx.brush.pigment.as_ref().is_some_and(|p| p.may_be_visible())
                || self.ctx.secondary.as_ref().is_some_and(|c| c.brush.pigment.as_ref().is_some_and(|p| p.may_be_visible())));
        let spatial = self.spatial_wet() || pigment_active;
        if !all && let Some(d) = &self.dual {
            self.cov.dirty.extend(d.dirty.iter().copied());
        }
        if spatial && !all && halo {
            Self::expand_keys(&mut self.cov.dirty);
        }
        let keys: Vec<(i32, i32)> = if all {
            self.cov.dirty.clear();
            let mut k: Vec<_> = self.cov.tiles.keys().copied().collect();
            if self.ctx.secondary.is_some()
                && let Some(d) = &self.dual
            {
                k.extend(d.tiles.keys().copied());
            }
            k.sort_unstable();
            k.dedup();
            k
        } else {
            self.cov.take_dirty()
        };
        if let Some(d) = self.dual.as_mut() {
            d.dirty.clear();
        }
        let bounds = self.bounds();
        let fmt = target.format();
        let n = fmt.channels();
        let a_idx = alpha_index(&fmt);
        let nc = fmt.mode.color_channels();
        let b = &self.ctx.brush;
        let opacity = if self.ctx.secondary.is_some() { 1.0 } else { b.opacity.clamp(0.0, 1.0) };
        let mut src = [0.0f32; 8];
        from_rgba_into(&fmt, b.color, &mut src);
        // A colour picked as CMYK paints its inks as they are in a CMYK target (#4344).
        if let (ColorMode::Cmyk, Some((rgb, inks))) = (fmt.mode, b.color_cmyk)
            && rgb == [b.color[0], b.color[1], b.color[2]]
        {
            src[..4].copy_from_slice(&inks);
        }
        if let Some(a) = a_idx {
            src[a] = b.color[3];
        }
        // 8-bit targets: the pixels the stroke changes are dithered as they are quantised, so
        // soft, low-flow strokes don't band (see `dither8`). Aliased strokes (the Pencil, the
        // Eraser's Pencil and Block modes) paint every pixel by the same amount, so they have no
        // gradient to band and are not dithered: a Pencil line at partial opacity is one flat
        // colour, as in Photoshop, not grain that a tolerance-0 Magic Wand or Paint Bucket trips on.
        let dither = fmt.sample == SampleType::U8 && !b.aliased;
        let mut dmg = Rect::EMPTY;
        let mut candidates = std::mem::take(&mut self.material_candidates);
        let mut material_regions = Vec::new();
        let mut region = Vec::new();
        // A stroke without wet edges or a secondary brush takes `stroke_mask_wet` directly.
        let plain_mask = b.wet_edge.is_none() && b.dual_brush.wet_edge.is_none() && self.ctx.secondary.is_none();
        // Each tile composites into a buffer of its own, so a large stroke's tiles run in
        // parallel; the writes to the target follow, one by one, in key order.
        const BATCH: usize = 256;
        let mut pool = std::mem::take(&mut self.regions.0);
        for batch in keys.chunks(BATCH) {
            let mut items: Vec<(i32, i32, Rect, Vec<f32>)> =
                batch.iter().map(|&(tx, ty)| (tx, ty, Rect::EMPTY, pool.pop().unwrap_or_default())).collect();
            let total_pixels = batch.len() * (COV_TILE * COV_TILE) as usize;
            for_each_item(&mut items, total_pixels, |item| {
            let (tx, ty, tr_out, region) = (&item.0, &item.1, &mut item.2, &mut item.3);
            let (tx, ty) = (*tx, *ty);
            let primary_tile = self.cov.tiles.get(&(tx, ty));
            let Some(tile) = primary_tile.or_else(|| self.dual.as_ref().and_then(|d| d.tiles.get(&(tx, ty)))) else { return };
            let tr = Rect::new(tx * COV_TILE, ty * COV_TILE, (tx + 1) * COV_TILE, (ty + 1) * COV_TILE).intersect(&bounds);
            if tr.is_empty() {
                return;
            }
            region.clear();
            pre.read_region_into(tr, region);
            // Per-dab colour rewrites the source per pixel: each tile works on its own copy.
            let mut src = src;
            let sel = selection.map(|s| (s.channels(), s.read_region(tr)));
            let w = tr.width() as usize;
            for y in tr.y0..tr.y1 {
                for x in tr.x0..tr.x1 {
                    let ti = ((y - ty * COV_TILE) * COV_TILE + (x - tx * COV_TILE)) as usize;
                    let c = primary_tile.map_or(0.0, |t| t.cov[ti]);
                    if c <= 0.0 && self.ctx.secondary.is_none() {
                        continue;
                    }
                    let i = (y - tr.y0) as usize * w + (x - tr.x0) as usize;
                    // Same value as `coverage_at`, without re-testing the stroke's wet-edge and
                    // dual-brush setup for every pixel (the hot path of a large composite).
                    let m = if plain_mask {
                        self.ctx.stroke_mask_wet(c, self.cov.depth(x, y), self.dual.as_ref().map(|d| d.get(x, y)), x, y)
                    } else {
                        self.coverage_at(x, y)
                    };
                    let s = sel.as_ref().map_or(1.0, |(sc, v)| v[i * sc]);
                    let mut k = (m * opacity * s).min(1.0);
                    if k <= 0.0 {
                        continue;
                    }
                    if b.mode == BlendMode::Dissolve {
                        k = if hash2(x, y, b.seed) < k { 1.0 } else { 0.0 };
                        if k == 0.0 {
                            continue;
                        }
                    }
                    let px = &mut region[i * n..(i + 1) * n];
                    if b.erase {
                        // With mixing on, the eraser lays the dragged colour into the pixel first and
                        // then removes the coverage, so erasing smears the paint it passes over
                        // instead of reading as a colourless wipe. Empty pixels stay empty.
                        if let Some(t) = self.transports[0].as_ref()
                            && a_idx.is_none_or(|a| px[a] > 0.0)
                        {
                            let carried = t.mix.rgba(x, y);
                            if carried[3] > 0.0 {
                                let mut encoded = [0.0; 8];
                                from_rgba_into(&fmt, carried, &mut encoded);
                                let nc = fmt.mode.color_channels();
                                px[..nc].copy_from_slice(&encoded[..nc]);
                            }
                        }
                        if let (Some(a), false) = (a_idx, lock_transparency) {
                            px[a] *= 1.0 - k;
                        }
                        if dither {
                            dither8(px, x, y);
                        }
                        continue;
                    }
                    if lock_transparency && a_idx.is_some_and(|a| px[a] <= 0.0) {
                        continue;
                    }
                    // Behind (`mode = behind`): paint under the existing pixels instead of over
                    // them. An opaque destination already hides the stroke, so it is left alone.
                    if b.behind {
                        let Some(a) = a_idx else { continue };
                        let da = px[a];
                        if lock_transparency || da >= 1.0 {
                            continue;
                        }
                        let mut base_src = src;
                        if self.per_dab_color && c > 0.0 && !tile.col.is_empty() {
                            let p = &tile.col[ti * nc..(ti + 1) * nc];
                            for ch in 0..nc {
                                base_src[ch] = p[ch] / c;
                            }
                        }
                        let underneath = base_src[a] * k * (1.0 - da);
                        let alpha = da + underneath;
                        if alpha > 0.0 {
                            for channel in 0..a {
                                px[channel] = (px[channel] * da + base_src[channel] * underneath) / alpha;
                            }
                            px[a] = alpha;
                        }
                        if dither {
                            dither8(px, x, y);
                        }
                        continue;
                    }
                    if self.per_dab_color && c > 0.0 && !tile.col.is_empty() {
                        let p = &tile.col[ti * nc..(ti + 1) * nc];
                        for ch in 0..nc {
                            src[ch] = p[ch] / c;
                        }
                    }
                    let mut pixel_src = src;
                    let primary_color = b.wet_edges
                        && b.wet_edge.as_ref().is_some_and(|w| w.outline == WetOutline::Stroke && w.color_mode != crate::material::WetColorMode::Normal);
                    let dual_color = b.dual_brush.enabled
                        && b.dual_brush.wet_edges
                        && b.dual_brush.wet_edge.as_ref().is_some_and(|w| w.color_mode != crate::material::WetColorMode::Normal);
                    if primary_color || dual_color {
                        let rgba = self.material_color(to_rgba(&fmt, &src[..n]), to_rgba(&fmt, px), x, y);
                        from_rgba_into(&fmt, rgba, &mut pixel_src);
                    }
                    let destination = to_rgba(&fmt, px);
                    let source = to_rgba(&fmt, &pixel_src[..n]);
                    let native_only = self.ctx.secondary.is_none()
                        && !b.color_attributes.enabled
                        && b.pigment_mix_strength <= 0.0
                        && matches!(b.mode, BlendMode::Normal | BlendMode::Dissolve);
                    let mut result = if native_only { destination } else { lane_composite(&fmt, b, destination, source, k, lock_transparency) };
                    if let (Some(ctx), Some(dm)) = (&self.ctx.secondary, &self.dual)
                        && let Some(tile) = dm.tiles.get(&(tx, ty))
                        && tile.cov[ti] > 0.0
                        && !tile.col.is_empty()
                    {
                        let coverage = tile.cov[ti];
                        let mut secondary = pixel_src;
                        for (ch, value) in secondary.iter_mut().enumerate().take(nc) {
                            *value = tile.col[ti * nc + ch] / coverage;
                        }
                        let mut other = to_rgba(&fmt, &secondary[..n]);
                        if ctx.brush.wet_edges
                            && let Some(ws) = ctx.brush.wet_edge.as_ref().filter(|w| w.outline != WetOutline::Single)
                        {
                            let (_, edge, wet) = self.wet_pixel(true, x, y);
                            other = ws.color(other, destination, edge, wet);
                        }
                        let secondary = lane_composite(&fmt, &ctx.brush, destination, other, k, lock_transparency);
                        let p = self.wet_pixel(false, x, y).0 * b.opacity * b.dual_brush.primary_strength;
                        let d = self.wet_pixel(true, x, y).0 * b.dual_brush.secondary_strength;
                        let weight = (d / (p + d).max(0.0001)).clamp(0.0, 1.0);
                        for ch in 0..4 {
                            result[ch] += (secondary[ch] - result[ch]) * weight;
                        }
                    }
                    if native_only {
                        over_native(&fmt, px, &pixel_src[..n], k, lock_transparency);
                    } else {
                        let mut encoded = [0.0; 8];
                        from_rgba_into(&fmt, result, &mut encoded);
                        px.copy_from_slice(&encoded[..n]);
                    }
                    if self.transports.iter().any(Option::is_some) {
                        let mut result = self.transported_output(destination, to_rgba(&fmt, px), x, y, s);
                        if lock_transparency {
                            result[3] = destination[3];
                        }
                        let mut encoded = [0.0; 8];
                        from_rgba_into(&fmt, result, &mut encoded);
                        px.copy_from_slice(&encoded[..n]);
                    }
                    if dither && !pigment_active {
                        dither8(px, x, y);
                    }
                }
            }
            *tr_out = tr;
            });
            // Writeback in key order, so the pool's buffers and the target's pixels stay deterministic.
            for (tx, ty, tr, region) in items {
                if tr.is_empty() {
                    pool.push(region);
                    continue;
                }
                let primary_tile = self.cov.tiles.get(&(tx, ty));
                let Some(_tile) = primary_tile.or_else(|| self.dual.as_ref().and_then(|d| d.tiles.get(&(tx, ty)))) else {
                    pool.push(region);
                    continue;
                };
                if pigment_active {
                    let pixels = candidates.entry((tx, ty)).or_insert_with(|| {
                        (0..COV_TILE).flat_map(|yy| (0..COV_TILE).map(move |xx| pre.rgba(tx * COV_TILE + xx, ty * COV_TILE + yy))).collect::<Vec<_>>().into()
                    });
                    let pixels = Arc::make_mut(pixels);
                    for y in tr.y0..tr.y1 {
                        for x in tr.x0..tr.x1 {
                            let i = (y - tr.y0) as usize * tr.width() as usize + (x - tr.x0) as usize;
                            let ti = ((y - ty * COV_TILE) * COV_TILE + x - tx * COV_TILE) as usize;
                            pixels[ti] = to_rgba(&fmt, &region[i * n..(i + 1) * n]);
                        }
                    }
                    material_regions.push(tr);
                } else {
                    target.write_region(tr, &region);
                }
                pool.push(region);
                dmg = dmg.union(&tr);
            }
        }
        self.regions.0 = pool;
        for tr in material_regions {
            region.clear();
            // One material value per pixel of this region, instead of one tile lookup per sample tap.
            let field = self.material_field(&self.cov, tr, selection);
            for y in tr.y0..tr.y1 {
                for x in tr.x0..tr.x1 {
                    let (tx, ty) = (x.div_euclid(COV_TILE), y.div_euclid(COV_TILE));
                    let ti = ((y - ty * COV_TILE) * COV_TILE + x - tx * COV_TILE) as usize;
                    let normal = candidates.get(&(tx, ty)).and_then(|v| v.get(ti)).copied().unwrap_or_else(|| pre.rgba(x, y));
                    let shaded = self.shade_pigment(normal, x, y, pre, &candidates, selection, Some((field.as_slice(), tr)));
                    let mut native = [0.0; 8];
                    from_rgba_into(&fmt, shaded, &mut native);
                    let mut selected = [0.0; 8];
                    let selection_value = selection.map_or(1.0, |s| {
                        s.read_pixel(x, y, &mut selected);
                        selected[0]
                    });
                    if dither && self.coverage_at(x, y) * selection_value > 0.0 {
                        dither8(&mut native[..n], x, y);
                    }
                    region.extend_from_slice(&native[..n]);
                }
            }
            target.write_region(tr, &region);
        }
        self.material_candidates = candidates;
        if all { bounds } else { dmg }
    }
}

fn lane_composite(fmt: &PixelFormat, b: &BrushSettings, destination: [f32; 4], source: [f32; 4], coverage: f32, lock: bool) -> [f32; 4] {
    let mut normal = if matches!(b.mode, BlendMode::Normal | BlendMode::Dissolve) {
        let n = fmt.channels();
        let mut d = [0.0; 8];
        let mut s = [0.0; 8];
        from_rgba_into(fmt, destination, &mut d);
        from_rgba_into(fmt, source, &mut s);
        over_native(fmt, &mut d[..n], &s[..n], coverage, lock);
        to_rgba(fmt, &d[..n])
    } else {
        photocraft_color::blend::composite(b.mode, destination, source, coverage)
    };
    if lock {
        normal[3] = destination[3];
    }
    let amount = coverage * source[3];
    if b.pigment_mix_strength > 0.0 && fmt.mode == ColorMode::Rgb {
        let combined = amount + destination[3] * (1.0 - amount);
        normal = crate::pigment::paint(normal, destination, source, amount / combined.max(0.000001), b.pigment_mix_strength);
    }
    if b.color_attributes.enabled {
        normal = b.color_attributes.composite(normal, destination, source, amount);
    }
    if lock {
        normal[3] = destination[3];
    }
    normal
}

fn c3_tip_falloff(distance: f32, radius: f32, hardness: f32, subpixel: bool) -> f32 {
    if hardness >= 0.999 {
        return if subpixel { if distance <= radius { 1.0 } else { 0.0 } } else { (radius - distance + 0.5).clamp(0.0, 1.0) };
    }
    let h = hardness.clamp(0.0, 0.999);
    let t = ((distance / radius.max(0.0005) - h) / (1.0 - h)).clamp(0.0, 1.0);
    let tail = (-2.5f32).exp();
    // metal 940: `max(0.0, (exp(-k t²) - exp(-k)) / (1 - exp(-k)))`.
    (((-2.5 * t * t).exp() - tail) / (1.0 - tail)).max(0.0)
}

/// Tip coverage at a pixel, shared by the fast path and the reference oracle so the two cannot
/// drift apart through expression order. `dd` is the roundness-scaled distance from the dab centre;
/// `ro` the roundness, `r` the un-scaled radius, `rm` the scaled one.
#[inline]
pub(crate) fn tip_coverage(c3: bool, aliased: bool, dd: f32, rm: f32, ro: f32, r: f32, hardness: f32) -> f32 {
    if aliased {
        return if dd <= rm { 1.0 } else { 0.0 };
    }
    if c3 { c3_tip_falloff(dd / ro, r, hardness, false) } else { dab_coverage(dd, rm, hardness) }
}

pub(crate) fn c3_sampled_mask(m: &Mips, u: f32, v: f32, hardness: f32) -> f32 {
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return 0.0;
    }
    let (w, h, _) = &m.levels[0];
    let (w, h) = (*w as f32, *h as f32);
    let sample = |x: f32, y: f32| m.sample_clamped(0, x.clamp(0.5 / w, 1.0 - 0.5 / w), y.clamp(0.5 / h, 1.0 - 0.5 / h));
    let base = sample(u, v);
    let hrd = hardness.clamp(0.0, 1.0);
    if hrd >= 0.999 {
        return base;
    }
    let boost = ((1.0 - hrd) * 5.0).clamp(0.0, 1.0);
    let rx = ((0.6 + boost * 6.4) / w).min(0.125);
    let ry = ((0.6 + boost * 6.4) / h).min(0.125);
    let mut blur = base * 0.28;
    for (x, y) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
        blur += sample(u + x * rx, v + y * ry) * 0.11;
    }
    for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        blur += sample(u + x * rx, v + y * ry) * 0.07;
    }
    base.min(base + (blur - base) * boost).clamp(0.0, 1.0).powf(2.2 - 1.2 * hrd)
        * c3_tip_falloff((u * 2.0 - 1.0).abs().max((v * 2.0 - 1.0).abs()), 1.0, hrd, false)
}

fn triangle_mask(u: f32, v: f32, radius: f32, roundness: f32, hardness: f32, subpixel: bool) -> f32 {
    // metal 1016: `q = local / max(halfExtent, float2(0.0001))`; Rust `v` is y-up, hence the `-v`.
    let qx = u / radius.max(0.0001);
    let qy = -v / (radius * roundness).max(0.0001);
    let edge = ((qy + 1.0 - 2.0 * qx) / 5.0f32.sqrt()).min(1.0 - qy).min((qy + 2.0 * qx + 1.0) / 5.0f32.sqrt());
    if edge <= 0.0 {
        return 0.0;
    }
    let distance = (1.0 - edge / 0.4472136).clamp(0.0, 1.0);
    c3_tip_falloff(distance * radius, radius, hardness, subpixel)
}

fn dab_motion(previous: &mut Option<Point>, d: &Dab) -> [f32; 2] {
    let last = previous.replace(d.center).unwrap_or(d.center);
    let dx = (d.center.x - last.x) as f32;
    let dy = (d.center.y - last.y) as f32;
    let length = dx.hypot(dy);
    let cap = (d.radius * 2.0 * 0.08).clamp(2.0, 8.0);
    let k = if length > cap { cap / length } else { 1.0 };
    [dx * k, dy * k]
}

/// Render a whole stroke onto `target` (one-shot). Returns the damaged rectangle.
pub fn render_stroke(
    target: &mut Surface,
    brush: &BrushSettings,
    points: &[StrokePoint],
    selection: Option<&Surface>,
    lock_transparency: bool,
    zoom: f32,
) -> Rect {
    if points.is_empty() {
        return Rect::EMPTY;
    }
    let pre = target.clone();
    let mut r = StrokeRenderer::on_surface(brush, &pre, zoom);
    r.push(points);
    r.finish();
    r.composite(&pre, target, selection, lock_transparency, true)
}

// The zero-angle transform only changes signs; squaring gives the same radial distance.
#[inline(never)]
fn rasterize_round(d: &Dab, rect: Rect, out: &mut [f32], hardness: f32, aliased: bool) {
    let (cx, cy) = if aliased { grid_center(d.center.x, d.center.y, 2.0 * d.radius) } else { (d.center.x as f32, d.center.y as f32) };
    let ro = d.roundness.max(0.5 / d.radius).min(1.0);
    let rm = (d.radius * ro).max(0.5);
    let w = rect.width() as usize;
    let reach = if aliased { rm } else { tip_reach(rm, hardness).max(rm) };
    let reach2 = (reach + 0.5) * (reach + 0.5);
    for (yy, row) in out.chunks_exact_mut(w).enumerate() {
        let dy = (rect.y0 + yy as i32) as f32 + 0.5 - cy;
        if aliased {
            for (xx, dst) in row.iter_mut().enumerate() {
                let dx = (rect.x0 + xx as i32) as f32 + 0.5 - cx;
                let d2 = (dx * ro).powi(2) + dy * dy;
                *dst = if d2 < reach2 && d2.sqrt() <= rm { d.alpha } else { 0.0 };
            }
        } else {
            for (xx, dst) in row.iter_mut().enumerate() {
                let dx = (rect.x0 + xx as i32) as f32 + 0.5 - cx;
                let d2 = (dx * ro).powi(2) + dy * dy;
                let val = if d2 >= reach2 { 0.0 } else { tip_falloff(d2.sqrt(), rm, hardness) };
                *dst = if val <= 0.0 { 0.0 } else { val * d.alpha };
            }
        }
    }
}

#[cfg(test)]
#[path = "render_reference.rs"]
mod reference;

#[cfg(test)]
mod material_regression {
    use super::*;
    use crate::material::{WetColorMode, WetEdgeSettings};
    use photocraft_geom::Point;

    #[test]
    fn transported_lanes_filter_their_own_completed_wet_colors_before_merging() {
        let first = WetEdgeSettings { outline: WetOutline::Stroke, color_mode: WetColorMode::Hsl, ..Default::default() };
        let second = WetEdgeSettings { outline: WetOutline::Stroke, color_mode: WetColorMode::Multiply, ..Default::default() };
        let a = [0.5, 0.3, 0.1, 1.0];
        let b = [0.2, 0.6, 0.8, 1.0];
        let secondary = BrushSettings {
            color: b,
            wet_edges: true,
            wet_edge: Some(second.clone()),
            mix_settings: crate::transport::BrushMixSettings { enabled: true, ..Default::default() },
            ..Default::default()
        };
        let brush = BrushSettings {
            color: a,
            wet_edges: true,
            wet_edge: Some(first.clone()),
            mix_settings: crate::transport::BrushMixSettings { enabled: true, ..Default::default() },
            dual_brush: crate::DualBrush { enabled: true, mode: crate::MaskMode::Normal, full_parameters: Some(Box::new(secondary.clone())), ..Default::default() },
            ..Default::default()
        };
        let fmt = PixelFormat::new(ColorMode::Rgb, SampleType::F32, true);
        let destination = [0.8, 0.6, 0.4, 1.0];
        let base = Surface::with_default(fmt, &destination);
        let mut renderer = StrokeRenderer::on_surface(&brush, &base, 1.0);
        let rect = Rect::new(0, 0, 1, 1);
        renderer.cov.accumulate_wet(rect, &[0.4], 1.0, None, None, Some((first.resolve(1.0, 20.0), None)), None);
        renderer.dual.as_mut().unwrap().accumulate_wet(rect, &[0.6], 1.0, None, None, Some((second.resolve(1.0, 20.0), None)), None);
        // The lane colour is the paint pipeline's; mixing and blur are separate surfaces weighted
        // per pixel, so with no mixing strength recorded the lane colour passes through untouched.
        let (_, edge_a, wet_a) = renderer.wet_pixel(false, 0, 0);
        let (_, edge_b, wet_b) = renderer.wet_pixel(true, 0, 0);
        let a = first.color(a, destination, edge_a, wet_a);
        let b = second.color(b, destination, edge_b, wet_b);
        let p = 0.4 * brush.dual_brush.primary_strength;
        let d = 0.6 * brush.dual_brush.secondary_strength;
        let weight = d / (p + d);
        // The reference blends the lanes' straight colours by weight, combines their masks with
        // the dual mode, and applies that once over the destination.
        let straight = |c: [f32; 4]| if c[3] > 0.0001 { [c[0] / c[3], c[1] / c[3], c[2] / c[3]] } else { [0.0; 3] };
        let (ac, bc) = (straight(a), straight(b));
        let alpha = crate::render::dual_blend(brush.dual_brush.mode, p.min(1.0), d.min(1.0)).clamp(0.0, 1.0);
        let expected: [f32; 4] = std::array::from_fn(|i| {
            let lane = if i < 3 { (ac[i] + (bc[i] - ac[i]) * weight) * alpha } else { alpha };
            destination[i] + (lane - destination[i]) * alpha
        });
        let actual = renderer.transported_output(destination, [0.0; 4], 0, 0, 1.0);
        for i in 0..4 {
            assert!((actual[i] - expected[i]).abs() < 1e-6, "{actual:?} != {expected:?}");
        }
    }

    #[test]
    fn the_live_renderer_only_mixes_after_the_start_pixels_are_attached() {
        // `new` has no surface, so it builds no transport lanes; the live painting path uses `new`
        // and must attach the pre-stroke pixels, or mixing never runs (the reported bug: the brush
        // painted pure foreground under a blue/red seam with mixing at 100%).
        let b = BrushSettings {
            size: 200.0,
            hardness: 1.0,
            spacing: 0.1,
            flow: 0.5,
            color: [0.1, 0.3, 0.9, 1.0],
            mix_settings: crate::transport::BrushMixSettings {
                enabled: true,
                strength: 1.0,
                algorithm: crate::transport::MixAlgorithm::V1,
                ..Default::default()
            },
            ..Default::default()
        };
        let fmt = PixelFormat::new(ColorMode::Rgb, SampleType::F32, true);
        let mut pre = Surface::with_default(fmt, &[1.0, 1.0, 1.0, 1.0]);
        for y in 0..120 {
            for x in 0..100 {
                pre.write_pixel(x, y, &[0.9, 0.15, 0.15, 1.0]);
            }
        }
        let points = [StrokePoint::new(60.0, 60.0, 1.0), StrokePoint::new(160.0, 60.0, 1.0)];
        let render = |attach: bool| {
            let mut r = StrokeRenderer::new(&b, Some(pre.format()), 1.0);
            if attach {
                r.attach_pre(&pre);
            }
            r.push(&points);
            r.finish();
            let mut target = pre.clone();
            r.composite(&pre, &mut target, None, false, true);
            (r.transports[0].is_some(), [60, 150].map(|x| target.rgba(x, 60)))
        };
        let (unattached_has_lanes, plain) = render(false);
        assert!(!unattached_has_lanes, "`new` must not fake a transport lane");
        // 100% mixing never paints the brush colour: every covered pixel carries the layer's own
        // colour, dragged along the stroke (red inside the red half, red still trailing in white).
        assert!(plain[0][2] < 0.9, "without a transport the stroke is the brush colour: {:?}", plain[0]);
        let (attached_has_lanes, mixed) = render(true);
        assert!(attached_has_lanes, "attaching the start pixels must create the lane");
        assert!(mixed[0][0] > 0.5 && mixed[0][2] < 0.6, "the dragged colour must replace the brush colour: {:?}", mixed[0]);
        assert!(mixed[1][0] > 0.5 && mixed[1][1] < 0.6, "the drag must follow into the white half: {:?}", mixed[1]);
    }

    #[test]
    fn saturated_alpha_still_accepts_the_next_dab_color() {
        let mut map = CoverageMap::new(3);
        let rect = Rect::new(0, 0, 1, 1);
        map.accumulate(rect, &[1.0], 1.0, Some(&[1.0, 0.0, 0.0]), None);
        map.accumulate(rect, &[1.0], 1.0, Some(&[0.0, 0.0, 1.0]), None);
        let tile = &map.tiles[&(0, 0)];
        assert_eq!(tile.cov[0], 1.0);
        assert_eq!(&tile.col[..3], &[0.0, 0.0, 1.0]);
    }

    #[test]
    fn c3_soft_tip_has_exact_gaussian_edge_and_sampled_hardness_only_removes_coverage() {
        let tail = (-2.5f32).exp();
        let expected = ((-2.5f32 * 0.25).exp() - tail) / (1.0 - tail);
        assert!((c3_tip_falloff(5.0, 10.0, 0.0, false) - expected).abs() < 1e-6);
        assert_eq!(c3_tip_falloff(10.0, 10.0, 0.0, false), 0.0);
        assert_eq!(c3_tip_falloff(10.1, 10.0, 1.0, true), 0.0);
        let tile = crate::GrayTile::from_fn(8, 8, |x, y| if x > 1 && x < 6 && y > 1 && y < 6 { 1.0 } else { 0.0 });
        let m = Mips::new(&tile);
        for y in 0..16 {
            for x in 0..16 {
                let u = (x as f32 + 0.5) / 16.0;
                let v = (y as f32 + 0.5) / 16.0;
                assert!(c3_sampled_mask(&m, u, v, 0.0) <= c3_sampled_mask(&m, u, v, 1.0) + 1e-6);
            }
        }
        assert_eq!(triangle_mask(9.0, 0.0, 10.0, 1.0, 1.0, false), 0.0);
        assert!(triangle_mask(0.0, 0.0, 10.0, 1.0, 1.0, false) > 0.9);
    }

    #[test]
    fn pigment_shading_is_incremental_and_selection_clipped() {
        use crate::pigment_material::PigmentSettings;
        let brush = BrushSettings {
            c3_tip_math: true,
            hardness: 0.6,
            color: [0.6, 0.2, 0.1, 1.0],
            pigment: Some(PigmentSettings {
                enabled: true,
                thickness: 80.0,
                visual_thickness: 70.0,
                highlight_strength: 0.6,
                liquidity: 0.4,
                ..Default::default()
            }),
            ..Default::default()
        };
        let fmt = PixelFormat::new(ColorMode::Rgb, SampleType::F32, true);
        let base = Surface::with_default(fmt, &[0.2, 0.3, 0.4, 1.0]);
        let mut renderer = StrokeRenderer::on_surface(&brush, &base, 1.0);
        let mut incremental = base.clone();
        let mut dabs = Vec::new();
        for x in [60.0, 64.0, 69.0] {
            let mut d = Dab::round(Point::new(x, 32.0), 9.0, 0.7);
            d.color = brush.color;
            d.hardness = brush.hardness;
            d.pigment = crate::pigment_material::ResolvedPigment {
                enabled: true,
                pressure: 1.0,
                thickness: 0.8,
                visual_thickness: 0.7,
                highlight_strength: 0.6,
                liquidity: 0.4,
                ..Default::default()
            };
            dabs.push(d);
        }
        for d in &dabs {
            renderer.dab_buf.push(*d);
            renderer.raster_pending();
            renderer.composite(&base, &mut incremental, None, false, false);
        }
        let mut whole = StrokeRenderer::on_surface(&brush, &base, 1.0);
        whole.dab_buf = dabs;
        whole.raster_pending();
        let mut target = base.clone();
        whole.composite(&base, &mut target, None, false, true);
        for y in 20..45 {
            for x in 48..82 {
                let a = incremental.rgba(x, y);
                let b = target.rgba(x, y);
                for ch in 0..4 {
                    assert!((a[ch] - b[ch]).abs() < 1e-5, "({x},{y}) {a:?} != {b:?}");
                }
            }
        }
        let sf = PixelFormat::new(ColorMode::Grayscale, SampleType::F32, false);
        let mut selection = Surface::new(sf);
        selection.write_pixel(64, 32, &[1.0]);
        let mut clipped = base.clone();
        whole.composite(&base, &mut clipped, Some(&selection), false, true);
        assert_eq!(clipped.rgba(63, 32), base.rgba(63, 32));
        assert_eq!(clipped.rgba(65, 32), base.rgba(65, 32));
        assert_ne!(clipped.rgba(64, 32), base.rgba(64, 32));
    }

    #[test]
    fn pigment_bilinear_sampling_interpolates_across_negative_tile_boundary() {
        let mut map = CoverageMap::new(0);
        map.accumulate(Rect::new(-1, 0, 1, 1), &[1.0, 1.0], 1.0, None, None);
        for ((tx, _), tile) in &mut map.tiles {
            tile.pigment = vec![[if *tx < 0 { 0.0 } else { 1.0 }; 4]; tile.cov.len()];
        }
        assert_eq!(map.pigment_sample(-0.25, 0.0), [0.75; 4]);
        assert_eq!(map.pigment_sample(-0.75, 0.0), [0.25; 4]);
    }

    #[test]
    fn wet_v4_pools_the_coverage_like_the_studio() {
        // ProCraft's wet edge: a = a·(1−s) + s·(1 − |sin(4.5a)/(4.5a)|). At full strength paint
        // laid at 1.0 settles to about 0.78 (pooling), and where it thins out at about 0.7 it comes
        // back to full — the rim.
        let pooled = |a: f32| {
            let x = 4.5 * a.max(1e-4);
            1.0 - (x.sin() / x).abs()
        };
        let settings = WetEdgeSettings { style: WetStyle::V4, strength: 2.0, ..Default::default() };
        let wet = settings.resolve(1.0, 40.0);
        let full = settings.alpha(1.0, 1.0, wet);
        assert!((full - pooled(1.0)).abs() < 1e-3, "{full}");
        assert!((full - 0.783).abs() < 0.002, "pooled to {full}");
        let rim = settings.alpha(0.7, 1.0, wet);
        assert!(rim > 0.99, "the rim comes back to full: {rim}");
        // Strength 0 leaves coverage alone.
        let off = WetEdgeSettings { strength: 0.0, ..settings.clone() };
        assert!((off.alpha(0.7, 1.0, off.resolve(0.0, 40.0)) - 0.7).abs() < 1e-6);
    }

    #[test]
    fn dual_brush_opacity_controls_mask_strength() {
        // Secondary Strength scales the dual coverage, Opacity the mask strength: both must be
        // set for the secondary tip to contribute at all.
        let brush = BrushSettings { dual_brush: crate::DualBrush { opacity: 0.25, secondary_strength: 1.0, ..Default::default() }, ..Default::default() };
        let ctx = BrushContext::new(&brush);
        let got = ctx.stroke_mask(0.8, Some(0.2), 0, 0);
        assert!((got - 0.64).abs() < 1e-6, "{got}");
    }

    #[test]
    fn material_single_filters_each_dab_before_accumulating_color() {
        for style in [WetStyle::Photoshop, WetStyle::Watercolor, WetStyle::Watercolor2, WetStyle::V4] {
            let settings = WetEdgeSettings { style, outline: WetOutline::Single, color_mode: WetColorMode::Hsl, ..Default::default() };
            let brush = BrushSettings { hardness: 0.2, wet_edges: true, wet_edge: Some(settings.clone()), ..Default::default() };
            let fmt = PixelFormat::new(ColorMode::Rgb, SampleType::F32, true);
            let base = Surface::with_default(fmt, &[0.8, 0.7, 0.6, 1.0]);
            let mut renderer = StrokeRenderer::on_surface(&brush, &base, 1.0);
            let mut a = Dab::round(Point::new(32.0, 32.0), 10.0, 0.5);
            a.color = [0.6, 0.3, 0.2, 1.0];
            a.wet_edge = settings.resolve(0.0, 20.0);
            a.wet_edge.strength = 0.0;
            let mut b = a;
            b.radius = 30.0;
            b.color = [0.2, 0.5, 0.4, 1.0];
            b.wet_edge = settings.resolve(1.0, 60.0);
            let rect = Rect::new(41, 32, 42, 33);
            let mut expected = [0.0; 3];
            let mut coverage = 0.0;
            for dab in [a, b] {
                let (mut alpha, mut edge) = (Vec::new(), Vec::new());
                renderer.ctx.rasterize_into(&dab, false, rect, &mut alpha, Some(&mut edge));
                let filtered = settings.color(dab.color, [0.8, 0.7, 0.6, 1.0], edge[0], dab.wet_edge);
                let next = coverage + alpha[0] * (dab.opacity - coverage);
                let weight = next.min(dab.opacity * alpha[0]);
                let retained = (next - weight).max(0.0);
                for ch in 0..3 {
                    expected[ch] = filtered[ch] * weight + if coverage > 0.0001 { expected[ch] / coverage * retained } else { 0.0 };
                }
                coverage = next;
            }
            renderer.dab_buf = vec![a, b];
            renderer.raster_pending();
            let tile = &renderer.cov.tiles[&(0, 0)];
            let offset = (32 * COV_TILE as usize + 41) * 3;
            for (got, want) in tile.col[offset..offset + 3].iter().zip(expected) {
                assert!((*got - want).abs() < 1e-6, "{style:?}: {got} != {want}");
            }
            let mut target = base.clone();
            renderer.composite(&base, &mut target, None, false, true);
            let actual = target.rgba(41, 32);
            for ch in 0..3 {
                let want = expected[ch] + [0.8, 0.7, 0.6][ch] * (1.0 - coverage);
                assert!((actual[ch] - want).abs() < 1e-6, "{style:?} composite channel {ch}");
            }
        }
    }
}
