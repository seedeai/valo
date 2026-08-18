//! The last step every draw takes: a uniform record, a pipeline key, a
//! [`Step`] on a frame. [`StepEmitter`] owns the GPU-facing services this
//! needs — and nothing else, so the compiler guarantees emission cannot
//! reach the target stack, the pass list, or the group-alpha stack. What a
//! step needs from those worlds arrives as arguments: the frame it appends
//! to and the already-resolved group alpha. That pair is the whole
//! interface between policy and emission.
//!
//! The second half of this file is the shader ABI: the payload-slot layout
//! shared with `shaders/solid.wgsl` and the encoders that fill it
//! (gradients, patterns, color filters, the conical-gradient setup).

use std::sync::Arc;

use valo_dl::{
    BlendMode, ColorFilter, DisplayList, FocalCircle, Image, Paint, Sampling, Shader, SpreadMode,
    TileMode, MAX_GRADIENT_STOPS,
};
use valo_geometry::{Color, FillRule, Matrix, Point, Rect};

use crate::frame::Step;
use crate::glyphs::{GlyphStore, PageRef};
use crate::host_buffer::{HostBuffer, VertexSlot};
use crate::images::ImageStore;
use crate::pipelines::{
    advanced_mode_id, blend_filter_id, blur_style_id, Frag, PipelineCache, PipelineKey,
    PipelineKind, TextMode,
};
use crate::ramps::RampCache;
use crate::raster::{ListRasterCache, RasterVerdict};

use super::layers::PassFrame;

// The `shaders/solid.wgsl` layout contract.
const PAYLOAD_RECT: usize = 0;
pub(super) const PAYLOAD_GEOM: usize = 1;
pub(super) const PAYLOAD_MISC: usize = 2;
const PAYLOAD_OFFSETS: usize = 3; // ..5: 8 stop offsets
const PAYLOAD_RADII: usize = 3; // rrect-blur corner radii (no gradient there)
const PAYLOAD_DECAL: usize = 3; // per-axis image decal flags (no gradient there)
const PAYLOAD_COLORS: usize = 5; // ..13: 8 premultiplied stop colors
const PAYLOAD_LOCAL: usize = 13; // ..15: inverse gradient local matrix
const PAYLOAD_CONICAL: usize = 15; // two-point conical case + constants
const PAYLOAD_CONICAL_FLAGS: usize = 16; // (swapped, focal on circle, well behaved)
const PAYLOAD_COLOR_MATRIX: usize = 17; // ..22: 4 matrix rows + the translation column;
                                        // doubles as the blend filter's source colour

/// `StepEmitter` turns resolved draw decisions into [`Step`]s — the only
/// maker of steps in the planner. Its fields are the GPU-facing services
/// (uniform arena, bind-group factory, pipeline layouts, texture caches,
/// target format); everything scene-shaped is a parameter.
pub(super) struct StepEmitter<'a> {
    host: &'a mut HostBuffer,
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    pipelines: &'a PipelineCache,
    images: &'a mut ImageStore,
    ramps: &'a mut RampCache,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
}

/// `UniformRecord` is one draw's uniform block: MVP, tint, payload slots.
pub(super) struct UniformRecord {
    bytes: [u8; crate::host_buffer::UNIFORM_SIZE as usize],
}

impl UniformRecord {
    fn new(mvp: [f32; 16], color: [f32; 4]) -> Self {
        let mut bytes = [0u8; crate::host_buffer::UNIFORM_SIZE as usize];
        bytes[0..64].copy_from_slice(bytemuck::cast_slice(&mvp));
        bytes[64..80].copy_from_slice(bytemuck::cast_slice(&color));
        Self { bytes }
    }

    pub(super) fn set_payload(&mut self, index: usize, v: [f32; 4]) {
        let start = 80 + index * 16;
        self.bytes[start..start + 16].copy_from_slice(bytemuck::cast_slice(&v));
    }

    /// `set_local_rect` stores the draw's local-space rect — fragments
    /// reconstruct local position from it.
    fn set_local_rect(&mut self, r: &Rect) {
        self.set_payload(PAYLOAD_RECT, [r.x, r.y, r.width, r.height]);
    }
}

impl<'a> StepEmitter<'a> {
    #[expect(
        clippy::too_many_arguments,
        reason = "one-shot wiring of the GPU-facing services"
    )]
    pub fn new(
        host: &'a mut HostBuffer,
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        pipelines: &'a PipelineCache,
        images: &'a mut ImageStore,
        ramps: &'a mut RampCache,
        sampler: wgpu::Sampler,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            host,
            device,
            queue,
            pipelines,
            images,
            ramps,
            sampler,
            format,
        }
    }

    /// `paint_quad` emits a paint quad after effects are gone: uniforms,
    /// the paint's shader payload/bind, opaque promotion when the paint
    /// covers every pixel it touches. `group_alpha` is the caller's
    /// resolved elided-group alpha.
    #[expect(
        clippy::too_many_arguments,
        reason = "the draw's full resolved inputs, passed explicitly by design"
    )]
    pub fn paint_quad(
        &mut self,
        frame: &mut PassFrame,
        group_alpha: f32,
        kind: PipelineKind,
        quad: &Rect,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let model = current.then(&rect_to_unit(quad));
        let tint = tinted(paint, group_alpha);
        let mut record = UniformRecord::new(ortho(frame, &model, z), tint);
        record.set_local_rect(quad);
        let bind = self.shader_payload(&mut record, paint);
        let kind = promote_opaque(kind, paint, group_alpha);
        self.push_step(frame, kind, paint.blend_mode, record, bind, None, z);
    }

    /// `blend_solid_quad` emits the fragment side of a solid advanced
    /// blend: given the caller's dst `snapshot`, one quad whose fragment
    /// runs the Porter-Duff / separable math. The pipeline blend is SrcOver
    /// — the result replaces what the snapshot captured.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors paint_quad plus the blend inputs"
    )]
    pub fn blend_solid_quad(
        &mut self,
        frame: &mut PassFrame,
        group_alpha: f32,
        kind: PipelineKind,
        quad: &Rect,
        paint: &Paint,
        current: &Matrix,
        z: f32,
        mode: BlendMode,
        snapshot: &wgpu::TextureView,
    ) {
        let model = current.then(&rect_to_unit(quad));
        let tint = scaled_premul(paint.color, group_alpha);
        let mut record = UniformRecord::new(ortho(frame, &model, z), tint);
        record.set_local_rect(quad);
        self.set_blend_misc(frame, &mut record, mode);
        let bind = self.texture_bind(snapshot);
        self.push_step(frame, kind, BlendMode::SrcOver, record, Some(bind), None, z);
    }

    /// `strip_step` emits one stroke strip along pre-built vertices. Local
    /// space equals position, so gradients compose without a local rect;
    /// `tint` already carries group alpha and subpixel-coverage fade.
    pub fn strip_step(
        &mut self,
        frame: &mut PassFrame,
        tint: [f32; 4],
        paint: &Paint,
        current: &Matrix,
        mesh: (VertexSlot, u32),
        z: f32,
    ) {
        let mut record = UniformRecord::new(ortho(frame, current, z), tint);
        let bind = self.shader_payload(&mut record, paint);
        self.push_step(
            frame,
            PipelineKind::Strip(paint_frag(paint)),
            paint.blend_mode,
            record,
            bind,
            Some(mesh),
            z,
        );
    }

    /// `image_step` emits one sampled-image draw after effects and advanced
    /// blends were peeled off. Tints with ALPHA only — the image shader
    /// multiplies samples by the tint, and the default paint colour is
    /// black. A colour filter runs on the sampled pixel in the same draw
    /// (Impeller's atlas path).
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the DrawImage op's fields 1:1"
    )]
    pub fn image_step(
        &mut self,
        frame: &mut PassFrame,
        group_alpha: f32,
        image: &Image,
        src: &Rect,
        dst: &Rect,
        sampling: Sampling,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let model = current.then(&rect_to_unit(dst));
        let tint = alpha_tint(paint.color.a * group_alpha);
        let mut record = UniformRecord::new(ortho(frame, &model, z), tint);
        record.set_local_rect(dst);
        record.set_payload(PAYLOAD_GEOM, uv_mapping(image, src, dst));
        record.set_payload(PAYLOAD_DECAL, decal_flags(sampling));
        let fragment = match paint.color_filter {
            None => Frag::Image,
            Some(filter) => match encode_color_filter(&mut record, filter) {
                EncodedColorFilter::Matrix => Frag::ImageMatrix,
                EncodedColorFilter::Blend => Frag::ImageBlend,
            },
        };
        let bind = self
            .images
            .bind_group(self.pipelines.texture_bind_layout(), image, sampling);
        self.push_step(
            frame,
            PipelineKind::Draw(fragment),
            paint.blend_mode,
            record,
            Some(bind),
            None,
            z,
        );
    }

    /// `rrect_blur_step` emits the closed-form blurred (r)rect: ONE quad
    /// spanning the 3σ spread, coverage evaluated analytically — no layer,
    /// no filter passes. `paint.mask_blur` is required; the recorded op
    /// always carries one.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the RRectBlur op's fields 1:1"
    )]
    pub fn rrect_blur_step(
        &mut self,
        frame: &mut PassFrame,
        group_alpha: f32,
        rect: &Rect,
        radii: [f32; 4],
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let mask = paint.mask_blur.expect("recorded with mask_blur");
        let quad = rect.expand(paint.mask_padding());
        let model = current.then(&rect_to_unit(&quad));
        let tint = scaled_premul(paint.color, group_alpha);
        let mut record = UniformRecord::new(ortho(frame, &model, z), tint);
        record.set_local_rect(&quad);
        record.set_payload(
            PAYLOAD_GEOM,
            [rect.x, rect.y, rect.x + rect.width, rect.y + rect.height],
        );
        record.set_payload(
            PAYLOAD_MISC,
            [
                mask.sigma.max(0.05),
                blur_style_id(mask.style) as f32,
                0.0,
                0.0,
            ],
        );
        record.set_payload(PAYLOAD_RADII, radii);
        self.push_step(
            frame,
            PipelineKind::Draw(Frag::RRectBlur),
            paint.blend_mode,
            record,
            None,
            None,
            z,
        );
    }

    /// `text_step` emits one batch of glyph quads: every glyph of a run
    /// that landed on one atlas page, in one mesh. `tint` arrives resolved —
    /// the caller has already folded in the group alpha and the mode's own
    /// tinting rule (colour glyphs keep their palette).
    #[expect(
        clippy::too_many_arguments,
        reason = "one batch's full resolved inputs, passed explicitly by design"
    )]
    pub fn text_step(
        &mut self,
        frame: &mut PassFrame,
        mode: TextMode,
        tint: [f32; 4],
        blend: BlendMode,
        model: &Matrix,
        mesh: (VertexSlot, u32),
        page: wgpu::BindGroup,
        z: f32,
    ) {
        let record = UniformRecord::new(ortho(frame, model, z), tint);
        self.push_step(
            frame,
            PipelineKind::Text { mode },
            blend,
            record,
            Some(page),
            Some(mesh),
            z,
        );
    }

    /// `atlas_bind` is a group-1 bind of one glyph atlas page. The store
    /// owns the pages and the emitter owns the bind-group layout, so the two
    /// meet here rather than either side reaching into the other.
    pub fn atlas_bind(&self, glyphs: &mut GlyphStore, page: PageRef) -> wgpu::BindGroup {
        glyphs.bind_group(self.pipelines.texture_bind_layout(), page)
    }

    /// `raster_verdict` asks the list-raster cache what to do with one
    /// hinted embed. Answering it can allocate the entry's texture, which is
    /// why the device reaches the cache from here.
    pub fn raster_verdict(
        &self,
        rasters: &mut ListRasterCache,
        list: &Arc<DisplayList>,
        needed_scale: f32,
    ) -> RasterVerdict {
        rasters.resolve(
            self.device,
            self.format,
            list,
            needed_scale,
            self.device.limits().max_texture_dimension_2d,
        )
    }

    /// `raster_quad_step` emits the sampled quad standing in for a whole
    /// cached sub-list — one full-texture composite, premultiplied SrcOver,
    /// exactly like a layer's. `extent` is the texture's size in destination
    /// units, which past the cached content is larger than `dest`.
    pub fn raster_quad_step(
        &mut self,
        frame: &mut PassFrame,
        dest: &Rect,
        extent: [f32; 2],
        view: &wgpu::TextureView,
        z: f32,
    ) {
        let mut record = self.quad_record(frame, dest, [1.0, 1.0, 1.0, 1.0], z);
        // UVs map dest coords onto the FULL texture, which is ceil-sized past
        // the content the way layer textures are — the same convention every
        // composite uses.
        let sample = Rect::new(dest.x, dest.y, extent[0], extent[1]);
        record.set_payload(PAYLOAD_GEOM, full_rect_uv(&sample));
        let bind = self.texture_bind(view);
        self.push_step(
            frame,
            PipelineKind::Draw(Frag::Image),
            BlendMode::SrcOver,
            record,
            Some(bind),
            None,
            z,
        );
    }

    /// `clip_cover_step` emits a Difference clip's ceiling: the shape's
    /// bounds through the current transform, writing the scope's expiry
    /// depth wherever the stencil marked the shape's INTERIOR.
    pub fn clip_cover_step(
        &mut self,
        frame: &mut PassFrame,
        bounds: &Rect,
        current: &Matrix,
        z: f32,
    ) {
        let model = current.then(&rect_to_unit(bounds));
        let record = UniformRecord::new(ortho(frame, &model, z), [0.0; 4]);
        self.push_step(
            frame,
            PipelineKind::ClipCover { difference: true },
            BlendMode::SrcOver,
            record,
            None,
            None,
            z,
        );
    }

    /// `clip_ceiling_step` emits an Intersect clip's ceiling: a cover over
    /// the whole frame in FRAME pixels, writing the scope's expiry depth
    /// wherever the stencil left the shape's EXTERIOR. It bypasses the layer
    /// origin shift on purpose, so the ceiling reaches the attachment's
    /// ceil padding too.
    pub fn clip_ceiling_step(&mut self, frame: &mut PassFrame, z: f32) {
        let viewport = Rect::new(0.0, 0.0, frame.size[0] as f32, frame.size[1] as f32);
        let record =
            UniformRecord::new(ortho_mvp(&rect_to_unit(&viewport), frame.size, z), [0.0; 4]);
        self.push_step(
            frame,
            PipelineKind::ClipCover { difference: false },
            BlendMode::SrcOver,
            record,
            None,
            None,
            z,
        );
    }

    /// `push_fan` emits the stencil half of stencil-then-cover: a fan at
    /// z = 0 so the subsequent cover's depth test is what actually occludes.
    pub fn push_fan(
        &mut self,
        frame: &mut PassFrame,
        rule: FillRule,
        current: &Matrix,
        mesh: (VertexSlot, u32),
        z: f32,
    ) {
        let record = UniformRecord::new(ortho(frame, current, 0.0), [0.0; 4]);
        self.push_step(
            frame,
            fan_kind(rule),
            BlendMode::SrcOver,
            record,
            None,
            Some(mesh),
            z,
        );
    }

    /// `alloc_mesh` uploads transient strip/fan vertices and returns the
    /// mesh handle a step carries.
    pub fn alloc_mesh(&mut self, vertices: &[f32]) -> (VertexSlot, u32) {
        let slot = self.host.alloc_vertices(bytemuck::cast_slice(vertices));
        (slot, (vertices.len() / 2) as u32)
    }

    /// `alloc_text_mesh` uploads glyph-quad vertices, which carry four
    /// floats each — position followed by atlas uv — where the strip and fan
    /// meshes carry two.
    pub fn alloc_text_mesh(&mut self, vertices: &[f32]) -> (VertexSlot, u32) {
        let slot = self.host.alloc_vertices(bytemuck::cast_slice(vertices));
        (slot, (vertices.len() / 4) as u32)
    }

    /// `quad_record` is a quad at an ABSOLUTE rect (composites) —
    /// origin-shifted like any draw, no paint machinery.
    pub fn quad_record(
        &self,
        frame: &PassFrame,
        rect: &Rect,
        tint: [f32; 4],
        z: f32,
    ) -> UniformRecord {
        let mut record = UniformRecord::new(ortho(frame, &rect_to_unit(rect), z), tint);
        record.set_local_rect(rect);
        record
    }

    /// `texture_bind` is a group-1 bind of one sampled view plus the frame
    /// sampler.
    pub fn texture_bind(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
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
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// `blend_bind` is a group-1 bind for an advanced blend: dst snapshot,
    /// sampler, src.
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
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(src),
                },
            ],
        })
    }

    /// `set_blend_misc` writes the advanced-blend mode id and the frame's
    /// pixel size into the misc payload (the fragment reconstructs dst UV
    /// from `gl_FragCoord`).
    pub fn set_blend_misc(&self, frame: &PassFrame, record: &mut UniformRecord, mode: BlendMode) {
        record.set_payload(
            PAYLOAD_MISC,
            [
                advanced_mode_id(mode) as f32,
                0.0,
                frame.size[0] as f32,
                frame.size[1] as f32,
            ],
        );
    }

    /// `filter_step` makes the one step of an independent filter pass —
    /// keyed on the pass target's format, never depth-tested.
    pub fn filter_step(
        &mut self,
        target_format: wgpu::TextureFormat,
        frag: Frag,
        record: UniformRecord,
        bind: wgpu::BindGroup,
    ) -> Step {
        let uniforms = self.host.alloc_uniform(&record.bytes);
        let key = PipelineKey::new(
            target_format,
            BlendMode::SrcOver,
            PipelineKind::Filter(frag),
        );
        Step {
            key,
            uniforms,
            texture: Some(bind),
            mesh: None,
            sort_z: 0.0,
        }
    }

    /// `filtered_image_entry` looks up (or creates) the cache slot for a
    /// colour-filtered copy of `source`; `true` means the caller must fill
    /// it with a filter pass now.
    pub fn filtered_image_entry(&mut self, source: &Image, filter: ColorFilter) -> (Image, bool) {
        self.images.filtered_image(source, filter)
    }

    /// `shader_payload` fills the paint's shader payload, returning the
    /// texture bind the draw must carry: a baked ramp for stop lists past
    /// the uniform budget, the image itself for a pattern, nothing for the
    /// rest.
    fn shader_payload(
        &mut self,
        record: &mut UniformRecord,
        paint: &Paint,
    ) -> Option<wgpu::BindGroup> {
        match paint.shader.as_ref()? {
            Shader::Image {
                image,
                sampling,
                local,
            } => {
                fill_pattern_payload(record, image, *sampling, local);
                Some(
                    self.images
                        .bind_group(self.pipelines.texture_bind_layout(), image, *sampling),
                )
            }
            shader => {
                let ramp = (shader.stops().len() > MAX_GRADIENT_STOPS).then(|| {
                    let (view, texels) = self.ramps.ensure(self.device, self.queue, shader.stops());
                    (self.texture_bind(&view), texels)
                });
                fill_gradient_payload(record, shader, ramp.as_ref().map(|(_, n)| *n));
                ramp.map(|(bind, _)| bind)
            }
        }
    }

    /// `push_step` is the one funnel every draw goes through: allocate a
    /// uniform slot, key the pipeline, append a [`Step`] to `frame`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the one funnel every draw goes through"
    )]
    pub fn push_step(
        &mut self,
        frame: &mut PassFrame,
        kind: PipelineKind,
        blend: BlendMode,
        record: UniformRecord,
        texture: Option<wgpu::BindGroup>,
        mesh: Option<(VertexSlot, u32)>,
        z: f32,
    ) {
        let uniforms = self.host.alloc_uniform(&record.bytes);
        let key = PipelineKey::new(self.format, blend, kind);
        frame.steps.push(Step {
            key,
            uniforms,
            texture,
            mesh,
            sort_z: z,
        });
    }
}

/// `promote_opaque` upgrades a provably-opaque paint to a depth-writing
/// pipeline so the reorder pass can hoist it front-to-back. Elided group
/// alpha makes any paint translucent.
fn promote_opaque(kind: PipelineKind, paint: &Paint, group_alpha: f32) -> PipelineKind {
    if group_alpha < 1.0 || !is_opaque_paint(paint) {
        return kind;
    }
    match kind {
        PipelineKind::Draw(f) => PipelineKind::OpaqueDraw(f),
        PipelineKind::Cover(f) => PipelineKind::OpaqueCover(f),
        other => other,
    }
}

/// `ortho` builds the MVP for `frame`: transforms live in the TOP pass's
/// coords; layer frames subtract their accumulated origin so children land
/// in layer pixels.
fn ortho(frame: &PassFrame, m: &Matrix, z: f32) -> [f32; 16] {
    let o = frame.origin;
    let shifted = Matrix::translation(-o.x, -o.y).then(m);
    ortho_mvp(&shifted, frame.size, z)
}

/// `rect_to_unit` maps the unit quad onto `r` (bakes geometry into the MVP).
fn rect_to_unit(r: &Rect) -> Matrix {
    Matrix::from_affine(r.width, 0.0, 0.0, r.height, r.x, r.y)
}

/// `tinted` is what multiplies the fragment family's output. Solid = the
/// color itself; image/gradient sources use paint ALPHA only (Skia's
/// drawImage semantics). `extra` folds in an elided group's alpha.
pub(super) fn tinted(paint: &Paint, extra: f32) -> [f32; 4] {
    if paint.shader.is_none() {
        scaled_premul(paint.color, extra)
    } else {
        alpha_tint(paint.color.a * extra)
    }
}

pub(super) fn scaled_premul(color: Color, alpha: f32) -> [f32; 4] {
    let [r, g, b, a] = color.premultiplied();
    [r * alpha, g * alpha, b * alpha, a * alpha]
}

pub(super) fn alpha_tint(a: f32) -> [f32; 4] {
    [a, a, a, a]
}

/// `full_rect_uv` maps a full texture stretched across `rect` into uv
/// (layer composites).
pub(super) fn full_rect_uv(rect: &Rect) -> [f32; 4] {
    let sx = 1.0 / rect.width;
    let sy = 1.0 / rect.height;
    [sx, sy, -rect.x * sx, -rect.y * sy]
}

/// `paint_frag` selects the fragment family for a paint's color source.
pub(super) fn paint_frag(paint: &Paint) -> Frag {
    let ramp = paint
        .shader
        .as_ref()
        .is_some_and(|s| s.stops().len() > MAX_GRADIENT_STOPS);
    match &paint.shader {
        None => Frag::Solid,
        Some(Shader::Linear { .. }) if ramp => Frag::LinearRamp,
        Some(Shader::Radial { .. }) if ramp => Frag::RadialRamp,
        Some(Shader::Sweep { .. }) if ramp => Frag::SweepRamp,
        Some(Shader::Linear { .. }) => Frag::Linear,
        Some(Shader::Radial { .. }) => Frag::Radial,
        Some(Shader::Sweep { .. }) => Frag::Sweep,
        Some(Shader::Image { .. }) => Frag::Pattern,
    }
}

fn fan_kind(rule: FillRule) -> PipelineKind {
    PipelineKind::StencilFan {
        even_odd: rule == FillRule::EvenOdd,
    }
}

fn is_opaque_paint(paint: &Paint) -> bool {
    let solid_blend = matches!(paint.blend_mode, BlendMode::SrcOver | BlendMode::Src);
    solid_blend
        && paint.mask_blur.is_none()
        && paint.effective_image_filter().is_none()
        && paint.color.a >= 1.0
        && paint.shader.as_ref().is_none_or(shader_opaque)
}

fn shader_opaque(shader: &Shader) -> bool {
    // A two-point conical gradient with a real start circle does not cover
    // the plane: outside its cone nothing is painted at all, so opaque
    // promotion would turn those pixels into replaced black. A gradient that
    // can leave gaps never qualifies, however opaque its stops are.
    if let Shader::Radial {
        center,
        focus: Some(circle),
        ..
    } = shader
    {
        if circle.radius > 0.0 || circle.center != *center {
            return false;
        }
    }
    let stops = match shader {
        Shader::Linear { stops, .. }
        | Shader::Radial { stops, .. }
        | Shader::Sweep { stops, .. } => stops,
        // A pattern's alpha lives in texels nobody has read at plan time.
        Shader::Image { .. } => return false,
    };
    stops.iter().all(|stop| stop.color.a >= 1.0)
}

/// `filter_quad_record` is the uniform block of one filter pass: a quad
/// mapped into the pass target's own pixel space, no depth, no tint.
pub(super) fn filter_quad_record(quad: &Rect, extent: [u32; 2]) -> UniformRecord {
    let mut record = UniformRecord::new(ortho_mvp(&rect_to_unit(quad), extent, 0.0), [0.0; 4]);
    record.set_local_rect(quad);
    record
}

/// `EncodedColorFilter` names which fragment variant consumes the payload
/// [`encode_color_filter`] wrote.
pub(super) enum EncodedColorFilter {
    Matrix,
    Blend,
}

/// `encode_color_filter` writes Impeller's mat4-plus-translation-vector
/// layout, or its constant premultiplied blend source. Draw and filter-pass
/// shaders share this ABI.
pub(super) fn encode_color_filter(
    record: &mut UniformRecord,
    filter: ColorFilter,
) -> EncodedColorFilter {
    match filter {
        ColorFilter::Matrix(matrix) => {
            for row in 0..4 {
                let start = row * 5;
                record.set_payload(
                    PAYLOAD_COLOR_MATRIX + row,
                    [
                        matrix[start],
                        matrix[start + 1],
                        matrix[start + 2],
                        matrix[start + 3],
                    ],
                );
            }
            record.set_payload(
                PAYLOAD_COLOR_MATRIX + 4,
                [matrix[4], matrix[9], matrix[14], matrix[19]],
            );
            EncodedColorFilter::Matrix
        }
        ColorFilter::Blend(color, mode) => {
            record.set_payload(PAYLOAD_COLOR_MATRIX, color.premultiplied());
            record.set_payload(PAYLOAD_MISC, [blend_filter_id(mode) as f32, 0.0, 0.0, 0.0]);
            EncodedColorFilter::Blend
        }
    }
}

/// `fill_gradient_payload` writes gradient geometry + stops into the
/// payload (see the WGSL layout contract above). A focal radial's fx/fy
/// ride the two spare floats (GEOM.w / MISC.w); focus == center encodes
/// "classic". The INVERSE of the shader's local matrix lands in
/// `PAYLOAD_LOCAL` — fragments map draw space into gradient space with it
/// (identity for plain gradients; a non-invertible matrix degenerates to a
/// constant ramp sample, never UB). `ramp_texels` = Some(N) when the stops
/// ride a baked texture: the count lane carries N for the fragment's
/// half-texel mapping, and the uniform stop arrays stay untouched (the
/// texture IS the ramp).
fn fill_gradient_payload(record: &mut UniformRecord, shader: &Shader, ramp_texels: Option<u32>) {
    let (geom, angle, misc_w) = match shader {
        Shader::Linear { start, end, .. } => ([start.x, start.y, end.x, end.y], 0.0, 0.0),
        Shader::Radial {
            center,
            radius,
            focus,
            ..
        } => {
            let f = focus.map_or(*center, |circle| circle.center);
            ([center.x, center.y, *radius, f.x], 0.0, f.y)
        }
        Shader::Sweep {
            center,
            start_angle,
            ..
        } => ([center.x, center.y, 0.0, 0.0], *start_angle, 0.0),
        Shader::Image { .. } => unreachable!("patterns fill their own payload"),
    };
    record.set_payload(PAYLOAD_GEOM, geom);

    let (Shader::Linear { local, .. }
    | Shader::Radial { local, .. }
    | Shader::Sweep { local, .. }
    | Shader::Image { local, .. }) = shader;
    let mut inverse = local
        .invert()
        .unwrap_or(Matrix::from_affine(0.0, 0.0, 0.0, 0.0, 0.0, 0.0));

    // A two-point conical gradient is solved in a space where the focal
    // point sits at the origin and the end circle is the unit circle. That
    // mapping is constant per draw, so it folds into the inverse local
    // matrix here and the fragment only runs the per-pixel half.
    let conical = match shader {
        Shader::Radial {
            center,
            radius,
            focus,
            ..
        } => ConicalSetup::solve(*center, *radius, *focus),
        _ => ConicalSetup::UNUSED,
    };
    if let Some(focal_map) = conical.focal_map {
        inverse = focal_map.then(&inverse);
    }
    record.set_payload(PAYLOAD_CONICAL, conical.constants);
    record.set_payload(PAYLOAD_CONICAL_FLAGS, conical.flags);

    // Gradient locals are affine by construction — the 2D block is exact.
    let [a, b, c, d, tx, ty] = inverse.to_affine();
    record.set_payload(PAYLOAD_LOCAL, [a, b, c, d]);
    record.set_payload(PAYLOAD_LOCAL + 1, [tx, ty, 0.0, 0.0]);

    let spread = match shader {
        Shader::Linear { spread, .. } | Shader::Radial { spread, .. } => *spread,
        // Sweep is periodic by nature; patterns tile through the sampler.
        Shader::Sweep { .. } | Shader::Image { .. } => SpreadMode::Pad,
    };
    let stops = shader.stops();
    let count = stops.len().min(MAX_GRADIENT_STOPS);
    let count_lane = match ramp_texels {
        Some(texels) => texels as f32,
        None => count as f32,
    };
    record.set_payload(
        PAYLOAD_MISC,
        [count_lane, angle, spread as u8 as f32, misc_w],
    );
    if ramp_texels.is_some() {
        return;
    }
    let mut offsets = [0.0f32; MAX_GRADIENT_STOPS];
    for (i, stop) in stops.iter().take(count).enumerate() {
        offsets[i] = stop.offset;
        record.set_payload(PAYLOAD_COLORS + i, stop.color.components());
    }
    record.set_payload(
        PAYLOAD_OFFSETS,
        [offsets[0], offsets[1], offsets[2], offsets[3]],
    );
    record.set_payload(
        PAYLOAD_OFFSETS + 1,
        [offsets[4], offsets[5], offsets[6], offsets[7]],
    );
}

/// `fill_pattern_payload` writes a pattern's mapping: `local⁻¹` into
/// pattern pixels, then the reciprocal image size to reach uv. Tiling and
/// filtering ride the sampler.
fn fill_pattern_payload(
    record: &mut UniformRecord,
    image: &Image,
    sampling: Sampling,
    local: &Matrix,
) {
    let size = image.size();
    record.set_payload(
        PAYLOAD_GEOM,
        [1.0 / size[0] as f32, 1.0 / size[1] as f32, 0.0, 0.0],
    );
    record.set_payload(PAYLOAD_DECAL, decal_flags(sampling));
    let inverse = local
        .invert()
        .unwrap_or(Matrix::from_affine(0.0, 0.0, 0.0, 0.0, 0.0, 0.0));
    let [a, b, c, d, tx, ty] = inverse.to_affine();
    record.set_payload(PAYLOAD_LOCAL, [a, b, c, d]);
    record.set_payload(PAYLOAD_LOCAL + 1, [tx, ty, 0.0, 0.0]);
}

/// `uv_mapping` is uv = local × scale + offset, mapping `dst` (local px)
/// onto `src` (texture px, normalized) — out-of-range uv is the sampler's
/// business.
fn uv_mapping(image: &Image, src: &Rect, dst: &Rect) -> [f32; 4] {
    let (tw, th) = (image.width(), image.height());
    let sx = src.width / (dst.width * tw);
    let sy = src.height / (dst.height * th);
    [sx, sy, src.x / tw - dst.x * sx, src.y / th - dst.y * sy]
}

/// `decal_flags` is the per-axis decal switch: 1 where the fragment must
/// cut off outside the image, 0 where the sampler's own address mode
/// already produces the right pixels. Every image-sampling fragment reads
/// these from `PAYLOAD_DECAL`, so a pattern and a direct `drawImage`
/// honour `TileMode::Decal` identically.
fn decal_flags(sampling: Sampling) -> [f32; 4] {
    [
        f32::from(sampling.tile_x == TileMode::Decal),
        f32::from(sampling.tile_y == TileMode::Decal),
        0.0,
        0.0,
    ]
}

/// `ConicalSetup` is which formula the fragment runs for a radial gradient,
/// plus the constants it needs. Skia's two-point conical algorithm
/// (skia.org/docs/dev/design/conical) splits into cases by where the focal
/// point lands; the choice and all of its precomputation are per-draw, so
/// they happen here rather than per fragment the way Impeller does it.
struct ConicalSetup {
    /// `(kind, local_r1, f, d_radius_sign)` — see `radial_t` in the shader.
    constants: [f32; 4],
    /// `(is_swapped, is_focal_on_circle, is_well_behaved, unused)`.
    flags: [f32; 4],
    /// Gradient space → focal space, when the general case needs it.
    focal_map: Option<Matrix>,
}

// Kinds, mirrored in `radial_t`.
const CONICAL_CONCENTRIC: f32 = 0.0;
const CONICAL_GENERAL: f32 = 1.0;
const CONICAL_EMPTY: f32 = 2.0;
const CONICAL_STRIP: f32 = 3.0;

impl ConicalSetup {
    const UNUSED: Self = Self {
        constants: [CONICAL_CONCENTRIC, 0.0, 0.0, 0.0],
        flags: [0.0; 4],
        focal_map: None,
    };

    /// `solve` runs Skia's `SkConicalGradient` decomposition, once per draw.
    fn solve(center: Point, radius: f32, focus: Option<FocalCircle>) -> Self {
        // Two different epsilons on purpose, following Impeller: case
        // SELECTION uses the looser `kEhCloseEnough` (conical_gradient_contents
        // .cc), because a separation just above the tight one produces a
        // near-singular 1/length map and fp32 noise with it. The tight
        // 1/4096 stays for the in-shader constants (gradient.glsl).
        const CASE_EPSILON: f32 = 1.0e-3;
        const NEARLY_ZERO: f32 = 1.0 / (1 << 12) as f32;
        let start = focus.unwrap_or(FocalCircle::point(center));
        let separation = (center.x - start.center.x).hypot(center.y - start.center.y);

        // Concentric circles need no focal machinery: t is just how far the
        // point sits between the two radii.
        if separation < CASE_EPSILON {
            if (radius - start.radius).abs() < CASE_EPSILON {
                return Self {
                    constants: [CONICAL_EMPTY, 0.0, 0.0, 0.0],
                    ..Self::UNUSED
                };
            }
            return Self {
                constants: [CONICAL_CONCENTRIC, start.radius, radius, 0.0],
                flags: [0.0; 4],
                focal_map: None,
            };
        }

        // Equal radii have no focal point at all — the circles sweep a strip
        // between their common tangents, and `focal` below would divide by
        // zero. Skia and Impeller both carve this out as its own case.
        if (radius - start.radius).abs() < CASE_EPSILON {
            let radius_in_unit_space = start.radius / separation;
            return Self {
                constants: [
                    CONICAL_STRIP,
                    radius_in_unit_space * radius_in_unit_space,
                    0.0,
                    0.0,
                ],
                flags: [0.0; 4],
                focal_map: Some(map_to_unit_x(start.center, center)),
            };
        }

        // Steps 1-2: the focal parameter, and the swap that keeps it finite
        // when the two radii are equal.
        let (mut first, mut second) = (start.center, center);
        let mut focal = start.radius / (start.radius - radius);
        let is_swapped = (focal - 1.0).abs() < NEARLY_ZERO;
        if is_swapped {
            std::mem::swap(&mut first, &mut second);
            focal = 0.0f32;
        }

        // Steps 3-4: map [focal centre, end centre] onto [(0,0), (1,0)], then
        // scale so the end circle becomes the unit circle.
        let focal_center = Point::new(
            first.x * (1.0 - focal) + second.x * focal,
            first.y * (1.0 - focal) + second.y * focal,
        );
        let radius_in_unit_space = (radius - start.radius).abs() / separation;
        let is_focal_on_circle = (radius_in_unit_space - 1.0).abs() < NEARLY_ZERO;
        let span = (1.0 - focal).abs();
        let (scale_x, scale_y) = if is_focal_on_circle {
            (span * 0.5, span * 0.5)
        } else {
            let squared = radius_in_unit_space * radius_in_unit_space;
            (
                span * radius_in_unit_space / (squared - 1.0),
                span / (squared - 1.0).abs().sqrt(),
            )
        };

        let is_well_behaved = !is_focal_on_circle && radius_in_unit_space > 1.0;
        Self {
            constants: [
                CONICAL_GENERAL,
                radius_in_unit_space,
                focal,
                (1.0 - focal).signum(),
            ],
            flags: [
                is_swapped as u32 as f32,
                is_focal_on_circle as u32 as f32,
                is_well_behaved as u32 as f32,
                0.0,
            ],
            focal_map: Some(scale_after(
                map_to_unit_x(focal_center, second),
                scale_x,
                scale_y,
            )),
        }
    }
}

/// `map_to_unit_x` maps `[from, to]` onto `[(0, 0), (1, 0)]`.
fn map_to_unit_x(from: Point, to: Point) -> Matrix {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = dx.hypot(dy);
    let (ux, uy) = (dx / length, dy / length);
    Matrix::from_affine(
        ux / length,
        -uy / length,
        uy / length,
        ux / length,
        -(ux * from.x + uy * from.y) / length,
        (uy * from.x - ux * from.y) / length,
    )
}

/// `scale_after` is `scale(x, y) ∘ matrix` for an affine 2D matrix.
fn scale_after(matrix: Matrix, x: f32, y: f32) -> Matrix {
    let [a, b, c, d, tx, ty] = matrix.to_affine();
    Matrix::from_affine(a * x, b * y, c * x, d * y, tx * x, ty * y)
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
