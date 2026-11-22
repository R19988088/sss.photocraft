//! What a GPU without compute passes or half floats to draw into (an old one's OpenGL ES 3.1, which
//! has neither storage buffers outside its compute shaders nor float targets) does in their place:
//! the same work in fragment passes over textures. Where a pass would read what it writes, it reads
//! one texture and draws into another, then copies back; numbers kept between passes are integer
//! textures holding a float's bits, which every such GPU draws into.
//!
//! - Strokes: laid by their rule pixel by pixel ([`Canvas::lay_by_rule_in_fragments`]), each kept
//!   twice: in 8 bits a channel, as everything else reads it, and in half floats packed into
//!   integers ([`BuiltUp`]), as it builds up (so faint stamps add up as they would in half floats).
//! - Smudges, wet paint and paint laid in place: the compute passes' work done a fragment a cell,
//!   plot texel or pixel ([`Canvas::lay_window_in_fragments`]), in a window of textures as the
//!   compute passes' is of buffers, and what the brush carries in integer textures ([`Held`]).
//! - Liquify: its stamps' sums and its field in integers ([`LiquifyBits`]).

use procraft_engine::geom::IRect;
use wgpu::util::DeviceExt;

use super::{
    Canvas, LAYER_FORMAT, LIQUIFY_LEAST_RADIUS, LiquifyStamp, LiquifyUniform, SMUDGE_GRID, SMUDGE_UNIFORM_SIZE,
    SmudgeUniform, StampInstance, UNIFORM_SIZE, UniformData, WET_PASS_SIZE, WET_PLOT_MOST, WET_PLOTS_TEXELS, by_rule,
    copy_at, extent, quad_reach, rule_tiles, smudge, texture,
};
use crate::Gpu;

/// What a stroke builds up in on such a GPU, beside it: its half floats, red and green then blue and
/// alpha, each pair packed into a word.
pub(super) const BUILT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Uint;

/// The most a smudge stroke's window may take, bytes (as a binding's room holds it for the compute
/// passes: see `smudge.rs`): past it, the work goes in windows of its own.
pub(super) const WINDOW_MOST: u64 = 16 << 20;

/// Liquify on a GPU that can't draw into floats (see `liquify.wgsl`): its stamps' sums, a lot of
/// stamps at a time, and the field carried along by them, both kept in integers holding each
/// float's bits; and the adjustment's last pass showing the layer through that field.
pub(super) struct LiquifyBits {
    sums: wgpu::RenderPipeline,
    sums_layout: wgpu::BindGroupLayout,
    step: wgpu::RenderPipeline,
    step_layout: wgpu::BindGroupLayout,
    pub(super) shown: wgpu::RenderPipeline,
    pub(super) shown_layout: wgpu::BindGroupLayout,
}

/// Liquify's field there: two floats' bits a pixel; and its stamps' sums (an offset, how much is
/// rebuilt).
pub(super) const LIQUIFY_FIELD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Uint;
const LIQUIFY_SUMS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;

/// The stamps a lot of liquify's sums takes at most, and the bytes of its uniform (as the shader's
/// `LiquifyLot`: three vectors, then two a stamp).
const LIQUIFY_LOT_MOST: usize = 256;
const LIQUIFY_LOT_BYTES: u64 = (3 + 2 * LIQUIFY_LOT_MOST as u64) * 16;

/// The stamps one fragment pass of the rule takes at most (as the shader's uniform array holds
/// them, 7 vectors each): a frame with more is laid in turns.
const RULE_STAMPS_MOST: usize = 144;
const RULE_STAMPS_BYTES: u64 = 1008 * 16;
const _: () = assert!(RULE_STAMPS_MOST * size_of::<StampInstance>() <= RULE_STAMPS_BYTES as usize);

/// Words a row of the rule's tile list texture holds (as the shader's `RULE_ROW`).
const RULE_ROW: u32 = 1024;

/// The pipelines such a GPU lays strokes with in place of the compute passes: the rule's, and a
/// smudge's and wet paint's (see `smudge_frag.wgsl`), each with what it is given.
pub(crate) struct FragmentPipelines {
    rule: wgpu::RenderPipeline,
    rule_layout: wgpu::BindGroupLayout,
    /// What strokes are in: 8 bits a channel (their half floats kept beside them), or half floats.
    stroke_format: wgpu::TextureFormat,
    /// Liquify's, where the GPU can't draw into floats.
    pub(super) liquify: Option<LiquifyBits>,
    pub(super) smudge_layout: wgpu::BindGroupLayout,
    pub(super) smudge_carry_layout: wgpu::BindGroupLayout,
    pub(super) carry: wgpu::RenderPipeline,
    pub(super) dull_part: wgpu::RenderPipeline,
    pub(super) dull: wgpu::RenderPipeline,
    pub(super) blur_down: wgpu::RenderPipeline,
    pub(super) blur_across: wgpu::RenderPipeline,
    /// A stamp laid, by whether the window keeps how much of the paint each pixel has taken, and
    /// how much shows (each its own target).
    pub(super) deposit: [[wgpu::RenderPipeline; 2]; 2],
    pub(super) synthesize: wgpu::RenderPipeline,
    pub(super) wet_plots: wgpu::RenderPipeline,
    pub(super) wet_carried: wgpu::RenderPipeline,
}

/// A layout entry for a texture the fragment stage loads from (`sample` its kind).
pub(super) fn loaded(binding: u32, sample: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: sample,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// A layout entry for a uniform block of `size` bytes (`dynamic`: at an offset given as it is bound).
pub(super) fn uniform_entry(binding: u32, size: u64, dynamic: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    }
}

pub(super) const FLOAT: wgpu::TextureSampleType = wgpu::TextureSampleType::Float { filterable: false };
pub(super) const UINT: wgpu::TextureSampleType = wgpu::TextureSampleType::Uint;

/// A uniform buffer holding `data` (written through the queue: a lost device makes no buffer
/// mapped as it is made).
fn uniform_buffer(gpu: &Gpu, label: &str, data: &[u8]) -> wgpu::Buffer {
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: data.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&buffer, 0, data);
    buffer
}

/// A colour target of `format`, unblended.
pub(super) fn target(format: wgpu::TextureFormat) -> Option<wgpu::ColorTargetState> {
    Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })
}

/// What a smudge stroke keeps a float's bits in (what the brush carries, wet paint's plots), and how
/// much of the paint each pixel has taken and shows.
pub(super) const CARRY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;
pub(super) const AMOUNT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;

impl FragmentPipelines {
    /// The pipelines for strokes in `stroke_format`, liquify's too where it can't run in floats
    /// (`liquify`: the layouts and module its passes take besides their own: the uniforms', the
    /// adjustments' first group, and their shaders).
    pub(super) fn new(
        device: &wgpu::Device,
        kept: Option<&wgpu::PipelineCache>,
        brush_layout: &wgpu::BindGroupLayout,
        stroke_format: wgpu::TextureFormat,
        liquify: Option<(&wgpu::BindGroupLayout, &wgpu::BindGroupLayout, &wgpu::ShaderModule)>,
    ) -> FragmentPipelines {
        let module = |label, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let eight = stroke_format == LAYER_FORMAT;
        let constants = [("EIGHT", if eight { 1.0 } else { 0.0 })];
        let pipeline_with = |label,
                             layout: &wgpu::PipelineLayout,
                             module: &wgpu::ShaderModule,
                             (vs, fs): (&str, &str),
                             targets: &[Option<wgpu::ColorTargetState>],
                             constants: &[(&str, f64)]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fs),
                    compilation_options: wgpu::PipelineCompilationOptions { constants, ..Default::default() },
                    targets,
                }),
                multiview_mask: None,
                cache: kept,
            })
        };
        let pipeline = |label, layout, module, entries, targets: &[Option<wgpu::ColorTargetState>]| {
            pipeline_with(label, layout, module, entries, targets, &[])
        };
        let layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let rule_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("stamp rule in fragments"),
            entries: &[
                uniform_entry(0, UNIFORM_SIZE, true),
                uniform_entry(1, RULE_STAMPS_BYTES, false),
                loaded(2, UINT),
                loaded(3, FLOAT),
                loaded(4, UINT),
            ],
        });
        let rule_module = module(
            "stamp rule in fragments",
            concat!(
                include_str!("../shaders/blend.wgsl"),
                include_str!("../shaders/stamp.wgsl"),
                include_str!("../shaders/stamp_rule.wgsl"),
                include_str!("../shaders/stamp_rule_frag.wgsl")
            ),
        );
        let rule = pipeline_with(
            "stamp rule in fragments",
            &layout("stamp rule in fragments", &[Some(&rule_layout), Some(brush_layout)]),
            &rule_module,
            ("vs_rule", "fs_stamp_rule"),
            &[target(stroke_format), if eight { target(BUILT_FORMAT) } else { None }],
            &constants,
        );
        // a smudge's: the uniforms, the window as it stands, a dual brush's second, the layer as it
        // was, as the last pass left it and the image as last shown (see the shader); the brush
        // images; and what the brush carries
        let smudge_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("smudge in fragments"),
            entries: &[
                uniform_entry(0, SMUDGE_UNIFORM_SIZE, true),
                loaded(1, FLOAT),
                loaded(3, FLOAT),
                loaded(4, UINT),
                uniform_entry(5, WET_PASS_SIZE, true),
                loaded(6, FLOAT),
                loaded(7, FLOAT),
                loaded(8, FLOAT),
                loaded(9, FLOAT),
                loaded(10, UINT),
                // 颜料混合's table, a 3D texture the hardware filters between its slices, and its
                // sampler (12 and 13, the bindings free in the composite's group as well)
                wgpu::BindGroupLayoutEntry {
                    binding: 12,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 13,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 14,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let smudge_carry_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("smudge carry in fragments"),
            entries: &[loaded(0, UINT), loaded(1, UINT)],
        });
        let smudge_module = module(
            "smudge in fragments",
            concat!(
                include_str!("../shaders/blend.wgsl"),
                include_str!("../shaders/pigment.wgsl"),
                include_str!("../shaders/smudge.wgsl"),
                include_str!("../shaders/smudge_frag.wgsl")
            ),
        );
        let smudge_pipeline_layout =
            layout("smudge in fragments", &[Some(&smudge_layout), Some(brush_layout), Some(&smudge_carry_layout)]);
        let pass = |label, fs, targets: &[Option<wgpu::ColorTargetState>]| {
            pipeline(label, &smudge_pipeline_layout, &smudge_module, ("vs_full", fs), targets)
        };
        let carried = [target(CARRY_FORMAT)];
        let deposit = |amount: bool, shown: bool| {
            let kept = |on: bool| if on { target(AMOUNT_FORMAT) } else { None };
            pass("smudge deposit", "fs_deposit", &[target(LAYER_FORMAT), kept(amount), kept(shown)])
        };
        let liquify = liquify.map(|(uniform_layout, transform_layout, filter_module)| {
            let liquify_module = module("liquify in integers", include_str!("../shaders/liquify.wgsl"));
            let sums_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("liquify sums in integers"),
                entries: &[uniform_entry(3, LIQUIFY_LOT_BYTES, false), loaded(4, UINT)],
            });
            let step_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("liquify step in integers"),
                entries: &[loaded(2, FLOAT), loaded(4, UINT), loaded(5, UINT)],
            });
            let shown_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("liquified in integers"),
                entries: &[loaded(0, FLOAT), loaded(2, UINT)],
            });
            let sums_pipeline_layout = layout("liquify sums in integers", &[Some(uniform_layout), Some(&sums_layout)]);
            let step_pipeline_layout = layout("liquify step in integers", &[Some(uniform_layout), Some(&step_layout)]);
            let shown_pipeline_layout =
                layout("liquified in integers", &[Some(transform_layout), Some(uniform_layout), Some(&shown_layout)]);
            let sums = ("vs_step", "fs_sums_bits");
            let step = ("vs_step", "fs_step_bits");
            let shown = ("vs_filter", "fs_liquified_bits");
            LiquifyBits {
                sums: pipeline_with(
                    "liquify sums in integers",
                    &sums_pipeline_layout,
                    &liquify_module,
                    sums,
                    &[target(LIQUIFY_SUMS_FORMAT)],
                    &[],
                ),
                step: pipeline_with(
                    "liquify step in integers",
                    &step_pipeline_layout,
                    &liquify_module,
                    step,
                    &[target(LIQUIFY_FIELD_FORMAT)],
                    &[],
                ),
                shown: pipeline_with(
                    "liquified in integers",
                    &shown_pipeline_layout,
                    filter_module,
                    shown,
                    &[target(LAYER_FORMAT)],
                    &[],
                ),
                sums_layout,
                step_layout,
                shown_layout,
            }
        });
        FragmentPipelines {
            rule,
            rule_layout,
            stroke_format,
            liquify,
            carry: pass("smudge carry", "fs_carry", &carried),
            dull_part: pass("smudge dull part", "fs_dull_part", &carried),
            dull: pass("smudge dull", "fs_dull", &carried),
            blur_down: pass("smudge blur down", "fs_blur_down", &carried),
            blur_across: pass("smudge blur across", "fs_blur_across", &carried),
            deposit: [[deposit(false, false), deposit(false, true)], [deposit(true, false), deposit(true, true)]],
            synthesize: pass("smudge synthesize", "fs_synthesize", &[target(LAYER_FORMAT)]),
            wet_plots: pass("smudge wet plots", "fs_wet_plots", &carried),
            wet_carried: pass("smudge wet carried", "fs_wet_carried", &carried),
            smudge_layout,
            smudge_carry_layout,
        }
    }
}

/// A stroke texture's build-up in half floats (see the module), made the first time stamps are laid
/// into it.
pub(super) struct BuiltUp {
    of: wgpu::Texture,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// What a canvas keeps for the fragment passes: its strokes' build-ups (the stroke's, a dual brush's
/// second's, an adjustment's painted-on mask's: the oldest let go past that), where a part of the
/// rule is drawn before it is copied in, the build-up under a stroke's tip, and the stand-ins bound
/// where a pass reads nothing.
#[derive(Default)]
pub(super) struct Fragments {
    built: Vec<BuiltUp>,
    rule_out: Option<[(wgpu::Texture, wgpu::TextureView); 2]>,
    tip_backup: Option<wgpu::Texture>,
    spares: Option<Spares>,
}

/// The most build-ups a canvas keeps (see [`Fragments`]).
const BUILT_KEPT: usize = 3;

impl Fragments {
    /// The textures it holds.
    pub(super) fn textures(&self) -> Vec<&wgpu::Texture> {
        let outs = self.rule_out.iter().flat_map(|pair| pair.iter().map(|(t, _)| t));
        self.built.iter().map(|b| &b.texture).chain(outs).chain(self.tip_backup.as_ref()).collect()
    }
}

impl Canvas {
    /// The build-up of the stroke texture `of` (see [`BuiltUp`]), made (clear) if it has none.
    fn built_up(&mut self, device: &wgpu::Device, of: &wgpu::Texture) -> Option<&BuiltUp> {
        let kept = &mut self.fragments.built;
        let fits = |b: &BuiltUp| b.of == *of && b.texture.size() == of.size();
        if let Some(i) = kept.iter().position(fits) {
            // the latest used last
            let found = kept.remove(i);
            kept.push(found);
        } else {
            kept.retain(|b| b.of != *of);
            if kept.len() >= BUILT_KEPT {
                kept.remove(0);
            }
            let texture = texture(device, "stroke built up", of.width(), of.height(), BUILT_FORMAT);
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            kept.push(BuiltUp { of: of.clone(), texture, view });
        }
        // (just pushed or moved last)
        kept.last()
    }

    /// Clears the build-up of the stroke texture `of` (it was cleared), where it has one.
    pub(super) fn clear_built_up(&self, encoder: &mut wgpu::CommandEncoder, of: &wgpu::Texture) {
        if let Some(built) = self.fragments.built.iter().find(|b| b.of == *of) {
            drop(super::render_pass(
                encoder,
                "built up clear",
                &built.view,
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            ));
        }
    }

    /// Keeps the stroke's build-up under `rect`, as the stroke's own is kept under its tip.
    pub(super) fn save_tip_built_up(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, rect: IRect) {
        let Some(built) = self.fragments.built.iter().find(|b| b.of == self.stroke) else { return };
        let (width, height) = (rect.width(), rect.height());
        let roomy = self.fragments.tip_backup.as_ref().is_some_and(|b| b.width() >= width && b.height() >= height);
        if !roomy {
            let room = |n: u32, most: u32| n.max(1).next_multiple_of(256).min(most);
            let (w, h) = self.fragments.tip_backup.as_ref().map_or((0, 0), |b| (b.width(), b.height()));
            let size = (room(width.max(w), self.width), room(height.max(h), self.height));
            self.fragments.tip_backup = Some(texture(device, "tip built up", size.0, size.1, BUILT_FORMAT));
        }
        if let Some(backup) = &self.fragments.tip_backup {
            encoder.copy_texture_to_texture(
                copy_at(&built.texture, rect.x0, rect.y0),
                copy_at(backup, 0, 0),
                extent(width, height),
            );
        }
    }

    /// Puts the stroke's build-up under `rect` back as it was kept (see [`Canvas::save_tip_built_up`]).
    pub(super) fn untip_built_up(&self, encoder: &mut wgpu::CommandEncoder, rect: IRect) {
        let Some(built) = self.fragments.built.iter().find(|b| b.of == self.stroke) else { return };
        if let Some(backup) = &self.fragments.tip_backup {
            encoder.copy_texture_to_texture(
                copy_at(backup, 0, 0),
                copy_at(&built.texture, rect.x0, rect.y0),
                extent(rect.width(), rect.height()),
            );
        }
    }

    /// The rule a stroke rendered `rendering` is laid by here, as the rule shader numbers it:
    /// every stroke's (see [`by_rule`]; over 0, the strongest 3).
    pub(super) fn fragment_rule(rendering: procraft_engine::brush::Rendering) -> u32 {
        use procraft_engine::brush::Rendering;
        by_rule(rendering).unwrap_or(match rendering {
            Rendering::Strongest => 3,
            _ => 0,
        })
    }

    /// Lays `instances` into the stroke texture `texture` by `rule` (see
    /// [`Canvas::fragment_rule`]) in fragment passes: a part of the canvas at most
    /// [`super::RULE_PART`] pixels a side at a time, each pixel going through the stamps over its
    /// tile in order as its paint stands after each (its build-up, where nothing else drew there
    /// since), at most [`RULE_STAMPS_MOST`] stamps a pass; then the part copied into the stroke and
    /// its build-up.
    pub(super) fn lay_by_rule_in_fragments(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        (uniform, brush): (u32, &wgpu::BindGroup),
        instances: &[StampInstance],
        texture: &wgpu::Texture,
        rule: u32,
    ) {
        let device = &gpu.device;
        let pipelines = std::sync::Arc::clone(&self.pipelines);
        let Some(fragments) = pipelines.fragments.as_ref() else { return };
        let part = super::RULE_PART;
        let outs = self.fragments.rule_out.get_or_insert_with(|| {
            [fragments.stroke_format, BUILT_FORMAT].map(|format| {
                let t = super::texture(device, "stamp rule", part, part, format);
                let v = t.create_view(&wgpu::TextureViewDescriptor::default());
                (t, v)
            })
        });
        let [(out8, out8_view), (out16, out16_view)] = outs.clone();
        // its half floats beside it, where it is in 8 bits (else a stand-in, not read)
        let eight = fragments.stroke_format == LAYER_FORMAT;
        let built = if eight {
            let Some(built) = self.built_up(device, texture) else { return };
            Some((built.texture.clone(), built.view.clone()))
        } else {
            None
        };
        let spare = self.spares(device).word.view.clone();
        let built_view = built.as_ref().map_or(&spare, |(_, view)| view);
        let shown = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let full = self.full();
        for chunk in instances.chunks(RULE_STAMPS_MOST) {
            let bounds = chunk.iter().fold(IRect::EMPTY, |b, s| b.union(quad_reach(s))).intersect(full);
            let mut data = bytemuck::cast_slice::<StampInstance, u8>(chunk).to_vec();
            data.resize(RULE_STAMPS_BYTES as usize, 0);
            let stamps = uniform_buffer(gpu, "stamp rule stamps", &data);
            let step = part as i32;
            let mut y0 = bounds.y0;
            while y0 < bounds.y1 {
                let mut x0 = bounds.x0;
                while x0 < bounds.x1 {
                    let rect = IRect { x0, y0, x1: (x0 + step).min(bounds.x1), y1: (y0 + step).min(bounds.y1) };
                    x0 += step;
                    let Some(mut words) = rule_tiles(chunk, 0, rect, rule) else { continue };
                    let rows = (words.len() as u32).div_ceil(RULE_ROW);
                    words.resize((rows * RULE_ROW) as usize, 0);
                    let tiles = device.create_texture_with_data(
                        &gpu.queue,
                        &wgpu::TextureDescriptor {
                            label: Some("stamp rule tiles"),
                            size: extent(RULE_ROW, rows),
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::R32Uint,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        },
                        wgpu::util::TextureDataOrder::LayerMajor,
                        bytemuck::cast_slice(&words),
                    );
                    let tiles_view = tiles.create_view(&wgpu::TextureViewDescriptor::default());
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("stamp rule in fragments"),
                        layout: &fragments.rule_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                    buffer: &self.uniform_buffer,
                                    offset: 0,
                                    size: wgpu::BufferSize::new(UNIFORM_SIZE),
                                }),
                            },
                            wgpu::BindGroupEntry { binding: 1, resource: stamps.as_entire_binding() },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(&tiles_view),
                            },
                            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&shown) },
                            wgpu::BindGroupEntry {
                                binding: 4,
                                resource: wgpu::BindingResource::TextureView(built_view),
                            },
                        ],
                    });
                    let attachment = |view| {
                        Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                        })
                    };
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("stamp rule in fragments"),
                        color_attachments: &[
                            attachment(&out8_view),
                            if eight { attachment(&out16_view) } else { None },
                        ],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_scissor_rect(0, 0, rect.width(), rect.height());
                    pass.set_pipeline(&fragments.rule);
                    pass.set_bind_group(0, &bind, &[uniform]);
                    pass.set_bind_group(1, brush, &[]);
                    pass.draw(0..3, 0..1);
                    drop(pass);
                    let size = extent(rect.width(), rect.height());
                    encoder.copy_texture_to_texture(copy_at(&out8, 0, 0), copy_at(texture, rect.x0, rect.y0), size);
                    if let Some((built, _)) = &built {
                        encoder.copy_texture_to_texture(copy_at(&out16, 0, 0), copy_at(built, rect.x0, rect.y0), size);
                    }
                }
                y0 += step;
            }
        }
    }
}

/// A texture and its view.
#[derive(Clone)]
pub(super) struct Tex {
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
}

impl Tex {
    pub(super) fn new(device: &wgpu::Device, label: &str, width: u32, height: u32, format: wgpu::TextureFormat) -> Tex {
        let texture = texture(device, label, width, height, format);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Tex { texture, view }
    }
}

/// Texels a row of wet paint's plots for a pass, and of a carried-over plot (as the shader's
/// `PLOTS_ROW` and `CARRIED_ROW`).
const PLOTS_ROW: u32 = 1024;
const CARRIED_ROW: u32 = WET_PLOT_MOST;

/// A smudge stroke's own textures on such a GPU, in place of its carry buffer: a smudge's three
/// grids (one under another, a cell a texel), where a pass draws its cells before they are copied
/// in, and a dulling stamp's parts' sums; or wet paint's two carried-over plots (read one, draw the
/// other) and the pass's plots.
pub(super) enum Held {
    Smudge { carry: Tex, scratch: Tex, dull: Tex },
    Wet { carried: [Tex; 2], plots: Tex },
}

impl Held {
    pub(super) fn new(device: &wgpu::Device, wet_paint: bool) -> Held {
        if wet_paint {
            let carried = |label| Tex::new(device, label, CARRIED_ROW, CARRIED_ROW, CARRY_FORMAT);
            let rows = (WET_PLOTS_TEXELS as u32).div_ceil(PLOTS_ROW);
            Held::Wet {
                carried: [carried("wet carried"), carried("wet carried")],
                plots: Tex::new(device, "wet plots", PLOTS_ROW, rows, CARRY_FORMAT),
            }
        } else {
            Held::Smudge {
                carry: Tex::new(device, "smudge carry", SMUDGE_GRID, 3 * SMUDGE_GRID, CARRY_FORMAT),
                scratch: Tex::new(device, "smudge carry scratch", SMUDGE_GRID, SMUDGE_GRID, CARRY_FORMAT),
                dull: Tex::new(device, "smudge dull", 2 * DULL_PARTS, 1, CARRY_FORMAT),
            }
        }
    }

    pub(super) fn textures(&self) -> Vec<&wgpu::Texture> {
        match self {
            Held::Smudge { carry, scratch, dull } => vec![&carry.texture, &scratch.texture, &dull.texture],
            Held::Wet { carried: [a, b], plots } => vec![&a.texture, &b.texture, &plots.texture],
        }
    }
}

/// A dulling stamp's sums are summed in this many parts (as the shader's `DULL_PARTS`).
const DULL_PARTS: u32 = 64;

/// The textures a smudge stroke's passes work in on such a GPU (as the buffers of
/// `smudge::Window` are elsewhere), each with another its passes draw into before they are copied
/// back: the layer as the stroke has left it, the stroke as laid, how much of the paint each pixel
/// has taken and how much shows (as each float's bits), each as the stroke keeps it; and the layer
/// as it was.
pub(super) struct FragWindow {
    work: [Tex; 2],
    grained: Option<[Tex; 2]>,
    amount: Option<[Tex; 2]>,
    shown: Option<[Tex; 2]>,
    before: Tex,
}

impl FragWindow {
    fn new(device: &wgpu::Device, (width, height): (u32, u32), keeps: smudge::Keeps) -> FragWindow {
        let pair = |label, format| [0, 1].map(|_| Tex::new(device, label, width, height, format));
        FragWindow {
            work: pair("smudge layer", LAYER_FORMAT),
            grained: keeps.grained.then(|| pair("smudge grained", LAYER_FORMAT)),
            amount: keeps.amount.then(|| pair("smudge amount", AMOUNT_FORMAT)),
            shown: keeps.shown.then(|| pair("smudge shown", AMOUNT_FORMAT)),
            before: Tex::new(device, "smudge window", width, height, LAYER_FORMAT),
        }
    }

    fn fits(&self, width: u32, height: u32) -> bool {
        let size = self.work[0].texture.size();
        size.width >= width && size.height >= height
    }

    pub(super) fn textures(&self) -> Vec<&wgpu::Texture> {
        let pairs = [Some(&self.work), self.grained.as_ref(), self.amount.as_ref(), self.shown.as_ref()];
        pairs.into_iter().flatten().flatten().map(|t| &t.texture).chain(std::iter::once(&self.before.texture)).collect()
    }
}

/// Stand-ins bound where a pass reads nothing (a texel each): 8 bits a channel, a word, four words.
pub(super) struct Spares {
    eight: Tex,
    word: Tex,
    words: Tex,
}

/// A render pass into `targets`, drawing a triangle over them with `pipeline` within `rect` (their
/// texels), its groups bound as given (the first with its two dynamic offsets).
fn draw_in(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    pipeline: &wgpu::RenderPipeline,
    targets: &[Option<&wgpu::TextureView>],
    rect: IRect,
    (uniforms, offsets, brush, carry): (&wgpu::BindGroup, [u32; 2], &wgpu::BindGroup, &wgpu::BindGroup),
) {
    if rect.is_empty() || rect.x0 < 0 || rect.y0 < 0 {
        return;
    }
    let attachments: Vec<Option<wgpu::RenderPassColorAttachment<'_>>> = targets
        .iter()
        .map(|view| {
            view.map(|view| wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })
        })
        .collect();
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_scissor_rect(rect.x0.unsigned_abs(), rect.y0.unsigned_abs(), rect.width(), rect.height());
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, uniforms, &offsets);
    pass.set_bind_group(1, brush, &[]);
    pass.set_bind_group(2, carry, &[]);
    pass.draw(0..3, 0..1);
}

/// `rect` of `from` copied into `to` at the same place, moved down by `down` rows there.
fn copy_rect(encoder: &mut wgpu::CommandEncoder, from: &Tex, to: &Tex, rect: IRect, down: i32) {
    if rect.is_empty() {
        return;
    }
    encoder.copy_texture_to_texture(
        copy_at(&from.texture, rect.x0, rect.y0),
        copy_at(&to.texture, rect.x0, rect.y0 + down),
        extent(rect.width(), rect.height()),
    );
}

/// The cells `[x0, x1)` across and `[y0, y1)` down of a stamp's window (its `half` cells each way of
/// the grid's middle), as the grid's cells, held to the `n` a side.
fn cells(half: i32, [x0, y0, x1, y1]: [u32; 4]) -> IRect {
    let n = (2 * half + 1).max(1).unsigned_abs();
    let at = |v: u32| SMUDGE_GRID as i32 / 2 - half + i32::try_from(v.min(n)).unwrap_or(0);
    IRect::new(at(x0), at(y0), at(x1), at(y1))
}

/// A part's corners as the shader takes them (see `smudge::WHOLE`).
fn part_corners(part: u32) -> [u32; 4] {
    [part & 255, (part >> 8) & 255, (part >> 16) & 255, part >> 24]
}

impl Canvas {
    /// The stand-ins bound where a pass reads nothing, made the first time.
    fn spares(&mut self, device: &wgpu::Device) -> &Spares {
        self.fragments.spares.get_or_insert_with(|| Spares {
            eight: Tex::new(device, "spare", 1, 1, LAYER_FORMAT),
            word: Tex::new(device, "spare", 1, 1, AMOUNT_FORMAT),
            words: Tex::new(device, "spare", 1, 1, CARRY_FORMAT),
        })
    }

    /// Lays `pieces` in one window over `reach` in fragment passes (as `Canvas::lay_window` does in
    /// compute passes); false when it couldn't be made (the stroke then lays nothing more).
    pub(super) fn lay_window_in_fragments(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        params: &mut UniformData,
        pieces: &[smudge::Piece],
        reach: IRect,
        (brush, second): (&wgpu::BindGroup, Option<&wgpu::TextureView>),
    ) -> bool {
        let device = &gpu.device;
        let reach = if reach.is_empty() { IRect::new(0, 0, 1, 1) } else { reach }.intersect(self.full());
        if reach.is_empty() || !self.fits_window(reach) {
            log::error!("smudging: {} by {} pixels is past what a window holds", reach.width(), reach.height());
            return true;
        }
        let each: Vec<IRect> = pieces.iter().map(|p| p.writes.intersect(reach)).collect();
        let writes = each.iter().fold(IRect::EMPTY, |r, w| r.union(*w));
        let (work, kept) = pieces.iter().fold((false, false), |(w, k), p| (w || p.work, k || p.kept));
        let count: usize = pieces.iter().map(|p| p.uniforms.len()).sum();
        self.smudge_uniform_room(gpu, params, count);
        // the layer as it was is read by a glaze and paint put together, else not filled in
        let before = pieces.iter().any(|p| {
            p.uniforms.iter().any(|u| u.source_before == 1)
                || p.dispatches.iter().any(|d| matches!(d.0, smudge::Pipe::Synthesize))
        });
        if !self.window_in_fragments(gpu, encoder, (reach, before), &each) {
            return false;
        }
        let from_image = pieces.iter().any(|p| p.uniforms.iter().any(|u| u.source_image == 1));
        let pipelines = std::sync::Arc::clone(&self.pipelines);
        let Some(f) = pipelines.fragments.as_ref() else { return false };
        let spares = self.spares(device);
        let (spare8, spare_word, spare_words) =
            (spares.eight.view.clone(), spares.word.view.clone(), spares.words.view.clone());
        let Some(smudge) = self.smudge.as_ref() else { return false };
        let (Some(window), Some(held)) = (smudge.frag_window.as_ref(), smudge.held.as_ref()) else { return false };
        let uniform = |binding, buffer, size| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: wgpu::BufferSize::new(size),
            }),
        };
        fn view(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
        }
        let first = |pair: &Option<[Tex; 2]>| pair.as_ref().map(|[a, _]| a.view.clone());
        let before_view = &window.before.view;
        let reads = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("smudge in fragments"),
            layout: &f.smudge_layout,
            entries: &[
                uniform(0, &self.smudge_params, SMUDGE_UNIFORM_SIZE),
                view(1, &window.work[0].view),
                view(3, &first(&window.grained).unwrap_or_else(|| spare8.clone())),
                view(4, &first(&window.amount).unwrap_or_else(|| spare_word.clone())),
                uniform(5, &self.wet_params, WET_PASS_SIZE),
                // a dual brush's second (else anything: not read)
                view(6, second.unwrap_or(before_view)),
                view(7, before_view),
                // the layer as a smudge's last pass left it (else anything: not read)
                view(8, smudge.passed.as_ref().map_or(before_view, |p| &p.1)),
                // the image as last shown, for a stroke that picks up from it (else anything)
                view(9, if from_image { &self.composite_view } else { before_view }),
                view(10, &first(&window.shown).unwrap_or_else(|| spare_word.clone())),
                // 颜料混合: the table paint and what the brush carries mix by, and its sampler
                view(12, &self.pipelines.pigment_lut),
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: wgpu::BindingResource::Sampler(&self.pipelines.pigment_sampler),
                },
                view(14, &self.pipelines.pigment_lut_b),
            ],
        });
        let carry_bind = |carry: &wgpu::TextureView, plots: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("smudge carry in fragments"),
                layout: &f.smudge_carry_layout,
                entries: &[view(0, carry), view(1, plots)],
            })
        };
        // what the brush carries as the passes read it: the grids and the dulling sums (or a stand-in
        // where those are drawn); wet paint's carried-over plot by which is read, and its plots
        let (main, parted) = match held {
            Held::Smudge { carry, dull, .. } => {
                (carry_bind(&carry.view, &dull.view), carry_bind(&carry.view, &spare_words))
            }
            Held::Wet { carried: [a, _], plots } => {
                (carry_bind(&a.view, &plots.view), carry_bind(&a.view, &spare_words))
            }
        };
        let plots_from = match held {
            Held::Wet { carried: [a, b], .. } => {
                Some([carry_bind(&a.view, &spare_words), carry_bind(&b.view, &spare_words)])
            }
            Held::Smudge { .. } => None,
        };
        let origin = [reach.x0, reach.y0];
        let stride = smudge::row_bytes(reach.width()) / 4;
        let in_window = |r: IRect| {
            let r = r.intersect(reach);
            IRect::new(r.x0 - reach.x0, r.y0 - reach.y0, r.x1 - reach.x0, r.y1 - reach.y0)
        };
        let grid = SMUDGE_GRID as i32;
        for piece in pieces {
            let laid: Vec<(u32, SmudgeUniform)> = piece
                .uniforms
                .iter()
                .map(|u| {
                    let u = SmudgeUniform { origin, stride, rows: reach.height(), ..*u };
                    (params.push(&u), u)
                })
                .collect();
            for &(pipe, which, _) in &piece.dispatches {
                let Some(&(offset, u)) = laid.get(which) else { continue };
                let binds = |carry| (&reads, [offset, piece.listed], brush, carry);
                let bbox = IRect::new(u.bbox[0], u.bbox[1], u.bbox[2], u.bbox[3]);
                match (pipe, held) {
                    (smudge::Pipe::Carry, Held::Smudge { carry, scratch, .. }) => {
                        let rect = cells(u.half, part_corners(u.part));
                        draw_in(encoder, "smudge carry", &f.carry, &[Some(&scratch.view)], rect, binds(&main));
                        copy_rect(encoder, scratch, carry, rect, 0);
                    }
                    (smudge::Pipe::Dull, Held::Smudge { carry, scratch, dull }) => {
                        let parts = IRect::new(0, 0, 2 * DULL_PARTS as i32, 1);
                        draw_in(encoder, "smudge dull part", &f.dull_part, &[Some(&dull.view)], parts, binds(&parted));
                        let one = IRect::new(0, 0, 1, 1);
                        draw_in(encoder, "smudge dull", &f.dull, &[Some(&scratch.view)], one, binds(&main));
                        copy_rect(encoder, scratch, carry, one, 2 * grid);
                    }
                    (smudge::Pipe::BlurDown | smudge::Pipe::BlurAcross, Held::Smudge { carry, scratch, .. }) => {
                        let rect = cells(u.half, part_corners(smudge::WHOLE));
                        let (pipeline, down) = match pipe {
                            smudge::Pipe::BlurDown => (&f.blur_down, grid),
                            _ => (&f.blur_across, 2 * grid),
                        };
                        draw_in(encoder, "smudge blur", pipeline, &[Some(&scratch.view)], rect, binds(&main));
                        copy_rect(encoder, scratch, carry, rect, down);
                    }
                    (smudge::Pipe::Deposit, _) => {
                        let rect = in_window(bbox);
                        let into = if u.laid_apart == 2 { window.grained.as_ref() } else { Some(&window.work) };
                        let Some([was, next]) = into else { continue };
                        let amount = window.amount.as_ref();
                        let shown = window.shown.as_ref();
                        let [[alone, with_shown], [with_amount, with_both]] = &f.deposit;
                        let pipeline = match (amount.is_some(), shown.is_some()) {
                            (false, false) => alone,
                            (false, true) => with_shown,
                            (true, false) => with_amount,
                            (true, true) => with_both,
                        };
                        let targets = [Some(&next.view), amount.map(|[_, n]| &n.view), shown.map(|[_, n]| &n.view)];
                        draw_in(encoder, "smudge deposit", pipeline, &targets, rect, binds(&main));
                        copy_rect(encoder, next, was, rect, 0);
                        for [was, next] in [amount, shown].into_iter().flatten() {
                            copy_rect(encoder, next, was, rect, 0);
                        }
                    }
                    (smudge::Pipe::Synthesize, _) => {
                        let rect = in_window(bbox);
                        let [was, next] = &window.work;
                        draw_in(encoder, "smudge synthesize", &f.synthesize, &[Some(&next.view)], rect, binds(&main));
                        copy_rect(encoder, next, was, rect, 0);
                    }
                    (smudge::Pipe::WetPlots, Held::Wet { carried, plots }) => {
                        let Some(from) = plots_from.as_ref().and_then(|p| p.get(u.index as usize)) else { continue };
                        let Some(into) = carried.get(1 - (u.index as usize).min(1)) else { continue };
                        let n = u64::from(u.plot.max(1));
                        let rows = |texels: u64, row: u32| u32::try_from(texels.div_ceil(u64::from(row))).unwrap_or(0);
                        let all = |row: u32, texels| IRect::new(0, 0, row as i32, rows(texels, row) as i32);
                        let pass_texels = n * n * u64::from(u.count);
                        draw_in(
                            encoder,
                            "wet plots",
                            &f.wet_plots,
                            &[Some(&plots.view)],
                            all(PLOTS_ROW, pass_texels),
                            binds(from),
                        );
                        draw_in(
                            encoder,
                            "wet carried",
                            &f.wet_carried,
                            &[Some(&into.view)],
                            all(CARRIED_ROW, n * n),
                            binds(from),
                        );
                    }
                    // (never laid: a smudge's grids and wet paint's plots don't meet)
                    _ => {}
                }
            }
        }
        self.window_out_fragments(encoder, reach, writes, (work, kept));
        true
    }

    /// Readies the open smudge stroke's window over `reach` in textures (as `Canvas::window_in`
    /// does in buffers), its passes writing within `writes`: the textures made big enough, the tiles
    /// they write to made (the layer as it was kept first), and all of it copied in (the layer as it
    /// was if they read it, `before`). False if what it takes couldn't be made: refused if the
    /// stroke has laid nothing yet, else it lays nothing more (and either way it is told).
    fn window_in_fragments(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        (reach, before): (IRect, bool),
        writes: &[IRect],
    ) -> bool {
        let device = &gpu.device;
        let full = self.full();
        let (canvas_w, canvas_h) = (self.width, self.height);
        let also = self.smudge_also();
        let Some(smudge) = self.smudge.as_mut() else { return false };
        let Some(layer) = self.layers.get(&smudge.into) else { return false };
        let also: Vec<&wgpu::Texture> = also.iter().filter_map(|id| self.layers.get(id)).map(|l| &l.texture).collect();
        if smudge.stalled {
            return false;
        }
        let keeps = smudge.keeps;
        let (width, height) = (reach.width(), reach.height());
        let small = smudge.frag_window.as_ref().is_none_or(|w| !w.fits(width, height));
        let mut missing: Vec<(u32, u32)> =
            writes.iter().flat_map(|w| smudge::tiles_of(*w)).filter(|t| !smudge.tiles.contains_key(t)).collect();
        missing.sort_unstable();
        missing.dedup();
        if small || !missing.is_empty() {
            let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
            let invalid = device.push_error_scope(wgpu::ErrorFilter::Validation);
            // grown in steps, so a stroke that reaches further doesn't make them every frame
            let side = |n: u32, of: u32| n.next_multiple_of(256).min(of).max(n);
            let window = small.then(|| FragWindow::new(device, (side(width, canvas_w), side(height, canvas_h)), keeps));
            let tile = |label, format| texture(device, label, smudge::TILE, smudge::TILE, format);
            let made: Vec<((u32, u32), smudge::Tile)> = missing
                .iter()
                .map(|&at| {
                    let tile = smudge::Tile {
                        before: tile("smudge before", LAYER_FORMAT),
                        grained: keeps.grained.then(|| tile("smudge grained", LAYER_FORMAT)),
                        amount: keeps.amount.then(|| tile("smudge amount", AMOUNT_FORMAT)),
                        shown: keeps.shown.then(|| tile("smudge shown", AMOUNT_FORMAT)),
                        more: also.iter().map(|_| tile("smudge kept", LAYER_FORMAT)).collect(),
                    };
                    (at, tile)
                })
                .collect();
            let failed = pollster::block_on(invalid.pop()).or(pollster::block_on(memory.pop()));
            if let Some(e) = failed {
                log::error!("smudging: {e}");
                self.smudge_untold = true;
                if smudge.laid {
                    smudge.stalled = true;
                } else {
                    self.smudge = None;
                    self.smudge_refused = true;
                }
                return false;
            }
            if let Some(window) = window {
                smudge.frag_window = Some(window);
            }
            for (at, tile) in made {
                // the layer there as it was: the stroke hasn't written to it yet
                let r = smudge::tile_rect(at).intersect(full);
                let kept = std::iter::once((&layer.texture, &tile.before))
                    .chain(tile.grained.iter().map(|g| (&layer.texture, g)))
                    .chain(also.iter().copied().zip(&tile.more));
                for (from, into) in kept {
                    encoder.copy_texture_to_texture(
                        copy_at(from, r.x0, r.y0),
                        copy_at(into, 0, 0),
                        extent(r.width(), r.height()),
                    );
                }
                smudge.tiles.insert(at, tile);
            }
        }
        smudge.laid = true;
        let Some(window) = smudge.frag_window.as_ref() else { return false };
        let size = extent(width, height);
        // the layer as the stroke has left it, and as it was where the stroke hasn't been
        let from_layer = copy_at(&layer.texture, reach.x0, reach.y0);
        let mut from_layer_into =
            |to: &Tex| encoder.copy_texture_to_texture(from_layer, copy_at(&to.texture, 0, 0), size);
        from_layer_into(&window.work[0]);
        if before {
            from_layer_into(&window.before);
        }
        if let Some([grained, _]) = &window.grained {
            from_layer_into(grained);
        }
        for [kept, _] in [&window.amount, &window.shown].into_iter().flatten() {
            drop(super::render_pass(
                encoder,
                "smudge amount clear",
                &kept.view,
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            ));
        }
        // what the stroke keeps where it has been
        for at in smudge::tiles_of(reach) {
            let Some(tile) = smudge.tiles.get(&at) else { continue };
            let r = smudge::tile_rect(at).intersect(reach);
            if r.is_empty() {
                continue;
            }
            let t = smudge::tile_rect(at);
            let (x, y, size) = (r.x0 - t.x0, r.y0 - t.y0, extent(r.width(), r.height()));
            let (wx, wy) = (r.x0 - reach.x0, r.y0 - reach.y0);
            if before {
                encoder.copy_texture_to_texture(
                    copy_at(&tile.before, x, y),
                    copy_at(&window.before.texture, wx, wy),
                    size,
                );
            }
            let kept = [(&tile.grained, &window.grained), (&tile.amount, &window.amount), (&tile.shown, &window.shown)];
            for (kept, pair) in kept {
                if let (Some(kept), Some([to, _])) = (kept, pair) {
                    encoder.copy_texture_to_texture(copy_at(kept, x, y), copy_at(&to.texture, wx, wy), size);
                }
            }
        }
        true
    }

    /// Puts what the passes in the window over `reach` changed within `changed` back (as
    /// `Canvas::window_out` does from buffers): into the layer if they changed it (`work`), and
    /// into the stroke's tiles if they changed what it keeps (`kept`).
    fn window_out_fragments(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        reach: IRect,
        changed: IRect,
        (work, kept): (bool, bool),
    ) {
        if changed.is_empty() {
            return;
        }
        let Some(smudge) = self.smudge.as_ref() else { return };
        let (Some(window), Some(layer)) = (smudge.frag_window.as_ref(), self.layers.get(&smudge.into)) else { return };
        fn from(w: &Tex, r: IRect, reach: IRect) -> wgpu::TexelCopyTextureInfo<'_> {
            copy_at(&w.texture, r.x0 - reach.x0, r.y0 - reach.y0)
        }
        if work {
            encoder.copy_texture_to_texture(
                from(&window.work[0], changed, reach),
                copy_at(&layer.texture, changed.x0, changed.y0),
                extent(changed.width(), changed.height()),
            );
            self.dirty = self.dirty.union(changed);
        }
        if !kept {
            return;
        }
        for at in smudge::tiles_of(changed) {
            let Some(tile) = smudge.tiles.get(&at) else { continue };
            let t = smudge::tile_rect(at);
            let r = t.intersect(changed);
            if r.is_empty() {
                continue;
            }
            let (x, y, size) = (r.x0 - t.x0, r.y0 - t.y0, extent(r.width(), r.height()));
            let kept = [(&tile.grained, &window.grained), (&tile.amount, &window.amount), (&tile.shown, &window.shown)];
            for (kept, pair) in kept {
                if let (Some(kept), Some([w, _])) = (kept, pair) {
                    encoder.copy_texture_to_texture(from(w, r, reach), copy_at(kept, x, y), size);
                }
            }
        }
    }
}

/// What a step of liquify in integers works in, for a part of the field (as `LiquifyScratch` is
/// in floats): the stamps' sums so far and the next lot's, and the field as they leave it.
pub(super) struct LiquifyBitsScratch {
    width: u32,
    height: u32,
    sums: [Tex; 2],
    next: Tex,
}

impl Canvas {
    /// Whether liquify works in integers here (see [`LiquifyBits`]).
    pub(super) fn liquifies_in_integers(&self) -> bool {
        self.pipelines.liquify.is_none() && self.pipelines.fragments.as_ref().is_some_and(|f| f.liquify.is_some())
    }

    /// The edit's copy and liquify's field `field` (in integers) bound as the adjustment's last
    /// pass reads them.
    pub(super) fn liquify_field_bind(
        &self,
        device: &wgpu::Device,
        field: &wgpu::TextureView,
    ) -> Option<wgpu::BindGroup> {
        let bits = self.pipelines.fragments.as_ref()?.liquify.as_ref()?;
        let fallback;
        let copy = match &self.transforming {
            Some(t) => {
                fallback = t.before.create_view(&wgpu::TextureViewDescriptor::default());
                &fallback
            }
            None => &self.everything,
        };
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("liquified in integers"),
            layout: &bits.shown_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(copy) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(field) },
            ],
        }))
    }

    /// A step of the liquify brush in integers (as `Canvas::liquify_step` does in floats): `stamps`
    /// added up over the part of the field they reach, a lot at a time, each rounded into the sums
    /// as half floats are; the field there carried along by that and `keep` of it left. No stamps:
    /// the whole field.
    pub(super) fn liquify_step_in_integers(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        uniforms: &mut UniformData,
        stamps: &[LiquifyStamp],
        keep: f32,
    ) {
        let device = &gpu.device;
        let (canvas_w, canvas_h) = (self.width as f32, self.height as f32);
        let chosen = self.selection.as_ref().map_or_else(|| self.everything.clone(), |s| s.view.clone());
        let selected = self.selection.is_some();
        let pipelines = std::sync::Arc::clone(&self.pipelines);
        let Some(bits) = pipelines.fragments.as_ref().and_then(|f| f.liquify.as_ref()) else { return };
        let uniform_bind = self.uniform_bind.clone();
        let Some(field) = self.liquify.as_mut() else { return };
        let (width, height) = (field.texture.width(), field.texture.height());
        let [across, down] = field.scale;
        let [x0, y0, x1, y1] = super::liquify_reach(stamps, (width, height), field.scale);
        let (area_w, area_h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        if area_w == 0 || area_h == 0 {
            return;
        }
        let scratch = match field.bits.take() {
            Some(s) if s.width >= area_w && s.height >= area_h => s,
            old => {
                // each way up to the next size a little over, to do this seldom
                let wide = old.as_ref().map_or(0, |s| s.width).max(area_w.next_multiple_of(128)).min(width);
                let high = old.as_ref().map_or(0, |s| s.height).max(area_h.next_multiple_of(128)).min(height);
                let sums = |_| Tex::new(device, "liquify sums", wide, high, LIQUIFY_SUMS_FORMAT);
                LiquifyBitsScratch {
                    width: wide,
                    height: high,
                    sums: [0, 1].map(sums),
                    next: Tex::new(device, "liquify step", wide, high, LIQUIFY_FIELD_FORMAT),
                }
            }
        };
        let area = [x0 as f32, y0 as f32, area_w as f32, area_h as f32];
        let canvas = [canvas_w, canvas_h, across, down];
        let offset = uniforms.push(&LiquifyUniform {
            stamp: [keep, 0.0, 0.0, 0.0],
            way: [0.0, 0.0, 0.0, if selected { 1.0 } else { 0.0 }],
            canvas,
            area,
        });
        let part = IRect::new(0, 0, area_w as i32, area_h as i32);
        let pass = |encoder: &mut wgpu::CommandEncoder, label, pipeline, into: &Tex, bind: &wgpu::BindGroup| {
            let mut pass = super::render_pass(encoder, label, &into.view, wgpu::LoadOp::Load);
            super::scissor(&mut pass, part);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &uniform_bind, &[offset]);
            pass.set_bind_group(1, bind, &[]);
            pass.draw(0..3, 0..1);
        };
        // the sums, a lot of stamps at a time (none: a lot of none, the sums all nothing)
        let lots: Vec<&[LiquifyStamp]> =
            if stamps.is_empty() { vec![&[]] } else { stamps.chunks(LIQUIFY_LOT_MOST).collect() };
        let mut latest = 0;
        for (k, lot) in lots.into_iter().enumerate() {
            let mut data: Vec<[f32; 4]> =
                vec![[lot.len() as f32, if k > 0 { 1.0 } else { 0.0 }, 0.0, 0.0], canvas, area];
            for st in lot {
                let (mode, narrow) = st.mode.code();
                data.push([st.centre[0], st.centre[1], st.radius.max(LIQUIFY_LEAST_RADIUS), st.amount]);
                data.push([st.way[0], st.way[1], mode, narrow]);
            }
            data.resize(3 + 2 * LIQUIFY_LOT_MOST, [0.0; 4]);
            let lot_buffer = uniform_buffer(gpu, "liquify lot", bytemuck::cast_slice(&data));
            // the first lot into the first sums; each after into the other, reading on from the last
            let into = if k == 0 { 0 } else { 1 - latest };
            let Some((into_sums, from_sums)) = scratch.sums.get(into).zip(scratch.sums.get(1 - into)) else { return };
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("liquify sums in integers"),
                layout: &bits.sums_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 3, resource: lot_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&from_sums.view) },
                ],
            });
            pass(encoder, "liquify sums", &bits.sums, into_sums, &bind);
            latest = into;
        }
        let Some(sums) = scratch.sums.get(latest) else { return };
        let reads = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("liquify step in integers"),
            layout: &bits.step_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&chosen) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&sums.view) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&field.view) },
            ],
        });
        pass(encoder, "liquify step", &bits.step, &scratch.next, &reads);
        encoder.copy_texture_to_texture(
            scratch.next.texture.as_image_copy(),
            wgpu::TexelCopyTextureInfo {
                texture: &field.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: x0, y: y0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            extent(area_w, area_h),
        );
        field.bits = Some(scratch);
    }
}
