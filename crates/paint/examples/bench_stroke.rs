//! Temporary micro-benchmark: brush stroke cost with/without sampled pattern and mixing.

use std::time::Instant;

use photocraft_color::PixelFormat;
use photocraft_geom::Rect;
use photocraft_paint::brush::BrushSettings;
use photocraft_paint::render::render_stroke;
use photocraft_paint::transport::{BrushMixSettings, MixAlgorithm};
use photocraft_paint::{ColorPattern, StrokePoint};
use photocraft_raster::Surface;

fn pattern(size: u32) -> ColorPattern {
    let pixels = (0..size * size)
        .map(|i| {
            let (x, y) = (i % size, i / size);
            [x as f32 / size as f32, y as f32 / size as f32, ((x + y) % 7) as f32 / 7.0, 1.0]
        })
        .collect::<Vec<_>>();
    ColorPattern { width: size, height: size, pixels: pixels.into() }
}

fn canvas() -> Surface {
    let mut s = Surface::new(PixelFormat::RGBA8);
    s.fill_rect(Rect::new(0, 0, 1500, 1500), &[0.6, 0.5, 0.4, 1.0]);
    for i in 0..1500 {
        s.fill_rect(Rect::new(0, i, 750, i + 1), &[0.2, 0.3, 0.8, 1.0]);
    }
    s
}

fn stroke_points() -> Vec<StrokePoint> {
    (0..=100)
        .map(|i| {
            let t = i as f64 / 100.0;
            StrokePoint::new(100.0 + t * 1200.0, 100.0 + t * 1200.0, 0.85)
        })
        .collect()
}

fn hash(target: &Surface) -> u64 {
    let data = target.read_region(Rect::new(0, 0, 1500, 1500));
    let mut h: u64 = 0xcbf29ce484222325;
    for v in data {
        let q = (v.clamp(0.0, 1.0) * 65535.0).round() as u64;
        h = (h ^ q).wrapping_mul(0x100000001b3);
    }
    h
}

fn run(name: &str, brush: &BrushSettings, points: &[StrokePoint]) -> f64 {
    let mut target = canvas();
    let t = Instant::now();
    render_stroke(&mut target, brush, points, None, false, 1.0);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("{name:28} {ms:9.1} ms  {:#018x}", hash(&target));
    ms
}

fn main() {
    let size: f32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(160.0);
    let points = stroke_points();
    let base = BrushSettings { size, spacing: 0.25, hardness: 0.6, color: [0.9, 0.2, 0.1, 1.0], ..Default::default() };
    println!("brush size {size}, 100 points, 1200 px diagonal");

    run("plain", &base, &points);
    let mut dyn_color = base.clone();
    dyn_color.color_dynamics.enabled = true;
    run("per-dab color", &dyn_color, &points);
    let mut dyn_and_pattern = dyn_color.clone();
    dyn_and_pattern.pattern_enabled = true;
    dyn_and_pattern.sampled_pattern = Some(pattern(512));
    run("per-dab color + pattern", &dyn_and_pattern, &points);
    run("pattern", &BrushSettings { pattern_enabled: true, sampled_pattern: Some(pattern(512)), ..base.clone() }, &points);
    for algorithm in [MixAlgorithm::V1, MixAlgorithm::V2, MixAlgorithm::V3] {
        let mix = BrushMixSettings { enabled: true, strength: 0.5, algorithm, ..Default::default() };
        run(&format!("mix {algorithm:?}"), &BrushSettings { mix_settings: mix.clone(), ..base.clone() }, &points);
        run(
            &format!("mix {algorithm:?} + blur"),
            &BrushSettings { mix_settings: BrushMixSettings { blur_strength: 0.6, ..mix.clone() }, ..base.clone() },
            &points,
        );
        run(
            &format!("mix {algorithm:?} + pattern"),
            &BrushSettings { mix_settings: mix, pattern_enabled: true, sampled_pattern: Some(pattern(512)), ..base.clone() },
            &points,
        );
    }
    use photocraft_paint::pigment_material::PigmentSettings;
    run("pigment on (all zero)", &BrushSettings { pigment: Some(PigmentSettings { enabled: true, ..Default::default() }), ..base.clone() }, &points);
    let relief = PigmentSettings { enabled: true, thickness: 60.0, highlight_strength: 0.3, ..Default::default() };
    run("pigment thickness 60", &BrushSettings { pigment: Some(relief.clone()), ..base.clone() }, &points);
    run("pigment + texture 40", &BrushSettings { pigment: Some(PigmentSettings { visual_thickness: 40.0, ..relief.clone() }), ..base.clone() }, &points);
    run("pigment + pattern", &BrushSettings { pigment: Some(relief), pattern_enabled: true, sampled_pattern: Some(pattern(512)), ..base.clone() }, &points);
}
