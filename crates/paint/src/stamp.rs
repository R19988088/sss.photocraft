//! The dab-stamping seam: a resolved [`StampJob`] for one dab, and a [`DabStamper`] the app can
//! inject (the GPU kernel in `photocraft-gpu`). Painting never depends on the GPU: without a
//! stamper, or for a dab the stamper refuses, `BrushContext::rasterize_into` runs the CPU path
//! exactly as before.
//!
//! The job carries the dab-invariant geometry too (`ro`, `rm`, `reach`, …) because the CPU and the
//! GPU must agree on the reach boundary to the last bit: one f32 ulp of `d²` there decides whether
//! a rim pixel is painted, so the geometry is evaluated once, here, with the rasteriser's own
//! expressions rather than twice with two roundings.

use std::fmt;
use std::sync::Arc;

use photocraft_geom::Rect;

use crate::Dab;

/// A dab, flattened for a stamper: the numbers, and no reference to the brush it came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StampJob {
    /// Dab centre in document pixels; already snapped for an aliased tip (`grid_center`).
    pub center: [f32; 2],
    pub radius: f32,
    /// Radians. Positive turns clockwise on screen under `c3_tip_math`.
    pub angle: f32,
    pub roundness: f32,
    /// Flow: per-dab build-up amount, already part of the coverage a stamper returns.
    pub alpha: f32,
    /// The hardness this dab resolved to (dynamic / pigment / contour override).
    pub hardness: f32,
    pub flip: [f32; 2],
    /// Brush Projection: 1 = none.
    pub proj_scale: f32,
    pub proj_angle: f32,
    pub c3_tip: bool,
    pub aliased: bool,
    /// The Eraser's Block mode.
    pub square: bool,
    pub triangle: bool,
    /// Single-dab Wet Edges: the coverage is shaped by the rim and the depth goes to `depth`.
    pub wet: bool,
    /// The dab's own texture depth (Texture › Depth, jittered per tip): what
    /// `texture.combine` takes, not the brush's nominal setting.
    pub texture_depth: f32,
    /// A sampled tip's level-0 scale (`tw / (2 r)`, see `rasterize_into`) and the mip level it
    /// samples: zero when the brush's tip is computed, not a bitmap.
    pub tip_inv_scale: f32,
    pub tip_level: u32,
    /// The dab-invariant geometry (see the module comment).
    pub geometry: StampGeometry,
    /// The dab's own straight colour: colour dynamics make every dab's its own, and the
    /// backend's colour lane averages them over a folded frame.
    pub color: [f32; 4],
}

/// The per-dab constants every pixel of a dab shares, evaluated with `rasterize_into`'s own
/// expressions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StampGeometry {
    /// Roundness as the tip maths uses it (clamped).
    pub ro: f32,
    /// `radius × ro`, floored at half a pixel.
    pub rm: f32,
    /// How far the tip paints before it is cut off.
    pub reach: f32,
    pub reach_squared: f32,
    /// A tip a few pixels across takes each pixel's covered area (a 4×4 grid inside it).
    pub small: bool,
    pub small_reach_squared: f32,
}

/// A dab-stamping backend. `false` from [`stamp`](DabStamper::stamp) means "not handled, use the
/// CPU" — never "stamped nothing".
pub trait DabStamper: Send + Sync {
    /// Stamp `job` over `rect` into `coverage` (row-major, `rect.width() × rect.height()` floats),
    /// and into `depth` when given. Buffers arrive already sized and zeroed.
    fn stamp(&self, job: &StampJob, rect: Rect, coverage: &mut Vec<f32>, depth: Option<&mut Vec<f32>>) -> bool;
}

/// Where a [`BrushContext`](crate::render::BrushContext) keeps its stamper. A newtype so the
/// context keeps its `Debug` (a trait object need not be printable).
#[derive(Clone, Default)]
pub struct StamperSlot(Option<Arc<dyn DabStamper>>);

impl StamperSlot {
    pub fn new(stamper: Arc<dyn DabStamper>) -> Self {
        Self(Some(stamper))
    }

    pub fn stamper(&self) -> Option<&dyn DabStamper> {
        self.0.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }
}

impl fmt::Debug for StamperSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() { "StamperSlot(gpu)" } else { "StamperSlot(none)" })
    }
}

/// The stamper every new `BrushContext` starts with. The app installs the GPU one here once its
/// canvas runs on the GPU, so strokes rendered anywhere in the process (the engine, the UI, a
/// headless render) stamp on the GPU without those crates depending on `photocraft-gpu`. Empty by
/// default: paint alone is all-CPU, and a stamp that can't run falls back per dab.
static PROCESS_STAMPER: std::sync::RwLock<StamperSlot> = std::sync::RwLock::new(StamperSlot(None));

/// Installs (or clears, with an empty slot) the process-wide stamper. Existing contexts keep the
/// stamper they were made with.
pub fn set_process_stamper(slot: StamperSlot) {
    *PROCESS_STAMPER.write().unwrap_or_else(std::sync::PoisonError::into_inner) = slot;
}

/// The process-wide stamper, cloned for a new context.
pub fn process_stamper() -> StamperSlot {
    PROCESS_STAMPER.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

impl StampJob {
    /// A job for `d` under `brush`'s settings, or `None` for a dab only the CPU rasterises
    /// (sampled tips, per-tip texture, Noise, Grain, single-dab Wet Edges outline, non-finite
    /// fields).
    pub(crate) fn of(d: &Dab, brush: &crate::render::BrushContext, with_depth: bool) -> Option<Self> {
        Self::of_with(d, brush, with_depth, false, false)
    }

    /// As [`of`](Self::of), but `allow_texture` lets a per-tip textured dab through and
    /// `allow_tip` a sampled-bitmap tip, for a backend that samples them itself (the GPU paint
    /// surface does both).
    pub(crate) fn of_with(d: &Dab, brush: &crate::render::BrushContext, with_depth: bool, allow_texture: bool, allow_tip: bool) -> Option<Self> {
        let b = &brush.brush;
        let sampled = brush.sampled_tip();
        if (sampled && !allow_tip) || b.noise || b.grain.is_some() || (brush.per_tip_texture() && !allow_texture) {
            return None;
        }
        if brush.single_wet_outline() {
            return None;
        }
        let finite = [d.radius, d.angle, d.roundness, d.alpha, d.hardness, d.proj_scale, d.proj_angle].iter().all(|v| v.is_finite());
        if !finite || b.hardness.is_nan() {
            return None;
        }
        // The hardness this dab resolves to, as `rasterize_into` decides it.
        let hardness = if b.hardness_dynamic.is_active() || b.pigment.is_some() || b.contour.is_some() { d.hardness } else { b.hardness };
        let aliased = b.aliased;
        let (cx, cy) = if aliased { crate::grid_center(d.center.x, d.center.y, 2.0 * d.radius) } else { (d.center.x as f32, d.center.y as f32) };
        if !cx.is_finite() || !cy.is_finite() {
            return None;
        }
        let (fx, fy) = (if d.flip_x { -1.0 } else { 1.0 }, if d.flip_y { -1.0 } else { 1.0 });
        let r = d.radius;
        // The four lines `rasterize_into` uses, in the same order: C3 clamps roundness at 1/100
        // and reaches a pixel past the scaled radius; the legacy tip is clamped by a half pixel.
        let ro = if b.c3_tip_math { d.roundness.max(0.01) } else { d.roundness.max(0.5 / r).min(1.0) };
        let rm = (r * ro).max(0.5);
        let reach = if b.c3_tip_math {
            rm + 1.0
        } else if aliased || sampled {
            rm
        } else {
            crate::tip_reach(rm, hardness).max(rm)
        };
        let reach_squared = (reach + 0.5) * (reach + 0.5);
        let small = !sampled && !aliased && if b.c3_tip_math { r * 2.0 < 5.0 } else { rm < SMALL_TIP_RADIUS };
        let small_reach_squared = (reach + 0.75) * (reach + 0.75);
        let wet = b.wet_edges && b.wet_edge.is_none() && !with_depth;
        // A sampled tip draws its bitmap at `2 r` pixels across: level 0's scale and the mip
        // level `rasterize_into` picks (`Mips::level_for`).
        let (tip_inv_scale, tip_level) = if sampled { brush.tip_scale_and_level(r, 2.0 * r * ro.max(0.25)) } else { (0.0, 0) };
        Some(Self {
            center: [cx, cy],
            radius: r,
            angle: d.angle,
            roundness: d.roundness,
            alpha: d.alpha,
            hardness,
            flip: [fx, fy],
            proj_scale: d.proj_scale,
            proj_angle: d.proj_angle,
            c3_tip: b.c3_tip_math,
            aliased,
            square: b.square,
            triangle: b.triangle,
            wet,
            texture_depth: d.depth.clamp(0.0, 1.0),
            tip_inv_scale,
            tip_level,
            geometry: StampGeometry { ro, rm, reach, reach_squared, small, small_reach_squared },
            color: d.color,
        })
    }
}

/// Below this radius a computed tip is rasterised by area, not at pixel centres.
use crate::render::SMALL_TIP_RADIUS;

// ---------------------------------------------------------------------------
// Batch masks: a whole frame of dabs laid down by one backend (the GPU)
// ---------------------------------------------------------------------------

/// The stroke's texture, as a mask backend takes it: the pattern image and the numbers
/// `texture_at_point` / `Texture::combine` fold into every covered pixel.
#[derive(Clone, Debug, Default)]
pub struct MaskTexture {
    /// The prepared, levelled pattern (`None` = the stroke has no texture).
    pub image: Option<std::sync::Arc<crate::tile::PatternImage>>,
    /// The combine mode, numbered as [`MaskMode::code`](crate::brush::MaskMode::code).
    pub mode: u32,
    /// 0 canvas-locked, 1 stroke-locked, 2 tip-local.
    pub coordinate: u32,
    pub scale: f32,
    /// The brush's nominal depth; each dab carries its own (`StampJob::texture_depth`).
    pub depth: f32,
    pub c3_math: bool,
    pub each_tip: bool,
    /// The stroke-locked origin: the stroke's first dab centre.
    pub origin: [f32; 2],
}

/// A stroke's tip bitmap as a mask backend takes it: level 0 of the prepared pyramid, 8-bit (the
/// samples Procreate and Photoshop ship, so the round trip is exact).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaskTip {
    pub width: u32,
    pub height: u32,
    /// Row-major, one byte a texel.
    pub data: Vec<u8>,
}

/// A batch of dabs a mask backend lays down in one submit, in submission order.
#[derive(Clone, Debug, Default)]
pub struct MaskBatch {
    pub jobs: Vec<StampJob>,
    /// Each job's rectangle in document pixels (the extent its mask must cover).
    pub rects: Vec<Rect>,
    pub texture: MaskTexture,
    /// The brush's sampled tip, when its dabs carry one.
    pub tip: Option<std::sync::Arc<MaskTip>>,
    /// Ask for the Wet Edges depth lane as well: a stroke without wet edges never reads it, and the
    /// lane is a second readback the backend would otherwise always pay.
    pub want_depth: bool,
}

/// One dab's masks, row-major over its rectangle.
#[derive(Clone, Debug, Default)]
pub struct MaskDab {
    /// `val · alpha`: exactly what `rasterize_into` leaves in its coverage buffer.
    pub coverage: Vec<f32>,
    /// `1 − rn`: the coverage map's depth lane (Wet Edges).
    pub depth: Vec<f32>,
}

/// One lane's share of a [`MaskFrame`]: the primary brush, or the Dual Brush's secondary one.
/// A dual brush lays both lanes in the same frame, each folded over its own union.
#[derive(Clone, Debug, Default)]
pub struct FrameLane {
    pub jobs: Vec<StampJob>,
    pub rects: Vec<Rect>,
    /// The rectangle this lane covers (the union of `rects`); its result is this big.
    pub union: Rect,
    /// The coverage ceiling every dab of this lane shares (`Dab::opacity` × the lane's opacity).
    pub ceil: f32,
    pub texture: MaskTexture,
    pub tip: Option<std::sync::Arc<MaskTip>>,
    /// Ask for the lane's colour too: every dab brings its own (colour dynamics), so the folded
    /// frame's per-pixel colour is the coverage-weighted average of them.
    pub want_color: bool,
}

/// A frame of dabs a backend folds **as one**, so the engine's per-dab readback disappears: the
/// backend accumulates them on the GPU and hands back the rectangle they cover, already folded.
#[derive(Clone, Debug, Default)]
pub struct MaskFrame {
    /// The primary lane, always present.
    pub lane: FrameLane,
    /// The Dual Brush's lane, when the stroke has one.
    pub dual: Option<FrameLane>,
}

/// One lane's folded coverage and depth, row-major over that lane's union.
#[derive(Clone, Debug, Default)]
pub struct LaneMasks {
    /// `ceil × (1 − Π(1 − v))`: what the lane's dabs leave over an empty pixel, so the engine can
    /// fold it into its own map with the same `c + v·(ceil − c)` law.
    pub coverage: Vec<f32>,
    /// `max(1 − rn)` over the lane's dabs.
    pub depth: Vec<f32>,
    /// Straight rgb per pixel when the lane asked for it ([`FrameLane::want_color`]): the
    /// coverage-weighted average of every dab's own colour.
    pub color: Vec<f32>,
}

/// One frame's folded lanes.
#[derive(Clone, Debug, Default)]
pub struct MaskFrameOut {
    pub coverage: Vec<f32>,
    pub depth: Vec<f32>,
    /// Straight rgb per pixel, when [`FrameLane::want_color`] asked for it; empty otherwise.
    pub color: Vec<f32>,
    /// The Dual Brush's lane, when the frame carried one.
    pub dual: Option<LaneMasks>,
}

/// A backend that lays a whole batch of dabs' masks down at once (on the GPU), so the engine's
/// per-dab pixel loops — the tip maths and the pattern sampling — never run on the CPU.
///
/// `None` from [`masks`](DabMaskBatch::masks) means "keep to the CPU", never "nothing painted":
/// the stroke then falls back to `rasterize_into` for that batch.
pub trait DabMaskBatch: Send + Sync {
    fn masks(&self, batch: &MaskBatch) -> Option<Vec<MaskDab>>;

    /// The frame folded as one rectangle, for a brush whose dabs share one ceiling and carry
    /// nothing per-dab (no pigment, mixing, pattern, per-pixel colour). Default: not supported.
    fn frame(&self, _frame: &MaskFrame) -> Option<MaskFrameOut> {
        None
    }
}

/// The pixels a smudge stroke works on, and the dabs that move them.
pub struct TransportFrame<'a> {
    /// The document rectangle `pixels` covers.
    pub region: Rect,
    /// Native channels per pixel; `pixels` is row-major over `region`.
    pub channels: usize,
    /// Index of the alpha channel in a pixel, when the lane's format has one.
    pub alpha: Option<usize>,
    /// The lane's working pixels, which the backend rewrites in place.
    pub pixels: &'a mut Vec<f32>,
    /// This frame's dabs, in submission order.
    pub dabs: &'a [TransportDab],
}

/// One dab's transport work: the carrier the reference builds and lays, plus its coverage.
#[derive(Clone, Debug)]
pub struct TransportDab {
    /// The dab's rectangle in document pixels; `coverage` is row-major over it.
    pub rect: Rect,
    pub center: [f64; 2],
    pub radius: f32,
    pub mix_strength: f32,
    pub concentration: f32,
    pub output_strength: f32,
    /// How much of the previous carrier this dab keeps (`keep` in the reference).
    pub keep: f32,
    /// The previous carrier's radius as a fraction of this dab's (1 when it was the same).
    pub scale: f32,
    /// Whether a previous carrier exists at all (false on a stroke's first dab).
    pub has_previous: bool,
    /// `val · alpha`, as `rasterize_into` leaves it.
    pub coverage: Vec<f32>,
}

/// A backend that moves a lane's pixels through a whole frame's dabs at once (on the GPU), so the
/// per-dab resampling never runs on the CPU.
///
/// `false` from [`transport`](DabTransport::transport) means "keep to the CPU": the caller has not
/// given up its own pixels and does the dabs itself.
pub trait DabTransport: Send + Sync {
    fn transport(&self, frame: &mut TransportFrame) -> bool;
}

/// Where a [`BrushContext`](crate::render::BrushContext) keeps its transport backend.
#[derive(Clone, Default)]
pub struct TransportSlot(Option<std::sync::Arc<dyn DabTransport>>);

impl TransportSlot {
    pub fn new(transport: std::sync::Arc<dyn DabTransport>) -> Self {
        Self(Some(transport))
    }

    pub fn transport(&self) -> Option<&dyn DabTransport> {
        self.0.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }
}

impl fmt::Debug for TransportSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() { "TransportSlot(gpu)" } else { "TransportSlot(none)" })
    }
}

/// The transport backend every new context starts with (empty: paint alone is all-CPU).
static PROCESS_TRANSPORT: std::sync::RwLock<TransportSlot> = std::sync::RwLock::new(TransportSlot(None));

/// Installs (or clears, with an empty slot) the process-wide transport backend.
pub fn set_process_transport(slot: TransportSlot) {
    *PROCESS_TRANSPORT.write().unwrap_or_else(std::sync::PoisonError::into_inner) = slot;
}

/// The process-wide transport backend a new context takes.
pub fn process_transport() -> TransportSlot {
    PROCESS_TRANSPORT.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// Where a [`BrushContext`](crate::render::BrushContext) keeps its batch mask backend.
#[derive(Clone, Default)]
pub struct MaskBatchSlot(Option<std::sync::Arc<dyn DabMaskBatch>>);
impl MaskBatchSlot {
    pub fn new(batch: std::sync::Arc<dyn DabMaskBatch>) -> Self {
        Self(Some(batch))
    }

    pub fn batch(&self) -> Option<&dyn DabMaskBatch> {
        self.0.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }
}

impl fmt::Debug for MaskBatchSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() { "MaskBatchSlot(gpu)" } else { "MaskBatchSlot(none)" })
    }
}

/// The batch backend every new context starts with (empty: paint alone is all-CPU).
static PROCESS_MASK_BATCH: std::sync::RwLock<MaskBatchSlot> = std::sync::RwLock::new(MaskBatchSlot(None));

/// Installs (or clears, with an empty slot) the process-wide batch mask backend. Contexts made
/// before this keep the one they were given.
pub fn set_process_mask_batch(slot: MaskBatchSlot) {
    *PROCESS_MASK_BATCH.write().unwrap_or_else(std::sync::PoisonError::into_inner) = slot;
}

/// The process-wide batch backend, cloned for a new context.
pub fn process_mask_batch() -> MaskBatchSlot {
    PROCESS_MASK_BATCH.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}
