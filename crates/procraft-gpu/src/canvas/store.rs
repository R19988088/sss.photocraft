//! What a brush keeps with a layer between strokes: the layer's store. A stroke whose style asks
//! for one ([`StrokeStyle::store`]) paints into planes the layer's size, kept beside the layer's
//! pixels, rather than into the pixels themselves, and the layer is drawn from them: what its
//! strokes leave goes on mattering to the strokes after.
//!
//! The one kind there is, [`STORE_LIT`], keeps two planes: the paint laid there (its colours before
//! they are lit) and a height (a grey, as opaque as the paint is thick there). Each stamp of such a
//! stroke lays its paint into the first, as a smudge's stamp does (picking up from it, not from the
//! layer), then its tip's grey over the second; the layer is then drawn from the two where the
//! stamps reached: the paint at the lightness the height says ([`Canvas::draw_lit`]).
//!
//! The planes are held as layers' pixels are, each under an id of its own that no document uses,
//! so they go through the history, the parking of layers and a save as a layer does. A stroke into
//! the store keeps the layer and both planes as they were where it reached, in one undo step. A
//! transform or a flip moves the planes with the layer. Any other change to the layer's pixels
//! (another brush's stroke, a fill, an adjustment, liquify, a merge, clearing it) dries the paint:
//! the store goes, the layer staying as it shows, and that change's undo step keeps the store to
//! bring back with it. A layer without a store costs nothing.

use procraft_engine::document::LayerId;
use procraft_engine::geom::{IRect, Vec2};
use procraft_engine::stroke::PickUp;

use super::{
    Canvas, LAYER_FORMAT, LayerGpu, PREMULTIPLIED_OVER, Pipelines, QUAD_REACH, StampInstance, Step, StrokeStyle,
    blended_pipeline, copy_at, extent, render_pass, scissor, texture,
};

/// A store keeping the paint laid and a height that lights it (see the module).
pub const STORE_LIT: u32 = 1;

/// The ids planes take, counted on from here: no document numbers a layer so high.
const PLANE_IDS: u64 = 1 << 62;

/// What a layer keeps besides its pixels for the brushes that paint into it (see the module): its
/// kind and its planes, each held as a layer's pixels are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerStore {
    pub kind: u32,
    pub planes: Vec<LayerId>,
}

impl LayerStore {
    /// How many planes a store of `kind` keeps: none for a kind there isn't.
    pub fn planes_of(kind: u32) -> usize {
        match kind {
            STORE_LIT => 2,
            _ => 0,
        }
    }
}

/// A stroke laying paint into its layer's store: the store's planes (the paint, the height) and,
/// when the stroke made the store it paints into, the one the layer had before.
#[derive(Clone, Debug)]
pub(super) struct StoreStroke {
    pub(super) planes: [LayerId; 2],
    pub(super) made: Option<Option<LayerStore>>,
}

/// What a store's planes are drawn with, made the first time a stroke wants them: a stamp's grey
/// over the height, and the layer from its planes (and what that pass reads).
pub(super) struct StorePipelines {
    height: wgpu::RenderPipeline,
    lit: wgpu::RenderPipeline,
    lit_layout: wgpu::BindGroupLayout,
}

/// A stamp's grey laid over a store's height, as the shader reads it (see `store.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HeightInstance {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
}

const HEIGHT_ATTRIBUTES: [wgpu::VertexAttribute; 5] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4];

impl Pipelines {
    fn store(&self, device: &wgpu::Device) -> &StorePipelines {
        self.store.get_or_init(|| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("store"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/store.wgsl").into()),
            });
            let plane = |binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            };
            let lit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("store planes"),
                entries: &[plane(5), plane(6)],
            });
            let layout = |label, group: &wgpu::BindGroupLayout| {
                device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(label),
                    bind_group_layouts: &[Some(group)],
                    immediate_size: 0,
                })
            };
            let height = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("store height"),
                layout: Some(&layout("store height", &self.brush_layout)),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_height"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<HeightInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &HEIGHT_ATTRIBUTES,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_height"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: LAYER_FORMAT,
                        blend: Some(PREMULTIPLIED_OVER),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: self.cache.as_ref(),
            });
            let lit = blended_pipeline(
                device,
                self.cache.as_ref(),
                &layout("store lit", &lit_layout),
                &module,
                "store lit",
                ("vs_lit", "fs_lit"),
                LAYER_FORMAT,
                None,
            );
            StorePipelines { height, lit, lit_layout }
        })
    }
}

impl Canvas {
    /// The layers that keep a store, each with its store, in the order of their ids.
    pub fn stores(&self) -> Vec<(LayerId, LayerStore)> {
        let mut out: Vec<(LayerId, LayerStore)> = self.stores.iter().map(|(l, s)| (*l, s.clone())).collect();
        out.sort_by_key(|(l, _)| l.0);
        out
    }

    /// The store layer `id` keeps, if it keeps one.
    pub fn store(&self, id: LayerId) -> Option<&LayerStore> {
        self.stores.get(&id)
    }

    /// Gives layer `layer` a store of `kind` whose planes' pixels are `planes` (premultiplied RGBA8,
    /// the canvas's size each), as an artwork opened kept it: no undo step. False, giving it none,
    /// where they don't hold up (a kind there isn't, planes of another size or number, no layer).
    pub fn put_store(&mut self, gpu: &crate::Gpu, layer: LayerId, kind: u32, planes: &[Vec<u8>]) -> bool {
        let size = self.width as usize * self.height as usize * 4;
        let known = self.layers.contains_key(&layer) || self.parked.contains_key(&layer);
        if !known || planes.len() != LayerStore::planes_of(kind) || planes.iter().any(|p| p.len() != size) {
            return false;
        }
        let ids: Vec<LayerId> = planes.iter().map(|_| self.next_plane()).collect();
        for (id, pixels) in ids.iter().zip(planes) {
            if self.upload_layer(gpu, *id, pixels).is_err() {
                for id in &ids {
                    self.layers.remove(id);
                }
                return false;
            }
        }
        self.stores.insert(layer, LayerStore { kind, planes: ids });
        true
    }

    fn next_plane(&mut self) -> LayerId {
        self.next_plane += 1;
        LayerId(PLANE_IDS + self.next_plane)
    }

    /// The store a stroke of `kind` on `layer` paints into: the one the layer keeps, if it is of
    /// that kind and its planes are here, else a new one, its paint the layer as it is and its
    /// height none (the store it had before kept with the stroke). None if it couldn't be made (the
    /// stroke then paints the layer itself).
    pub(super) fn store_for_stroke(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        layer: LayerId,
        kind: u32,
    ) -> Option<StoreStroke> {
        if kind != STORE_LIT {
            return None;
        }
        if let Some(s) = self.stores.get(&layer)
            && s.kind == kind
            && let [paint, height] = s.planes[..]
            && self.layers.contains_key(&paint)
            && self.layers.contains_key(&height)
        {
            return Some(StoreStroke { planes: [paint, height], made: None });
        }
        let from = &self.layers.get(&layer)?.texture;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let invalid = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let made = [
            texture(device, "store paint", self.width, self.height, LAYER_FORMAT),
            texture(device, "store height", self.width, self.height, LAYER_FORMAT),
        ];
        if let Some(e) = pollster::block_on(invalid.pop()).or(pollster::block_on(memory.pop())) {
            log::error!("a layer's store: {e}");
            return None;
        }
        let [paint, height] = made;
        encoder.copy_texture_to_texture(copy_at(from, 0, 0), copy_at(&paint, 0, 0), extent(self.width, self.height));
        let height_view = height.create_view(&wgpu::TextureViewDescriptor::default());
        drop(render_pass(encoder, "store height clear", &height_view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)));
        let ids = [self.next_plane(), self.next_plane()];
        let paint_view = paint.create_view(&wgpu::TextureViewDescriptor::default());
        self.layers.insert(ids[0], LayerGpu { texture: paint, view: paint_view });
        self.layers.insert(ids[1], LayerGpu { texture: height, view: height_view });
        let before = self.stores.insert(layer, LayerStore { kind, planes: ids.to_vec() });
        Some(StoreStroke { planes: ids, made: Some(before) })
    }

    /// The open smudge stroke's `instances` (with what each picks up) laid into its layer's store
    /// after their paint: each one's tip's grey over the height, as opaque as it says, then the
    /// layer drawn from the planes wherever they reached. `brush`: the stroke's shapes.
    pub(super) fn lay_store(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        brush: &wgpu::BindGroup,
        (instances, picks): (&[StampInstance], &[Option<PickUp>]),
        style: &StrokeStyle,
    ) {
        let Some((layer, [paint, height])) =
            self.smudge.as_ref().and_then(|s| Some((s.layer, s.store.as_ref()?.planes)))
        else {
            return;
        };
        let full = self.full();
        let (w, h) = (self.width as f32, self.height as f32);
        let mut laid = Vec::new();
        let mut rects = Vec::new();
        for (s, pick) in instances.iter().zip(picks) {
            let center = Vec2::new(s.center[0], s.center[1]);
            let bbox = IRect::around(center, (s.radius + QUAD_REACH) * s.roundness.max(1.0) * std::f32::consts::SQRT_2)
                .intersect(full);
            if bbox.is_empty() || s.radius <= 0.0 {
                continue;
            }
            rects.push(bbox);
            // (the stroke's opacity in it, as in the paint it lays)
            let opacity = pick.and_then(|p| p.height).filter(|o| o.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0)
                * style.opacity.clamp(0.0, 1.0);
            if opacity <= 0.0 || style.shape.is_none() {
                continue;
            }
            let layer_of = style.shape.map_or(0, |shape| shape + s.tip as u32) as f32;
            laid.push(HeightInstance {
                a: [s.center[0], s.center[1], s.radius.max(0.5), s.roundness],
                b: [s.angle, s.flip[0], s.flip[1], layer_of],
                c: [opacity, s.lightness, style.tip_kind as u32 as f32, if style.shape_sharp { 1.0 } else { 0.0 }],
                d: [w, h, style.shape_lod_cap, 0.0],
                e: [bbox.x0 as f32, bbox.y0 as f32, bbox.x1 as f32, bbox.y1 as f32],
            });
        }
        if rects.is_empty() {
            return;
        }
        // what it changes there kept as it was first, for its undo step
        if !self.smudge_keep(device, encoder, &rects) {
            return;
        }
        let pipelines = self.pipelines.clone();
        let store = pipelines.store(device);
        if !laid.is_empty()
            && let Some(target) = self.layers.get(&height)
        {
            use wgpu::util::DeviceExt;
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("store heights"),
                contents: bytemuck::cast_slice(&laid),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let mut pass = render_pass(encoder, "store heights", &target.view, wgpu::LoadOp::Load);
            pass.set_pipeline(&store.height);
            pass.set_bind_group(0, brush, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..laid.len() as u32);
        }
        self.draw_lit(device, encoder, layer, [paint, height], &rects);
    }

    /// Layer `layer` drawn from its store's planes (`planes`: the paint, the height) within `rects`.
    pub(super) fn draw_lit(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        layer: LayerId,
        [paint, height]: [LayerId; 2],
        rects: &[IRect],
    ) {
        let full = self.full();
        let rects: Vec<IRect> = rects.iter().map(|r| r.intersect(full)).filter(|r| !r.is_empty()).collect();
        let (Some(target), Some(paint), Some(height)) =
            (self.layers.get(&layer), self.layers.get(&paint), self.layers.get(&height))
        else {
            return;
        };
        if rects.is_empty() {
            return;
        }
        let store = self.pipelines.store(device);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("store planes"),
            layout: &store.lit_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&paint.view) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&height.view) },
            ],
        });
        let mut pass = render_pass(encoder, "store lit", &target.view, wgpu::LoadOp::Load);
        pass.set_pipeline(&store.lit);
        pass.set_bind_group(0, &bind, &[]);
        for rect in &rects {
            scissor(&mut pass, *rect);
            pass.draw(0..3, 0..1);
        }
        drop(pass);
        self.dirty = rects.iter().fold(self.dirty, |d, r| d.union(*r));
    }

    /// The stores of the layers whose pixels `step` changes without changing their planes too:
    /// their paint dried as it shows, each store kept in the step to come back with it.
    pub(super) fn dry_changed(&mut self, step: &mut Step) {
        if self.stores.is_empty() {
            return;
        }
        let changed: std::collections::HashSet<LayerId> = step.pixels.iter().map(|s| s.layer).collect();
        let dried: Vec<LayerId> = self
            .stores
            .iter()
            .filter(|(layer, s)| changed.contains(layer) && !s.planes.iter().all(|p| changed.contains(p)))
            .map(|(layer, _)| *layer)
            .collect();
        for layer in dried {
            let store = self.stores.remove(&layer);
            if !step.stores.iter().any(|(l, _)| *l == layer) {
                step.stores.push((layer, store));
            }
        }
    }

    /// The stores a step keeps (each layer's as it was) put back, and those they replace given
    /// back, for the step back.
    pub(super) fn swap_stores(
        &mut self,
        kept: Vec<(LayerId, Option<LayerStore>)>,
    ) -> Vec<(LayerId, Option<LayerStore>)> {
        kept.into_iter()
            .map(|(layer, store)| {
                let now = match store {
                    Some(s) => self.stores.insert(layer, s),
                    None => self.stores.remove(&layer),
                };
                (layer, now)
            })
            .collect()
    }

    /// Layer `id`'s pixels change with no undo step: its paint dries, its store gone for good.
    pub(super) fn dry(&mut self, id: LayerId) {
        self.stores.remove(&id);
    }

    /// The planes of the stores of `layers` (theirs as well as the layers).
    pub(super) fn with_planes(&self, layers: &[LayerId]) -> Vec<LayerId> {
        let planes = layers.iter().filter_map(|l| self.stores.get(l)).flat_map(|s| s.planes.iter().copied());
        layers.iter().copied().chain(planes).collect()
    }

    /// The layers being transformed that keep a store have its planes transformed with them (copied
    /// aside once, as they are); the layers whose planes are, to draw from them once transformed.
    pub(super) fn transform_stores_too(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Vec<(LayerId, [LayerId; 2])> {
        let moving: Vec<LayerId> = self.transforming.iter().chain(&self.transform_more).map(|t| t.layer).collect();
        let mut lit = Vec::new();
        for layer in &moving {
            let Some(store) = self.stores.get(layer).filter(|s| s.kind == STORE_LIT) else { continue };
            let [paint, height] = store.planes[..] else { continue };
            for plane in [paint, height] {
                if !moving.contains(&plane)
                    && !self.transform_more.iter().any(|t| t.layer == plane)
                    && let Some(t) = self.copy_aside(device, encoder, plane)
                {
                    self.transform_more.push(t);
                }
            }
            if self.transform_more.iter().any(|t| t.layer == paint)
                && self.transform_more.iter().any(|t| t.layer == height)
            {
                lit.push((*layer, [paint, height]));
            }
        }
        lit
    }

    /// The planes the history and the open work refer to, kept with their layers: those of the
    /// stores the layers `keep` keep, and of those the steps keep.
    pub(super) fn planes_kept(&self, keep: &std::collections::HashSet<LayerId>) -> Vec<LayerId> {
        let mut planes: Vec<LayerId> = self
            .stores
            .iter()
            .filter(|(layer, _)| keep.contains(layer))
            .flat_map(|(_, s)| s.planes.iter().copied())
            .collect();
        for step in self.undo.iter().chain(self.redo.iter()) {
            planes.extend(step.stores.iter().filter_map(|(_, s)| s.as_ref()).flat_map(|s| s.planes.iter().copied()));
        }
        planes
    }
}
