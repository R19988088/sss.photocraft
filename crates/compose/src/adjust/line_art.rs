//! C3's line art (「线条强化」): ink lines grown along the edges of the artwork below.
//!
//! Ported from `Sources/Shaders/C2AdjustmentShaders.metal` (`webApplyLineArt`): for every pixel
//! the pass looks at a ring of neighbours within `thickness + softEdge`, measures how much their
//! tone and colour differ from the centre, and turns that difference into ink with the zone
//! opacity of the centre's own tone (shadows / midtones / highlights). The ink then meets the
//! artwork with one of C3's four blend modes.

use photocraft_doc::adjust::LineArtBlend;

use crate::Buffer;
use photocraft_geom::Rect;

/// C3's own defaults for the pass: what a new 线条强化 layer starts as, and what the Properties
/// panel's double-click resets each row to. They are the payload defaults C3 writes into an
/// `.sssp` project (`LineArt` layer JSON: intensity 100, sharpness 100, thickness 13,
/// shadows/midtones/highlights 24/32/90, black ink, multiply), so a layer created here and one
/// created in C3 look the same.
pub const DEFAULT_INTENSITY: f32 = 100.0;
pub const DEFAULT_SHARPNESS: f32 = 100.0;
pub const DEFAULT_THICKNESS: f32 = 13.0;
pub const DEFAULT_SHADOWS: f32 = 24.0;
pub const DEFAULT_MIDTONES: f32 = 32.0;
pub const DEFAULT_HIGHLIGHTS: f32 = 90.0;
/// Thickness range in pixels. C3's own default (13) sits inside it — an earlier `0.1..=10` did
/// not, so every new layer was silently clamped to 10 and no longer matched C3.
pub const THICKNESS_MIN: f32 = 0.1;
pub const THICKNESS_MAX: f32 = 30.0;
/// C3's `sampleRadius = 10`: the ring never reads farther than this, whatever the thickness. The
/// thickness itself still shapes the weights above it, as in C3 (`op.p0.z` there, here the ring
/// table), so a thicker line is softer rather than unreachable.
pub const MAX_RADIUS: f32 = 10.0;

/// Rec. 709 luma weights, as C3's shader spells them.
fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn zone_weights(tone: f32) -> [f32; 3] {
    let t = clamp01(tone);
    [clamp01((0.5 - t) * 2.0), clamp01(1.0 - (t - 0.5).abs() * 2.0), clamp01((t - 0.5) * 2.0)]
}

/// C3's `lineArtSampleWeight`: samples right next to the centre count less, and the reach fades
/// out over a soft edge, so a line has a body and a soft end.
fn sample_weight(distance: f32, thickness: f32) -> f32 {
    let reach = thickness.max(0.1);
    let soft_edge = 1.35f32.max(reach * 0.42);
    let radial = 1.0 - smoothstep(reach, reach + soft_edge, distance);
    let center_fade = smoothstep(0.12, 0.45f32.max(reach * 0.28), distance);
    clamp01(radial * center_fade)
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if edge0 >= edge1 {
        return if value < edge0 { 0.0 } else { 1.0 };
    }
    let t = clamp01((value - edge0) / (edge1 - edge0));
    t * t * (3.0 - 2.0 * t)
}

/// Bilinear sample of the source pixels, the way C3's `webSampleLinearPixel` reads its texture.
/// Kept for reference: every ring tap here lands on a whole pixel, so the field reads pixels.
#[allow(dead_code)]
fn sample(source: &[[f32; 4]], width: usize, height: usize, x: f32, y: f32) -> [f32; 3] {
    if width == 0 || height == 0 {
        return [0.0; 3];
    }
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let x0 = x0 as i64;
    let y0 = y0 as i64;
    let at = |x: i64, y: i64| -> [f32; 3] {
        let x = x.clamp(0, width as i64 - 1) as usize;
        let y = y.clamp(0, height as i64 - 1) as usize;
        let p = source[y * width + x];
        [p[0], p[1], p[2]]
    };
    let top_left = at(x0, y0);
    let top_right = at(x0 + 1, y0);
    let bottom_left = at(x0, y0 + 1);
    let bottom_right = at(x0 + 1, y0 + 1);
    std::array::from_fn(|i| {
        let top = top_left[i] + (top_right[i] - top_left[i]) * tx;
        let bottom = bottom_left[i] + (bottom_right[i] - bottom_left[i]) * tx;
        top + (bottom - top) * ty
    })
}

/// The parameters [`Adjustment::LineArt`] folds into one pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineArtParams {
    pub intensity: f32,
    pub sharpness: f32,
    pub thickness: f32,
    pub shadows: f32,
    pub midtones: f32,
    pub highlights: f32,
    pub color: [f32; 3],
    pub blend: LineArtBlend,
}

impl LineArtParams {
    /// C3's own defaults, for the panel and tests.
    pub fn defaults() -> Self {
        Self {
            intensity: DEFAULT_INTENSITY,
            sharpness: DEFAULT_SHARPNESS,
            thickness: DEFAULT_THICKNESS,
            shadows: DEFAULT_SHADOWS,
            midtones: DEFAULT_MIDTONES,
            highlights: DEFAULT_HIGHLIGHTS,
            color: [0.0, 0.0, 0.0],
            blend: LineArtBlend::Multiply,
        }
    }
}

/// The edge strength (`maxDiff`) of every pixel: what C3 precomputes into its cached field.
///
/// It depends on the source pixels and the thickness alone, so intensity, sharpness, the three
/// zone opacities, the line colour and the blend mode all reuse one field (that is what makes
/// C3's sliders cheap, and what lets the GPU apply it pointwise with no neighbourhood reads).
#[derive(Clone, Debug)]
pub struct Field {
    pub rect: Rect,
    /// `maxDiff`, row-major over `rect`, in 0..1.
    pub max_diff: Vec<f32>,
}

impl Field {
    /// The field value at a document coordinate (0 outside the field).
    pub fn at(&self, x: i32, y: i32) -> f32 {
        let (dx, dy) = (x - self.rect.x0, y - self.rect.y0);
        let (w, h) = (self.rect.width() as i32, self.rect.height() as i32);
        if dx < 0 || dy < 0 || dx >= w || dy >= h {
            return 0.0;
        }
        self.max_diff[(dy * w + dx) as usize]
    }
}

/// C3's `maxDiff` for the artwork in `buf` (the region being drawn), reading its neighbours from
/// the same buffer, so a visible region carries real neighbours on both sides of every tile seam.
///
/// This is the expensive half of the pass (one ring of up to `MAX_RADIUS²` taps per pixel), which
/// is why it is cached: see `Compositor::sync_fields` on the GPU path and `Ctx::set_line_field` on
/// the CPU path. Everything downstream of it is a pointwise read, so the appearance sliders cost
/// one multiply per pixel instead of a ring.
pub fn field(buf: &Buffer, thickness: f32) -> Field {
    use rayon::prelude::*;

    let (width, height) = (buf.rect.width().max(1) as usize, buf.rect.height().max(1) as usize);
    let source = &buf.px;
    let ring = ring(thickness);
    let radius = ring.radius.max(0) as usize;
    // Each pixel's luma once: the ring reads it for its centre and for every one of its taps, so
    // computing it inside the tap loop repeated the same weighted sum hundreds of times per pixel.
    let luma: Vec<f32> = source.iter().map(|p| luma([p[0], p[1], p[2]])).collect();
    let mut max_diff = vec![0.0f32; width * height];
    max_diff.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        // A row (or a column) closer to the edge than the ring has to clamp its taps.
        let inside_row = y >= radius && y + radius < height;
        for (x, out) in row.iter_mut().enumerate() {
            let p = source[y * width + x];
            if p[3] <= 0.0 {
                continue;
            }
            let c = [p[0], p[1], p[2]];
            let center_luma = luma[y * width + x];
            let inside = inside_row && x >= radius && x + radius < width;
            let mut accumulated = 0.0;
            for (ox, oy, weight) in &ring.offsets {
                let at = if inside {
                    // Inside the ring's reach of every edge: no clamp, no bounds work.
                    ((y as i32 + *oy) as usize) * width + ((x as i32 + *ox) as usize)
                } else {
                    let sx = (x as i32 + *ox).clamp(0, width as i32 - 1) as usize;
                    let sy = (y as i32 + *oy).clamp(0, height as i32 - 1) as usize;
                    sy * width + sx
                };
                accumulated += tap_value(center_luma, c, luma[at], source[at], *weight);
            }
            *out = clamp01(accumulated / ring.weight_sum.max(1e-5));
        }
    });
    Field { rect: buf.rect, max_diff }
}

/// One ring tap's contribution: C3's `max(|Δluma| * 2.4, |Δcolour| * 1.45)`, capped at 1, times
/// the tap's weight.
///
/// The colour difference's square root is the pass's dearest step, so it is skipped whenever the
/// luma term already decides the result — exactly, by comparing the squares with a margin wide
/// enough that rounding cannot change which of the two is larger.
#[inline]
fn tap_value(center_luma: f32, center: [f32; 3], neighbor_luma: f32, neighbor: [f32; 4], weight: f32) -> f32 {
    let luma_term = (neighbor_luma - center_luma).abs() * 2.4;
    if luma_term >= 1.0 {
        // `max(luma, chroma).min(1)` is 1 whatever the colour difference is.
        return weight;
    }
    let d = [neighbor[0] - center[0], neighbor[1] - center[1], neighbor[2] - center[2]];
    let chroma_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    // (1.45 · |Δ|)² = 2.1025 · chroma_sq: below this the luma term wins and the root is not needed.
    if chroma_sq * 2.1025 <= luma_term * luma_term * 0.9999 {
        return luma_term * weight;
    }
    (chroma_sq.sqrt() * 1.45).max(luma_term).min(1.0) * weight
}

/// C3's `lineArtSampleTable`: the ring's offsets and weights, once per thickness. Every tap within
/// `thickness + softEdge` that also falls inside C3's `sampleRadius` carries a weight, and the sum
/// normalises `maxDiff` back into 0..1.
struct Ring {
    offsets: Vec<(i32, i32, f32)>,
    weight_sum: f32,
    /// The ring's box radius in pixels (at most [`MAX_RADIUS`]).
    radius: i32,
}

/// How far the ring reaches from a pixel, in whole pixels: C3's `min(ceil(thickness + softEdge),
/// sampleRadius)`.
pub fn radius(thickness: f32) -> u32 {
    let thickness = thickness.max(THICKNESS_MIN);
    let soft_edge = 1.35f32.max(thickness * 0.42);
    (thickness + soft_edge).ceil().min(MAX_RADIUS) as u32
}

fn ring(thickness: f32) -> Ring {
    let thickness = thickness.max(THICKNESS_MIN);
    let soft_edge = 1.35f32.max(thickness * 0.42);
    let outer_reach = thickness + soft_edge;
    let outer_squared = outer_reach * outer_reach;
    let radius = radius(thickness) as i32;
    let mut offsets = Vec::with_capacity(((2 * radius + 1) * (2 * radius + 1)) as usize);
    let mut weight_sum = 0.0;
    for oy in -radius..=radius {
        for ox in -radius..=radius {
            if ox == 0 && oy == 0 {
                continue;
            }
            let distance_squared = (ox * ox + oy * oy) as f32;
            if distance_squared >= outer_squared {
                continue;
            }
            let weight = sample_weight(distance_squared.sqrt(), thickness);
            if weight == 0.0 {
                continue;
            }
            offsets.push((ox, oy, weight));
            weight_sum += weight;
        }
    }
    Ring { offsets, weight_sum, radius }
}

/// The ring's normalising total for `thickness`: the sum of C3's sample weights, i.e. what
/// [`field`] divides the accumulated taps by. `fs_line_field` takes it as `op.p0.w` instead of
/// re-deriving it for every pixel of the pass.
pub fn ring_weight_sum(thickness: f32) -> f32 {
    ring(thickness).weight_sum.max(1e-5)
}

/// Grow ink along the edges of `buf` from a [`Field`]: the cheap part C3 reruns while the
/// intensity, sharpness, zone, colour and blend sliders move.
pub fn apply_field(buf: &mut Buffer, field: &Field, params: &LineArtParams) {
    let intensity = (params.intensity.clamp(0.0, 300.0)) / 100.0;
    if intensity <= 1e-6 {
        return;
    }
    let sharpness = (params.sharpness.clamp(0.0, 100.0)) / 100.0;
    let zone_opacity = [clamp01(params.shadows / 100.0), clamp01(params.midtones / 100.0), clamp01(params.highlights / 100.0)];
    let line_color = [clamp01(params.color[0]), clamp01(params.color[1]), clamp01(params.color[2])];
    let blend = params.blend;
    let width = buf.rect.width().max(1) as usize;
    let edge_soft = (0.085 - sharpness.powf(1.35) * 0.082).max(1e-5);
    // A field built for the whole render rect covers every tile of it; one built for this buffer
    // exactly is read straight. (Only a field narrower than the buffer would be a bug.)
    debug_assert!(field.rect.contains_rect(&buf.rect), "the field must cover the buffer it inks");
    let direct = field.rect == buf.rect && field.max_diff.len() == buf.px.len();
    for (i, p) in buf.px.iter_mut().enumerate() {
        if p[3] <= 0.0 {
            continue;
        }
        let c = [p[0], p[1], p[2]];
        let weights = zone_weights(luma(c));
        let opacity = clamp01(weights[0] * zone_opacity[0] + weights[1] * zone_opacity[1] + weights[2] * zone_opacity[2]);
        if opacity <= 1e-6 {
            continue;
        }
        let max_diff = if direct {
            field.max_diff.get(i).copied().unwrap_or(0.0)
        } else {
            let (col, row) = ((i % width) as i32, (i / width) as i32);
            field.at(buf.rect.x0 + col, buf.rect.y0 + row)
        };
        let edge_mask = smoothstep(0.06, 0.06 + edge_soft, max_diff);
        let alpha = clamp01(edge_mask * (max_diff * 1.2).clamp(0.0, 1.0) * intensity * opacity);
        if alpha <= 0.0 {
            continue;
        }
        let blended = match blend {
            LineArtBlend::Normal => line_color,
            LineArtBlend::Multiply => [c[0] * line_color[0], c[1] * line_color[1], c[2] * line_color[2]],
            LineArtBlend::Screen => [
                1.0 - (1.0 - c[0]) * (1.0 - line_color[0]),
                1.0 - (1.0 - c[1]) * (1.0 - line_color[1]),
                1.0 - (1.0 - c[2]) * (1.0 - line_color[2]),
            ],
            LineArtBlend::Overlay => std::array::from_fn(|ch| {
                if c[ch] >= 0.5 {
                    1.0 - 2.0 * (1.0 - c[ch]) * (1.0 - line_color[ch])
                } else {
                    2.0 * c[ch] * line_color[ch]
                }
            }),
        };
        for ch in 0..3 {
            p[ch] = clamp01(c[ch] + (blended[ch] - c[ch]) * alpha);
        }
    }
}

/// Grow ink along the edges of `buf`. `buf` is read as the source and written in place, so the
/// neighbouring samples always see the artwork below, as C3 does.
/// Applies line art to `buf` from the render's own field when it covers the buffer (a field built
/// for the whole render rect, so tile and band edges cannot cut a pixel's neighbours), and from a
/// field of this buffer otherwise — the per-buffer [`apply`].
pub fn apply_shared(buf: &mut Buffer, params: &LineArtParams, field: Option<&Field>) {
    match field {
        Some(f) if f.rect.contains_rect(&buf.rect) => apply_field(buf, f, params),
        _ => apply(buf, params),
    }
}

pub fn apply(buf: &mut Buffer, params: &LineArtParams) {
    if (params.intensity.clamp(0.0, 300.0)) <= 1e-4 {
        return;
    }
    let f = field(buf, params.thickness);
    apply_field(buf, &f, params);
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_geom::Rect;

    fn edge_buffer() -> Buffer {
        let mut px = vec![[1.0, 1.0, 1.0, 1.0]; 32 * 32];
        for y in 0..32 {
            for x in 16..32 {
                px[y * 32 + x] = [0.05, 0.05, 0.05, 1.0];
            }
        }
        Buffer { rect: Rect { x0: 0, y0: 0, x1: 32, y1: 32 }, px }
    }

    #[test]
    fn ink_grows_on_the_edge_and_leaves_flat_areas_alone() {
        let mut buf = edge_buffer();
        let before = buf.px.clone();
        apply(&mut buf, &LineArtParams::defaults());
        // The border between the halves darkens; the far corners do not.
        let border = buf.px[16 * 32 + 15][0];
        let corner = buf.px[0][0];
        assert!(border < before[16 * 32 + 15][0] - 0.05, "the edge takes ink: {border}");
        assert!((corner - before[0][0]).abs() < 1e-6, "a flat corner stays flat: {corner}");
    }

    #[test]
    fn zero_intensity_is_a_pass_through() {
        let mut buf = edge_buffer();
        let before = buf.px.clone();
        let params = LineArtParams { intensity: 0.0, ..LineArtParams::defaults() };
        apply(&mut buf, &params);
        assert_eq!(buf.px, before);
    }

    /// The defaults are C3's own (the payload defaults of an `sssp` line-art layer), thickness
    /// included: a range that clamps 13 to 10 would create a different line than C3's.
    #[test]
    fn defaults_are_c3s_payload_defaults() {
        let d = LineArtParams::defaults();
        assert_eq!((d.intensity, d.sharpness, d.thickness), (100.0, 100.0, 13.0));
        assert_eq!((d.shadows, d.midtones, d.highlights), (24.0, 32.0, 90.0));
        assert_eq!(d.color, [0.0, 0.0, 0.0]);
        assert_eq!(d.blend, LineArtBlend::Multiply);
        assert!(d.thickness >= THICKNESS_MIN && d.thickness <= THICKNESS_MAX, "the default must sit inside the range the engine accepts");
    }

    /// The optimised ring pass against a plain transcription of C3's maths: same tap table, luma
    /// recomputed per tap, root always taken. The speed-ups (one luma table, a skipped root) must
    /// not change a single pixel.
    #[test]
    fn optimised_field_matches_the_plain_ring() {
        let mut px = vec![[0.0f32, 0.0, 0.0, 1.0]; 64 * 48];
        for (i, p) in px.iter_mut().enumerate() {
            let seed = i as u32;
            let v = |k: u32| f32::from(((seed.wrapping_mul(2654435761).wrapping_add(k * 40503)) >> 8) as u16) / 65535.0;
            *p = [v(0), v(1), v(2), 1.0];
        }
        let buf = Buffer { rect: Rect { x0: 0, y0: 0, x1: 64, y1: 48 }, px: px.clone() };
        for thickness in [0.5f32, 3.0, 8.0, DEFAULT_THICKNESS, THICKNESS_MAX] {
            let fast = field(&buf, thickness);
            let plain = plain_field(&buf, thickness);
            for (i, (a, b)) in fast.max_diff.iter().zip(&plain.max_diff).enumerate() {
                assert!((a - b).abs() <= 1e-6, "thickness {thickness}, pixel {i}: {a} vs {b}");
            }
        }
    }

    /// [`field`] as C3's shader spells it, before the speed-ups.
    fn plain_field(buf: &Buffer, thickness: f32) -> Field {        let (width, height) = (buf.rect.width() as usize, buf.rect.height() as usize);
        let r = ring(thickness);
        let mut max_diff = vec![0.0; width * height];
        for y in 0..height {
            for x in 0..width {
                let p = buf.px[y * width + x];
                if p[3] <= 0.0 {
                    continue;
                }
                let c = [p[0], p[1], p[2]];
                let center_luma = luma(c);
                let mut accumulated = 0.0;
                for (ox, oy, weight) in &r.offsets {
                    let sx = (x as i32 + *ox).clamp(0, width as i32 - 1) as usize;
                    let sy = (y as i32 + *oy).clamp(0, height as i32 - 1) as usize;
                    let neighbor = buf.px[sy * width + sx];
                    let luma_diff = (luma([neighbor[0], neighbor[1], neighbor[2]]) - center_luma).abs();
                    let d = [neighbor[0] - c[0], neighbor[1] - c[1], neighbor[2] - c[2]];
                    let chroma_diff = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    accumulated += (luma_diff * 2.4).max(chroma_diff * 1.45).min(1.0) * weight;
                }
                max_diff[y * width + x] = clamp01(accumulated / r.weight_sum.max(1e-5));
            }
        }
        Field { rect: buf.rect, max_diff }
    }
}
