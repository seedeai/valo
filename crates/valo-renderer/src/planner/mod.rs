//! The planner: a `DisplayList` becomes a [`FramePlan`] the encoder replays
//! blindly. It is split along the seams every 2D engine shares:
//!
//! - [`replay`]: walking one list — the ONLY place that matches on `Op`.
//! - [`raster_embed`]: an embed the host hinted as cacheable, drawn from
//!   its cached raster.
//! - [`draw_state`]: what every draw is handed — transform, depth, group
//!   alpha.
//! - [`source`]: what a draw draws — its shape, or an image, the analytic
//!   blurred rrect, a glyph run — and the paints its parts take.
//! - [`route`]: the ONE per-draw decision (direct / effect layer / dst-read).
//! - [`drawing`]: the [`Drawing`](drawing::Drawing) view, which appends a
//!   draw's steps to the top context and can do nothing else; its geometry
//!   in [`primitives`], its text in [`text`].
//! - [`layers`]: the layer lifecycle — what becomes of a save layer
//!   ([`LayerPlan`](layers::LayerPlan)), then open, elide, skip, close,
//!   composite.
//! - [`layer_coverage`]: how big a save layer's texture is (Impeller's
//!   `ComputeSaveLayerCoverage`).
//! - [`backdrop`]: a layer that opens on the filtered scene beneath it.
//! - [`draw_context`]: the stack of draw contexts, one per texture being
//!   drawn, each with its pixel area and depth range.
//! - [`segments`]: when a context's pending draws become a pass, and the
//!   copies that split one.
//! - [`plan_writer`]: the plan's passes and stats — the only appender of
//!   passes.
//! - [`filter_tree`]: a paint's effects as Impeller's filter tree, in the
//!   order a draw or a save layer gives them.
//! - [`filters`]: renders a filter tree, node by node.
//! - [`filter_output`]: what a tree hands on, and how it draws itself.
//! - [`filter_passes`]: the [`FilterPasses`](filter_passes::FilterPasses)
//!   view, which appends independent one-quad passes and can touch no
//!   context; the Gaussian blur's passes in [`gaussian`].
//! - [`pattern_filter`]: a pattern's colour filter, baked into a cached
//!   copy of its image.
//! - [`snapshot`]: what a filter stage hands on — a texture and where its
//!   texels go.
//! - [`gaussian`]: Impeller's Gaussian blur — σ, kernel and passes — for
//!   every blur the recipes run.
//! - [`shading`]: what a draw's fragment reads — its fragment, record and
//!   bindings, made together by one encoder per fragment family.
//! - [`emit`]: an entity placed in a context as one draw — the only maker
//!   of draws, enforced by ownership: `StepEmitter` holds the GPU-facing
//!   services, and scene state reaches it only as arguments.

mod backdrop;
mod draw_context;
mod draw_state;
mod drawing;
mod emit;
mod filter_output;
mod filter_passes;
mod filter_tree;
mod filters;
pub(crate) mod gaussian;
mod layer_coverage;
mod layers;
mod pattern_filter;
mod plan_writer;
mod primitives;
mod raster_embed;
mod replay;
mod route;
mod segments;
mod shading;
mod snapshot;
mod source;
mod text;

use valo_dl::DisplayList;

use crate::contours::ContourCache;
use crate::frame::FramePlan;
use crate::glyphs::GlyphStore;
use crate::pool::TargetPool;
use crate::raster::ListRasterCache;
use crate::renderer::RenderTarget;

use draw_context::{DrawContext, DrawContexts};
use drawing::Drawing;
use emit::StepEmitter;
use filter_passes::FilterPasses;
use plan_writer::PlanWriter;
use replay::ReplayState;

pub(crate) use emit::{EmitterServices, LinearSamplers};

/// `Planner` is one frame's planning pass, run by [`Planner::plan`].
/// Planning is CPU work — it culls, assigns depth, opens layers, and emits
/// GPU passes, but never encodes or submits.
///
/// Its fields are grouped by lifetime: what it owns for the frame (the
/// contexts being drawn, the plan being written) and what it borrows from
/// the renderer (the tools that make steps and textures, the caches). Only
/// orchestration — the walk, routing, layers, the filter tree — sees all
/// four; a draw goes through [`Drawing`] and a filter pass through
/// [`FilterPasses`], whose fields say what each can touch.
pub(crate) struct Planner<'a> {
    contexts: DrawContexts,
    plan: PlanWriter,
    tools: Tools<'a>,
    caches: &'a mut Caches,
}

/// `Tools` are the GPU-facing services a frame's planning borrows to make
/// steps and textures.
struct Tools<'a> {
    /// The only maker of [`crate::frame::Draw`]s. It owns the GPU-facing
    /// services outright, so emission provably cannot touch the contexts or
    /// the plan — those arrive as arguments.
    emit: StepEmitter<'a>,
    pool: &'a mut TargetPool,
    /// What a cache texture is made on.
    device: &'a wgpu::Device,
    /// The format of every target and filter pass this frame.
    format: wgpu::TextureFormat,
}

/// `Caches` are the renderer's caches that planning reads and fills.
pub(crate) struct Caches {
    /// Atlas pages, outline paths, and the text-tier thresholds. Picking a
    /// run's tier is planning, so the store lives here; the one part of it
    /// that is emission — an atlas page's bind group — goes through
    /// [`StepEmitter::atlas_bind`].
    pub glyphs: GlyphStore,
    pub contours: ContourCache,
    /// Persistent textures for the embeds a host hinted as cacheable.
    pub rasters: ListRasterCache,
}

impl<'a> Planner<'a> {
    /// `plan` walks `list` into `target` and returns the frame's plan,
    /// borrowing the renderer's emitter `services`, target `pool` and
    /// `caches` for the walk.
    pub fn plan(
        device: &'a wgpu::Device,
        services: &'a mut EmitterServices,
        pool: &'a mut TargetPool,
        caches: &'a mut Caches,
        list: &DisplayList,
        target: &RenderTarget,
    ) -> FramePlan {
        let attachments = pool.take_attachments(target.size, target.format);
        let main = DrawContext::main(attachments, target, list);
        let mut planner = Self {
            contexts: DrawContexts::new(main),
            plan: PlanWriter::default(),
            tools: Tools {
                emit: StepEmitter::new(services, device, target.format),
                pool,
                device,
                format: target.format,
            },
            caches,
        };
        if target.clear.is_none() {
            planner.start_from_existing_pixels();
        }
        planner.replay_list(list, &mut ReplayState::root());
        planner.emit_segment();
        planner.plan.finish()
    }

    /// `drawing` is the view a draw goes through: the top context, the
    /// emitter, the caches and the stats, and nothing that opens a context
    /// or adds a pass.
    fn drawing(&mut self) -> Drawing<'_, 'a> {
        Drawing {
            context: self.contexts.top_mut(),
            emit: &mut self.tools.emit,
            caches: self.caches,
            stats: &mut self.plan.stats,
        }
    }

    /// `filter_passes` is the view a filter pass goes through: the pool, the
    /// emitter and the plan, and no context.
    fn filter_passes(&mut self) -> FilterPasses<'_, 'a> {
        FilterPasses {
            pool: self.tools.pool,
            emit: &mut self.tools.emit,
            plan: &mut self.plan,
            format: self.tools.format,
        }
    }
}
