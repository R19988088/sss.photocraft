//! The GPU paint surface a stroke draws into (`paint_surface.wgsl`): every dab of a frame is one
//! instance of one draw, laid straight into two attachments, with **no readback** — the shape
//! ProCraft's brush has and ours did not (see `docs/custom/PAINT-GPU.md`).
//!
//! - coverage (`Rgba16Float`, alpha carries the dab's mask): blending `(one, one − src alpha)`
//!   evaluates exactly the coverage map's `c ← c + v·(ceil − c)`, dab by dab, in submission order;
//! - Wet Edges' depth (`R16Float`): blended with `max`, as `CoverageMap::depth` keeps it.
//!
//! 16-bit floats keep these surfaces demoable on every backend (`R32Float` is not blendable
//! without a device feature). Float16 carries ~11 bits of mantissa, a coverage step of ~5e-4 —
//! far under the compositor's 1/255, and the tolerance this module's tests hold to.
//!
//! The pigment-mixing stage stays the brush engine's own CPU code (`render.rs`), untouched: this
//! surface is where its masks come from on the GPU path.

use bytemuck::{Pod, Zeroable};
use photocraft_geom::Rect;
use photocraft_paint::StampJob;

use crate::Unsupported;

/// The stamp shader's source.
pub const PAINT_SURFACE_SHADER: &str = include_str!("paint_surface.wgsl");

/// Coverage accumulation, `Rgba16Float`: `ceil·val` premultiplied by `val` in alpha, blended
/// `(one, one − src alpha)`. The alpha channel is what the blend reads; only red is read back.
/// (A two-channel target cannot express this: a format without alpha blends as if it were 1.)
pub const COV_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The paint lane, `Rgba16Float`: `colour × val` premultiplied, blended the same way.
pub const PAINT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Wet Edges' depth, `R16Float`, blended with `max`.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

/// One dab as the shader takes it: `StampJob`'s resolved numbers plus the surface rectangle.
///
/// 28 floats, 112 bytes — the layout `Dab` in `paint_surface.wgsl` reads.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct DabStamp {
    /// x0, y0, x1, y1 in surface pixels.
    pub rect: [f32; 4],
    /// Centre x, centre y, radius, flow alpha.
    pub center: [f32; 4],
    /// Angle (radians), proj_scale, proj_angle, hardness.
    pub misc: [f32; 4],
    /// flip_x, flip_y, the dab's texture depth, the sampled tip's level-0 scale.
    pub flip: [f32; 4],
    /// c3_tip_math, aliased, square (Block), triangle.
    pub flags: [u32; 4],
    /// small (area-sampled tip), wet_edges, the sampled tip's mip level, sampled (1 = bitmap tip).
    pub flags2: [u32; 4],
    /// Roundness, scaled radius, reach², small reach².
    pub geom: [f32; 4],
    /// Where this dab's quad sits on the surface (`rect` minus this is the quad), and the
    /// document-pixel origin a fragment adds back to `@builtin(position)`; zero for a surface
    /// that *is* the document.
    pub atlas: [f32; 4],
    /// The dab's own straight colour (rgb): colour dynamics give each dab its own, and the paint
    /// lane accumulates `colour × coverage` so a folded frame can hand back per-pixel colours.
    pub color: [f32; 4],
}

impl DabStamp {
    /// The stamp for `job` clipped to `rect` (surface pixels).
    pub fn of(job: &StampJob, rect: Rect) -> Self {
        let g = job.geometry;
        Self {
            rect: [rect.x0 as f32, rect.y0 as f32, rect.x1 as f32, rect.y1 as f32],
            center: [job.center[0], job.center[1], job.radius, job.alpha],
            misc: [job.angle, job.proj_scale, job.proj_angle, job.hardness],
            flip: [job.flip[0], job.flip[1], job.texture_depth, job.tip_inv_scale],
            flags: [u32::from(job.c3_tip), u32::from(job.aliased), u32::from(job.square), u32::from(job.triangle)],
            flags2: [u32::from(g.small), u32::from(job.wet), job.tip_level, u32::from(job.tip_inv_scale > 0.0)],
            geom: [g.ro, g.rm, g.reach_squared, g.small_reach_squared],
            atlas: [0.0; 4],
            color: job.color,
        }
    }
}

/// Style uniform: the numbers every dab of a stroke shares (surface size, the stroke's ceiling,
/// its colour, and its texture's own settings).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
struct Style {
    size: [f32; 4],
    color: [f32; 4],
    /// Texture scale, texture depth, unused, unused.
    tex: [f32; 4],
    /// present, each_tip, coordinate mode (0 canvas, 1 stroke, 2 tip), c3_math.
    tex_flags: [u32; 4],
    /// The combine mode (see the shader's `mask_combine`), unused, unused, unused.
    tex_mode: [u32; 4],
    /// The stroke-locked texture origin.
    tex_origin: [f32; 4],
    /// The sampled tip: present, mip levels (0 = none), unused, unused.
    tip_flags: [u32; 4],
}

/// What the surface holds once the dabs are laid: the coverage map's `cov` and `depth` lanes (what
/// `CoverageMap::accumulate` keeps), and the straight paint colour per pixel (`col / cov`).
#[derive(Clone, Debug, Default)]
pub struct SurfaceRead {
    /// Mask coverage, row-major over the whole surface.
    pub coverage: Vec<f32>,
    /// Wet Edges' depth (`1 − rn`, the deepest any dab reached the pixel).
    pub depth: Vec<f32>,
    /// Straight paint colour, row-major (0 where nothing was painted).
    pub color: Vec<[f32; 3]>,
}

/// A stroke surface on the GPU: coverage and depth, drawn into by instanced dabs.
pub struct PaintSurface {
    width: u32,
    height: u32,
    cov: wgpu::Texture,
    paint: wgpu::Texture,
    depth: wgpu::Texture,
    cov_view: wgpu::TextureView,
    paint_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    style: wgpu::Buffer,
    dabs: wgpu::Buffer,
    /// Dabs the instance buffer holds (it grows by re-creating, with a fresh bind group).
    capacity: u32,
    bind: wgpu::BindGroup,
    ceiling: f32,
    color: [f32; 4],
    tex: [f32; 4],
    tex_flags: [u32; 4],
    tex_mode: [u32; 4],
    tex_origin: [f32; 4],
    tex_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    tip_flags: [u32; 4],
    tip_view: wgpu::TextureView,
    tip_sampler: wgpu::Sampler,
    /// The mappable readback buffers, kept from one call to the next: a mask readback can ask for
    /// 26 MB, and allocating that several times a frame costs more than the copy.
    staging: std::sync::Mutex<Staging>,
}

/// The three readback buffers a surface reuses (see [`PaintSurface::stage`]).
#[derive(Default)]
struct Staging {
    cov: Option<(wgpu::Buffer, u64)>,
    depth: Option<(wgpu::Buffer, u64)>,
    paint: Option<(wgpu::Buffer, u64)>,
}

/// Which readback buffer a copy targets.
#[derive(Clone, Copy)]
enum Stage {
    Cov,
    Depth,
    Paint,
}

impl PaintSurface {
    /// A `width × height` surface, cleared to nothing. A texture or pipeline the driver can't
    /// build is an `Err`, so painting keeps to the CPU.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) -> Result<Self, Unsupported> {
        if width == 0 || height == 0 {
            return Err(Unsupported("a paint surface needs a pixel".into()));
        }
        match crate::first_error(device, || Self::build(device, queue, width, height)) {
            (Ok(surface), None) => Ok(surface),
            (_, Some(e)) => Err(Unsupported(format!("couldn't build the paint surface: {e}"))),
            (Err(e), None) => Err(e),
        }
    }

    fn build(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) -> Result<Self, Unsupported> {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pc_paint_surface"),
            source: wgpu::ShaderSource::Wgsl(PAINT_SURFACE_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pc_paint_surface_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<Style>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pc_paint_surface_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let blend = |color: wgpu::BlendComponent| Some(wgpu::BlendState { color, alpha: ZERO_ALPHA_BLEND });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pc_paint_surface_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[
                    Some(wgpu::ColorTargetState { format: COV_FORMAT, blend: blend(COVERAGE_BLEND), write_mask: wgpu::ColorWrites::ALL }),
                    Some(wgpu::ColorTargetState { format: PAINT_FORMAT, blend: blend(COVERAGE_BLEND), write_mask: wgpu::ColorWrites::ALL }),
                    Some(wgpu::ColorTargetState { format: DEPTH_FORMAT, blend: blend(DEPTH_BLEND), write_mask: wgpu::ColorWrites::ALL }),
                ],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let texture = |label: &str, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let cov = texture("pc_paint_surface_cov", COV_FORMAT);
        let paint = texture("pc_paint_surface_paint", PAINT_FORMAT);
        let depth = texture("pc_paint_surface_depth", DEPTH_FORMAT);
        let cov_view = cov.create_view(&wgpu::TextureViewDescriptor::default());
        let paint_view = paint.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let style = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pc_paint_surface_style"),
            size: size_of::<Style>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let capacity = INITIAL_CAPACITY;
        let dabs = instance_buffer(device, capacity);
        // A 1×1 white texture until a stroke brings its own (the binding never goes away).
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pc_paint_surface_dummy_texture"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &dummy, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &half::f16::from_f32(1.0).to_ne_bytes(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: None, rows_per_image: None },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        let tex_view = dummy.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pc_paint_surface_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        // The tip binding: a 1×1 white bitmap until the brush brings its own, clamped at the edge
        // (the CPU's `sample_clamped`), mips sampled linearly.
        let tip_view = dummy_tip(device, queue);
        let tip_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pc_paint_surface_tip_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let bind = bind_group(device, &layout, &style, &dabs, &tex_view, &sampler, &tip_view, &tip_sampler);
        Ok(Self {
            width,
            height,
            cov,
            paint,
            depth,
            cov_view,
            paint_view,
            depth_view,
            pipeline,
            layout,
            style,
            dabs,
            capacity,
            bind,
            ceiling: 1.0,
            color: [0.0, 0.0, 0.0, 1.0],
            tex: [1.0, 1.0, 0.0, 0.0],
            tex_flags: [0, 0, 0, 0],
            tex_mode: [1, 0, 0, 0],
            tex_origin: [0.0, 0.0, 0.0, 0.0],
            tex_view,
            sampler,
            tip_flags: [0, 0, 0, 0],
            tip_view,
            tip_sampler,
            staging: std::sync::Mutex::new(Staging::default()),
        })
    }

    /// The stroke's texture (an already-levelled pattern image), sampled for every dab when
    /// `each_tip` is set. `None`/empty clears it.
    pub fn set_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, image: Option<&photocraft_paint::PatternImage>, options: TextureOptions) {
        self.tex = [options.scale, options.depth, 0.0, 0.0];
        self.tex_mode = [options.mode, 0, 0, 0];
        self.tex_origin = [options.origin[0], options.origin[1], 0.0, 0.0];
        self.tex_flags = [0, u32::from(options.each_tip), options.coordinate as u32, u32::from(options.c3_math)];
        if let Some(image) = image.filter(|i| i.width > 0 && i.height > 0 && i.data.len() >= i.width * i.height) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("pc_paint_surface_texture"),
                size: wgpu::Extent3d { width: image.width as u32, height: image.height as u32, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let row = image.width * 2;
            let mut half_data = Vec::with_capacity(image.data.len() * 2);
            for v in &image.data {
                half_data.extend_from_slice(&half::f16::from_f32(*v).to_ne_bytes());
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &half_data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row as u32), rows_per_image: Some(image.height as u32) },
                wgpu::Extent3d { width: image.width as u32, height: image.height as u32, depth_or_array_layers: 1 },
            );
            self.tex_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.tex_flags[0] = 1;
        } else {
            self.tex_flags[0] = 0;
        }
        self.bind = bind_group(device, &self.layout, &self.style, &self.dabs, &self.tex_view, &self.sampler, &self.tip_view, &self.tip_sampler);
    }

    /// The brush's sampled tip (a `Shape` bitmap), uploaded with its mip chain the way `Mips`
    /// builds one, so dabs can sample it here instead of per pixel on the CPU. `None` clears it.
    pub fn set_tip(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, tip: Option<&photocraft_paint::stamp::MaskTip>) {
        let Some(tip) = tip.filter(|t| t.width > 0 && t.height > 0 && t.data.len() >= (t.width * t.height) as usize) else {
            self.tip_flags = [0, 0, 0, 0];
            self.tip_view = dummy_tip(device, queue);
            self.bind = bind_group(device, &self.layout, &self.style, &self.dabs, &self.tex_view, &self.sampler, &self.tip_view, &self.tip_sampler);
            return;
        };
        let levels = mip_levels(tip.width, tip.height);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pc_paint_surface_tip"),
            size: wgpu::Extent3d { width: tip.width, height: tip.height, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // Box-filter each level down from the one above, over the texels that exist (the CPU's
        // `Mips::new` averages a 2×2 block the same way; it makes one level more for odd sizes).
        let mut data = tip.data.clone();
        let (mut w, mut h) = (tip.width, tip.height);
        for level in 0..levels {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            if level + 1 >= levels {
                break;
            }
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut next = vec![0u8; (nw * nh) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    let mut sum = 0u32;
                    let mut count = 0u32;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let (sx, sy) = (x * 2 + dx, y * 2 + dy);
                            if sx < w && sy < h {
                                sum += u32::from(data[(sy * w + sx) as usize]);
                                count += 1;
                            }
                        }
                    }
                    next[(y * nw + x) as usize] = ((sum as f32 / count.max(1) as f32).round()).clamp(0.0, 255.0) as u8;
                }
            }
            data = next;
            w = nw;
            h = nh;
        }
        self.tip_flags = [1, levels, 0, 0];
        self.tip_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.bind = bind_group(device, &self.layout, &self.style, &self.dabs, &self.tex_view, &self.sampler, &self.tip_view, &self.tip_sampler);
    }

    /// The opacity ceiling coverage builds up to (the stroke's `ceil`, see
    /// `CoverageMap::accumulate`).
    pub fn set_ceiling(&mut self, queue: &wgpu::Queue, ceiling: f32) {
        self.ceiling = ceiling;
        self.write_style(queue);
    }

    /// The stroke's paint colour (straight, 0..1).
    pub fn set_color(&mut self, queue: &wgpu::Queue, color: [f32; 3]) {
        self.color = [color[0], color[1], color[2], 1.0];
        self.write_style(queue);
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn write_style(&self, queue: &wgpu::Queue) {
        let style = Style {
            size: [self.width as f32, self.height as f32, self.ceiling, 0.0],
            color: self.color,
            tex: self.tex,
            tex_flags: self.tex_flags,
            tex_mode: self.tex_mode,
            tex_origin: self.tex_origin,
            tip_flags: self.tip_flags,
        };
        queue.write_buffer(&self.style, 0, bytemuck::bytes_of(&style));
    }

    /// Draws every dab in `dabs` into the surface as one instanced draw. Empty is a no-op.
    pub fn stamp(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, dabs: &[DabStamp]) -> Result<(), Unsupported> {
        if dabs.is_empty() {
            return Ok(());
        }
        if dabs.len() > MAX_INSTANCES as usize {
            return Err(Unsupported(format!("{} dabs in one draw exceeds the {MAX_INSTANCES} instance budget", dabs.len())));
        }
        if dabs.len() as u32 > self.capacity {
            self.capacity = (dabs.len() as u32).max(INITIAL_CAPACITY).next_power_of_two();
            self.dabs = instance_buffer(device, self.capacity);
            self.bind = bind_group(device, &self.layout, &self.style, &self.dabs, &self.tex_view, &self.sampler, &self.tip_view, &self.tip_sampler);
        }
        self.write_style(queue);
        queue.write_buffer(&self.dabs, 0, bytemuck::cast_slice(dabs));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pc_paint_surface_encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pc_paint_surface_pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.cov_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.paint_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.depth_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    }),
                ],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.draw(0..6, 0..dabs.len() as u32);
        }
        queue.submit([encoder.finish()]);
        Ok(())
    }

    /// Reads the coverage and depth lanes only, over the `used` top-left region: what a batch
    /// masker needs, without the colour lane or the unused part of the atlas. `depth` is the Wet
    /// Edges lane: a stroke without wet edges never reads it, so it is not copied back at all.
    pub fn read_masks(&self, device: &wgpu::Device, queue: &wgpu::Queue, used: (u32, u32), depth: bool) -> Result<(Vec<f32>, Vec<f32>), Unsupported> {
        let (w, h) = (used.0.clamp(1, self.width), used.1.clamp(1, self.height));
        let cov_row = align_256(u64::from(w) * 8);
        let depth_row = align_256(u64::from(w) * 2);
        let mut staging = self.staging.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure_stage(&mut staging, device, Stage::Cov, cov_row * u64::from(h));
        ensure_stage(&mut staging, device, Stage::Depth, depth_row * u64::from(h));
        let Staging { cov: Some((cov_out, _)), depth: Some((depth_out, _)), .. } = &*staging else {
            return Err(Unsupported("no staging buffer".into()));
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pc_paint_surface_read_masks") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.cov, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &cov_out,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(cov_row as u32), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        if depth {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo { texture: &self.depth, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyBufferInfo {
                    buffer: depth_out,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(depth_row as u32), rows_per_image: Some(h) },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        }
        let index = queue.submit([encoder.finish()]);
        cov_out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        if depth {
            depth_out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        }
        device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).map_err(|e| Unsupported(format!("GPU wait failed: {e}")))?;
        let cov = decode_rgba_r(cov_out, cov_row, w, h, 8)?;
        let depth = if depth { decode(depth_out, depth_row, w, h, 2)? } else { Vec::new() };
        Ok((cov, depth))
    }

    /// As [`read_masks`](Self::read_masks), plus the colour lane as straight rgb per pixel: what
    /// a folded frame needs when every dab brings its own colour (colour dynamics). The colour
    /// lane holds `colour × coverage`, so each pixel divides its own coverage back in.
    pub fn read_masks_color(&self, device: &wgpu::Device, queue: &wgpu::Queue, used: (u32, u32)) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>), Unsupported> {
        let (w, h) = (used.0.clamp(1, self.width), used.1.clamp(1, self.height));
        let cov_row = align_256(u64::from(w) * 8);
        let depth_row = align_256(u64::from(w) * 2);
        let mut staging = self.staging.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure_stage(&mut staging, device, Stage::Cov, cov_row * u64::from(h));
        ensure_stage(&mut staging, device, Stage::Depth, depth_row * u64::from(h));
        ensure_stage(&mut staging, device, Stage::Paint, cov_row * u64::from(h));
        let Staging { cov: Some((cov_out, _)), depth: Some((depth_out, _)), paint: Some((paint_out, _)) } = &*staging else {
            return Err(Unsupported("no staging buffer".into()));
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pc_paint_surface_read_masks_color") });
        for (texture, buffer, row) in [(&self.cov, cov_out, cov_row), (&self.depth, depth_out, depth_row), (&self.paint, paint_out, cov_row)] {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row as u32), rows_per_image: Some(h) },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        }
        let index = queue.submit([encoder.finish()]);
        cov_out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        depth_out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        paint_out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).map_err(|e| Unsupported(format!("GPU wait failed: {e}")))?;
        let cov = decode_rgba_r(cov_out, cov_row, w, h, 8)?;
        let depth = decode(depth_out, depth_row, w, h, 2)?;
        let paint = decode_rgba(paint_out, cov_row, w, h)?;
        let mut color = Vec::with_capacity(cov.len() * 3);
        for (coverage, [r, g, b, _]) in cov.iter().zip(&paint) {
            let c = coverage.clamp(0.0, 1.0);
            if c > 1e-4 {
                color.extend_from_slice(&[r / c, g / c, b / c]);
            } else {
                color.extend_from_slice(&[0.0, 0.0, 0.0]);
            }
        }
        Ok((cov, depth, color))
    }

    /// Clears the surface back to nothing (a new stroke).
    pub fn clear(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pc_paint_surface_clear") });
        for (view, value) in
            [(&self.cov_view, wgpu::Color::TRANSPARENT), (&self.paint_view, wgpu::Color::TRANSPARENT), (&self.depth_view, wgpu::Color::TRANSPARENT)]
        {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pc_paint_surface_clear_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(value), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        queue.submit([encoder.finish()]);
    }

    /// Reads the surface back: coverage, depth and paint colour, row-major, whole surface. The
    /// commit (or per-frame composite) path's one readback; never called per dab.
    pub fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<SurfaceRead, Unsupported> {
        let (w, h) = (self.width, self.height);
        // `copy_texture_to_buffer` wants rows padded to 256 bytes.
        let cov_row = align_256(u64::from(w) * 8);
        let paint_row = align_256(u64::from(w) * 8);
        let depth_row = align_256(u64::from(w) * 2);
        let mut staging = self.staging.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure_stage(&mut staging, device, Stage::Cov, cov_row * u64::from(h));
        ensure_stage(&mut staging, device, Stage::Paint, paint_row * u64::from(h));
        ensure_stage(&mut staging, device, Stage::Depth, depth_row * u64::from(h));
        let Staging { cov: Some((cov_out, _)), paint: Some((paint_out, _)), depth: Some((depth_out, _)) } = &*staging else {
            return Err(Unsupported("no staging buffer".into()));
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pc_paint_surface_read") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.cov, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: cov_out,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(cov_row as u32), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.paint, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: paint_out,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(paint_row as u32), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.depth, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: depth_out,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(depth_row as u32), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let index = queue.submit([encoder.finish()]);
        cov_out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        paint_out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        depth_out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None }).map_err(|e| Unsupported(format!("GPU wait failed: {e}")))?;
        let cov = decode_rgba_r(cov_out, cov_row, w, h, 8)?;
        let paint = decode_rgba(paint_out, paint_row, w, h)?;
        let depth = decode(depth_out, depth_row, w, h, 2)?;
        let mut out = SurfaceRead { coverage: Vec::with_capacity(cov.len()), depth, color: Vec::with_capacity(cov.len()) };
        for (coverage, [r, g, b, _]) in cov.iter().zip(&paint) {
            let coverage = coverage.clamp(0.0, 1.0);
            out.coverage.push(coverage);
            // The paint attachment holds `colour × coverage`; the CPU's colour lane is straight.
            out.color.push(if coverage > 1e-4 { [r / coverage, g / coverage, b / coverage] } else { [0.0, 0.0, 0.0] });
        }
        Ok(out)
    }
}

/// The stroke texture's settings (`Texture` in the brush engine), as the shader takes them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureOptions {
    pub scale: f32,
    /// The combine mode's depth (0..1).
    pub depth: f32,
    /// The combine mode, as the shader's `mask_combine` numbers it (`MaskMode`'s order).
    pub mode: u32,
    /// 0 canvas-locked, 1 stroke-locked, 2 tip-local.
    pub coordinate: u32,
    pub c3_math: bool,
    /// Apply the texture to every dab (the path this surface covers), not once to the stroke.
    pub each_tip: bool,
    /// The stroke-locked origin (a dab centre, the first one of the stroke).
    pub origin: [f32; 2],
}

impl Default for TextureOptions {
    fn default() -> Self {
        Self { scale: 1.0, depth: 1.0, mode: 1, coordinate: 0, c3_math: false, each_tip: true, origin: [0.0, 0.0] }
    }
}

/// Dabs one draw may hold (the instance buffer's ceiling).
pub const MAX_INSTANCES: u32 = 1 << 16;
const INITIAL_CAPACITY: u32 = 64;

/// `(one, one − src alpha)`: the coverage map's `c + v·(ceil − c)`.
const COVERAGE_BLEND: wgpu::BlendComponent = wgpu::BlendComponent {
    src_factor: wgpu::BlendFactor::One,
    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
    operation: wgpu::BlendOperation::Add,
};
/// `max`: the deepest a dab reaches a pixel.
const DEPTH_BLEND: wgpu::BlendComponent = wgpu::BlendComponent {
    src_factor: wgpu::BlendFactor::One,
    dst_factor: wgpu::BlendFactor::One,
    operation: wgpu::BlendOperation::Max,
};
/// The coverage target's alpha channel stays zero (only its colour channel is read).
const ZERO_ALPHA_BLEND: wgpu::BlendComponent = wgpu::BlendComponent {
    src_factor: wgpu::BlendFactor::Zero,
    dst_factor: wgpu::BlendFactor::One,
    operation: wgpu::BlendOperation::Add,
};

/// Make sure one readback slot holds a buffer of `want` bytes, growing it when asked for more.
fn ensure_stage(staging: &mut Staging, device: &wgpu::Device, which: Stage, want: u64) {
    let (label, slot) = match which {
        Stage::Cov => ("pc_paint_surface_cov_staging", &mut staging.cov),
        Stage::Depth => ("pc_paint_surface_depth_staging", &mut staging.depth),
        Stage::Paint => ("pc_paint_surface_paint_staging", &mut staging.paint),
    };
    if slot.as_ref().is_some_and(|(_, size)| *size >= want) {
        return;
    }
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: want,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    *slot = Some((buffer, want));
}

fn instance_buffer(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pc_paint_surface_dabs"),
        size: u64::from(capacity) * size_of::<DabStamp>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    style: &wgpu::Buffer,
    dabs: &wgpu::Buffer,
    tex: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    tip: &wgpu::TextureView,
    tip_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pc_paint_surface_bind"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: style.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: dabs.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(tex) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(tip) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(tip_sampler) },
        ],
    })
}

/// The 1×1 white tip texture (an opaque `Shape` that leaves the dab's shape alone).
fn dummy_tip(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pc_paint_surface_dummy_tip"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &[255u8],
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: None, rows_per_image: None },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A full box-filtered chain: `floor(log2(max(w, h))) + 1` levels.
fn mip_levels(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

/// A mapped `Rgba16Float` surface: one `[f32; 4]` a pixel, rows padded to `row` bytes.
fn decode_rgba(buffer: &wgpu::Buffer, row: u64, width: u32, height: u32) -> Result<Vec<[f32; 4]>, Unsupported> {
    let data = buffer.slice(..).get_mapped_range().map_err(|e| Unsupported(format!("GPU readback failed: {e:?}")))?;
    let mut out = Vec::with_capacity((width * height) as usize);
    for y in 0..u64::from(height) {
        let start = (y * row) as usize;
        for x in 0..u64::from(width) {
            let at = start + (x * 8) as usize;
            if at + 8 > data.len() {
                break;
            }
            let mut px = [0.0f32; 4];
            for (c, slot) in px.iter_mut().enumerate() {
                let bits = u16::from_le_bytes([data[at + c * 2], data[at + c * 2 + 1]]);
                *slot = half::f16::from_bits(bits).to_f32();
            }
            out.push(px);
        }
    }
    drop(data);
    buffer.unmap();
    Ok(out)
}

/// A mapped half-float buffer's red channel, `step` bytes a texel (`Rg16Float` reads 4, and an
/// `Rgba16Float` lane 8). Rows are taken as whole half-float slices, not byte by byte.
fn decode_rgba_r(buffer: &wgpu::Buffer, row: u64, width: u32, height: u32, step: u64) -> Result<Vec<f32>, Unsupported> {
    let data = buffer.slice(..).get_mapped_range().map_err(|e| Unsupported(format!("GPU readback failed: {e:?}")))?;
    let mut out = Vec::with_capacity((width * height) as usize);
    let (w, step) = (width as usize, step as usize);
    for y in 0..height as usize {
        let start = y * row as usize;
        if start + w * step > data.len() {
            break;
        }
        let texels: &[u16] = bytemuck::cast_slice(&data[start..start + w * step]);
        let stride = step / 2;
        out.extend(texels.iter().step_by(stride).map(|b| half::f16::from_bits(*b).to_f32()));
    }
    drop(data);
    buffer.unmap();
    Ok(out)
}

/// A mapped single-channel half-float buffer (`stride` bytes a pixel): depth, or any lane stored
/// that way.
fn decode(buffer: &wgpu::Buffer, row: u64, width: u32, height: u32, stride: u64) -> Result<Vec<f32>, Unsupported> {
    let data = buffer.slice(..).get_mapped_range().map_err(|e| Unsupported(format!("GPU readback failed: {e:?}")))?;
    let mut out = Vec::with_capacity((width * height) as usize);
    let w = width as usize;
    let step = stride as usize;
    if step == 2 {
        for y in 0..height as usize {
            let start = y * row as usize;
            if start + w * 2 > data.len() {
                break;
            }
            let texels: &[u16] = bytemuck::cast_slice(&data[start..start + w * 2]);
            out.extend(texels.iter().map(|b| half::f16::from_bits(*b).to_f32()));
        }
    } else {
        for y in 0..height as usize {
            let start = y * row as usize;
            for x in 0..w {
                let at = start + x * step;
                if at + 2 > data.len() {
                    break;
                }
                out.push(half::f16::from_bits(u16::from_le_bytes([data[at], data[at + 1]])).to_f32());
            }
        }
    }
    drop(data);
    buffer.unmap();
    Ok(out)
}

/// The next multiple of 256 (what `copy_texture_to_buffer` wants for a row).
fn align_256(bytes: u64) -> u64 {
    bytes.div_ceil(256) * 256
}
