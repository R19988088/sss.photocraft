//! CMYK screen print (C3's `cmykScreenPrint`, 「CMYK」调整层): the tone is split into cyan,
//! magenta, yellow and black ink and every ink is printed as its own dot screen at its own angle,
//! then the inks multiply onto paper. This is C3's own pass, ported level for level from
//! `Sources/Shaders/C2AdjustmentShaders.metal` (`webApplyCMYKScreenPrint`), so the CPU render here
//! and C3's GPU render agree.

use crate::Buffer;

const PI: f32 = 3.141_592_7;

/// C3's `webAdjustCMYKInkBySaturation`: a strength above 0 pushes an ink towards solid, a
/// negative one thins it towards the paper.
fn ink_strength(ink: f32, adjustment: f32) -> f32 {
    let amount = (adjustment / 100.0).clamp(-1.0, 1.0);
    let base = ink.clamp(0.0, 1.0);
    if amount >= 0.0 { base + (1.0 - base) * (amount * (1.0 - base * 0.35)) } else { base * (1.0 + amount) }
}

/// The fractional part below one (`x - floor(x)`), the way the shaders spell `fract`.
fn fract01(v: f32) -> f32 {
    v - v.floor()
}

/// C3's `webScreenPrintDot`: a round dot of the ink's area inside a cell rotated by `angle`.
fn dot(x: f32, y: f32, angle_degrees: f32, cell_size: f32, ink: f32) -> f32 {
    let angle = angle_degrees * PI / 180.0;
    let (sin, cos) = angle.sin_cos();
    let rotated = (x * cos - y * sin, x * sin + y * cos);
    let cell = cell_size.max(1.0);
    // Floor-based, as `fract` is in Metal and WGSL (Rust's own `fract` keeps the sign, which would
    // turn the screen inside out left of the origin and away from the GPU render).
    let cx = fract01(rotated.0 / cell) - 0.5;
    let cy = fract01(rotated.1 / cell) - 0.5;
    let radius = ink.clamp(0.0, 1.0).sqrt() * 0.62;
    let distance = (cx * cx + cy * cy).sqrt();
    let edge = radius + 0.055;
    if distance <= radius {
        1.0
    } else if distance >= edge {
        0.0
    } else {
        let t = (distance - radius) / (edge - radius);
        1.0 - t * t * (3.0 - 2.0 * t)
    }
}

/// Print `buf` as four-colour ink: `dot_size` is the cell's side in pixels, `angle` the base
/// screen angle, and the four strengths are the cyan / magenta / yellow / black ink adjustments
/// in percent.
pub fn apply(buf: &mut Buffer, dot_size: f32, angle: f32, strengths: [f32; 4]) {
    let cell_size = dot_size.clamp(2.0, 96.0);
    let (w, x0, y0) = (buf.rect.width().max(1) as usize, buf.rect.x0, buf.rect.y0);
    for (i, p) in buf.px.iter_mut().enumerate() {
        if p[3] <= 0.0 {
            continue;
        }
        let x = x0 + (i % w) as i32;
        let y = y0 + (i / w) as i32;
        let pos = (x as f32 + 0.5, y as f32 + 0.5);
        let rgb = [p[0], p[1], p[2]];
        let cmy = [1.0 - rgb[0].clamp(0.0, 1.0), 1.0 - rgb[1].clamp(0.0, 1.0), 1.0 - rgb[2].clamp(0.0, 1.0)];
        let mut k = cmy[0].min(cmy[1]).min(cmy[2]);
        let cmy_only = [
            ink_strength((cmy[0] - k) / (1.0 - k).max(1e-4), strengths[0]),
            ink_strength((cmy[1] - k) / (1.0 - k).max(1e-4), strengths[1]),
            ink_strength((cmy[2] - k) / (1.0 - k).max(1e-4), strengths[2]),
        ];
        k = ink_strength(k.clamp(0.0, 1.0), strengths[3]);
        let c = dot(pos.0 + 0.35, pos.1 - 0.2, angle + 15.0, cell_size, cmy_only[0]);
        let m = dot(pos.0 - 0.25, pos.1 + 0.3, angle + 75.0, cell_size, cmy_only[1]);
        let y_ink = dot(pos.0 + 0.15, pos.1 + 0.18, angle, cell_size, cmy_only[2]);
        let black = dot(pos.0, pos.1, angle + 45.0, cell_size * 0.86, k);
        let inked = [
            (1.0 - c * 0.92).clamp(0.0, 1.0),
            (1.0 - m * 0.90).clamp(0.0, 1.0),
            (1.0 - y_ink * 0.86).clamp(0.0, 1.0),
        ];
        let paper = inked[0] * inked[1] * inked[2] * (1.0 - black * 0.94);
        for channel in 0..3 {
            p[channel] = (rgb[channel] + (paper - rgb[channel]) * 0.86).clamp(0.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_geom::Rect;

    #[test]
    fn a_screen_print_turns_a_flat_grey_into_a_dot_pattern() {
        let mut buf = Buffer { rect: Rect { x0: 0, y0: 0, x1: 16, y1: 16 }, px: vec![[0.5, 0.5, 0.5, 1.0]; 256] };
        apply(&mut buf, 6.0, 15.0, [0.0; 4]);
        let dark = buf.px.iter().filter(|p| p[0] < 0.45).count();
        let light = buf.px.iter().filter(|p| p[0] > 0.55).count();
        assert!(dark > 0 && light > 0, "a screen keeps dots and paper apart");
        // The print is bimodal — ink where the dots land, paper between them — rather than the
        // flat grey it started from. (The four ink screens sit at 0/15/45/75°, so the four lattices
        // only repeat together over a very long period; the ink/paper split is what a screen means.)
        let near_paper = buf.px.iter().filter(|p| p[0] > 0.85).count();
        let near_ink = buf.px.iter().filter(|p| p[0] < 0.2).count();
        assert!(near_paper + near_ink > buf.px.len() / 2, "most pixels are ink or paper: {near_paper} + {near_ink}");
    }

    #[test]
    fn a_negative_strength_thins_the_ink() {
        let mut strong = Buffer { rect: Rect { x0: 0, y0: 0, x1: 8, y1: 8 }, px: vec![[0.2, 0.2, 0.2, 1.0]; 64] };
        let mut thin = strong.clone();
        apply(&mut strong, 4.0, 0.0, [100.0; 4]);
        apply(&mut thin, 4.0, 0.0, [-100.0; 4]);
        let sum = |b: &Buffer| b.px.iter().map(|p| p[0]).sum::<f32>();
        assert!(sum(&strong) < sum(&thin), "raising the ink darkens the print");
    }
}
