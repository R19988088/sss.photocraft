// C3 colour transport over the GPU: the per-dab carrier feedback of
// `photocraft_paint::transport::Transport` as two compute passes on storage buffers.
//
// It exists because a big brush's smear is two full-resolution resamples per dab (a 900 px dab is
// 810 k pixels ×2), which costs ~17 ms of CPU per dab at the limit of memory latency. The passes
// here do the same arithmetic with the same clamps, one thread per pixel.
//
// The lane's pixels ping-pong between `layer_a`/`layer_b` and the carrier between `carrier_a`/
// `carrier_b`, so nothing is read while it is written. A whole frame is one submit: every dab's
// uniform and coverage sits at its own offset in two shared buffers, so nothing has to be written
// between passes; one readback at the end brings the pixels home.

use std::sync::Mutex;

use photocraft_paint::stamp::{DabTransport, TransportFrame};

const SHADER: &str = include_str!("paint_transport.wgsl");

/// The transport shader, for the fingerprint the GPU backend reports.
pub const TRANSPORT_SHADER: &str = SHADER;

/// Working pixels a frame may pin on the GPU (16 bytes a pixel).
const MAX_REGION_PIXELS: i64 = 4_000_000;
/// Dabs one frame may hand the backend (a frame is a handful: 1%-spacing big dabs).
const MAX_DABS: usize = 64;
const MAX_SIDE: u32 = 4096;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    region: [u32; 4],
    rect: [u32; 4],
    center: [f32; 4],
    flow: [f32; 4],
    misc: [u32; 4],
}

const PARAMS_SIZE: u64 = 256;

struct Buffers {
    layer: [wgpu::Buffer; 2],
    layer_region: (u32, u32),
    layer_cur: usize,
    carrier: [wgpu::Buffer; 2],
    carrier_side: u32,
    carrier_have: usize,
    /// Whether the carrier holds the previous dab's (false right after a resize).
    carrier_valid: bool,
    /// One frame's params and coverages, each dab at its own offset.
    params: wgpu::Buffer,
    coverage: wgpu::Buffer,
    stride_px: u64,
    dabs_cap: usize,
}

fn storage_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(64),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl Buffers {
    fn new(device: &wgpu::Device, region: (u32, u32), side: u32) -> Self {
        Self {
            layer: [
                storage_buffer(device, "pc_transport_layer_a", u64::from(region.0) * u64::from(region.1) * 16),
                storage_buffer(device, "pc_transport_layer_b", u64::from(region.0) * u64::from(region.1) * 16),
            ],
            layer_region: region,
            layer_cur: 0,
            carrier: [
                storage_buffer(device, "pc_transport_carrier_a", u64::from(side) * u64::from(side) * 16),
                storage_buffer(device, "pc_transport_carrier_b", u64::from(side) * u64::from(side) * 16),
            ],
            carrier_side: side,
            carrier_have: 0,
            carrier_valid: false,
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pc_transport_params"),
                size: PARAMS_SIZE * MAX_DABS as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            coverage: storage_buffer(device, "pc_transport_coverage", 4 * MAX_DABS as u64 * 16),
            stride_px: 16,
            dabs_cap: MAX_DABS,
        }
    }

    /// Resize what this frame needs. The layer is re-uploaded every frame, so only the carrier
    /// losing its pixels on a radius change matters (and that only costs one dab's carry).
    fn fit(&mut self, device: &wgpu::Device, region: (u32, u32), side: u32, stride_px: u64, dabs: usize) {
        if self.layer_region != region {
            self.layer = [
                storage_buffer(device, "pc_transport_layer_a", u64::from(region.0) * u64::from(region.1) * 16),
                storage_buffer(device, "pc_transport_layer_b", u64::from(region.0) * u64::from(region.1) * 16),
            ];
            self.layer_region = region;
            self.layer_cur = 0;
        }
        if self.carrier_side != side {
            self.carrier = [
                storage_buffer(device, "pc_transport_carrier_a", u64::from(side) * u64::from(side) * 16),
                storage_buffer(device, "pc_transport_carrier_b", u64::from(side) * u64::from(side) * 16),
            ];
            self.carrier_side = side;
            self.carrier_have = 0;
            self.carrier_valid = false;
        }
        if self.stride_px < stride_px || self.dabs_cap < dabs {
            self.stride_px = self.stride_px.max(stride_px);
            self.dabs_cap = self.dabs_cap.max(dabs);
            self.coverage = storage_buffer(device, "pc_transport_coverage", 4 * self.stride_px * self.dabs_cap as u64);
        }
    }
}

struct State {
    build: wgpu::ComputePipeline,
    lay: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    buffers: Option<Buffers>,
}

/// The process-wide transport backend (`photocraft_paint::stamp::set_process_transport`).
pub struct GpuTransport {
    device: wgpu::Device,
    queue: wgpu::Queue,
    state: Mutex<Option<State>>,
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

impl GpuTransport {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self { device, queue, state: Mutex::new(None) }
    }

    #[allow(clippy::too_many_arguments)]
    fn bind(&self, state: &State, buffers: &Buffers, layer_in: &wgpu::Buffer, carrier_in: &wgpu::Buffer, out: &wgpu::Buffer, dab: usize) -> wgpu::BindGroup {
        let params = wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &buffers.params, offset: dab as u64 * PARAMS_SIZE, size: std::num::NonZeroU64::new(std::mem::size_of::<Params>() as u64) });
        let coverage = wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &buffers.coverage,
            offset: dab as u64 * buffers.stride_px * 4,
            size: std::num::NonZeroU64::new(buffers.stride_px * 4),
        });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pc_transport_bind"),
            layout: &state.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params },
                wgpu::BindGroupEntry { binding: 1, resource: layer_in.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: carrier_in.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: out.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: coverage },
            ],
        })
    }
}

impl DabTransport for GpuTransport {
    fn transport(&self, frame: &mut TransportFrame) -> bool {
        if frame.alpha != Some(3) || frame.channels != 4 || frame.dabs.is_empty() || frame.dabs.len() > MAX_DABS {
            return false;
        }
        let (w, h) = (frame.region.width(), frame.region.height());
        if w <= 0 || h <= 0 {
            return false;
        }
        let pixels = u64::from(w as u32) * u64::from(h as u32);
        if pixels > MAX_REGION_PIXELS as u64 || frame.pixels.len() != pixels as usize * 4 {
            return false;
        }
        let side = frame.dabs.iter().map(|d| (d.radius.max(0.5) * 2.0).ceil() as u32).max().unwrap_or(1).max(1);
        if side > MAX_SIDE {
            return false;
        }
        let stride_px = frame.dabs.iter().map(|d| u64::from(d.rect.width() as u32) * u64::from(d.rect.height() as u32)).max().unwrap_or(1).next_multiple_of(64);

        let mut guard = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_none() {
            let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("pc_transport_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                        count: None,
                    },
                    storage_entry(1, true),
                    storage_entry(2, true),
                    storage_entry(3, false),
                    storage_entry(4, true),
                ],
            });
            let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("pc_transport_shader"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
            let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("pc_transport_pipelines"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let make = |entry: &str, label: &str| {
                self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: None,
                })
            };
            *guard = Some(State { build: make("build", "pc_transport_build"), lay: make("lay", "pc_transport_lay"), layout, buffers: None });
        }
        let Some(state) = guard.as_mut() else { return false };
        let region = (w as u32, h as u32);
        if state.buffers.is_none() {
            state.buffers = Some(Buffers::new(&self.device, region, side));
        }
        if let Some(buffers) = state.buffers.as_mut() {
            buffers.fit(&self.device, region, side, stride_px, frame.dabs.len());
        }
        let profile = std::env::var_os("PC_TRANSPORT_DEBUG").is_some();
        let t_upload = profile.then(std::time::Instant::now);
        let final_flip = {
            let state: &State = state;
            let Some(buffers) = state.buffers.as_ref() else { return false };
            let n = frame.dabs.len();
            self.queue.write_buffer(&buffers.layer[buffers.layer_cur], 0, bytemuck::cast_slice(frame.pixels.as_slice()));

            // Every dab's uniforms and coverage go up in one go, each at its own offset: nothing is
            // written between the passes, so the frame is a single submit.
            let carries = buffers.carrier_valid;
            let mut params = vec![Params { region: [0; 4], rect: [0; 4], center: [0.0; 4], flow: [0.0; 4], misc: [0; 4] }; n];
            let mut coverage = vec![0.0f32; buffers.stride_px as usize * n];
            for (i, dab) in frame.dabs.iter().enumerate() {
                params[i] = Params {
                    region: [frame.region.x0 as u32, frame.region.y0 as u32, w as u32, h as u32],
                    rect: [dab.rect.x0 as u32, dab.rect.y0 as u32, dab.rect.x1 as u32, dab.rect.y1 as u32],
                    center: [dab.center[0] as f32, dab.center[1] as f32, dab.radius, side as f32],
                    flow: [dab.keep, dab.scale, dab.mix_strength * dab.concentration * dab.output_strength, if dab.has_previous && carries { 1.0 } else { 0.0 }],
                    misc: [4, 3, side, side],
                };
                let at = i * buffers.stride_px as usize;
                coverage[at..at + dab.coverage.len()].copy_from_slice(&dab.coverage);
            }
            self.queue.write_buffer(&buffers.params, 0, bytemuck::cast_slice(&params));
            self.queue.write_buffer(&buffers.coverage, 0, bytemuck::cast_slice(&coverage));
            let t_record = profile.then(std::time::Instant::now);

            let mut layer_cur = buffers.layer_cur;
            let mut carrier_have = buffers.carrier_have;
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pc_transport_frame") });
            for (i, dab) in frame.dabs.iter().enumerate() {
                let rw = dab.rect.width() as u32;
                let rh = dab.rect.height() as u32;
                let layer_in = &buffers.layer[layer_cur];
                let layer_out = &buffers.layer[1 - layer_cur];
                let carrier_in = &buffers.carrier[carrier_have];
                let carrier_out = &buffers.carrier[1 - carrier_have];
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("pc_transport_build"), timestamp_writes: None });
                    pass.set_pipeline(&state.build);
                    pass.set_bind_group(0, &self.bind(state, buffers, layer_in, carrier_in, carrier_out, i), &[]);
                    pass.dispatch_workgroups(side.div_ceil(8), side.div_ceil(8), 1);
                }
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("pc_transport_lay"), timestamp_writes: None });
                    pass.set_pipeline(&state.lay);
                    pass.set_bind_group(0, &self.bind(state, buffers, layer_in, carrier_out, layer_out, i), &[]);
                    pass.dispatch_workgroups(rw.div_ceil(8), rh.div_ceil(8), 1);
                }
                layer_cur = 1 - layer_cur;
                carrier_have = 1 - carrier_have;
            }
            let size = pixels * 16;
            let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pc_transport_staging"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            encoder.copy_buffer_to_buffer(&buffers.layer[layer_cur], 0, &staging, 0, size);
            let index = self.queue.submit([encoder.finish()]);
            let t_submit = profile.then(std::time::Instant::now);
            staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            if self.device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).is_err() {
                return false;
            }
            let t_wait = profile.then(std::time::Instant::now);
            let copied = {
                let Ok(view) = staging.slice(..).get_mapped_range() else {
                    staging.unmap();
                    return false;
                };
                let data: &[f32] = bytemuck::cast_slice(&view);
                let want = frame.pixels.len();
                if data.len() < want {
                    false
                } else {
                    frame.pixels.copy_from_slice(&data[..want]);
                    true
                }
            };
            staging.unmap();
            if !copied {
                return false;
            }
            if let (Some(a), Some(b), Some(c), Some(d)) = (t_upload, t_record, t_submit, t_wait) {
                eprintln!(
                    "[transport-gpu] {n} dabs: upload {:5.2} record {:5.2} submit {:5.2} wait {:5.2} copy {:5.2} ms",
                    (b - a).as_secs_f64() * 1000.0,
                    (c - b).as_secs_f64() * 1000.0,
                    (d - c).as_secs_f64() * 1000.0,
                    0.0,
                    (std::time::Instant::now() - d).as_secs_f64() * 1000.0
                );
            }
            (layer_cur, carrier_have, true)
        };
        if let Some(buffers) = guard.as_mut().and_then(|s| s.buffers.as_mut()) {
            buffers.layer_cur = final_flip.0;
            buffers.carrier_have = final_flip.1;
            buffers.carrier_valid = final_flip.2;
        }
        true
    }
}

/// The transport backend as the slot the brush engine takes (`set_process_transport`).
pub fn transport_slot(device: wgpu::Device, queue: wgpu::Queue) -> photocraft_paint::stamp::TransportSlot {
    photocraft_paint::stamp::TransportSlot::new(std::sync::Arc::new(GpuTransport::new(device, queue)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_fit_their_slot() {
        assert_eq!(std::mem::size_of::<Params>(), 80);
        assert!(std::mem::size_of::<Params>() as u64 <= PARAMS_SIZE);
    }
}
