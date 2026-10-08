//! The last step every draw takes: an [`Entity`] — what it covers, how it is
//! shaded, how it blends, its depth, and the stencil its cover tests,
//! Impeller's `Entity` — placed in a draw context's texels as one [`Draw`].
//! [`StepEmitter`] owns the GPU-facing services this needs — and nothing
//! else, so the compiler guarantees emission cannot reach the context
//! stack, the pass list, or the walk's scopes: the context a draw goes into
//! arrives as an argument. The fragments' encoders, which make an entity's
//! shading, are in `shading`.

use valo_dl::{GradientStop, Image, Sampling, TileMode};
use valo_geometry::{FillRule, Matrix, Rect};

use crate::frame::{Draw, Geometry, Mesh, Stencil};
use crate::glyphs::{GlyphStore, PageRef};
use crate::host_buffer::{HostBuffer, UniformSlot};
use crate::images::ImageStore;
use crate::pipelines::{
    Frag, PipelineBlend, PipelineCache, PipelineKey, PipelineKind, GLYPH_VERTEX_FLOATS,
    MESH_VERTEX_FLOATS,
};
use crate::ramps::RampCache;
use crate::shader_abi::{slot, UniformRecord};

use super::draw_context::DrawContext;
use super::gaussian::MAX_KERNEL_SAMPLES;
use super::shading::Shading;

/// `EmitterServices` are the GPU-facing services the renderer keeps for
/// the step emitter, which borrows them each frame.
pub(crate) struct EmitterServices {
    /// The uniform and vertex arena every draw allocates from.
    pub host: HostBuffer,
    pub pipelines: PipelineCache,
    pub images: ImageStore,
    pub ramps: RampCache,
    pub samplers: LinearSamplers,
}

/// `LinearSamplers` are the linear samplers every filter and composite bind
/// group shares — created once (a per-frame create is a JS hop on wasm): one
/// clamping to the edge, which every draw of a texture reads with, and the
/// ones a blur's tile mode reads past its input's edge with, as Impeller sets
/// a blur's tile mode on its downsample's sampler.
pub(crate) struct LinearSamplers {
    clamp: wgpu::Sampler,
    repeat: wgpu::Sampler,
    mirror: wgpu::Sampler,
}

impl LinearSamplers {
    pub fn new(device: &wgpu::Device) -> Self {
        let linear = |label, address_mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: address_mode,
                address_mode_v: address_mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            })
        };
        Self {
            clamp: linear("valo.linear", wgpu::AddressMode::ClampToEdge),
            repeat: linear("valo.linear.repeat", wgpu::AddressMode::Repeat),
            mirror: linear("valo.linear.mirror", wgpu::AddressMode::MirrorRepeat),
        }
    }

    /// `for_tile_mode` reads past a texture's edge the way `tile_mode` says.
    /// Decal reads as clamp here and the shader cuts it off: WebGPU has no
    /// transparent border colour in its baseline.
    fn for_tile_mode(&self, tile_mode: TileMode) -> &wgpu::Sampler {
        match tile_mode {
            TileMode::Clamp | TileMode::Decal => &self.clamp,
            TileMode::Repeat => &self.repeat,
            TileMode::Mirror => &self.mirror,
        }
    }
}

/// `Entity` is one draw before it is placed in its context's texels.
pub(super) struct Entity {
    /// The stencil write its cover tests, drawn right before it.
    pub stencil: Option<StencilWrite>,
    /// What it covers.
    pub cover: Cover,
    /// How its pipeline treats depth and stencil.
    pub role: Role,
    pub shading: Shading,
    pub blend: PipelineBlend,
    /// Its depth on the context's line.
    pub z: f32,
}

/// `StencilWrite` is a mesh marked into the stencil: a fill's fan, wound by
/// its rule, or a stroke's strip, every triangle counting. It lies at depth
/// 0, so its cover's depth test is what occludes.
pub(super) struct StencilWrite {
    pub transform: Matrix,
    pub mesh: Mesh,
    pub marking: Marking,
}

/// `Marking` is how a stencil write counts its triangles.
pub(super) enum Marking {
    Fan(FillRule),
    Strip,
}

/// `Cover` is what a draw covers.
pub(super) enum Cover {
    /// The unit quad over `rect` — in local coordinates, the fragment's
    /// local space too — under `transform`.
    Quad { transform: Matrix, rect: Rect },
    /// A mesh of local positions under `transform`.
    Mesh { transform: Matrix, mesh: Mesh },
    /// The context's whole texture in its own texels, past its area's
    /// origin, so a clip's ceiling reaches the texels the area's rounding
    /// added.
    Texels,
}

/// `Role` is how a draw's pipeline treats depth and stencil.
#[derive(Clone, Copy)]
pub(super) enum Role {
    /// A depth-tested quad; an opaque one writes its depth.
    Fill,
    /// A quad drawn where its stencil was marked, resetting it.
    StencilledFill,
    /// A depth-tested triangle strip.
    Stroke,
    /// Depth-tested glyph quads.
    Glyphs,
    /// A clip's depth ceiling, over what its stencil marked
    /// (`difference`) or what it left.
    Clip { difference: bool },
}

impl Role {
    /// `kind` is the pipeline kind of a draw of this role shaded `frag`.
    fn kind(self, frag: Frag, opaque: bool) -> PipelineKind {
        match (self, opaque) {
            (Role::Fill, false) => PipelineKind::Draw(frag),
            (Role::Fill, true) => PipelineKind::OpaqueDraw(frag),
            (Role::StencilledFill, false) => PipelineKind::Cover(frag),
            (Role::StencilledFill, true) => PipelineKind::OpaqueCover(frag),
            (Role::Stroke, _) => PipelineKind::Strip(frag),
            (Role::Glyphs, _) => PipelineKind::Glyphs(frag),
            (Role::Clip { difference }, _) => PipelineKind::ClipCover { difference },
        }
    }
}

/// `StepEmitter` places entities in draw contexts — the only maker of
/// draws in the planner. Its fields are the GPU-facing services (uniform
/// arena, bind-group factory, pipeline layouts, texture caches, target
/// format); everything scene-shaped is a parameter.
pub(super) struct StepEmitter<'a> {
    host: &'a mut HostBuffer,
    device: &'a wgpu::Device,
    pipelines: &'a PipelineCache,
    images: &'a mut ImageStore,
    ramps: &'a mut RampCache,
    samplers: &'a LinearSamplers,
    format: wgpu::TextureFormat,
}

impl<'a> StepEmitter<'a> {
    /// `new` is the emitter of one frame into targets of `format`, over the
    /// renderer's `services`.
    pub fn new(
        services: &'a mut EmitterServices,
        device: &'a wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Self {
        let EmitterServices {
            host,
            pipelines,
            images,
            ramps,
            samplers,
        } = services;
        Self {
            host,
            device,
            pipelines,
            images,
            ramps,
            samplers,
            format,
        }
    }

    /// `push` places `entity` in `context` and appends it to the context's
    /// draws.
    pub fn push(&mut self, context: &mut DrawContext, entity: Entity) {
        let draw = self.place(context, entity);
        context.draws.push(draw);
    }

    /// `push_clip` places a clip's `entity` in `context`, appends it, and
    /// keeps it for a fresh pass to replay.
    pub fn push_clip(&mut self, context: &mut DrawContext, entity: Entity) {
        let draw = self.place(context, entity);
        context.clips.push(draw.clone());
        context.draws.push(draw);
    }

    /// `place` is `entity` placed in `context`'s texels: its record's
    /// transform written, its record and its stencil's allocated, its
    /// pipeline keyed.
    fn place(&mut self, context: &DrawContext, entity: Entity) -> Draw {
        let Entity {
            stencil,
            cover,
            role,
            mut shading,
            blend,
            z,
        } = entity;
        let stencil = stencil.map(|stencil| self.stencil(context, stencil));
        let geometry = match cover {
            Cover::Quad { transform, rect } => {
                shading.record.set(slot::RECT, rect_payload(&rect));
                let model = transform.then(&rect_to_unit(&rect));
                shading.record.set_mvp(ortho(context, &model, z));
                Geometry::Quad
            }
            Cover::Mesh { transform, mesh } => {
                shading.record.set_mvp(ortho(context, &transform, z));
                Geometry::Mesh(mesh)
            }
            Cover::Texels => {
                let size = context.area.size();
                let texels = Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32);
                shading
                    .record
                    .set_mvp(ortho_mvp(&rect_to_unit(&texels), size, z));
                Geometry::Quad
            }
        };
        // An opaque colour replaces what it covers only where it would
        // have covered it anyway.
        let opaque = shading.opaque && matches!(blend, PipelineBlend::SrcOver | PipelineBlend::Src);
        let kind = role.kind(shading.frag, opaque);
        Draw {
            stencil,
            key: PipelineKey::new(self.format, blend, kind),
            uniforms: self.host.alloc_uniform(shading.record.bytes()),
            bindings: shading.bindings,
            geometry,
            z,
        }
    }

    /// `stencil` is a stencil write placed in `context`'s texels, at depth
    /// 0.
    fn stencil(&mut self, context: &DrawContext, write: StencilWrite) -> Stencil {
        let mut record = UniformRecord::tinted([0.0; 4]);
        record.set_mvp(ortho(context, &write.transform, 0.0));
        let kind = match write.marking {
            Marking::Fan(rule) => PipelineKind::StencilFan {
                even_odd: rule == FillRule::EvenOdd,
            },
            Marking::Strip => PipelineKind::StencilStrip,
        };
        Stencil {
            key: PipelineKey::new(self.format, PipelineBlend::SrcOver, kind),
            uniforms: self.host.alloc_uniform(record.bytes()),
            mesh: write.mesh,
        }
    }

    /// `filter_draw` is the one draw of a filter pass into a texture of
    /// `format`, `extent` texels big: `quad`, in the texture's own pixels,
    /// shaded by `shading`, never depth-tested.
    pub fn filter_draw(
        &mut self,
        format: wgpu::TextureFormat,
        quad: &Rect,
        extent: [u32; 2],
        mut shading: Shading,
    ) -> Draw {
        shading.record.set(slot::RECT, rect_payload(quad));
        shading
            .record
            .set_mvp(ortho_mvp(&rect_to_unit(quad), extent, 0.0));
        Draw {
            stencil: None,
            key: PipelineKey::new(
                format,
                PipelineBlend::SrcOver,
                PipelineKind::Filter(shading.frag),
            ),
            uniforms: self.host.alloc_uniform(shading.record.bytes()),
            bindings: shading.bindings,
            geometry: Geometry::Quad,
            z: 0.0,
        }
    }

    /// `alloc_mesh` uploads transient strip or fan vertices: local
    /// positions.
    pub fn alloc_mesh(&mut self, vertices: &[f32]) -> Mesh {
        self.alloc_vertices(vertices, MESH_VERTEX_FLOATS)
    }

    /// `alloc_glyph_mesh` uploads glyph-quad vertices: a position followed
    /// by an atlas uv each.
    pub fn alloc_glyph_mesh(&mut self, vertices: &[f32]) -> Mesh {
        self.alloc_vertices(vertices, GLYPH_VERTEX_FLOATS)
    }

    fn alloc_vertices(&mut self, vertices: &[f32], floats_per_vertex: usize) -> Mesh {
        Mesh {
            slot: self.host.alloc_vertices(bytemuck::cast_slice(vertices)),
            vertices: (vertices.len() / floats_per_vertex) as u32,
        }
    }

    /// `alloc_kernel` uploads a blur pass's kernel — Impeller's
    /// `KernelSamples` block, too big for the shared payload — into the host
    /// buffer beside the draw records, for its draw to bind.
    pub fn alloc_kernel(&mut self, kernel: &[[f32; 4]; MAX_KERNEL_SAMPLES]) -> UniformSlot {
        self.host.alloc_kernel(bytemuck::cast_slice(kernel))
    }

    /// `texture_bind` is a group-1 bind of one sampled view plus the linear
    /// sampler that clamps at the texture's edge.
    pub fn texture_bind(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.tiled_texture_bind(view, TileMode::Clamp)
    }

    /// `tiled_texture_bind` is [`StepEmitter::texture_bind`] with a sampler
    /// that reads past the texture's edge the way `tile_mode` says (decal
    /// reads as clamp, for the shader to cut off).
    pub fn tiled_texture_bind(
        &self,
        view: &wgpu::TextureView,
        tile_mode: TileMode,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("valo.step.texture"),
            layout: self.pipelines.texture_bind_layout(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(
                        self.samplers.for_tile_mode(tile_mode),
                    ),
                },
            ],
        })
    }

    /// `blend_bind` is a group-1 bind of two textures, a destination copy
    /// and a source, and the clamping sampler: an advanced blend's, or a
    /// two-input merge's.
    pub fn blend_bind(&self, dst: &wgpu::TextureView, src: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("valo.step.blend"),
            layout: self.pipelines.blend_bind_layout(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(dst),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.samplers.clamp),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(src),
                },
            ],
        })
    }

    /// `image_bind` is a group-1 bind of `image`, sampled as `sampling`
    /// says.
    pub fn image_bind(&mut self, image: &Image, sampling: Sampling) -> wgpu::BindGroup {
        self.images
            .bind_group(self.pipelines.texture_bind_layout(), image, sampling)
    }

    /// `ramp` is a group-1 bind of the baked ramp for `stops`, and how many
    /// texels it holds.
    pub fn ramp(&mut self, stops: &[GradientStop]) -> (wgpu::BindGroup, u32) {
        let (view, texels) = self.ramps.ensure(self.device, stops);
        (self.texture_bind(&view), texels)
    }

    /// `atlas_bind` is a group-1 bind of one glyph atlas page. The store
    /// owns the pages and the emitter owns the bind-group layout, so the two
    /// meet here rather than either side reaching into the other.
    pub fn atlas_bind(&self, glyphs: &mut GlyphStore, page: PageRef) -> wgpu::BindGroup {
        glyphs.bind_group(self.pipelines.texture_bind_layout(), page)
    }

    /// `filtered_image` is the cache slot of a colour-filtered copy of
    /// `source`; `true` means it was made now, for a filter pass to fill.
    pub fn filtered_image(
        &mut self,
        source: &Image,
        filter: valo_dl::ColorFilter,
    ) -> (Image, bool) {
        self.images.filtered_image(source, filter)
    }
}

/// `ortho` builds the MVP for `context`: transforms live in the space
/// beneath its texture; its area's origin is subtracted so children land in
/// its texels.
fn ortho(context: &DrawContext, m: &Matrix, z: f32) -> [f32; 16] {
    let o = context.area.origin();
    let shifted = Matrix::translation(-o.x, -o.y).then(m);
    ortho_mvp(&shifted, context.area.size(), z)
}

/// `rect_to_unit` maps the unit quad onto `r` (bakes geometry into the MVP).
fn rect_to_unit(r: &Rect) -> Matrix {
    Matrix::from_affine(r.width, 0.0, 0.0, r.height, r.x, r.y)
}

/// `rect_payload` is `rect` as the record's local-rect slot holds it.
fn rect_payload(rect: &Rect) -> [f32; 4] {
    [rect.x, rect.y, rect.width, rect.height]
}

/// `ortho_mvp` maps model → column-major mat4 MVP: y-down ortho
/// (x: [0,w]→[-1,1], y: [0,h]→[1,-1]) with the draw's depth slot folded in.
/// The z row is REPLACED by z × (w row): after the hardware divide every
/// fragment lands exactly at the draw's slot, perspective or not — the
/// model's own z output is meaningless for 2D content (Impeller's
/// convention).
#[rustfmt::skip]
fn ortho_mvp(m: &Matrix, size: [u32; 2], z: f32) -> [f32; 16] {
    let (w, h) = (size[0] as f32, size[1] as f32);
    let projection = glam::Mat4::from_cols_array(&[
        2.0 / w, 0.0,      0.0, 0.0,
        0.0,     -2.0 / h, 0.0, 0.0,
        0.0,     0.0,      1.0, 0.0,
        -1.0,    1.0,      0.0, 1.0,
    ]);
    let mut mvp = projection * m.to_mat4();
    mvp.x_axis.z = z * mvp.x_axis.w;
    mvp.y_axis.z = z * mvp.y_axis.w;
    mvp.z_axis.z = z * mvp.z_axis.w;
    mvp.w_axis.z = z * mvp.w_axis.w;
    mvp.to_cols_array()
}
