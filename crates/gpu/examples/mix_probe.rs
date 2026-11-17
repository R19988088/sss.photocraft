//! Sizes a GPU port of the V1 mixing loop before doing it: the same per-pixel carrier/destination
//! mixing, measured on the CPU (parallel rows, as `render.rs` runs it) and on the GPU, both as one
//! batched dispatch over the stroke box and as a dispatch per dab box.
//!
//! Run: cargo run --release -p photocraft-gpu --example mix_probe --features native
//!
//! Kernel-only numbers. A real port also pays the tip, coverage and wet-edge records, and a
//! readback of the touched strip each frame; this probe says how much compute there is to win and
//! how much of it a per-dab dispatch would eat.

use std::time::{Duration, Instant};

use photocraft_gpu::NativeContext;
use wgpu::util::DeviceExt;

const W: u32 = 1200;
const H: u32 = 1200;
/// The box a 160 px round tip covers, as `plan_region` gives it.
const TILE: u32 = 320;
const DABS: usize = 100;
const RADIUS: f32 = 80.0;
const FLOW: f32 = 0.35;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Dab {
    x: f32,
    y: f32,
    flow: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    ox: f32,
    oy: f32,
    radius: f32,
    dabs: u32,
    /// Row stride, in pixels: the carrier buffer is the whole canvas.
    w: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
}

const SHADER: &str = r#"
struct Dab { pos: vec4<f32> };
struct Params { ox: f32, oy: f32, radius: f32, dabs: u32, w: u32, p0: u32, p1: u32, p2: u32 };

@group(0) @binding(0) var<storage, read> dabs: array<Dab>;
@group(0) @binding(1) var<storage, read_write> carrier: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> dest: array<vec4<f32>>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let gx = gid.x + u32(params.ox);
    let gy = gid.y + u32(params.oy);
    let i = gy * params.w + gx;
    let p = vec2<f32>(f32(gx), f32(gy));
    var c = carrier[i];
    var d = dest[i];
    for (var k: u32 = 0u; k < params.dabs; k = k + 1u) {
        let dab = dabs[k].pos;
        let dist = distance(p, dab.xy);
        if (dist >= params.radius) { continue; }
        let t = 1.0 - dist / params.radius;
        let amount = t * t * dab.z;
        c = mix(c, d, amount);
        d = mix(d, c, amount);
    }
    carrier[i] = c;
    dest[i] = d;
}
"#;

fn dabs() -> Vec<Dab> {
    (0..DABS)
        .map(|k| {
            let t = k as f32 / (DABS - 1) as f32;
            Dab { x: 80.0 + t * (W as f32 - 160.0), y: 80.0 + t * (H as f32 - 160.0), flow: FLOW, _pad: 0.0 }
        })
        .collect()
}

fn box_origin(dab: &Dab) -> (u32, u32) {
    let ox = (dab.x - TILE as f32 / 2.0).max(0.0) as u32;
    let oy = (dab.y - TILE as f32 / 2.0).max(0.0) as u32;
    (ox.min(W - TILE), oy.min(H - TILE))
}

fn pixels(w: u32, h: u32) -> Vec<[f32; 4]> {
    (0..(w * h) as usize)
        .map(|i| {
            let x = (i as u32 % w) as f32 / w as f32;
            let y = (i as u32 / w) as f32 / h as f32;
            [0.2 + 0.5 * x, 0.3 + 0.4 * y, 0.6, 1.0]
        })
        .collect()
}

/// The CPU loop as `render.rs` runs it: one dab box at a time, rows in parallel.
fn cpu_tiles() -> Duration {
    use rayon::prelude::*;
    let dabs = dabs();
    let mut carrier = pixels(W, H);
    let mut dest = pixels(W, H);
    let mut total = Duration::ZERO;
    let w = W as usize;
    for dab in &dabs {
        let t0 = Instant::now();
        let (ox, oy) = box_origin(dab);
        let (c_all, d_all) = (&mut carrier, &mut dest);
        c_all
            .par_chunks_mut(w)
            .zip(d_all.par_chunks_mut(w))
            .skip(oy as usize)
            .take(TILE as usize)
            .enumerate()
            .for_each(|(ry, (c_row, d_row))| {
                let py = (oy + ry as u32) as f32;
                let dy = py - dab.y;
                for x in 0..TILE as usize {
                    let px = (ox as usize + x) as f32;
                    let dx = px - dab.x;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist >= RADIUS {
                        continue;
                    }
                    let t = 1.0 - dist / RADIUS;
                    let amount = t * t * dab.flow;
                    let (c, d) = (&mut c_row[x], &mut d_row[x]);
                    for ch in 0..3 {
                        c[ch] += (d[ch] - c[ch]) * amount;
                        d[ch] += (c[ch] - d[ch]) * amount;
                    }
                }
            });
        total += t0.elapsed();
    }
    total
}

/// The same loop the shader runs, over the whole stroke box, on the CPU: the like-for-like
/// counterpart of the batched dispatch (both test every pixel against every dab).
fn cpu_full() -> Duration {
    use rayon::prelude::*;
    let dabs = dabs();
    let mut carrier = pixels(W, H);
    let mut dest = pixels(W, H);
    let w = W as usize;
    let t0 = Instant::now();
    let (c_all, d_all) = (&mut carrier, &mut dest);
    c_all
        .par_chunks_mut(w)
        .zip(d_all.par_chunks_mut(w))
        .enumerate()
        .for_each(|(y, (c_row, d_row))| {
            let py = y as f32;
            for x in 0..w {
                let px = x as f32;
                let (c, d) = (&mut c_row[x], &mut d_row[x]);
                for dab in &dabs {
                    let dx = px - dab.x;
                    let dy = py - dab.y;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist >= RADIUS {
                        continue;
                    }
                    let t = 1.0 - dist / RADIUS;
                    let amount = t * t * dab.flow;
                    for ch in 0..3 {
                        c[ch] += (d[ch] - c[ch]) * amount;
                        d[ch] += (c[ch] - d[ch]) * amount;
                    }
                }
            }
        });
    t0.elapsed()
}

fn entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

fn main() {
    println!("V1 mixing loop, {DABS} dabs over {W}×{H}:");
    let cpu = cpu_tiles();
    println!("  cpu, rows in parallel, one dab box at a time:  {:>8.1} ms", cpu.as_secs_f64() * 1e3);
    let cpu_box = cpu_full();
    println!("  cpu, whole stroke box, pixels × dabs:        {:>8.1} ms", cpu_box.as_secs_f64() * 1e3);

    let ctx = match NativeContext::new() {
        Ok(c) => c,
        Err(e) => {
            println!("  gpu: no device ({e})");
            return;
        }
    };
    println!("  gpu: {} ({:?})", ctx.adapter_name(), ctx.backend());

    let module = ctx.device().create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("mix-probe"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = ctx.device().create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("mix-probe"),
        entries: &[
            entry(0, true),
            entry(1, false),
            entry(2, false),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
        ],
    });
    let pl = ctx.device().create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("mix-probe"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = ctx.device().create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("mix-probe"),
        layout: Some(&pl),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });

    let mut carrier = ctx.device().create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("carrier"),
        contents: bytemuck::cast_slice(&pixels(W, H)),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
    });
    let dest = ctx.device().create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("dest"),
        contents: bytemuck::cast_slice(&pixels(W, H)),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let dab_buf = ctx.device().create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("dabs"),
        contents: bytemuck::cast_slice(&dabs()),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let params = ctx.device().create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("params"),
        contents: bytemuck::bytes_of(&Params { ox: 0.0, oy: 0.0, radius: RADIUS, dabs: DABS as u32, w: W, _p0: 0, _p1: 0, _p2: 0 }),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let bind = ctx.device().create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mix-probe"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: dab_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: carrier.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: dest.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: params.as_entire_binding() },
        ],
    });

    // One dispatch over the whole stroke box: every pixel tests every dab (128× the work the
    // engine needs, because it skips dabs outside their own box).
    let mut encoder = ctx.device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("batch") });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("batch"), timestamp_writes: None });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(W.div_ceil(8), H.div_ceil(8), 1);
    }
    let t0 = Instant::now();
    ctx.queue().submit([encoder.finish()]);
    let _ = ctx.device().poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    let batch = t0.elapsed();
    println!("  gpu, one dispatch over the stroke box (pixels × dabs):  {:>8.1} ms", batch.as_secs_f64() * 1e3);

    // A dispatch per dab box: what a streaming port submits while the stroke runs.
    let per_buf = ctx.device().create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("per-dab params"),
        contents: bytemuck::bytes_of(&Params { ox: 0.0, oy: 0.0, radius: RADIUS, dabs: 1, w: W, _p0: 0, _p1: 0, _p2: 0 }),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let one = ctx.device().create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("one dab"),
        contents: bytemuck::cast_slice(&dabs()[0..1]),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let per_bind = ctx.device().create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("per-dab"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: one.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: carrier.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: dest.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: per_buf.as_entire_binding() },
        ],
    });
    let mut per = Duration::ZERO;
    let mut encode = Duration::ZERO;
    for dab in dabs() {
        let (ox, oy) = box_origin(&dab);
        ctx.queue().write_buffer(&one, 0, bytemuck::cast_slice(&[dab]));
        ctx.queue().write_buffer(
            &per_buf,
            0,
            bytemuck::bytes_of(&Params { ox: ox as f32, oy: oy as f32, radius: RADIUS, dabs: 1, w: W, _p0: 0, _p1: 0, _p2: 0 }),
        );
        let t1 = Instant::now();
        let mut encoder = ctx.device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("per-dab") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("per-dab"), timestamp_writes: None });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &per_bind, &[]);
            pass.dispatch_workgroups(TILE.div_ceil(8), TILE.div_ceil(8), 1);
        }
        encode += t1.elapsed();
        let t2 = Instant::now();
        ctx.queue().submit([encoder.finish()]);
        let _ = ctx.device().poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        per += t2.elapsed();
    }
    println!("  gpu, a dispatch per dab box ({DABS} dispatches):  {:>8.1} ms  (encode {:.1} + submit/wait {:.1})",
        per.as_secs_f64() * 1e3,
        encode.as_secs_f64() * 1e3,
        (per - encode).as_secs_f64() * 1e3);

    // Reset both buffers so the readback below has something to name.
    let mut enc = ctx.device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("reset") });
    enc.clear_buffer(&carrier, 0, None);
    ctx.queue().submit([enc.finish()]);
    let _ = &mut carrier;
}
