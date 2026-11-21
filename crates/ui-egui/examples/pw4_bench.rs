//! Times a real stroke from every brush in a `.brushset` — CPU only, then with the GPU mask
//! backend installed — so the slow brushes and what each stage costs are visible.
//!
//! `cargo run --release -p photocraft-ui-egui --example pw4_bench -- <file.brushset>`

use std::time::Instant;

use photocraft_color::PixelFormat;
use photocraft_geom::Rect;
use photocraft_paint::stamp::set_process_mask_batch;
use photocraft_paint::{render_stroke, BrushSettings, StrokePoint};
use photocraft_raster::Surface;

fn points(w: i32, h: i32) -> Vec<StrokePoint> {
    let mut pts = Vec::new();
    for i in 0..24 {
        let x = (200.0 + i as f64 * 80.0).min(w as f64 - 200.0);
        let y = (h as f64) * 0.45 + (i as f64 * 0.55).sin() * (h as f64) * 0.2;
        pts.push(StrokePoint::new(x, y, 0.72));
    }
    pts
}

fn time_stroke(b: &BrushSettings, pts: &[StrokePoint], w: i32, h: i32) -> f64 {
    let mut target = Surface::new(PixelFormat::RGBA8);
    target.fill_rect(Rect::new(0, 0, w, h), &[0.9, 0.9, 0.9, 1.0]);
    let t = Instant::now();
    let rect = render_stroke(&mut target, b, pts, None, false, 1.0);
    std::hint::black_box(&rect);
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let path = std::env::args().nth(1).expect("a .brushset path");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let name = std::path::Path::new(&path).file_name().map_or("brushes".to_string(), |n| n.to_string_lossy().into_owned());
    let t0 = Instant::now();
    let import = photocraft_io::brush_import::read_any(&bytes, &name, "bench").unwrap_or_else(|e| panic!("import: {e}"));
    eprintln!("imported {} presets ({}) in {:.0} ms", import.presets.len(), import.format, t0.elapsed().as_secs_f64() * 1000.0);
    let (w, h) = (1600i32, 1000i32);
    let pts = points(w, h);

    let mut rows = Vec::new();
    let mut cpu_total = 0.0;
    let limit: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
    let only = std::env::args().nth(4).unwrap_or_default();
    // `PW4_FAST=1` skips the all-CPU pass (its whole stroke is the slow part of every probe).
    let fast = std::env::var_os("PW4_FAST").is_some();
    for p in import.presets.iter().take(limit).filter(|p| only.is_empty() || p.name.contains(&only)) {
        let b = p.brush.clone();
        let ms = if only.is_empty() && !fast {
            let _ = time_stroke(&b, &pts, w, h);
            time_stroke(&b, &pts, w, h)
        } else {
            0.0
        };
        eprintln!("  {ms:8.1} ms  {}", p.name);
        cpu_total += ms;
        rows.push((ms, p.name.clone(), b));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("CPU only:");
    for (ms, name, b) in rows.iter().take(10) {
        println!(
            "  {ms:8.1} ms  size {:6.1} spacing {:4.0}%  sampled_tip {} grain {} wet {} mix {:.2}  {name}",
            b.size,
            b.spacing * 100.0,
            b.tip.bitmap().is_some(),
            b.grain.is_some(),
            b.wet_edges,
            b.mix_settings.strength,
        );
    }
    println!("  total {cpu_total:.0} ms over {} brushes", rows.len());

    let instance = eframe::wgpu::Instance::new(eframe::wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = match pollster::block_on(instance.request_adapter(&eframe::wgpu::RequestAdapterOptions::default())) {
        Ok(a) => a,
        Err(e) => {
            println!("no adapter for the GPU half: {e}");
            return;
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&eframe::wgpu::DeviceDescriptor::default())).expect("device");
    let mode = std::env::args().nth(3).unwrap_or_default();
    set_process_mask_batch(photocraft_gpu::masker_slot(device.clone(), queue.clone()));
    let mut gpu_rows = Vec::new();
    let mut gpu_total = 0.0;
    for (_, name, b) in &rows {
        let _ = time_stroke(b, &pts, w, h);
        let ms = time_stroke(b, &pts, w, h);
        gpu_total += ms;
        gpu_rows.push((ms, name.clone()));
    }
    // The live path paints a short segment per frame, not the whole stroke at once: time each
    // two-point segment the way the canvas does.
    if mode == "seg" || mode == "dab" {
        println!("Live frames ({}):", if mode == "seg" { "two-point segments" } else { "single dabs" });
        for (_, name, b) in &rows {
            let mut frames = Vec::new();
            let steps: Vec<Vec<photocraft_paint::StrokePoint>> =
                if mode == "seg" { pts.windows(2).map(|s| s.to_vec()).collect() } else { pts.iter().map(|p| vec![*p]).collect() };
            for seg in &steps {
                frames.push(time_stroke(b, seg, w, h));
            }
            frames.sort_by(f64::total_cmp);
            let p50 = frames[frames.len() / 2];
            let p95 = frames[(frames.len() * 19).div_euclid(20).min(frames.len() - 1)];
            println!("  p50 {p50:7.2} ms  p95 {p95:7.2} ms  max {:7.2} ms  {name}", frames[frames.len() - 1]);
        }
    }
    // What a 60 Hz drag costs: one input point a frame (9 px apart), the renderer kept across
    // frames exactly as the canvas keeps it, so the per-frame work is what the eye waits for.
    if mode == "live" {
        println!("Live frames (one input point a frame; {}):", adapter.get_info().name);
        for (_, name, b) in &rows {
            eprintln!("== {name}");
            let mut target = Surface::new(PixelFormat::RGBA8);
            target.fill_rect(Rect::new(0, 0, w, h), &[0.9, 0.9, 0.9, 1.0]);
            let mut r = photocraft_paint::StrokeRenderer::on_surface(b, &target, 1.0);
            let mut times = Vec::new();
            for i in 0..40 {
                let x = (200.0 + i as f64 * 9.0).min(w as f64 - 200.0);
                let y = (h as f64) * 0.45 + (i as f64 * 0.28).sin() * (h as f64) * 0.2;
                let p = StrokePoint::new(x, y, 0.72);
                let t = Instant::now();
                r.push(&[p]);
                std::hint::black_box(&r);
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            let t = Instant::now();
            r.finish();
            let finish = t.elapsed().as_secs_f64() * 1000.0;
            times.sort_by(f64::total_cmp);
            let p50 = times[times.len() / 2];
            let p95 = times[(times.len() * 19).div_euclid(20).min(times.len() - 1)];
            println!("  p50 {p50:7.2} ms  p95 {p95:7.2} ms  finish {finish:8.2} ms  {name}");
        }
    }
    // The ProCraft shape: the whole stroke laid on one GPU surface, zero readback until the end.
    if mode == "draw" {        println!("Draw-only (one surface, no readback; {}):", adapter.get_info().name);
        for (_, name, b) in &rows {
            let mut pre = Surface::new(PixelFormat::RGBA8);
            pre.fill_rect(Rect::new(0, 0, w, h), &[0.9, 0.9, 0.9, 1.0]);
            let mut rec = photocraft_paint::StrokeRenderer::on_surface(b, &pre, 1.0).record_dabs();
            rec.push(&pts);
            rec.finish();
            let dabs = rec.dabs().to_vec();
            let ctx = photocraft_paint::BrushContext::new(b);
            let mut s = photocraft_gpu::PaintSurface::new(&device, &queue, w as u32, h as u32).expect("paint surface");
            s.set_color(&queue, [0.3, 0.4, 0.7]);
            s.set_ceiling(&queue, 1.0);
            let tip = ctx.mask_tip();
            s.set_tip(&device, &queue, tip.as_deref());
            let t = Instant::now();
            let mut frame = 0usize;
            let mut laid = 0usize;
            for chunk in dabs.chunks(31) {
                frame += 1;
                let stamps: Vec<photocraft_gpu::DabStamp> = chunk
                    .iter()
                    .filter_map(|d| ctx.mask_job(d, false).map(|j| (j, ctx.dab_rect(d, false))))
                    .map(|(j, rect)| photocraft_gpu::DabStamp::of(&j, rect))
                    .collect();
                if !stamps.is_empty() {
                    laid += stamps.len();
                    s.stamp(&device, &queue, &stamps).expect("stamp");
                }
            }
            let total = t.elapsed().as_secs_f64() * 1000.0;
            println!(
                "  {total:8.1} ms  {} dabs / {laid} laid / {} frames → {:.2} ms a frame  {name}",
                dabs.len(),
                frame,
                total / frame.max(1) as f64
            );
            let t2 = Instant::now();
            let read = s.read(&device, &queue).expect("read");
            let sum: f64 = read.coverage.iter().map(|c| f64::from(*c)).sum();
            println!("     the one readback: {:.1} ms, coverage total {sum:.0}", t2.elapsed().as_secs_f64() * 1000.0);
        }
    }
    set_process_mask_batch(Default::default());
    gpu_rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("GPU masks ({}):", adapter.get_info().name);    for (ms, name) in gpu_rows.iter().take(10) {
        println!("  {ms:8.1} ms  {name}");
    }
    println!("  total {gpu_total:.0} ms vs CPU {cpu_total:.0} ms");
}
