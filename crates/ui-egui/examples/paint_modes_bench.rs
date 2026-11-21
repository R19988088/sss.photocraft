//! Live-frame cost of the brush's colour sources and mixing modes on a big brush — the solid
//! colour path against the sampled-pattern (图案) path and the C3 smudge (颜色涂抹) path — drawn
//! the way the canvas draws them: one input point a frame on a kept `StrokeRenderer`, with the
//! GPU mask backend installed.
//!
//! `cargo run --release -p photocraft-ui-egui --example paint_modes_bench [size] [spacing%]`

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
    vec![("solid", solid), ("pattern", patterned), ("smudge", smudge), ("smudge+pattern", smudge_pattern)]
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
    for (name, b) in brushes(size, spacing).into_iter().filter(|(n, _)| only.is_empty() || *n == only) {
        let mut target = Surface::new(PixelFormat::RGBA8);
        // Something under the stroke for the smudge to drag.
        target.fill_rect(Rect::new(0, 0, w, h), &[0.2, 0.35, 0.6, 1.0]);
        target.fill_rect(Rect::new(0, h / 2, w / 2, h), &[0.85, 0.6, 0.2, 1.0]);
        let mut r = photocraft_paint::StrokeRenderer::on_surface(&b, &target, 1.0);
        let mut times = Vec::new();
        let mut dabs = 0usize;
        for i in 0..48 {
            let x = (220.0 + i as f64 * 9.0).min(w as f64 - 220.0);
            let y = (h as f64) * 0.45 + (i as f64 * 0.28).sin() * (h as f64) * 0.2;
            let p = StrokePoint::new(x, y, 0.72);
            let t = Instant::now();
            r.push(&[p]);
            std::hint::black_box(&r);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            dabs += 1;
        }
        let t = Instant::now();
        r.finish();
        let finish = t.elapsed().as_secs_f64() * 1000.0;
        times.sort_by(f64::total_cmp);
        let p50 = times[times.len() / 2];
        let p95 = times[(times.len() * 19).div_euclid(20).min(times.len() - 1)];
        println!(
            "  {name:14} p50 {p50:7.2} ms  p95 {p95:7.2} ms  max {:7.2} ms  finish {finish:7.2} ms  ({dabs} frames)",
            times[times.len() - 1]
        );
    }
    set_process_mask_batch(Default::default());
}

