//! The batch mask backend against the CPU: a frame's dabs (overlapping, textured, of different
//! sizes, one rotated) come back tile by tile exactly as `rasterize_into` lays them, and the same
//! batch answers the same twice (the atlas keeps nothing between batches).

use photocraft_geom::Point;
use photocraft_gpu::GpuMasker;
use photocraft_paint::stamp::{DabMaskBatch, MaskBatch};
use photocraft_paint::{
    brush::{TextureCoordinateMode, TipShape}, BrushContext, BrushSettings, Dab, GrayTile, MaskMode, Pattern, Texture,
};

/// Two 16-bit float steps (the surface's format) with a margin: a batch goes through the same
/// attachments as a single dab, but a big tip's gradient lands on values whose 16-bit step is
/// coarser than the 1.5e-3 the per-dab tests hold to. Still far under the compositor's 1/255.
const TOL: f32 = 4e-3;

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
            eprintln!("skipping mask batch GPU tests: no adapter ({e})");
            return None;
        }
    };
    let (device, queue) = match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) {
        Ok(d) => d,
        Err(e) => {
            assert!(!required, "WGPU_BACKEND is set but {} has no device: {e}", adapter.get_info().name);
            eprintln!("skipping mask batch GPU tests: no device");
            return None;
        }
    };
    eprintln!("mask batch GPU tests on {:?} ({})", adapter.get_info().backend, adapter.get_info().name);
    Some(Gpu { device, queue, _lock: lock })
}

fn textured() -> BrushSettings {
    let tile = GrayTile::from_fn(8, 8, |x, y| ((x * 3 + y * 5) % 7) as f32 / 6.0);
    BrushSettings {
        c3_tip_math: true,
        hardness: 0.4,
        texture: Texture {
            enabled: true,
            each_tip: true,
            scale: 2.5,
            depth: 0.7,
            mode: MaskMode::Multiply,
            coordinate_mode: TextureCoordinateMode::StrokeLocked,
            pattern: Pattern::Tile(tile),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A big-brush frame: overlapping dabs, two sizes, rotated, some with their own texture depth —
/// enough pixels that the backend (not the CPU) is the one that rasterises them.
fn frame() -> Vec<Dab> {
    let mut dabs = Vec::new();
    for i in 0..10 {
        let mut d = Dab::round(Point::new(f64::from(200 + i * 30), f64::from(200 + (i % 3) * 14)), 60.0 + (i % 2) as f32 * 8.0, 0.5);
        d.depth = if i % 3 == 0 { 0.35 } else { 1.0 };
        d.roundness = 0.8;
        d.angle = i as f32 * 0.17;
        dabs.push(d);
    }
    dabs
}

fn batch_for(brush: &BrushSettings, dabs: &[Dab]) -> MaskBatch {
    let ctx = BrushContext::new(brush);
    let mut batch = MaskBatch { jobs: Vec::new(), rects: Vec::new(), texture: ctx.mask_texture(), tip: ctx.mask_tip() };
    for d in dabs {
        let job = ctx.mask_job(d, false).unwrap_or_else(|| panic!("a textured dab must be maskable"));
        batch.jobs.push(job);
        batch.rects.push(ctx.dab_rect(d, false));
    }
    batch
}

/// A sampled tip: a soft 64×64 `Shape`-like bitmap, so every dab samples the pyramid.
fn sampled() -> BrushSettings {
    let tip = GrayTile::from_fn(64, 64, |x, y| {
        let (dx, dy) = (x as f32 - 31.5, y as f32 - 31.5);
        let d = (dx * dx + dy * dy).sqrt() / 32.0;
        (1.0 - d).clamp(0.0, 1.0)
    });
    BrushSettings { tip: TipShape::Sampled(tip), hardness: 0.7, ..Default::default() }
}

#[test]
fn a_sampled_tip_masks_like_the_cpu() {
    let Some(g) = gpu() else { return };
    let masker = GpuMasker::new(g.device.clone(), g.queue.clone());
    let brush = sampled();
    let dabs = frame();
    let batch = batch_for(&brush, &dabs);
    assert!(batch.jobs[0].tip_inv_scale > 0.0, "the batch must carry the tip's scale");
    assert!(batch.tip.is_some(), "the batch must carry the tip bitmap");
    let masks = masker.masks(&batch).unwrap_or_else(|| panic!("the batch must fit the atlas"));
    assert_eq!(masks.len(), dabs.len());
    let ctx = BrushContext::new(&brush);
    for (i, (mask, rect)) in masks.iter().zip(&batch.rects).enumerate() {
        let mut cov = Vec::new();
        ctx.rasterize_into(&dabs[i], false, *rect, &mut cov, None);
        assert_eq!(mask.coverage.len(), cov.len(), "dab {i} over {rect:?}");
        let worst = mask.coverage.iter().zip(&cov).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst <= TOL, "dab {i}: a sampled tip differs by {worst:e} over {rect:?}");
    }
}

#[test]
fn a_batch_masks_like_the_cpu() {
    let Some(g) = gpu() else { return };
    let masker = GpuMasker::new(g.device.clone(), g.queue.clone());
    let brush = textured();
    let dabs = frame();
    let batch = batch_for(&brush, &dabs);
    assert!(batch.texture.image.is_some(), "the stroke must carry its pattern");
    let masks = masker.masks(&batch).unwrap_or_else(|| panic!("the batch must fit the atlas"));
    assert_eq!(masks.len(), dabs.len());
    let ctx = BrushContext::new(&brush);
    for (i, (mask, rect)) in masks.iter().zip(&batch.rects).enumerate() {
        let mut cov = Vec::new();
        let mut depth = Vec::new();
        ctx.rasterize_into(&dabs[i], false, *rect, &mut cov, Some(&mut depth));
        assert_eq!(mask.coverage.len(), cov.len(), "dab {i} over {rect:?}");
        let worst = mask.coverage.iter().zip(&cov).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst <= TOL, "dab {i}: coverage differs by {worst:e} over {rect:?}");
        let worst = mask.depth.iter().zip(&depth).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst <= TOL, "dab {i}: depth differs by {worst:e}");
    }
}

#[test]
fn the_atlas_keeps_nothing_between_batches() {
    let Some(g) = gpu() else { return };
    let masker = GpuMasker::new(g.device.clone(), g.queue.clone());
    let brush = textured();
    let dabs = frame();
    let batch = batch_for(&brush, &dabs);
    let first = masker.masks(&batch).unwrap_or_else(|| panic!("the batch must fit the atlas"));
    let again = masker.masks(&batch).unwrap_or_else(|| panic!("the second batch must fit too"));
    for (a, b) in first.iter().zip(&again) {
        let worst = a.coverage.iter().zip(&b.coverage).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        assert!(worst <= 1e-4, "the atlas leaks state between batches ({worst:e})");
    }
}
