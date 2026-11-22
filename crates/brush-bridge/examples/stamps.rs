//! How long ProCraft's stamp generator takes for a stroke, and how many stamps it makes.
//!
//! `cargo run --release --example stamps -p photocraft-brush-bridge -- 900 1`
//! (size in px, spacing in percent)

use std::time::Instant;

use photocraft_brush_bridge as bridge;
use photocraft_paint::brush::BrushSettings;
use procraft_engine::geom::Vec2;
use procraft_engine::input::{Pointer, Sample};
use procraft_engine::stroke::StrokeBuilder;

fn main() {
    let mut args = std::env::args().skip(1);
    let size: f32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(900.0);
    let spacing: f32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(1.0) / 100.0;
    let points: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(300);

    let mut settings = BrushSettings::default();
    settings.size = size;
    settings.spacing = spacing;
    settings.opacity = 1.0;
    settings.flow = 1.0;
    settings.smoothing.amount = 0.0;

    let brush = bridge::brush(&settings);
    let diameter = bridge::stroke_diameter(&settings);

    let mut builder = StrokeBuilder::new(brush, diameter, Pointer::Stylus);
    let mut stamps = 0usize;
    let mut elapsed = 0f64;

    // A stroke that crosses the canvas once, with its points already in hand: this measures stamp
    // generation alone, not input, not painting.
    for i in 0..points {
        let x = 20.0 + i as f32 * (2000.0 / points as f32);
        let y = 20.0 + (i as f32 * 0.7).sin() * 40.0;
        let sample = Sample::at(Vec2::new(x, y), 1.0, i as f64 * 8.0);
        let started = Instant::now();
        stamps += builder.add(&[sample]).len();
        elapsed += started.elapsed().as_secs_f64();
    }

    // The pen-up taper: ProCraft lays these last.
    let started = Instant::now();
    let (main, _tipped) = builder.add_tipped(&[]);
    stamps += main.len();
    elapsed += started.elapsed().as_secs_f64();

    println!(
        "size {size} px, spacing {:.1} %, {} points: {stamps} stamps, {:.2} ms total, {:.3} ms a point",
        spacing * 100.0,
        points,
        elapsed * 1000.0,
        elapsed * 1000.0 / points as f64
    );
}
