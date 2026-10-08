//! What the GPU has been told: the plan's passes, in the order the encoder
//! runs them, the copies queued between them, and the frame's stats.
//! [`PlanWriter`] is the only appender of passes — a context's or an
//! independent filter pass — so the order of the plan is the order of these
//! calls.

use valo_geometry::Color;

use crate::frame::{ContextAttachments, Draw, FramePlan, PassTarget, PlannedPass, TextureCopy};
use crate::renderer::RenderStats;

use super::segments::reorder_draws;

/// `PlanWriter` holds the plan under construction and the frame's stats.
#[derive(Default)]
pub(super) struct PlanWriter {
    passes: Vec<PlannedPass>,
    /// Copies of what the passes so far wrote, run before the next pass.
    queued_copies: Vec<TextureCopy>,
    pub stats: RenderStats,
}

impl PlanWriter {
    /// `push_context_pass` appends one pass of a draw context: `draws`, the
    /// opaque ones reordered, into `attachments` cleared to `clear`.
    pub fn push_context_pass(
        &mut self,
        attachments: ContextAttachments,
        clear: Color,
        draws: Vec<Draw>,
    ) {
        let mut hoisted = 0;
        let draws = reorder_draws(draws, &mut hoisted);
        self.stats.opaque_reordered += hoisted;
        self.push(PassTarget::Context {
            attachments,
            clear,
            draws,
        });
    }

    /// `push_filter_pass` appends an independent pass that draws `draw`, a
    /// filter draw, into `view`.
    pub fn push_filter_pass(&mut self, view: wgpu::TextureView, draw: Draw) {
        self.push(PassTarget::Filter { view, draw });
        self.stats.filter_passes += 1;
    }

    /// `queue_copy` runs `copy` after the passes so far, before the next.
    pub fn queue_copy(&mut self, copy: TextureCopy) {
        self.queued_copies.push(copy);
    }

    fn push(&mut self, target: PassTarget) {
        self.passes.push(PlannedPass {
            pre_copies: std::mem::take(&mut self.queued_copies),
            target,
        });
    }

    /// `finish` is the plan the encoder replays.
    pub fn finish(self) -> FramePlan {
        debug_assert!(
            self.queued_copies.is_empty(),
            "a copy is queued only for a pass that follows it"
        );
        FramePlan {
            passes: self.passes,
            stats: self.stats,
        }
    }
}
