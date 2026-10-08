//! The encoder: a [`FramePlan`] replayed into one command encoder, blindly —
//! every decision is the planner's. Each pass runs its copies, begins its
//! render pass with the load and store ops its kind gives its attachments,
//! and draws its draws, a draw's stencil right before it.
//!
//! The encoder draws with the pipelines [`PipelineCache::compile`] compiled
//! for the plan: a render pass borrows them for as long as it records.
//!
//! [`PipelineCache::compile`]: crate::PipelineCache::compile

use crate::frame::{
    Bindings, ContextAttachments, Draw, FramePlan, Geometry, Mesh, PassTarget, TextureCopy,
};
use crate::gpu_timer::GpuTimer;
use crate::host_buffer::{HostBuffer, UniformSlot};
use crate::pipelines::{CompiledPipelines, PipelineKey};

/// `Encoder` records a frame's passes, over the services they read.
pub(crate) struct Encoder<'r> {
    pub host: &'r HostBuffer,
    pub pipelines: &'r CompiledPipelines<'r>,
}

/// `EncodeStats` counts what the encoder recorded.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct EncodeStats {
    pub draw_calls: u32,
    pub pipeline_switches: u32,
}

impl Encoder<'_> {
    /// `encode` records every pass of `plan` into `encoder`, the frame's
    /// first and last passes carrying `timer`'s timestamps.
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        plan: &FramePlan,
        timer: &GpuTimer,
    ) -> EncodeStats {
        let mut stats = EncodeStats::default();
        for (index, pass) in plan.passes.iter().enumerate() {
            encode_copies(encoder, &pass.pre_copies);
            let timing = timer.pass_writes(index, plan.passes.len());
            let mut recorder = DrawRecorder {
                pass: begin_render_pass(encoder, &pass.target, timing),
                host: self.host,
                pipelines: self.pipelines,
                bound: None,
                stats: &mut stats,
            };
            for draw in pass.target.draws() {
                recorder.draw(draw);
            }
        }
        stats
    }
}

/// `DrawRecorder` records one pass's draws, switching pipelines only where
/// the key changes.
struct DrawRecorder<'e, 'r> {
    pass: wgpu::RenderPass<'e>,
    host: &'r HostBuffer,
    pipelines: &'r CompiledPipelines<'r>,
    bound: Option<PipelineKey>,
    stats: &'r mut EncodeStats,
}

impl DrawRecorder<'_, '_> {
    /// `draw` records one draw: its stencil, then the draw itself.
    fn draw(&mut self, draw: &Draw) {
        if let Some(stencil) = &draw.stencil {
            self.use_pipeline(stencil.key);
            self.bind_uniforms(stencil.uniforms);
            self.draw_mesh(stencil.mesh);
        }
        self.use_pipeline(draw.key);
        self.bind_uniforms(draw.uniforms);
        self.bind(&draw.bindings);
        match draw.geometry {
            Geometry::Quad => {
                self.pass.draw(0..6, 0..1);
                self.stats.draw_calls += 1;
            }
            Geometry::Mesh(mesh) => self.draw_mesh(mesh),
        }
    }

    fn use_pipeline(&mut self, key: PipelineKey) {
        if self.bound != Some(key) {
            self.pass.set_pipeline(self.pipelines.get(&key));
            self.bound = Some(key);
            self.stats.pipeline_switches += 1;
        }
    }

    fn bind_uniforms(&mut self, uniforms: UniformSlot) {
        self.pass
            .set_bind_group(0, self.host.bind_group(uniforms.block), &[uniforms.offset]);
    }

    /// `bind` sets what the fragment reads besides its record.
    fn bind(&mut self, bindings: &Bindings) {
        match bindings {
            Bindings::Plain => {}
            Bindings::Textured(texture) => self.pass.set_bind_group(1, texture, &[]),
            Bindings::Blurred { texture, kernel } => {
                self.pass.set_bind_group(1, texture, &[]);
                self.pass.set_bind_group(
                    2,
                    self.host.kernel_bind_group(kernel.block),
                    &[kernel.offset],
                );
            }
        }
    }

    fn draw_mesh(&mut self, mesh: Mesh) {
        let slot = mesh.slot;
        let buffer = self.host.vertex_buffer(slot.block);
        self.pass
            .set_vertex_buffer(0, buffer.slice(slot.offset..slot.offset + slot.bytes));
        self.pass.draw(0..mesh.vertices, 0..1);
        self.stats.draw_calls += 1;
    }
}

/// `begin_render_pass` begins a pass on its target: a context's pass
/// renders ×4, clears colour, depth and stencil and keeps only what
/// resolves; a filter pass renders one sample straight into its view and
/// keeps it.
fn begin_render_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    target: &PassTarget,
    timing: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'e> {
    let (color, depth) = match target {
        PassTarget::Context {
            attachments, clear, ..
        } => context_attachments(attachments, *clear),
        PassTarget::Filter { view, .. } => (filter_attachment(view), None),
    };
    let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("valo.pass"),
        color_attachments: &[Some(color)],
        depth_stencil_attachment: depth,
        timestamp_writes: timing,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    render_pass.set_stencil_reference(0);
    render_pass
}

/// `encode_copies` lands a pass's copies BEFORE the pass that samples them.
fn encode_copies(encoder: &mut wgpu::CommandEncoder, copies: &[TextureCopy]) {
    for copy in copies {
        encoder.copy_texture_to_texture(
            copy.src.as_image_copy(),
            copy.dst.as_image_copy(),
            wgpu::Extent3d {
                width: copy.size[0],
                height: copy.size[1],
                depth_or_array_layers: 1,
            },
        );
    }
}

/// `context_attachments` are a context pass's: ×4 colour cleared to
/// `clear` and resolved, depth and stencil cleared; none kept, the
/// multisample attachments being tile-only scratch no later pass loads.
fn context_attachments(
    attachments: &ContextAttachments,
    clear: valo_geometry::Color,
) -> (
    wgpu::RenderPassColorAttachment<'_>,
    Option<wgpu::RenderPassDepthStencilAttachment<'_>>,
) {
    let color = wgpu::RenderPassColorAttachment {
        view: &attachments.msaa,
        depth_slice: None,
        resolve_target: Some(&attachments.resolve),
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(premultiplied(clear)),
            store: wgpu::StoreOp::Discard,
        },
    };
    let depth = wgpu::RenderPassDepthStencilAttachment {
        view: &attachments.depth,
        depth_ops: Some(wgpu::Operations {
            load: wgpu::LoadOp::Clear(0.0),
            store: wgpu::StoreOp::Discard,
        }),
        stencil_ops: Some(wgpu::Operations {
            load: wgpu::LoadOp::Clear(0),
            store: wgpu::StoreOp::Discard,
        }),
    };
    (color, Some(depth))
}

/// `filter_attachment` is a filter pass's one sample, cleared to
/// transparent and kept for the next pass to sample.
fn filter_attachment(view: &wgpu::TextureView) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
        },
    }
}

fn premultiplied(color: valo_geometry::Color) -> wgpu::Color {
    let [r, g, b, a] = color.premultiplied();
    wgpu::Color {
        r: r as f64,
        g: g as f64,
        b: b as f64,
        a: a as f64,
    }
}
