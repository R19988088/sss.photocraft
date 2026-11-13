//! Brush dab stamping on the GPU: the computed-tip paths (the C3 tip maths and the legacy
//! Photoshop-table tip) with their aliased, Block, Triangle and Wet Edges variants.
//!
//! A dab reaches here as a [`StampJob`] — the brush engine's own resolved numbers and geometry
//! (`crates/paint/src/stamp.rs`), so the GPU and CPU answers share every dab-invariant rounding;
//! this module only evaluates the per-pixel maths (`brush_stamp.wgsl`). Dabs the kernel can't
//! draw (sampled tips, per-tip texture, Noise, Grain, single-dab Wet Edges) never become a job, so
//! nothing is silently dropped.
//!
//! [`GpuStamper`] is what the app injects into a `BrushContext`
//! ([`photocraft_paint::BrushContext::set_stamper`]); [`BrushStamp::stamp`] is the same pass
//! called directly, one dispatch per dab (the maths a batched dab list will call next).

use std::sync::Arc;

use photocraft_geom::Rect;
use photocraft_paint::{DabStamper, StampJob};

use crate::Unsupported;

/// The pass's WGSL source.
pub const BRUSH_STAMP_SHADER: &str = include_str!("brush_stamp.wgsl");

/// `Stamp` in the shader: eight rows of 16 bytes.
const PARAMS_SIZE: u64 = 8 * 16;

/// What one stamped dab covered.
pub struct StampOutput {
    /// Row-major coverage over the rectangle, already multiplied by the dab's alpha.
    pub coverage: Vec<f32>,
    /// `1 − rn` per pixel (Wet Edges' depth inside the dab); all zero without `with_depth`.
    pub depth: Vec<f32>,
}

/// The stamp pipeline. Create once per device.
pub struct BrushStamp {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl BrushStamp {
    /// Builds the pipeline. A shader or pipeline the driver can't build is an `Err`, so the caller
    /// keeps stamping on the CPU.
    pub fn new(device: &wgpu::Device) -> Result<Self, Unsupported> {
        match crate::first_error(device, || Self::build(device)) {
            (Ok(dev), None) => Ok(dev),
            (_, Some(e)) => Err(Unsupported(format!("couldn't build the brush stamp pipeline: {e}"))),
            (Err(e), None) => Err(e),
        }
    }

    fn build(device: &wgpu::Device) -> Result<Self, Unsupported> {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pc_brush_stamp"),
            source: wgpu::ShaderSource::Wgsl(BRUSH_STAMP_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pc_brush_stamp_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(PARAMS_SIZE),
                    },
                    count: None,
                },
                storage_entry(1),
                storage_entry(2),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pc_brush_stamp_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("pc_brush_stamp_pipeline"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ok(Self { pipeline, layout })
    }

    /// Stamps `job` over `rect` (row-major, `rect.width() * rect.height()` pixels). `with_depth`
    /// also returns `1 − rn` per pixel, for the stroke to apply Wet Edges over its accumulated
    /// coverage.
    pub fn stamp(&self, device: &wgpu::Device, queue: &wgpu::Queue, job: &StampJob, rect: Rect, with_depth: bool) -> Result<StampOutput, Unsupported> {
        let (result, error) = crate::first_error(device, || self.stamp_inner(device, queue, job, rect, with_depth));
        match error {
            Some(e) => Err(Unsupported(format!("GPU brush stamp failed: {e}"))),
            None => result,
        }
    }

    fn stamp_inner(&self, device: &wgpu::Device, queue: &wgpu::Queue, job: &StampJob, rect: Rect, with_depth: bool) -> Result<StampOutput, Unsupported> {
        let (w, h) = (rect.width(), rect.height());
        if w == 0 || h == 0 {
            return Ok(StampOutput { coverage: Vec::new(), depth: Vec::new() });
        }
        let pixels = u64::from(w) * u64::from(h);
        // WebGPU storage bindings and the dispatch grid are bounded; a huge single dab keeps
        // the CPU path, which rasterises it in rows.
        const MAX_DISPATCH: u32 = 65_535;
        let limits = device.limits();
        if w > MAX_DISPATCH || h > MAX_DISPATCH || pixels * 4 > u64::from(limits.max_storage_buffer_binding_size) {
            return Err(Unsupported(format!("a {w}×{h} dab doesn't fit the GPU's dispatch or storage limits")));
        }
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pc_brush_stamp_params"),
            size: PARAMS_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&params, 0, bytemuck::cast_slice(&param_words(job, rect, with_depth)));
        let storage = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: pixels * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let coverage = storage("pc_brush_stamp_coverage");
        let depth = storage("pc_brush_stamp_depth");
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pc_brush_stamp_bind_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: coverage.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: depth.as_entire_binding() },
            ],
        });
        let staging = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: pixels * 4,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let coverage_out = staging("pc_brush_stamp_coverage_staging");
        let depth_out = staging("pc_brush_stamp_depth_staging");
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pc_brush_stamp") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("pc_brush_stamp_pass"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
        encoder.copy_buffer_to_buffer(&coverage, 0, &coverage_out, 0, pixels * 4);
        encoder.copy_buffer_to_buffer(&depth, 0, &depth_out, 0, pixels * 4);
        let index = queue.submit([encoder.finish()]);
        coverage_out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        depth_out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).map_err(|e| Unsupported(format!("GPU wait failed: {e}")))?;
        let read = |buffer: &wgpu::Buffer| -> Result<Vec<f32>, Unsupported> {
            let data = buffer.slice(..).get_mapped_range().map_err(|e| Unsupported(format!("GPU readback failed: {e:?}")))?;
            let out = bytemuck::pod_collect_to_vec::<u8, f32>(&data);
            drop(data);
            buffer.unmap();
            Ok(out)
        };
        Ok(StampOutput { coverage: read(&coverage_out)?, depth: read(&depth_out)? })
    }
}

/// The stamper the brush engine injects ([`photocraft_paint::BrushContext::set_stamper`]): the
/// stamp pass bound to one device and queue.
pub struct GpuStamper {
    device: wgpu::Device,
    queue: wgpu::Queue,
    stamp: BrushStamp,
}

impl GpuStamper {
    /// Builds the pass for the app's device. `Err` means the adapter can't run it: keep the CPU.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Result<Self, Unsupported> {
        let stamp = BrushStamp::new(&device)?;
        Ok(Self { device, queue, stamp })
    }
}

/// Installs the GPU stamper for the whole process
/// ([`photocraft_paint::set_process_stamper`]): what the app calls once its canvas runs on the GPU,
/// so every stroke rendered anywhere (the engine, the UI) stamps on the GPU. An `Err` leaves
/// painting on the CPU, dab by dab.
pub fn install_process_stamper(device: wgpu::Device, queue: wgpu::Queue) -> Result<(), Unsupported> {
    let stamper = GpuStamper::new(device, queue)?;
    photocraft_paint::set_process_stamper(photocraft_paint::StamperSlot::new(Arc::new(stamper)));
    Ok(())
}

impl DabStamper for GpuStamper {
    fn stamp(&self, job: &StampJob, rect: Rect, coverage: &mut Vec<f32>, depth: Option<&mut Vec<f32>>) -> bool {
        let with_depth = depth.is_some();
        match self.stamp.stamp(&self.device, &self.queue, job, rect, with_depth) {
            Ok(out) => {
                if out.coverage.len() != coverage.len() {
                    return false;
                }
                coverage.copy_from_slice(&out.coverage);
                if let Some(dp) = depth
                    && out.depth.len() == dp.len()
                {
                    dp.copy_from_slice(&out.depth);
                }
                true
            }
            Err(_) => false,
        }
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// `Stamp` in the shader, packed as eight `vec4` rows (see `brush_stamp.wgsl`).
fn param_words(job: &StampJob, rect: Rect, with_depth: bool) -> [u32; 32] {
    let g = job.geometry;
    let mut words = [0u32; 32];
    words[0..4].copy_from_slice(&[rect.x0 as u32, rect.y0 as u32, rect.width(), rect.height()]);
    words[4..8].copy_from_slice(&[job.center[0].to_bits(), job.center[1].to_bits(), job.proj_scale.to_bits(), job.proj_angle.to_bits()]);
    words[8..12].copy_from_slice(&[job.radius.to_bits(), job.angle.to_bits(), job.roundness.to_bits(), job.alpha.to_bits()]);
    words[12..16].copy_from_slice(&[job.hardness.to_bits(), job.flip[0].to_bits(), job.flip[1].to_bits(), 0]);
    words[16..20].copy_from_slice(&[job.c3_tip as u32, job.aliased as u32, job.square as u32, job.triangle as u32]);
    words[20..24].copy_from_slice(&[job.wet as u32, with_depth as u32, g.small as u32, 0]);
    words[24..28].copy_from_slice(&[g.ro.to_bits(), g.rm.to_bits(), g.reach.to_bits(), g.reach_squared.to_bits()]);
    words[28..32].copy_from_slice(&[g.small_reach_squared.to_bits(), 0, 0, 0]);
    words
}
