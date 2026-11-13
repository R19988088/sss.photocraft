//! Sequential C3 transport, with independent state for each brush lane.
use photocraft_geom::Point;
use serde::{Deserialize, Serialize};

use crate::brush::{BrushSettings, Dynamic};
use crate::retouch::{Footprint, Region};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MixAlgorithm {
    #[default]
    V1,
    V2,
    V3,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BrushMixSettings {
    pub enabled: bool,
    pub outputs_smudge: bool,
    pub smudge_enabled: bool,
    pub blur_enabled: bool,
    pub color_output: bool,
    pub strength: f32,
    pub strength_dynamic: Dynamic,
    pub mix_pressure_curve: Option<crate::pigment_material::SignedCurve>,
    pub algorithm: MixAlgorithm,
    pub length: f32,
    pub length_dynamic: Dynamic,
    pub length_pressure_inverted: bool,
    pub concentration: f32,
    pub water: f32,
    pub smooth: f32,
    pub quality: f32,
    pub output_strength: f32,
    pub blur_strength: f32,
    pub blur_dynamic: Dynamic,
    pub blur_pressure_curve: Option<crate::pigment_material::SignedCurve>,
    pub blur_pressure_inverted: bool,
    pub blur_color_target: Option<[f32; 4]>,
}

impl Default for BrushMixSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            outputs_smudge: false,
            smudge_enabled: true,
            blur_enabled: true,
            color_output: true,
            strength: 0.0,
            strength_dynamic: Dynamic::default(),
            mix_pressure_curve: None,
            algorithm: MixAlgorithm::V1,
            length: 0.5,
            length_dynamic: Dynamic::default(),
            length_pressure_inverted: false,
            concentration: 1.0,
            water: 0.0,
            smooth: 0.0,
            quality: 4.0,
            output_strength: 1.0,
            blur_strength: 0.0,
            blur_dynamic: Dynamic::default(),
            blur_pressure_curve: None,
            blur_pressure_inverted: false,
            blur_color_target: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Transport {
    settings: BrushMixSettings,
    alpha: Option<usize>,
    previous: Option<Point>,
    /// The layer as it was when the stroke began: `blur_color_target` compares against this, so
    /// this stroke's own paint never makes an excluded area look eligible.
    blur_base: Option<Region>,
    carrier: Option<Region>,
    carrier_radius: f32,
    pigment: f32,
}

impl Transport {
    pub fn new(brush: &BrushSettings, alpha: Option<usize>) -> Self {
        Self {
            settings: brush.mix_settings.clone(),
            alpha,
            previous: None,
            blur_base: None,
            carrier: None,
            carrier_radius: 0.0,
            pigment: brush.pigment_mix_strength,
        }
    }

    pub fn dab(&mut self, work: &mut Region, fp: &Footprint) {
        self.smudge_dab(work, fp);
        self.blur_dab(work, fp);
    }

    /// Smudge precedes colour deposition; blur follows it.
    pub(crate) fn smudge_dab(&mut self, work: &mut Region, fp: &Footprint) {
        if work.rect.is_empty() || work.ch == 0 {
            return;
        }
        // The first dab of a stroke records the layer it started from, for `blur_color_target`.
        if self.blur_base.is_none() && self.settings.blur_color_target.is_some() {
            self.blur_base = Some(work.clone());
        }
        let settings = &self.settings;
        if settings.enabled && settings.smudge_enabled && fp.dab.mix_strength > 0.0001 {
            match settings.algorithm {
                MixAlgorithm::V1 => self.feedback_dab(work, fp),
                MixAlgorithm::V2 => self.sai_dab(work, fp),
                MixAlgorithm::V3 => self.pickup_dab(work, fp),
            }
        }
        self.previous = Some(fp.dab.center);
    }

    pub(crate) fn blur_dab(&self, work: &mut Region, fp: &Footprint) {
        if !work.rect.is_empty() && work.ch > 0 && self.settings.enabled && self.settings.blur_enabled && fp.dab.blur_strength > 0.0001 {
            self.blur(work, fp);
        }
    }

    /// V1: the reference's carrier feedback. Each dab refreshes the carrier from the layer
    /// under it, keeping `keep` of the previous carrier (so paint trails as the stroke moves),
    /// then lays that carrier over the destination through the dab's coverage.
    fn feedback_dab(&mut self, work: &mut Region, fp: &Footprint) {
        let radius = fp.dab.radius.max(0.5);
        let side = ((radius * 2.0).ceil() as i32).max(1);
        let local = photocraft_geom::Rect::new(0, 0, side, side);
        let mut next = Region::new(local, work.ch);
        let alpha = self.alpha.filter(|a| *a < work.ch);
        let center = fp.dab.center;
        let spatial_step = self.previous.map_or(0.0, |p| (center.x - p.x).hypot(center.y - p.y) as f32 / (radius * 2.0).max(1.0));
        // The reference's retention grows with the mixing length and the distance travelled
        // (`pow(0.2 + 0.75*length, max(0.08, spatialStep*4))`), so a longer length drags more.
        // Reference (ContentView.swift:9413): pow(0.2 + 0.75 * mixLength, max(0.08, spatialStep * 4)).
        // The 0.08 floor matters: with a zero step the old form gave keep = 1 and the first dab
        // never refreshed its carrier at all.
        let keep = (0.2 + 0.75 * fp.dab.mix_length).powf((spatial_step * 4.0).max(0.08));
        let keep = keep.clamp(0.0, 1.0);
        let previous_carrier = self.carrier.as_ref().map(|c| {
            let scale = self.carrier_radius / radius;
            (c, scale)
        });
        for y in 0..side {
            for x in 0..side {
                let (dx, dy) = (f64::from(x) + 0.5 - f64::from(side) * 0.5, f64::from(y) + 0.5 - f64::from(side) * 0.5);
                let sampled = premultiplied(&sample(work, center.x + dx - 0.5, center.y + dy - 0.5), alpha);
                let fresh: Vec<f32> = if let Some(a) = alpha {
                    sampled.iter().enumerate().map(|(i, v)| if i == a { *v } else { v * sampled[a] }).collect()
                } else {
                    sampled.clone()
                };
                let previous = previous_carrier.map(|(c, scale)| {
                    sample(c, dx * f64::from(scale) + f64::from(c.rect.width()) * 0.5 - 0.5, dy * f64::from(scale) + f64::from(c.rect.height()) * 0.5 - 0.5)
                });
                let pixel = next.px_mut(x, y);
                match previous {
                    Some(prev) if alpha.map_or(1.0, |a| prev[a]) > 0.0001 => {
                        for (out, (f, p)) in pixel.iter_mut().zip(fresh.iter().zip(&prev)) {
                            *out = f + (*p - *f) * keep;
                        }
                    }
                    _ => pixel.copy_from_slice(&fresh),
                }
            }
        }
        let rect = fp.rect.intersect(&work.rect);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let local_x = f64::from(x) + 0.5 - center.x + f64::from(side) * 0.5 - 0.5;
                let local_y = f64::from(y) + 0.5 - center.y + f64::from(side) * 0.5 - 0.5;
                let carried = sample(&next, local_x, local_y);
                let coverage = (fp.at(x, y) * fp.dab.mix_strength * self.settings.concentration * self.settings.output_strength).clamp(0.0, 1.0);
                over_carried(work.px_mut(x, y), &carried, alpha, coverage, self.pigment);
            }
        }
        self.carrier = Some(next);
        self.carrier_radius = radius;
    }

    fn pickup_dab(&self, work: &mut Region, fp: &Footprint) {
        let center = fp.dab.center;
        let previous = self.previous.unwrap_or(center);
        let (dx, dy) = (center.x - previous.x, center.y - previous.y);
        let distance = dx.hypot(dy);
        // Reference: `min(radius, max(distance, radius * length * 0.45))`. The floor keeps a slow
        // stroke picking up paint even when the dabs nearly coincide.
        let r = f64::from(fp.dab.radius);
        let push_distance = r.min(distance.max(r * f64::from(fp.dab.mix_length) * 0.45));
        let push = push_distance * f64::from((0.18 + fp.dab.mix_length * 1.45).min(1.0));
        let (ox, oy) = if distance > 0.0001 { (-dx / distance * push, -dy / distance * push) } else { (0.0, 0.0) };
        let snapshot = work.clone();
        let rect = fp.rect.intersect(&work.rect);
        let alpha = self.alpha.filter(|a| *a < work.ch);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let source = sample(&snapshot, f64::from(x) + ox, f64::from(y) + oy);
                let coverage = (fp.at(x, y) * fp.dab.mix_strength).clamp(0.0, 1.0);
                let destination = work.px_mut(x, y);
                let source_alpha = alpha.map_or(1.0, |a| source[a]);
                let old_alpha = alpha.map_or(1.0, |a| destination[a]);
                let carried_alpha = coverage.min(source_alpha);
                let result_alpha = old_alpha.max(carried_alpha + old_alpha * (1.0 - carried_alpha));
                let weight = (coverage * source_alpha).clamp(0.0, 1.0);
                let old = destination.to_vec();
                for c in 0..destination.len() {
                    if Some(c) != alpha {
                        destination[c] += (source[c] - destination[c]) * weight;
                    }
                }
                if alpha == Some(3) && destination.len() == 4 && self.pigment > 0.0 {
                    let normal = [destination[0], destination[1], destination[2], result_alpha];
                    let result = crate::pigment::paint(
                        normal,
                        [old[0], old[1], old[2], old_alpha],
                        [source[0], source[1], source[2], source_alpha],
                        weight,
                        self.pigment,
                    );
                    destination.copy_from_slice(&result);
                }
                if let Some(a) = alpha {
                    destination[a] = result_alpha;
                }
            }
        }
    }

    fn sai_dab(&mut self, work: &mut Region, fp: &Footprint) {
        let radius = fp.dab.radius.max(0.5);
        let side = ((radius * 2.0).ceil() as i32).max(1);
        let local = photocraft_geom::Rect::new(0, 0, side, side);
        let mut next = Region::new(local, work.ch);
        let alpha = self.alpha.filter(|a| *a < work.ch);
        let center = fp.dab.center;
        let spatial_step = self.previous.map_or(0.0, |p| (center.x - p.x).hypot(center.y - p.y) as f32 / (radius * 2.0).max(1.0));
        let stretch = fp.dab.mix_length.clamp(0.0, 1.0);
        let retention = sai_stretch_retention(stretch, self.settings.quality, spatial_step);
        let dynamic = if self.settings.strength > 0.0001 { fp.dab.mix_strength / self.settings.strength } else { 0.0 };
        for y in 0..side {
            for x in 0..side {
                let (dx, dy) = (f64::from(x) + 0.5 - f64::from(side) * 0.5, f64::from(y) + 0.5 - f64::from(side) * 0.5);
                let sampled = sample(work, center.x + dx - 0.5, center.y + dy - 0.5);
                let mut sampled = premultiplied(&sampled, alpha);
                let retained = self.carrier.as_ref().map_or_else(
                    || sampled.clone(),
                    |c| {
                        let scale = self.carrier_radius / radius;
                        sample(c, dx * f64::from(scale) + f64::from(c.rect.width()) * 0.5 - 0.5, dy * f64::from(scale) + f64::from(c.rect.height()) * 0.5 - 0.5)
                    },
                );
                let a = alpha.map_or(1.0, |a| sampled[a]);
                // C3 (SimpleShaders.metal:3490-3493): a stroke that starts on transparency must
                // still pick up paint it meets later, so the empty carrier is replaced outright
                // instead of blended into.
                let retained_alpha = alpha.map_or(1.0, |a| retained[a]);
                if retained_alpha <= 0.0001 && a > 0.0001 {
                    next.px_mut(x, y).copy_from_slice(&sampled);
                    continue;
                }
                let sample_alpha16 = (a.clamp(0.0, 1.0) * 65535.0).floor();
                if sample_alpha16 == 0.0 {
                    let old_a = alpha.map_or(1.0, |a| retained[a]);
                    for c in 0..sampled.len() {
                        if Some(c) != alpha {
                            sampled[c] = if old_a > 0.0 { retained[c] / old_a * a } else { 0.0 };
                        }
                    }
                }
                // The reference caps the blend at 0.85: past that the carrier would stop picking
                // up paint entirely.
                let blend = self.settings.strength.min(0.85);
                let base = if sample_alpha16 > 0.0 { (1.0 - blend).powi(2) } else { (1.0 - self.settings.water).powi(2) };
                let effective = (base * dynamic).clamp(0.0, 1.0) * (1.0 - retention);
                let pixel = next.px_mut(x, y);
                for c in 0..pixel.len() {
                    pixel[c] = (retained[c] + (sampled[c] - retained[c]) * effective).clamp(0.0, 1.0);
                }
            }
        }
        let rect = fp.rect.intersect(&work.rect);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let local_x = f64::from(x) + 0.5 - center.x + f64::from(side) * 0.5 - 0.5;
                let local_y = f64::from(y) + 0.5 - center.y + f64::from(side) * 0.5 - 0.5;
                let carried = sample(&next, local_x, local_y);
                let coverage = (fp.at(x, y) * fp.dab.mix_strength * self.settings.concentration * self.settings.output_strength).clamp(0.0, 1.0);
                over_carried(work.px_mut(x, y), &carried, alpha, coverage, self.pigment);
            }
        }
        self.carrier = Some(next);
        self.carrier_radius = radius;
    }

    /// Separable Gaussian weights for `radius`, as `(offset, weight)` pairs, the positive half
    /// only (the kernel is symmetric). Same construction as the reference: `sigma = radius/3`,
    /// two consecutive taps folded into one pair.
    fn gaussian_weights(radius: f32) -> Vec<(f32, f32)> {
        let radius = radius.clamp(1.5, 127.0);
        let support = radius.ceil() as usize;
        let sigma = f64::from(radius) / 3.0;
        let raw: Vec<f64> = (0..=support).map(|i| (-((i * i) as f64) / (2.0 * sigma * sigma)).exp()).collect();
        let total = raw[0] + 2.0 * raw.iter().skip(1).sum::<f64>();
        if total <= 0.0 {
            return vec![(0.0, 1.0)];
        }
        let mut out = vec![(0.0f32, (raw[0] / total) as f32)];
        let mut i = 1usize;
        while i <= support {
            let a = raw[i];
            let b = if i < support { raw[i + 1] } else { 0.0 };
            let denom = a + b;
            let offset = if denom > 0.0 { i as f32 + (b / denom) as f32 } else { i as f32 };
            out.push((offset, (denom / total) as f32));
            i += 2;
        }
        out
    }

    fn blur(&self, work: &mut Region, fp: &Footprint) {
        let alpha = self.alpha.filter(|a| *a < work.ch);
        let radius = (fp.dab.radius * (0.18 + fp.dab.blur_strength * 0.82)).clamp(1.5, 127.0);
        let rect = fp.rect.intersect(&work.rect);
        if rect.is_empty() {
            return;
        }
        // Convolve the whole region once (separable, both axes), rather than resampling nine
        // points per output pixel: a large radius needs every texel inside its support.
        let weights = Self::gaussian_weights(radius);
        let mut vertical = Region::new(work.rect, work.ch);
        let mut blurred = Region::new(work.rect, work.ch);
        Self::gaussian_blur_axis(work, &mut vertical, &weights, alpha, true);
        Self::gaussian_blur_axis(&vertical, &mut blurred, &weights, alpha, false);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                if let Some(target) = self.settings.blur_color_target
                    && work.ch >= 3
                    && target[3] >= 0.0
                {
                    // Compare against the stroke's starting pixels, as the reference does: this
                    // stroke's own paint must not make an excluded area look eligible.
                    let p = self.blur_base.as_ref().map_or_else(|| work.px(x, y), |b| b.px(x, y));
                    let distance = ((p[0] - target[0]).powi(2) + (p[1] - target[1]).powi(2) + (p[2] - target[2]).powi(2)).sqrt();
                    if alpha.is_some_and(|a| p[a] <= 0.0001) || distance > target[3] {
                        continue;
                    }
                }
                let sampled = blurred.px(x, y);
                let sampled_alpha = alpha.map_or(1.0, |a| sampled[a]);
                if sampled_alpha <= 0.0 {
                    continue;
                }
                // `sampled` is premultiplied; the merge works in straight colour.
                let sampled: Vec<f32> = match alpha {
                    Some(a) => sampled.iter().enumerate().map(|(i, v)| if i == a { *v } else { v / sampled_alpha }).collect(),
                    None => sampled.to_vec(),
                };
                let weight = (fp.at(x, y) * fp.dab.blur_strength).clamp(0.0, 1.0);
                let old = work.px(x, y).to_vec();
                let old_alpha = alpha.map_or(1.0, |a| old[a]);
                let result_alpha = old_alpha + (old_alpha.max(sampled_alpha) - old_alpha) * weight;
                let out = work.px_mut(x, y);
                for c in 0..out.len() {
                    out[c] = if Some(c) == alpha {
                        result_alpha
                    } else {
                        (old[c] * old_alpha * (1.0 - weight) + sampled[c] / sampled_alpha * old_alpha.max(sampled_alpha) * weight) / result_alpha.max(1e-6)
                    };
                }
            }
        }
    }

    /// One axis of the separable blur, `src` into `dst`.
    fn gaussian_blur_axis(src: &Region, dst: &mut Region, weights: &[(f32, f32)], alpha: Option<usize>, horizontal: bool) {
        let (w, h) = (src.width() as i32, src.height() as i32);
        let n = src.ch;
        for y in 0..h {
            for x in 0..w {
                let mut acc = vec![0.0f64; n];
                let mut total = 0.0f64;
                let mut add = |dx: i32, dy: i32, weight: f64, acc: &mut Vec<f64>, total: &mut f64| {
                    let sx = (x + dx).clamp(0, w - 1);
                    let sy = (y + dy).clamp(0, h - 1);
                    let px = src.px(src.rect.x0 + sx, src.rect.y0 + sy);
                    let a = alpha.map_or(1.0, |ai| px[ai]);
                    for (i, v) in px.iter().enumerate() {
                        acc[i] += f64::from(if Some(i) == alpha { *v } else { v * a }) * weight;
                    }
                    *total += weight;
                };
                add(0, 0, f64::from(weights[0].1), &mut acc, &mut total);
                for &(offset, weight) in weights.iter().skip(1) {
                    let o = offset as i32;
                    let ww = f64::from(weight);
                    if horizontal {
                        add(-o, 0, ww, &mut acc, &mut total);
                        add(o, 0, ww, &mut acc, &mut total);
                    } else {
                        add(0, -o, ww, &mut acc, &mut total);
                        add(0, o, ww, &mut acc, &mut total);
                    }
                }
                // `acc` is premultiplied and divided by the summed weight; the alpha that comes
                // out is the average coverage, and the colour is unpremultiplied against it.
                if total > 0.0 {
                    for v in acc.iter_mut() {
                        *v /= total;
                    }
                }
                // Keep the premultiplied result: the merge un-premultiplies against this alpha,
                // so a nearly transparent texel never divides a tiny number into a huge one.
                let px = dst.px_mut(src.rect.x0 + x, src.rect.y0 + y);
                for (i, v) in acc.iter().enumerate() {
                    px[i] = *v as f32;
                }
            }
        }
    }
}

pub fn sai_stretch_retention(stretch: f32, quality: f32, spatial_step: f32) -> f32 {
    if spatial_step <= 0.0 {
        return 1.0;
    }
    (1.0 - (1.0 - stretch.clamp(0.0, 1.0)).powi(2)).powf(32.0 * spatial_step / quality.max(1.0))
}

fn sample(region: &Region, x: f64, y: f64) -> Vec<f32> {
    let x = x.clamp(f64::from(region.rect.x0), f64::from(region.rect.x1 - 1));
    let y = y.clamp(f64::from(region.rect.y0), f64::from(region.rect.y1 - 1));
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let (jx, jy) = ((ix + 1).min(region.rect.x1 - 1), (iy + 1).min(region.rect.y1 - 1));
    let (fx, fy) = ((x - x.floor()) as f32, (y - y.floor()) as f32);
    let (a, b, c, d) = (region.px(ix, iy), region.px(jx, iy), region.px(ix, jy), region.px(jx, jy));
    (0..region.ch).map(|n| (a[n] + (b[n] - a[n]) * fx) * (1.0 - fy) + (c[n] + (d[n] - c[n]) * fx) * fy).collect()
}

fn premultiplied(pixel: &[f32], alpha: Option<usize>) -> Vec<f32> {
    let mut out = pixel.to_vec();
    if let Some(a) = alpha {
        for (c, v) in out.iter_mut().enumerate() {
            if c != a {
                *v *= pixel[a];
            }
        }
    }
    out
}

fn over_carried(destination: &mut [f32], carried: &[f32], alpha: Option<usize>, coverage: f32, pigment: f32) {
    let da = alpha.map_or(1.0, |a| destination[a]);
    let sa = alpha.map_or(1.0, |a| carried[a]) * coverage;
    if sa <= 0.0 {
        return;
    }
    let result_a = sa + da * (1.0 - sa);
    let old = destination.to_vec();
    for c in 0..destination.len() {
        if Some(c) != alpha {
            destination[c] = (carried[c] * coverage + destination[c] * da * (1.0 - sa)) / result_a.max(1e-6);
        }
    }
    if alpha == Some(3) && destination.len() == 4 && pigment > 0.0 {
        let source_a = carried[3];
        let source = [carried[0] / source_a, carried[1] / source_a, carried[2] / source_a, source_a];
        let result = crate::pigment::paint(
            [destination[0], destination[1], destination[2], result_a],
            [old[0], old[1], old[2], da],
            source,
            sa / result_a.max(1e-6),
            pigment,
        );
        destination.copy_from_slice(&result);
    }
    if let Some(a) = alpha {
        destination[a] = result_a;
    }
}

/// C3's absolute two-segment pressure curve; mouse strokes use its maximum.
pub fn pressure_value(curve: Option<&crate::pigment_material::SignedCurve>, strength: f32, pressure: f32, is_pen: bool) -> f32 {
    if curve.is_some_and(|c| !c.enabled) {
        return 0.0;
    }
    let bell = crate::pigment_material::SignedCurve {
        points: [[0.0, 0.0], [0.1, 0.0], [0.2, strength], [0.3, strength], [0.53, strength], [0.77, 0.0], [1.0, 0.0]],
        enabled: true,
    };
    let curve = curve.unwrap_or(&bell);
    if is_pen { curve.value(pressure).clamp(0.0, 1.0) } else { curve_maximum(curve).clamp(0.0, 1.0) }
}

fn curve_maximum(curve: &crate::pigment_material::SignedCurve) -> f32 {
    let mut maximum = curve.points[0][1].max(curve.points[3][1]).max(curve.points[6][1]);
    for start in [0, 3] {
        let [a, b, c, d] = [curve.points[start][1], curve.points[start + 1][1], curve.points[start + 2][1], curve.points[start + 3][1]];
        let q = -a + 3.0 * b - 3.0 * c + d;
        let l = 2.0 * (a - 2.0 * b + c);
        let k = b - a;
        let discriminant = l * l - 4.0 * q * k;
        let roots = if q.abs() < 1e-12 {
            if l.abs() < 1e-12 { [f32::NAN; 2] } else { [-k / l, f32::NAN] }
        } else if discriminant < 0.0 {
            [f32::NAN; 2]
        } else {
            [(-l - discriminant.sqrt()) / (2.0 * q), (-l + discriminant.sqrt()) / (2.0 * q)]
        };
        for t in roots {
            if t > 0.0 && t < 1.0 {
                let u = 1.0 - t;
                maximum = maximum.max(u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d);
            }
        }
    }
    maximum
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Control, Dab, Dynamic};
    use photocraft_geom::Rect;

    fn footprint(x: f64, index: usize) -> Footprint {
        let dab = Dab { mix_strength: 0.6, mix_length: 0.5, blur_strength: 0.0, ..Dab::round(Point::new(x, 2.0), 1.0, 1.0) };
        crate::retouch::footprint(&dab, index, 1.0)
    }
    fn paint_region() -> Region {
        let mut r = Region::new(Rect::new(0, 0, 12, 5), 4);
        for y in 0..5 {
            for x in 0..4 {
                r.px_mut(x, y).copy_from_slice(&[0., 0., 1., 1.]);
            }
        }
        r
    }
    #[test]
    fn v1_carries_paint_forward_and_v2_carries_color_without_erasing() {
        // V1 is the reference's carrier feedback: it drags the colour under the dab along the
        // stroke, so blue paint reaches past where it was drawn.
        let b = BrushSettings { mix_settings: BrushMixSettings { enabled: true, strength: 0.6, ..Default::default() }, ..Default::default() };
        let mut transport = Transport::new(&b, Some(3));
        let mut a = paint_region();
        for (i, x) in [2., 5., 8.].into_iter().enumerate() {
            transport.dab(&mut a, &footprint(x, i));
        }
        assert!(a.px(6, 2)[3] > 0.0, "the carrier drags paint past the last dab: {:?}", a.px(6, 2));
        assert!((a.px(6, 2)[2] - 1.0).abs() < 0.01, "and it is the same blue: {:?}", a.px(6, 2));
        let mut b = b;
        b.mix_settings.algorithm = MixAlgorithm::V2;
        let mut transport = Transport::new(&b, Some(3));
        let mut a = paint_region();
        transport.dab(&mut a, &footprint(2., 0));
        transport.dab(&mut a, &footprint(5., 1));
        assert!(a.px(4, 2)[3] > 0.);
        assert_eq!(a.px(2, 2)[3], 1.);
        assert!(a.px(4, 2)[2] > 0.99);
    }
    #[test]
    fn mixing_master_and_internal_switches_gate_effects_without_resetting_strengths() {
        let mut b = BrushSettings { mix_settings: BrushMixSettings { strength: 0.8, blur_strength: 1.0, ..Default::default() }, ..Default::default() };
        let mut fp = footprint(4., 0);
        fp.dab.mix_strength = 0.8;
        fp.dab.blur_strength = 1.;
        let original = paint_region();
        let mut disabled = original.clone();
        Transport::new(&b, Some(3)).dab(&mut disabled, &fp);
        assert_eq!(disabled, original, "master off gates both effects");
        b.mix_settings.enabled = true;
        b.mix_settings.smudge_enabled = false;
        b.mix_settings.blur_enabled = false;
        Transport::new(&b, Some(3)).dab(&mut disabled, &fp);
        assert_eq!(disabled, original, "both internal switches off preserve pixels");
        b.mix_settings.blur_enabled = true;
        Transport::new(&b, Some(3)).dab(&mut disabled, &fp);
        assert_ne!(disabled, original, "blur operates independently of smudge");
        assert_eq!((b.mix_settings.strength, b.mix_settings.blur_strength), (0.8, 1.0));
    }

    #[test]
    fn c3_pressure_curves_are_absolute_and_nonmonotone() {
        assert!((pressure_value(None, 0.7, 1., false) - 0.7).abs() < 1e-6);
        assert!(pressure_value(None, 0.7, 0.3, true) > 0.69);
        assert_eq!(pressure_value(None, 0.7, 1., true), 0.);
        assert_eq!(sai_stretch_retention(0., 4., 1.), 0.);
        assert_eq!(sai_stretch_retention(1., 4., 1.), 1.);
        let b = BrushSettings {
            mix_settings: BrushMixSettings { strength: 0.7, strength_dynamic: Dynamic::controlled(Control::PenPressure), ..Default::default() },
            ..Default::default()
        };
        assert_eq!(b.mix_settings.strength, 0.7);
    }
    #[test]
    fn gaussian_weights_normalise_and_widen_with_the_radius() {
        let small = Transport::gaussian_weights(2.0);
        let large = Transport::gaussian_weights(40.0);
        // The positive half's weights plus their mirror sum to one.
        let sum = |w: &[(f32, f32)]| w[0].1 + 2.0 * w.iter().skip(1).map(|(_, x)| *x).sum::<f32>();
        assert!((sum(&small) - 1.0).abs() < 1e-4, "{}", sum(&small));
        assert!((sum(&large) - 1.0).abs() < 1e-4, "{}", sum(&large));
        // A wider radius spreads over more taps, which is what a box kernel cannot do.
        assert!(large.len() > small.len() + 10, "{} vs {}", large.len(), small.len());
        // Centred and symmetric: offsets ascend from zero.
        assert_eq!(small[0].0, 0.0);
        assert!(small.windows(2).all(|w| w[1].0 > w[0].0));
        // The largest weight is the centre one.
        assert!(small[0].1 > small[1].1);
    }

    #[test]
    fn blur_averages_premultiplied_color_and_obeys_target_tolerance() {
        let mut b = BrushSettings { mix_settings: BrushMixSettings { enabled: true, blur_strength: 1., ..Default::default() }, ..Default::default() };
        let mut fp = footprint(4., 0);
        fp.dab.blur_strength = 1.;
        let mut a = paint_region();
        let original = a.clone();
        b.mix_settings.blur_color_target = Some([1., 0., 0., 0.01]);
        Transport::new(&b, Some(3)).dab(&mut a, &fp);
        assert_eq!(a, original);
        b.mix_settings.blur_color_target = None;
        Transport::new(&b, Some(3)).dab(&mut a, &fp);
        assert!(a.px(4, 2)[3] > 0.);
    }
}
