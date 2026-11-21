//! The GPU paint surface against the CPU rasteriser: a dab laid into the surface's coverage and
//! depth attachments reads back the same mask `rasterize_into` gives, and dabs drawn after it
//! build up exactly as `CoverageMap::accumulate` does (`c ← c + v·(ceil − c)`, `max` for depth).
//!
//! The surface is 16-bit float, so the tolerance is that format's step (~5e-4), not the compute
//! kernel's 1e-5: one dab a pixel, far under the compositor's 1/255.
//!
//! Skips, with a message, when there is no adapter or device. The shader also validates without a
//! GPU, so a WGSL error fails on every runner.

use photocraft_geom::{Point, Rect};
use photocraft_gpu::{DabStamp, PaintSurface, PAINT_SURFACE_SHADER};
use photocraft_paint::{BrushContext, BrushSettings, Dab};

/// The format's own step, with a margin: one 16-bit float ULP at 1.0 is 4.9e-4.
const TOL: f32 = 1.5e-3;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    _lock: std::sync::MutexGuard<'static, ()>,
}

fn gpu() -> Option<Gpu> {
    let lock = crate::GPU_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let required = std::env::var_os("WGPU_BACKEND").is_some();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) {
        Ok(a) => a,
        Err(e) => {
            assert!(!required, "WGPU_BACKEND is set but has no adapter: {e}");
            eprintln!("skipping paint surface GPU tests: no adapter ({e})");
            return None;
        }
    };
    let info = adapter.get_info();
    let (device, queue) = match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) {
        Ok(d) => d,
        Err(e) => {
            assert!(!required, "WGPU_BACKEND is set but {} has no device: {e}", info.name);
            eprintln!("skipping paint surface GPU tests: no device");
            return None;
        }
    };
    eprintln!("paint surface GPU tests on {:?} ({})", info.backend, info.name);
    Some(Gpu { device, queue, _lock: lock })
    }

/// The stamp shader parses and validates (naga, as wgpu does before any driver sees it), with no
/// adapter: the GPU tests below skip on runners without one.
#[test]
fn the_shader_validates_without_a_gpu() {
    let module = wgpu::naga::front::wgsl::parse_str(PAINT_SURFACE_SHADER).unwrap_or_else(|e| panic!("{}", e.emit_to_string(PAINT_SURFACE_SHADER)));
    wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::empty())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{}\n{e:?}", e.emit_to_string(PAINT_SURFACE_SHADER)));
    assert!(module.entry_points.iter().any(|e| e.name == "vs"), "a vertex entry point");
    assert!(module.entry_points.iter().any(|e| e.name == "fs"), "a fragment entry point");
}

/// `d` stamped on the CPU, over the surface's own pixel grid.
fn cpu_dab(b: &BrushSettings, d: &Dab, surface: (u32, u32)) -> (Rect, Vec<f32>, Vec<f32>) {
    let ctx = BrushContext::new(b);
    let rect = ctx.dab_rect(d, false);
    assert!(rect.x0 >= 0 && rect.y0 >= 0 && rect.x1 <= surface.0 as i32 && rect.y1 <= surface.1 as i32, "the dab must sit inside the surface");
    let mut cov = Vec::new();
    let mut depth = Vec::new();
    ctx.rasterize_into(d, false, rect, &mut cov, Some(&mut depth));
    (rect, cov, depth)
}

/// One dab, both ways.
#[test]
fn a_dab_lands_in_the_surface_like_the_cpu() {
    let Some(g) = gpu() else { return };
    let surface = (96u32, 96u32);
    let cases: [(&str, BrushSettings, Dab); 5] = [
        ("c3 soft", BrushSettings { c3_tip_math: true, hardness: 0.3, ..Default::default() }, Dab::round(Point::new(48.5, 47.25), 21.0, 0.62)),
        ("legacy soft", BrushSettings { hardness: 0.0, ..Default::default() }, Dab::round(Point::new(40.0, 52.0), 17.0, 0.8)),
        ("hard aliased", BrushSettings { hardness: 1.0, aliased: true, ..Default::default() }, Dab::round(Point::new(46.25, 49.5), 13.0, 0.9)),
        ("triangle", BrushSettings { triangle: true, hardness: 0.6, ..Default::default() }, Dab::round(Point::new(50.0, 50.0), 18.0, 0.71)),
        ("block eraser", BrushSettings { aliased: true, square: true, hardness: 0.9, ..Default::default() }, Dab::round(Point::new(45.0, 45.0), 11.0, 0.5)),
    ];
    for (label, b, mut d) in cases {
        if label == "triangle" {
            d.roundness = 0.6;
            d.angle = 0.4;
        }
        let (rect, cpu_cov, cpu_depth) = cpu_dab(&b, &d, surface);
        let ctx = BrushContext::new(&b);
        let job = ctx.stamp_job(&d, true).unwrap_or_else(|| panic!("{label}: no stamp job for a computed tip"));
        let mut st = PaintSurface::new(&g.device, &g.queue, surface.0, surface.1).unwrap_or_else(|e| panic!("{e}"));
        st.stamp(&g.device, &g.queue, &[DabStamp::of(&job, rect)]).unwrap_or_else(|e| panic!("{label}: {e}"));
        let read = st.read(&g.device, &g.queue).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(read.coverage.len(), (surface.0 * surface.1) as usize, "{label}: coverage length");
        let w = rect.width() as usize;
        let mut worst = 0.0f32;
        let mut cov_at = (0, 0);
        let mut depth_worst = 0.0f32;
        for y in 0..rect.height() as usize {
            for x in 0..w {
                let surface_at = (rect.y0 as usize + y) * surface.0 as usize + rect.x0 as usize + x;
                let gpu = read.coverage[surface_at];
                let cpu = cpu_cov[y * w + x];
                let df = (gpu - cpu).abs();
                if df > worst {
                    worst = df;
                    cov_at = (rect.x0 + x as i32, rect.y0 + y as i32);
                }
                let dz = (read.depth[surface_at] - cpu_depth[y * w + x]).abs();
                depth_worst = depth_worst.max(dz);
            }
        }
        assert!(worst <= TOL, "{label}: coverage differs by {worst:e} at {cov_at:?} (bound {TOL:e})");
        assert!(depth_worst <= TOL, "{label}: depth differs by {depth_worst:e}");
    }
}

/// Dabs drawn one after another build up as `CoverageMap::accumulate` builds them: the first dab
/// against an empty surface, the second against what the first left, up to the stroke's ceiling.
#[test]
fn dabs_accumulate_in_submission_order() {
    let Some(g) = gpu() else { return };
    let surface = (128u32, 128u32);
    let b = BrushSettings { c3_tip_math: true, hardness: 0.35, ..Default::default() };
    let ctx = BrushContext::new(&b);
    let dabs = [Dab::round(Point::new(56.0, 60.0), 26.0, 0.55), Dab::round(Point::new(68.0, 66.0), 26.0, 0.8)];
    let ceiling = 0.7f32;
    let mut expect_cov = vec![0.0f32; (surface.0 * surface.1) as usize];
    let mut expect_depth = vec![0.0f32; expect_cov.len()];
    let mut stamps = Vec::new();
    for d in &dabs {
        let rect = ctx.dab_rect(d, false);
        let mut cov = Vec::new();
        let mut depth = Vec::new();
        ctx.rasterize_into(d, false, rect, &mut cov, Some(&mut depth));
        let job = ctx.stamp_job(d, true).unwrap_or_else(|| panic!("no stamp job"));
        stamps.push(DabStamp::of(&job, rect));
        let w = rect.width() as usize;
        for y in 0..rect.height() as usize {
            for x in 0..w {
                let at = (rect.y0 as usize + y) * surface.0 as usize + rect.x0 as usize + x;
                let v = cov[y * w + x];
                if v <= 0.0 {
                    continue;
                }
                let c = expect_cov[at];
                expect_cov[at] = if ceiling > c { c + v * (ceiling - c) } else { c };
                expect_depth[at] = expect_depth[at].max(depth[y * w + x]);
            }
        }
    }
    let mut st = PaintSurface::new(&g.device, &g.queue, surface.0, surface.1).unwrap_or_else(|e| panic!("{e}"));
    st.set_ceiling(&g.queue, ceiling);
    st.stamp(&g.device, &g.queue, &stamps).unwrap_or_else(|e| panic!("{e}"));
    let read = st.read(&g.device, &g.queue).unwrap_or_else(|e| panic!("{e}"));
    let mut worst = 0.0f32;
    let mut at = (0, 0);
    for (i, (a, e)) in read.coverage.iter().zip(&expect_cov).enumerate() {
        let df = (a - e).abs();
        if df > worst {
            worst = df;
            at = (i as u32 % surface.0, i as u32 / surface.0);
        }
    }
    assert!(worst <= 2.0 * TOL, "coverage differs by {worst:e} at {at:?}");
    let mut worst_depth = 0.0f32;
    for (a, e) in read.depth.iter().zip(&expect_depth) {
        worst_depth = worst_depth.max((a - e).abs());
    }
    assert!(worst_depth <= TOL, "depth differs by {worst_depth:e}");
}

/// A stroke's second frame draws over the first: the surface keeps what the last frame laid.
#[test]
fn the_surface_keeps_what_earlier_frames_laid() {    let Some(g) = gpu() else { return };
    let b = BrushSettings { c3_tip_math: true, hardness: 0.5, ..Default::default() };
    let ctx = BrushContext::new(&b);
    let mut st = PaintSurface::new(&g.device, &g.queue, 128, 128).unwrap_or_else(|e| panic!("{e}"));
    let mut max_cov = 0.0f32;
    for i in 1..=3 {
        let d = Dab::round(Point::new(40.0 + 12.0 * f64::from(i), 64.0), 14.0, 0.4);
        let rect = ctx.dab_rect(&d, false);
        let job = ctx.stamp_job(&d, false).unwrap_or_else(|| panic!("no stamp job"));
        st.stamp(&g.device, &g.queue, &[DabStamp::of(&job, rect)]).unwrap_or_else(|e| panic!("{e}"));
    }
    let read = st.read(&g.device, &g.queue).unwrap_or_else(|e| panic!("{e}"));
    for c in read.coverage {
        max_cov = max_cov.max(c);
    }
    assert!(max_cov > 0.6, "three overlapping dabs built up ({max_cov})");
}

/// The surface carries the stroke's colour: every covered pixel reads back the colour the stroke
/// set, at the coverage's own precision (the CPU's `col` lane is this divided by `cov`).
#[test]
fn the_surface_carries_the_strokes_colour() {
    let Some(g) = gpu() else { return };
    let b = BrushSettings { c3_tip_math: true, hardness: 0.8, ..Default::default() };
    let ctx = BrushContext::new(&b);
    let mut d = Dab::round(Point::new(48.0, 48.0), 20.0, 0.9);
    let rect = ctx.dab_rect(&d, false);
    let color = [0.2f32, 0.45, 0.8];
    d.color = [color[0], color[1], color[2], 1.0];
    let job = ctx.stamp_job(&d, false).unwrap_or_else(|| panic!("no stamp job"));
    let mut st = PaintSurface::new(&g.device, &g.queue, 96, 96).unwrap_or_else(|e| panic!("{e}"));
    st.set_color(&g.queue, color);
    st.stamp(&g.device, &g.queue, &[DabStamp::of(&job, rect)]).unwrap_or_else(|e| panic!("{e}"));
    let read = st.read(&g.device, &g.queue).unwrap_or_else(|e| panic!("{e}"));
    let at = 48usize * 96 + 48;
    assert!(read.coverage[at] > 0.5, "the dab's centre is painted ({})", read.coverage[at]);
    let got = read.color[at];
    for (a, e) in got.iter().zip(&color) {
        assert!((a - e).abs() <= 5e-3, "colour {got:?} differs from {color:?}");
    }
}

/// A per-tip texture stamped on the GPU against the CPU's own: the same wrapped bilinear read and
/// the same combine mode, at the sampler's 8-bit filtering weights.
#[test]
fn a_per_tip_texture_matches_the_cpu() {
    let Some(g) = gpu() else { return };
    let surface = (128u32, 128u32);
    let data: Vec<f32> = (0..5).flat_map(|y| (0..8).map(move |x| ((x + y * 2) % 7) as f32 / 6.0)).collect();
    let tile = photocraft_paint::GrayTile::from_fn(8, 5, |x, y| ((x + y * 2) % 7) as f32 / 6.0);
    let textured = BrushSettings {
        c3_tip_math: true,
        hardness: 0.45,
        texture: photocraft_paint::Texture {
            enabled: true,
            each_tip: true,
            scale: 2.0,
            depth: 0.8,
            mode: photocraft_paint::MaskMode::Multiply,
            pattern: photocraft_paint::Pattern::Tile(tile),
            ..Default::default()
        },
        ..Default::default()
    };
    let plain = BrushSettings { c3_tip_math: true, hardness: 0.45, ..Default::default() };
    let d = Dab::round(Point::new(64.5, 63.25), 34.0, 0.85);
    let (rect, cpu_cov, _) = cpu_dab(&textured, &d, surface);
    let job = BrushContext::new(&plain).stamp_job(&d, false).unwrap_or_else(|| panic!("no stamp job"));
    let image = photocraft_paint::PatternImage { width: 8, height: 5, data };
    let mut st = PaintSurface::new(&g.device, &g.queue, 128, 128).unwrap_or_else(|e| panic!("{e}"));
    st.set_texture(
        &g.device,
        &g.queue,
        Some(&image),
        photocraft_gpu::TextureOptions { scale: 2.0, depth: 0.8, mode: 1, coordinate: 0, c3_math: false, each_tip: true, origin: [0.0, 0.0] },
    );
    st.stamp(&g.device, &g.queue, &[DabStamp::of(&job, rect)]).unwrap_or_else(|e| panic!("{e}"));
    let read = st.read(&g.device, &g.queue).unwrap_or_else(|e| panic!("{e}"));
    let w = rect.width() as usize;
    let mut worst = 0.0f32;
    let mut at = (0, 0);
    for y in 0..rect.height() as usize {
        for x in 0..w {
            let gpu = read.coverage[(rect.y0 as usize + y) * 128 + rect.x0 as usize + x];
            let cpu = cpu_cov[y * w + x];
            let df = (gpu - cpu).abs();
            if df > worst {
                worst = df;
                at = (rect.x0 + x as i32, rect.y0 + y as i32);
            }
        }
    }
    assert!(worst <= 1e-2, "textured coverage differs by {worst:e} at {at:?} (gpu {}, cpu {})", read.coverage[(at.1 as usize) * 128 + at.0 as usize], cpu_cov[(at.1 - rect.y0) as usize * w + (at.0 - rect.x0) as usize]);
}
