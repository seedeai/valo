//! The planner: a `DisplayList` becomes a [`FramePlan`] the encoder replays
//! blindly. It is split along the seams every 2D engine shares:
//!
//! - [`replay`]: walking one list — the ONLY place that matches on `Op`.
//! - [`route`]: the ONE per-draw decision (direct / effect layer / dst-read).
//! - [`primitives`]: geometry emitters and clips; no routing decisions.
//! - [`text`]: which shape a glyph run takes — atlas quads or real outlines.
//! - [`layers`]: the open-target stack; group-alpha bookkeeping.
//! - [`segments`]: frame-segment emission and dst-read breaks.
//! - [`filters`]: multi-pass filter recipes (blur chains, colour filters,
//!   drop shadows, mask combines); appends its own independent passes.
//! - [`emit`]: uniforms, payloads, pipeline keys — the only maker of
//!   `Step`s, enforced by ownership: `StepEmitter` holds the GPU-facing
//!   services, and scene state reaches it only as arguments.
//!
//! This is the only planner; the module it replaced is gone.

mod emit;
mod filters;
mod layers;
mod primitives;
mod replay;
mod route;
mod segments;
mod text;

use valo_dl::DisplayList;

use crate::frame::{FramePlan, PassColor, PlannedPass};
use crate::host_buffer::HostBuffer;
use crate::pool::TargetPool;
use crate::renderer::{RenderStats, RenderTarget};

use emit::StepEmitter;
use layers::PassFrame;
use replay::ReplayState;

/// `Planner` is one frame's planning pass: `new` borrows the renderer's
/// caches and opens the main-target frame, `run` walks the list and
/// consumes the planner. Planning is CPU work — it culls, assigns depth,
/// opens layers, and emits GPU passes, but never encodes or submits.
pub(crate) struct Planner<'a> {
    // Fields are module-private: the planner's child modules see them, the
    // rest of the crate goes through `new`/`run`.
    /// The only maker of [`crate::frame::Step`]s. It owns the GPU-facing
    /// services outright, so emission provably cannot touch the frames,
    /// the pass list, or the alpha stack — those arrive as arguments.
    emit: StepEmitter<'a>,
    pool: &'a mut TargetPool,
    contours: &'a mut crate::contours::ContourCache,
    /// Atlas pages, outline paths, and the text-tier thresholds. Picking a
    /// run's tier is planning, so the store lives here; the one part of it
    /// that is emission — an atlas page's bind group — goes through
    /// [`StepEmitter::atlas_bind`].
    glyphs: &'a mut crate::glyphs::GlyphStore,
    /// Persistent textures for the embeds a host hinted as cacheable.
    rasters: &'a mut crate::raster::ListRasterCache,
    format: wgpu::TextureFormat,
    /// Open render targets: main at 0, innermost layer last.
    frames: Vec<PassFrame>,
    /// Stacked group alphas of elided opacity layers (multiplied, innermost
    /// last). Pushed by layer elision; read by every tint. A materialized
    /// layer absorbs the pending value into its composite paint and starts
    /// its children on an empty stack (the outer stack rides the scope
    /// entry in `replay`).
    elisions: Vec<f32>,
    /// The plan under construction. Two writers, on purpose:
    /// `segments` appends frame segments, `filters` appends its own
    /// independent single-quad passes.
    passes: Vec<PlannedPass>,
    /// Set while replaying INTO a raster-cache texture. A cacheable embed
    /// met during a fill replays inline, so one fill never schedules
    /// another and the cached pixels stay a plain rendering of the list.
    filling_raster: bool,
    stats: RenderStats,
}

impl<'a> Planner<'a> {
    #[expect(
        clippy::too_many_arguments,
        reason = "one-shot wiring of the renderer's caches"
    )]
    pub fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        host: &'a mut HostBuffer,
        images: &'a mut crate::images::ImageStore,
        pool: &'a mut TargetPool,
        pipelines: &'a crate::pipelines::PipelineCache,
        glyphs: &'a mut crate::glyphs::GlyphStore,
        contours: &'a mut crate::contours::ContourCache,
        ramps: &'a mut crate::ramps::RampCache,
        rasters: &'a mut crate::raster::ListRasterCache,
        sampler: &wgpu::Sampler,
        target: &RenderTarget,
        dl: &DisplayList,
    ) -> Self {
        // A load-existing target (clear: None) must keep its msaa contents
        // across FRAMES — only cleared targets can go tile-only.
        let transient = target.clear.is_some();
        let scratch = pool.main_scratch(target.size, target.format, transient);
        let main = PassFrame::main(
            PassColor::Main { msaa: scratch.msaa },
            scratch.depth,
            target,
            dl,
            transient,
        );
        Self {
            emit: StepEmitter::new(
                host,
                device,
                queue,
                pipelines,
                images,
                ramps,
                sampler.clone(),
                target.format,
            ),
            pool,
            contours,
            glyphs,
            rasters,
            format: target.format,
            frames: vec![main],
            elisions: Vec::new(),
            passes: Vec::new(),
            filling_raster: false,
            stats: RenderStats::default(),
        }
    }

    /// `run` walks `dl` and returns the finished plan. Consumes the planner.
    pub fn run(mut self, dl: &DisplayList) -> FramePlan {
        let mut state = ReplayState::root();
        self.replay_list(dl, &mut state);
        self.emit_segment();
        FramePlan {
            passes: self.passes,
            stats: self.stats,
        }
    }
}
