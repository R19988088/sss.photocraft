//! Live-frame cost of the brush's colour sources and mixing modes on a big brush — the solid
//! colour path against the sampled-pattern (图案) path, the C3 smudge (颜色涂抹) path and an
//! eraser — drawn the way the canvas draws them: one input point a frame on a kept
//! `StrokeRenderer`, with the GPU mask backend installed.
//!
//! `cargo run --release -p photocraft-ui-egui --example paint_modes_bench [size] [spacing%] [mode]`
//!
//! `PC_COMPARE=1` draws each stroke twice, folded (the GPU backend) and per-dab (no backend) and
//! reports how far apart the two results are.

use std::sync::Arc;
use std::time::Instant;

use photocraft_color::PixelFormat;
use photocraft_geom::Rect;
use photocraft_paint::brush::{ColorPattern, Taper};
use photocraft_paint::stamp::{set_process_mask_batch, set_process_transport};
use photocraft_paint::transport::BrushMixSettings;
use photocraft_paint::{BrushSettings, StrokePoint};
use photocraft_raster::Surface;

fn pattern(n: u32) -> ColorPattern {
    // What the eyedropper hands the brush: a patch of the canvas, here a checkered texture with
    // soft grains so the five-tap alpha-weighted sample has something to weigh.
    let mut pixels = Vec::with_capacity((n * n) as usize);
    for y in 0..n {
        for x in 0..n {
            let cell = ((x / 6) + (y / 6)) % 2;
            let v = if cell == 0 { 0.18 } else { 0.72 };
            let grain = ((x * 7 + y * 13) % 5) as f32 / 40.0;
            pixels.push([v + grain, (v * 0.55 + 0.2) + grain, 1.0 - v, 1.0]);
        }
    }
    ColorPattern { width: n, height: n, pixels: Arc::from(pixels.into_boxed_slice()) }
}

fn brushes(size: f32, spacing: f32) -> Vec<(&'static str, BrushSettings)> {
    let base = BrushSettings {
        size,
        spacing,
        hardness: 0.0,
        pressure_size: false,
        taper: Taper::NONE,
        ..Default::default()
    };
    let solid = base.clone();
    let patterned = BrushSettings { pattern_enabled: true, sampled_pattern: Some(pattern(256)), ..base.clone() };
    let smudge = BrushSettings {
        mix_settings: BrushMixSettings { enabled: true, strength: 0.6, ..Default::default() },
        ..base.clone()
    };
    let smudge_pattern = BrushSettings {
        mix_settings: BrushMixSettings { enabled: true, strength: 0.6, ..Default::default() },
        pattern_enabled: true,
        sampled_pattern: Some(pattern(256)),
        ..base.clone()
    };
    let erase = BrushSettings { erase: true, ..base.clone() };
    // What a ProCraft brush does with a pattern: the texture reaches the coverage on the GPU (the
    // shader's `mask_combine`), so there is no per-dab colour to sample.
    let grain = BrushSettings {
        texture: photocraft_paint::brush::Texture { enabled: true, depth: 1.0, scale: 1.0, ..Default::default() },
        ..base.clone()
    };
    vec![
        ("solid", solid),
        ("pattern", patterned),
        ("grain", grain),
        ("smudge", smudge),
        ("smudge+pattern", smudge_pattern),
        ("erase", erase),
    ]
}

struct Run {
    target: Surface,
    times: Vec<f64>,
    finish: f64,
}

fn stroke(b: &BrushSettings, w: i32, h: i32) -> Run {
    let mut target = Surface::new(PixelFormat::RGBA8);
    // Something under the stroke for the smudge to drag, and an eraser to take away.
    target.fill_rect(Rect::new(0, 0, w, h), &[0.2, 0.35, 0.6, 1.0]);
    target.fill_rect(Rect::new(0, h / 2, w / 2, h), &[0.85, 0.6, 0.2, 1.0]);
    let mut r = photocraft_paint::StrokeRenderer::on_surface(b, &target, 1.0);
    // `PC_PROCRAFT=1`: the dabs come from ProCraft's brush (its `StrokeBuilder`) instead of this
    // renderer's own generator, laid by the same renderer.
    let procraft = std::env::var_os("PC_PROCRAFT").is_some();
    let mut builder = procraft.then(|| {
        let brush = photocraft_brush_bridge::brush(b);
        procraft_engine::stroke::StrokeBuilder::new(brush, photocraft_brush_bridge::stroke_diameter(b), procraft_engine::input::Pointer::Stylus)
    });
    let mut times = Vec::new();
    for i in 0..48 {
        let x = (220.0 + i as f64 * 9.0).min(w as f64 - 220.0);
        let y = (h as f64) * 0.45 + (i as f64 * 0.28).sin() * (h as f64) * 0.2;
        let p = StrokePoint::new(x, y, 0.72);
        let t = Instant::now();
        match builder.as_mut() {
            Some(builder) => {
                let sample = procraft_engine::input::Sample::at(
                    procraft_engine::geom::Vec2::new(x as f32, y as f32),
                    0.72,
                    i as f64 * 8.0,
                );
                let stamps = builder.add_tipped(&[sample]).0; // the stamps drawn for good (the tip is the canvas's own)
                let dabs = photocraft_brush_bridge::dabs(&stamps, b, b.color);
                r.push_dabs(&dabs);
            }
            None => r.push(&[p]),
        }
        std::hint::black_box(&r);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let t = Instant::now();
    match builder.as_mut() {
        Some(builder) => {
            // Pen up: what the tapers still hold back, then the tip as the stroke ends. ProCraft's
            // `add` is what the app's canvas calls each frame (`add_tipped`, drawing the tip on its
            // own); here the tip is left out so the number is the stroke's own page, not the
            // preview layer's.
            let (kept, _tip) = builder.add_tipped(&[]);
            let dabs = photocraft_brush_bridge::dabs(&kept, b, b.color);
            r.push_dabs(&dabs);
            r.finish();
        }
        None => r.finish(),
    }
    let finish = t.elapsed().as_secs_f64() * 1000.0;
    times.sort_by(f64::total_cmp);
    Run { target, times, finish }
}

fn main() {
    let size: f32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(300.0);
    let spacing: f32 = std::env::args().nth(2).and_then(|s| s.parse::<f32>().ok()).map_or(0.05, |p| p / 100.0);
    let (w, h) = (2520i32, 1575i32);
    let instance = eframe::wgpu::Instance::new(eframe::wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = match pollster::block_on(instance.request_adapter(&eframe::wgpu::RequestAdapterOptions::default())) {
        Ok(a) => a,
        Err(e) => {
            println!("no adapter: {e}");
            return;
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&eframe::wgpu::DeviceDescriptor::default())).expect("device");
    set_process_mask_batch(photocraft_gpu::masker_slot(device.clone(), queue.clone()));
    if std::env::var_os("PC_TRANSPORT_GPU").is_some() {
        set_process_transport(photocraft_gpu::transport_slot(device.clone(), queue.clone()));
    }
    println!("{} — size {size}, spacing {:.0}%, {w}x{h}", adapter.get_info().name, spacing * 100.0);
    let only = std::env::args().nth(3).unwrap_or_default();
    let compare = std::env::var_os("PC_COMPARE").is_some();
    for (name, b) in brushes(size, spacing).into_iter().filter(|(n, _)| only.is_empty() || *n == only) {
        let gpu = stroke(&b, w, h);
        let times = &gpu.times;
        println!(
            "  {name:14} p50 {:7.2} ms  p95 {:7.2} ms  max {:7.2} ms  finish {:7.2} ms  ({} frames)",
            times[times.len() / 2],
            times[(times.len() * 19).div_euclid(20).min(times.len() - 1)],
            times[times.len() - 1],
            gpu.finish,
            times.len()
        );
        if compare {
            set_process_mask_batch(Default::default());
            let cpu = stroke(&b, w, h);
            set_process_mask_batch(photocraft_gpu::masker_slot(device.clone(), queue.clone()));
            let (a, c) = (gpu.target, cpu.target);
            let full = Rect::new(0, 0, w, h);
            let (x, y) = (a.try_read_region(full).unwrap(), c.try_read_region(full).unwrap());
            let (mut max, mut sum, mut over) = (0.0f32, 0.0f64, 0usize);
            for (p, q) in x.chunks(4).zip(y.chunks(4)) {
                for (u, v) in p.iter().zip(q) {
                    let d = (u - v).abs();
                    max = max.max(d);
                    sum += f64::from(d);
                    if d > 1.0 / 255.0 {
                        over += 1;
                    }
                }
            }
            let mean = sum / (x.len().max(1)) as f64;
            println!("  {name:14} folded vs per-dab: max {max:.5}, mean {mean:.6}, {over} channels over 1/255");
        }
    }
    set_process_mask_batch(Default::default());
}
