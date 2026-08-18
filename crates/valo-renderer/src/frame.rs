//! The plan the encoder replays: an ordered sequence of render passes
//! (main-target segments, layer passes, filter passes) plus the texture
//! copies between them. The planner produces it; the encoder replays it
//! blindly.

use valo_geometry::Color;

use crate::host_buffer::{DrawSlot, VertexSlot};
use crate::pipelines::PipelineKey;
use crate::renderer::RenderStats;

pub(crate) struct FramePlan {
    pub passes: Vec<PlannedPass>,
    pub stats: RenderStats,
}

pub(crate) struct PlannedPass {
    pub color: PassColor,
    /// `None` for filter passes — bare color work, no clips, no StC.
    pub depth: Option<wgpu::TextureView>,
    /// `Some` clears; `None` loads (a resumed segment, or the caller asked
    /// to draw over existing content).
    pub clear: Option<Color>,
    /// First segment of a target clears depth/stencil; resumed ones load
    /// (clip ceilings must survive pass breaks).
    pub clear_depth: bool,
    /// Keep msaa + depth contents at pass end. True only when a later
    /// segment resumes this target (Impeller's
    /// kStoreAndMultisampleResolve vs kMultisampleResolve) — the final
    /// segment discards, which on tiled GPUs skips the 4× tile flush.
    pub store: bool,
    /// Snapshot copies that must complete before this pass runs.
    pub pre_copies: Vec<TextureCopy>,
    pub steps: Vec<Step>,
}

pub(crate) fn replace_msaa(color: &mut PassColor, msaa: &wgpu::TextureView) {
    match color {
        PassColor::Main { msaa: attachment }
        | PassColor::Layer {
            msaa: attachment, ..
        } => *attachment = msaa.clone(),
        PassColor::Filter { .. } => unreachable!("filter passes never open a frame"),
    }
}

pub(crate) enum PassColor {
    /// MSAA scratch resolving into the caller's view.
    Main { msaa: wgpu::TextureView },
    /// A pooled layer: both attachments ours.
    Layer {
        msaa: wgpu::TextureView,
        resolve: wgpu::TextureView,
    },
    /// A gaussian filter pass: 1-sample pooled target, drawn then sampled.
    Filter { view: wgpu::TextureView },
}

/// Region copy at the SAME coordinates in src and dst: the snapshot is
/// target-sized, so `dst_sample`'s uv math needs no remapping — only the
/// pixels the dst-reading draw can actually sample get copied.
pub(crate) struct TextureCopy {
    pub src: wgpu::Texture,
    pub dst: wgpu::Texture,
    pub origin: [u32; 2],
    pub size: [u32; 2],
}

/// One encoded drawing step: a pipeline, one uniform record, an optional
/// group-1 bind group (image / snapshot / blend), and either the built-in
/// unit quad or a transient mesh.
pub(crate) struct Step {
    pub key: PipelineKey,
    pub uniforms: DrawSlot,
    pub texture: Option<wgpu::BindGroup>,
    pub mesh: Option<(VertexSlot, u32)>,
    /// The draw's z — the reorder pass sorts hoisted opaque units by it.
    pub sort_z: f32,
}
