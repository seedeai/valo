//! The plan the encoder replays: an ordered sequence of render passes — a
//! draw context's passes and filter passes — and the texture copies between
//! them. The planner produces it; the encoder replays it blindly.
//!
//! Every pass starts fresh, as in Skia Graphite and Impeller: a context's
//! pass clears its multisample colour and its depth and stencil and
//! discards them at its end, keeping only what resolves. A context that is
//! split for a copy of what it holds draws that copy back as the first draw
//! of its next pass and replays its active clips after it, so nothing has
//! to survive a pass in the multisample attachments.

use valo_geometry::Color;

use crate::host_buffer::{UniformSlot, VertexSlot};
use crate::pipelines::PipelineKey;
use crate::renderer::RenderStats;

pub(crate) struct FramePlan {
    pub passes: Vec<PlannedPass>,
    pub stats: RenderStats,
}

impl FramePlan {
    /// `pipeline_keys` are the pipelines the plan's draws and their stencils
    /// run.
    pub fn pipeline_keys(&self) -> impl Iterator<Item = PipelineKey> + '_ {
        self.passes
            .iter()
            .flat_map(|pass| pass.target.draws())
            .flat_map(|draw| {
                draw.stencil
                    .as_ref()
                    .map(|stencil| stencil.key)
                    .into_iter()
                    .chain([draw.key])
            })
    }
}

impl PassTarget {
    /// `draws` are the draws the pass draws, in order.
    pub fn draws(&self) -> &[Draw] {
        match self {
            PassTarget::Context { draws, .. } => draws,
            PassTarget::Filter { draw, .. } => std::slice::from_ref(draw),
        }
    }
}

/// `PlannedPass` is one render pass and the copies that run before it.
pub(crate) struct PlannedPass {
    /// Copies that must complete before this pass runs: what a context held
    /// when it was split, for its draw-back and for whatever reads it.
    pub pre_copies: Vec<TextureCopy>,
    pub target: PassTarget,
}

/// `PassTarget` is what a pass draws into, and what it draws.
pub(crate) enum PassTarget {
    /// A pass of a draw context: its 4-sample colour cleared to `clear`, its
    /// depth and stencil cleared, both discarded at the pass's end once the
    /// colour resolves.
    Context {
        attachments: ContextAttachments,
        clear: Color,
        draws: Vec<Draw>,
    },
    /// A filter pass: one sample, cleared to transparent, drawn into `view`
    /// and kept for the next pass to sample; one draw, no depth.
    Filter { view: wgpu::TextureView, draw: Draw },
}

/// `ContextAttachments` are a context's 4-sample colour and depth/stencil,
/// tile-only, and the single-sample texture its colour resolves into: a
/// layer's own, a cache texture, or the caller's.
#[derive(Clone)]
pub(crate) struct ContextAttachments {
    pub msaa: wgpu::TextureView,
    pub depth: wgpu::TextureView,
    pub resolve: wgpu::TextureView,
}

/// `TextureCopy` is a whole texture copied into another of its size.
pub(crate) struct TextureCopy {
    pub src: wgpu::Texture,
    pub dst: wgpu::Texture,
    pub size: [u32; 2],
}

/// `Draw` is one draw as the encoder runs it, built whole and moved whole,
/// Impeller's `Entity`: the stencil its cover tests, if it has one, drawn
/// right before it, then the draw itself.
#[derive(Clone)]
pub(crate) struct Draw {
    /// The stencil half of stencil-then-cover: a path's fan wound by its
    /// rule, or a stroke's strip counted.
    pub stencil: Option<Stencil>,
    /// The pipeline the draw runs.
    pub key: PipelineKey,
    /// Its uniform record.
    pub uniforms: UniformSlot,
    /// What its fragment reads besides the record.
    pub bindings: Bindings,
    /// What it covers.
    pub geometry: Geometry,
    /// The draw's z: the reorder sorts the opaque draws it moves ahead by
    /// it, and a clip's is the depth its ceiling holds until.
    pub z: f32,
}

/// `Stencil` is the stencil write a draw's cover tests: its pipeline, its
/// record and its mesh.
#[derive(Clone)]
pub(crate) struct Stencil {
    pub key: PipelineKey,
    pub uniforms: UniformSlot,
    pub mesh: Mesh,
}

/// `Geometry` is what a draw covers: the unit quad its record's transform
/// places, or a mesh.
#[derive(Clone, Copy)]
pub(crate) enum Geometry {
    Quad,
    Mesh(Mesh),
}

/// `Mesh` is a transient vertex range and how many vertices it holds.
#[derive(Clone, Copy)]
pub(crate) struct Mesh {
    pub slot: VertexSlot,
    pub vertices: u32,
}

/// `Bindings` is what a draw's fragment reads besides its uniform record,
/// by the fragment's bind layout.
#[derive(Clone)]
pub(crate) enum Bindings {
    /// The record alone.
    Plain,
    /// One bind group at group 1: a texture and its sampler, or a blend's
    /// destination copy, sampler and source.
    Textured(wgpu::BindGroup),
    /// A texture at group 1 and a blur pass's kernel at group 2.
    Blurred {
        texture: wgpu::BindGroup,
        kernel: UniformSlot,
    },
}
