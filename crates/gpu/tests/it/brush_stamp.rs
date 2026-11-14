//! The GPU dab stamp against the CPU rasteriser, pixel by pixel: the C3 tip maths and the legacy
//! Photoshop-table tip, across radii, hardness, roundness, angles, flips, and the aliased / Block /
//! Triangle / Wet Edges variants. Plus a whole stroke painted through the injected stamper against
//! the same stroke painted by the CPU, to check the pipeline seam end to end.
//!
//! Skips, with a message, when there is no adapter or device; with `WGPU_BACKEND` set those fail
//! instead, as in `parity.rs`. The shader is validated without a GPU
//! ([`the_shader_validates_without_a_gpu`]), so a WGSL error fails on every runner.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use photocraft_color::PixelFormat;
use photocraft_geom::{Point, Rect};
use photocraft_gpu::{BRUSH_STAMP_SHADER, BrushStamp, GpuStamper};
use photocraft_paint::{BrushContext, BrushSettings, Dab, DabStamper, StampJob, StamperSlot, StrokePoint, StrokeRenderer};
use photocraft_raster::Surface;

/// Largest coverage difference from the CPU rasteriser. C3's edge is two `exp` calls the GPU
/// evaluates a few ulps off the CPU's, and the legacy tip's `soft_exp` is exact arithmetic, so
/// what is left is the driver's transcendental rounding. 1e-5 is a 400× margin under one 8-bit
/// step (1/255), the tolerance the compositor works to.
const TOL: f32 = 1e-5;
/// The coverage a soft tip still has at its reach (`soft_exp` of the 2.6-shoulder tail, ~8e-4):
/// the most a rim pixel can lose by the distance's last bit.
const RIM_TOL: f32 = 2e-3;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    stamp: BrushStamp,
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
            eprintln!("skipping brush stamp GPU tests: no adapter ({e})");
            return None;
        }
    };
    let info = adapter.get_info();
    let (device, queue) = match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) {
        Ok(d) => d,
        Err(e) => {
            assert!(!required, "WGPU_BACKEND is set but {} has no device: {e}", info.name);
            eprintln!("skipping brush stamp GPU tests: no device");
            return None;
        }
    };
    eprintln!("brush stamp GPU tests on {:?} ({})", info.backend, info.name);
    let stamp = BrushStamp::new(&device).unwrap_or_else(|e| panic!("the stamp pipeline didn't build on {}: {}", info.name, e));
    Some(Gpu { device, queue, stamp, _lock: lock })
}

/// The stamp shader parses and validates (naga, as wgpu does before any driver sees it), with no
/// adapter: the GPU tests below skip on runners without one.
#[test]
fn the_shader_validates_without_a_gpu() {
    let module = wgpu::naga::front::wgsl::parse_str(BRUSH_STAMP_SHADER).unwrap_or_else(|e| panic!("{}", e.emit_to_string(BRUSH_STAMP_SHADER)));
    wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::empty())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(BRUSH_STAMP_SHADER)));
    assert!(module.entry_points.iter().any(|e| e.name == "main"));
}

/// A dab stamped both ways: the largest difference, how many pixels exceed [`TOL`], and how many
/// of those are the tip's own rim (one side paints the tip's reach coverage, the other cuts it).
struct Diff {
    max: f32,
    over: usize,
    rim: usize,
    at: (i32, i32),
    gpu: f32,
    cpu: f32,
}

impl Diff {
    fn of(out: &[f32], cpu: &[f32], rect: Rect) -> Self {
        let mut diff = Diff { max: 0.0, over: 0, rim: 0, at: (0, 0), gpu: 0.0, cpu: 0.0 };
        for (i, (a, b)) in out.iter().zip(cpu).enumerate() {
            let df = (a - b).abs();
            if df > TOL {
                diff.over += 1;
                if *a == 0.0 || *b == 0.0 {
                    diff.rim += 1;
                }
            }
            if df > diff.max {
                diff.max = df;
                diff.gpu = *a;
                diff.cpu = *b;
                diff.at = (rect.x0 + (i % rect.width() as usize) as i32, rect.y0 + (i / rect.width() as usize) as i32);
            }
        }
        diff
    }
}

/// Stamps `d` both ways and diffs the coverage (and depth). The job comes from the brush engine
/// itself ([`BrushContext::stamp_job`]) — the path painting takes.
fn compare(g: &Gpu, b: &BrushSettings, d: &Dab, with_depth: bool) -> (Diff, Diff) {
    let ctx = BrushContext::new(b);
    let rect = ctx.dab_rect(d, false);
    let job = ctx.stamp_job(d, with_depth).unwrap_or_else(|| panic!("no stamp job for a dab the GPU claims to draw"));
    let (mut cpu, mut cpu_depth) = (Vec::new(), Vec::new());
    ctx.rasterize_into(d, false, rect, &mut cpu, with_depth.then_some(&mut cpu_depth));
    let out = match g.stamp.stamp(&g.device, &g.queue, &job, rect, with_depth) {
        Ok(o) => o,
        Err(e) => panic!("GPU stamp refused a dab the CPU stamps: {e}"),
    };
    assert_eq!(out.coverage.len(), cpu.len(), "coverage length");
    let coverage = Diff::of(&out.coverage, &cpu, rect);
    let depth = if with_depth {
        assert_eq!(out.depth.len(), cpu_depth.len(), "depth length");
        Diff::of(&out.depth, &cpu_depth, rect)
    } else {
        Diff { max: 0.0, over: 0, rim: 0, at: (0, 0), gpu: 0.0, cpu: 0.0 }
    };
    (coverage, depth)
}

/// Every pixel agrees to [`TOL`].
fn check(g: &Gpu, label: &str, b: &BrushSettings, d: &Dab, with_depth: bool) {
    let (c, z) = compare(g, b, d, with_depth);
    assert!(c.over == 0, "{label}: {} pixels differ, worst {:e} at ({}, {}) gpu={} cpu={}", c.over, c.max, c.at.0, c.at.1, c.gpu, c.cpu);
    assert!(z.over == 0, "{label}: depth differs, worst {:e} at ({}, {})", z.max, z.at.0, z.at.1);
}

/// [`check`], but a tip whose rim is wider than a pixel's worth of f32 distance is allowed to flip
/// rim pixels: the CPU distance and the GPU's differ by one ulp of `d²` (≈ 1e-4 px at a 1500 px
/// radius), and at the very edge of the reach that decides whether a pixel paints the tip's own
/// reach coverage (≤ 1e-3 by definition of the reach) or nothing. Only that flip is allowed: no
/// pixel may differ by more than the tip's reach coverage, and no pixel may shift inside the tip.
fn check_rim(g: &Gpu, label: &str, b: &BrushSettings, d: &Dab) {
    let (c, _) = compare(g, b, d, false);
    eprintln!("[{label}] {} rim flips, worst {:e}", c.over, c.max);
    assert!(c.over <= 8, "{label}: {} rim flips, more than a rim band should hold", c.over);
    assert!(c.rim == c.over, "{label}: {} pixels differ off the rim (worst {:e} at ({}, {}) gpu={} cpu={})", c.over - c.rim, c.max, c.at.0, c.at.1, c.gpu, c.cpu);
    assert!(c.max <= RIM_TOL, "{label}: rim flip of {:e} at ({}, {}) (bound {RIM_TOL:e})", c.max, c.at.0, c.at.1);
}

/// The grid `computed_tips_match_scalar_bits` runs on the CPU, both maths: radii across the
/// small-tip and sub-pixel thresholds, every hardness band, every roundness, a negative alpha.
#[test]
fn computed_tips_match_the_cpu_bits() {
    let Some(g) = gpu() else { return };
    for c3 in [false, true] {
        for aliased in [false, true] {
            for hardness in [-0.5, 0.0, 0.1, 0.5, 0.9999, 1.0, 1.5] {
                for radius in [0.5, 1.0, 2.99, 3.0, 3.01, 15.0, 31.5] {
                    for roundness in [0.0, 0.1, 0.4, 1.0] {
                        for alpha in [-0.5, 0.0, 0.37, 1.0] {
                            let b = BrushSettings { c3_tip_math: c3, aliased, hardness, ..Default::default() };
                            let mut d = Dab::round(Point::new(-31.25, 63.75), radius, alpha);
                            d.roundness = roundness;
                            d.flip_x = true;
                            d.flip_y = true;
                            check(&g, &format!("c3={c3} aliased={aliased} hardness={hardness} radius={radius} roundness={roundness} alpha={alpha}"), &b, &d, false);
                        }
                    }
                }
            }
        }
    }
}

/// Rotation and Brush Projection: where the driver's `sin`/`cos` enters.
#[test]
fn rotated_and_projected_tips_match_the_cpu() {
    let Some(g) = gpu() else { return };
    for c3 in [false, true] {
        for (angle, proj_scale, proj_angle) in [(0.0, 1.0, 0.0), (0.7, 1.0, 0.0), (-2.3, 1.0, 0.0), (0.0, 0.6, 0.3), (1.1, 0.4, -0.8)] {
            for hardness in [0.0, 0.25, 0.6, 1.0] {
                let b = BrushSettings { c3_tip_math: c3, hardness, ..Default::default() };
                let mut d = Dab::round(Point::new(63.5, -0.25), 15.0, 0.47);
                d.roundness = 0.4;
                d.angle = angle;
                d.proj_scale = proj_scale;
                d.proj_angle = proj_angle;
                check(&g, &format!("c3={c3} hardness={hardness} angle={angle} proj={proj_scale}"), &b, &d, false);
            }
        }
    }
}

/// Block (aliased square), Triangle, and Wet Edges on the dab (and its depth for the stroke).
#[test]
fn exceptional_tip_shapes_match_the_cpu() {
    let Some(g) = gpu() else { return };
    for c3 in [false, true] {
        for hardness in [0.0, 0.4, 1.0] {
            let block = BrushSettings { c3_tip_math: c3, aliased: true, square: true, hardness, ..Default::default() };
            let mut d = Dab::round(Point::new(50.3, 60.7), 12.0, 0.8);
            d.roundness = 0.5;
            check(&g, "block", &block, &d, false);

            let b = BrushSettings { c3_tip_math: c3, triangle: true, hardness, ..Default::default() };
            let mut d = Dab::round(Point::new(50.3, 60.7), 18.0, 0.8);
            d.roundness = 0.6;
            d.angle = 0.4;
            check(&g, "triangle", &b, &d, false);

            let b = BrushSettings { c3_tip_math: c3, wet_edges: true, hardness, ..Default::default() };
            for with_depth in [false, true] {
                let mut d = Dab::round(Point::new(50.3, 60.7), 20.0, 0.8);
                d.roundness = 0.7;
                check(&g, if with_depth { "wet edges depth" } else { "wet edges" }, &b, &d, with_depth);
            }
        }
    }
}

/// Tips against the canvas edge and off-canvas coverage survive (the layer is an infinite plane).
#[test]
fn off_canvas_tips_match_the_cpu() {
    let Some(g) = gpu() else { return };
    for c3 in [false, true] {
        let b = BrushSettings { c3_tip_math: c3, hardness: 0.0, ..Default::default() };
        let mut d = Dab::round(Point::new(-1050.25, -1050.75), 1500.0, 0.73);
        d.roundness = 1.0;
        check_rim(&g, "large corner dab", &b, &d);
        let b = BrushSettings { c3_tip_math: c3, hardness: 0.5, ..Default::default() };
        let mut d = Dab::round(Point::new(0.0, 0.0), 30.0, 0.5);
        d.roundness = 0.3;
        check(&g, "dab on the canvas corner", &b, &d, false);
    }
}

/// The dabs the kernel can't draw never become a job: painting keeps them on the CPU.
#[test]
fn cpu_only_dabs_become_no_job() {
    let tip = photocraft_paint::GrayTile::from_fn(8, 5, |x, y| ((x + y) % 7) as f32 / 6.0);
    let plain = BrushSettings { c3_tip_math: true, ..Default::default() };
    let dab = Dab::round(Point::new(10.0, 10.0), 12.0, 1.0);
    assert!(BrushContext::new(&plain).stamp_job(&dab, false).is_some(), "a plain computed tip is a job");

    let cases = [
        ("sampled tip", BrushSettings { tip: photocraft_paint::TipShape::Sampled(tip.clone()), ..plain.clone() }, dab),
        // `noise` is the legacy alias of Grain: paint rewrites it to `grain.is_some()`.
        ("grain", BrushSettings { grain: Some(Default::default()), ..plain.clone() }, dab),
        (
            "per-tip texture",
            BrushSettings { texture: photocraft_paint::Texture { enabled: true, each_tip: true, ..Default::default() }, ..plain.clone() },
            dab,
        ),
        (
            "single-dab wet outline",
            BrushSettings {
                wet_edges: true,
                wet_edge: Some(photocraft_paint::material::WetEdgeSettings {
                    outline: photocraft_paint::material::WetOutline::Single,
                    ..Default::default()
                }),
                ..plain.clone()
            },
            dab,
        ),
        ("non-finite radius", plain.clone(), Dab { radius: f32::NAN, ..dab }),
        ("non-finite angle", plain.clone(), Dab { angle: f32::INFINITY, ..dab }),
    ];
    for (label, b, d) in cases {
        let ctx = BrushContext::new(&b);
        assert!(ctx.stamp_job(&d, false).is_none(), "{label} must stay a CPU dab");
    }
}

/// The empty rectangle is a no-op on both.
#[test]
fn empty_rect_stamps_nothing() {
    let Some(g) = gpu() else { return };
    let b = BrushSettings { c3_tip_math: true, ..Default::default() };
    let ctx = BrushContext::new(&b);
    let d = Dab::round(Point::new(2.0, 2.0), 6.0, 1.0);
    let job = ctx.stamp_job(&d, false).unwrap_or_else(|| panic!("a plain computed tip is a job"));
    let rect = Rect::new(0, 0, 0, 0);
    for with_depth in [false, true] {
        let out = g.stamp.stamp(&g.device, &g.queue, &job, rect, with_depth).unwrap_or_else(|e| panic!("{e}"));
        assert!(out.coverage.is_empty() && out.depth.is_empty());
    }
}

/// Counts what reached the GPU, so "the stroke painted through the GPU" isn't taken on trust.
struct Counting {
    inner: GpuStamper,
    offered: AtomicUsize,
    stamped: AtomicUsize,
}

impl DabStamper for Counting {
    fn stamp(&self, job: &StampJob, rect: Rect, coverage: &mut Vec<f32>, depth: Option<&mut Vec<f32>>) -> bool {
        self.offered.fetch_add(1, Ordering::Relaxed);
        let done = self.inner.stamp(job, rect, coverage, depth);
        if done {
            self.stamped.fetch_add(1, Ordering::Relaxed);
        }
        done
    }
}

/// A whole stroke through the pipeline seam: the same points painted with the injected GPU stamper
/// and by the CPU give the same 8-bit pixels. Run at 8 bits, the depth painting actually writes,
/// on both maths and with Wet Edges.
#[test]
fn a_whole_stroke_paints_the_same_pixels_through_the_gpu() {
    let Some(g) = gpu() else { return };
    let (device, queue) = (g.device.clone(), g.queue.clone());
    let fmt = PixelFormat::RGBA8;
    let points: Vec<StrokePoint> = (0..420)
        .map(|i| {
            let t = i as f64 / 419.0;
            let x = 40.0 + 176.0 * t;
            let y = 128.0 + 64.0 * (t * std::f64::consts::PI * 2.0).sin();
            StrokePoint::new(x, y, 0.35 + 0.65 * t as f32)
        })
        .collect();
    for (label, c3, wet) in [("legacy tip", false, false), ("C3 tip", true, false), ("C3 tip, wet edges", true, true)] {
        let brush = BrushSettings { size: 26.0, hardness: 0.4, spacing: 0.25, c3_tip_math: c3, wet_edges: wet, color: [0.2, 0.45, 0.9, 1.0], ..Default::default() };
        let mut pre = Surface::new(fmt);
        pre.fill_rect(Rect::new(0, 0, 256, 256), &[0.5, 0.5, 0.5, 1.0]);
        let (mut cpu, mut on_gpu) = (pre.clone(), pre.clone());

        let mut renderer = StrokeRenderer::new(&brush, Some(fmt), 1.0);
        renderer.push(&points);
        renderer.finish();
        let damaged = renderer.composite(&pre, &mut cpu, None, false, true);
        let dabs = renderer.dab_count();

        let stamp = GpuStamper::new(device.clone(), queue.clone()).unwrap_or_else(|e| panic!("{e}"));
        let counting = Arc::new(Counting { inner: stamp, offered: AtomicUsize::new(0), stamped: AtomicUsize::new(0) });
        let mut renderer = StrokeRenderer::new(&brush, Some(fmt), 1.0);
        renderer.ctx.set_stamper(StamperSlot::new(counting.clone()));
        renderer.push(&points);
        renderer.finish();
        let damaged_gpu = renderer.composite(&pre, &mut on_gpu, None, false, true);

        assert_eq!(damaged, damaged_gpu, "{label}: both runs damage the same rectangle");
        assert!(dabs > 20, "{label}: a real stroke ({dabs} dabs)");
        assert_eq!(counting.offered.load(Ordering::Relaxed), dabs, "{label}: every dab was offered to the GPU");
        assert_eq!(counting.stamped.load(Ordering::Relaxed), dabs, "{label}: the GPU stamped every dab");

        // 8-bit output: a coverage difference of 1e-5 can move a channel by one quantization step
        // at most, so one step is the bound; nothing may differ by more.
        let (a, b) = (cpu.read_region(damaged), on_gpu.read_region(damaged));
        assert_eq!(a.len(), b.len(), "{label}: same region");
        let step = 1.0 / 255.0;
        let mut moved = 0usize;
        let mut max = 0.0f32;
        for (x, y) in a.iter().zip(&b) {
            let df = (x - y).abs();
            if df > step / 2.0 + 1e-6 {
                moved += 1;
            }
            max = max.max(df);
        }
        assert!(max <= step + 1e-6, "{label}: a channel differs by {max} ({moved} of {} samples), more than one 8-bit step", a.len());
        eprintln!("[{label}] {dabs} dabs on the GPU, {moved} of {} channel samples moved, worst {max:.4}", a.len());
    }
}
