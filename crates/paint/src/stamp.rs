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
    /// The dab-invariant geometry (see the module comment).
    pub geometry: StampGeometry,
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
        let b = &brush.brush;
        if brush.sampled_tip() || b.noise || b.grain.is_some() || brush.per_tip_texture() {
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
        } else if aliased {
            rm
        } else {
            crate::tip_reach(rm, hardness).max(rm)
        };
        let reach_squared = (reach + 0.5) * (reach + 0.5);
        let small = !aliased && if b.c3_tip_math { r * 2.0 < 5.0 } else { rm < SMALL_TIP_RADIUS };
        let small_reach_squared = (reach + 0.75) * (reach + 0.75);
        let wet = b.wet_edges && b.wet_edge.is_none() && !with_depth;
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
            geometry: StampGeometry { ro, rm, reach, reach_squared, small, small_reach_squared },
        })
    }
}

/// Below this radius a computed tip is rasterised by area, not at pixel centres.
use crate::render::SMALL_TIP_RADIUS;
