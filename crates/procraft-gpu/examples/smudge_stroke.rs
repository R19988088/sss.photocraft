//! Whether ProCraft's own canvas can draw our smudge stroke, and at what price.
//!
//! This is the "rent a window" path put to the test: a canvas of our document's size, a paint band
//! to carry, then the smudge stroke our benchmark measures — 900 px across a 900 px brush — drawn
//! with ProCraft's own stamps on ProCraft's own layer.
//!
//! `cargo run --release -p procraft-gpu --example smudge_stroke`

use std::time::Instant;

use procraft_engine::document::Document;
use procraft_engine::geom::Vec2;
use procraft_engine::stroke::Stamp;
use procraft_gpu::{Canvas, Gpu, StrokeStyle, StrokeTool};

fn ms(from: Instant) -> f64 {
    from.elapsed().as_secs_f64() * 1e3
}

fn main() {
    // plain paint: the pigment mix is a preference, and this measures the smudge itself
    procraft_gpu::set_pigment_mix(0.0);
    let gpu = match Gpu::headless() {
        Ok(gpu) => gpu,
        Err(error) => {
            println!("no GPU: {error}");
            return;
        }
    };
    let (width, height) = (2520, 1575);
    let mut doc = Document::new(width, height);

    let made = Instant::now();
    let mut canvas = match Canvas::new(&gpu, &doc) {
        Ok(canvas) => canvas,
        Err(error) => {
            println!("no canvas: {error}");
            return;
        }
    };
    println!("canvas made          {:8.1} ms  (pipelines, once per canvas)", ms(made));

    // What the smudge carries: a band of paint a third of the canvas wide.
    canvas.begin_stroke(doc.current(), StrokeStyle::new(StrokeTool::Paint, [0.2, 0.4, 0.9], 1.0));
    let band: Vec<Stamp> =
        (0..200).map(|i| Stamp::round(Vec2::new(200.0 + i as f32 * 4.0, 500.0), 120.0, 1.0)).collect();
    canvas.add_stamps(&band);
    canvas.end_stroke();
    canvas.render(&gpu, &mut doc, None);
    gpu.flush();
    let id = doc.current();

    // The smudge: 900 px across, a 900 px brush, 1 % spacing — our benchmark's stroke.
    for (label, stamps) in [("100 stamps (1 %)", 100usize), ("975 stamps (our dabs)", 975usize)] {
        canvas.begin_stroke(id, StrokeStyle::new(StrokeTool::Smudge, [0.0; 3], 1.0));
        let dots: Vec<Stamp> = (0..stamps)
            .map(|i| {
                let t = i as f32 / stamps as f32;
                Stamp::round(Vec2::new(500.0 + 900.0 * t, 800.0), 450.0, 1.0)
            })
            .collect();
        // ProCraft lays a display frame of stamps at a time; so does the app. Sixty a frame here.
        let mut frames = Vec::new();
        for chunk in dots.chunks(60) {
            let frame = Instant::now();
            canvas.add_stamps(chunk);
            gpu.flush();
            frames.push(ms(frame));
        }
        let end = Instant::now();
        canvas.end_stroke();
        canvas.render(&gpu, &mut doc, None);
        gpu.flush();
        let finish = ms(end);
        let mut sorted = frames.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p50 = sorted[sorted.len() / 2];
        println!(
            "{label}: frame p50 {p50:8.2} ms  p95 {:8.2}  max {:8.2}  finish {finish:6.2}  ({} frames)",
            sorted[sorted.len() * 95 / 100],
            sorted[sorted.len() - 1],
            frames.len()
        );
        if let Some(last) = frames.last() {
            let _ = last;
        }
    }
    println!("GPU errors           {:8}", gpu.errors());
}
