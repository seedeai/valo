//! The view every filter pass goes through. [`FilterPasses`] borrows the
//! target pool, the step emitter and the plan — and no draw context, so a
//! filter pass provably cannot draw into a texture being drawn. A pass is
//! one quad into a fresh, exactly-sized 1-sample texture, appended to the
//! plan as an independent pass at the current position, and hands on a
//! [`Snapshot`] of what it wrote. The Gaussian blur's passes are in
//! `gaussian`, the filter nodes made of passes in `filters`, a pattern's
//! baked colour filter in `pattern_filter`.

use valo_dl::ColorFilter;
use valo_geometry::Rect;

use crate::pool::TargetPool;

use super::draw_context::PixelArea;
use super::emit::StepEmitter;
use super::filter_output::Merge;
use super::plan_writer::PlanWriter;
use super::shading::Shading;
use super::snapshot::Snapshot;

/// `FilterPasses` appends independent filter passes to the plan.
pub(super) struct FilterPasses<'p, 'a> {
    pub pool: &'p mut TargetPool,
    pub emit: &'p mut StepEmitter<'a>,
    pub plan: &'p mut PlanWriter,
    /// The format of the textures the passes write.
    pub format: wgpu::TextureFormat,
}

impl FilterPasses<'_, '_> {
    /// `push_resample` copies what `source` holds of `region` into an
    /// exactly-sized texture, one bilinear tap per pixel.
    pub fn push_resample(&mut self, source: &Snapshot, region: &Rect) -> Snapshot {
        let shading = self.emit.resample_shading(source, region);
        self.push_region_pass(region, shading)
    }

    /// `push_recolour` recolours `source` into a texture that covers what
    /// it covers, at replay resolution: Impeller's colour filter drawn into a
    /// snapshot of its coverage. `input_alpha` is taken in before the filter.
    pub fn push_recolour(
        &mut self,
        source: &Snapshot,
        filter: ColorFilter,
        input_alpha: f32,
    ) -> Snapshot {
        let region = source.placement.coverage();
        let shading = self
            .emit
            .recolour_pass_shading(source, &region, filter, input_alpha);
        self.push_region_pass(&region, shading)
    }

    /// `push_merge` is a [`Merge`] rendered into a texture: one pass over
    /// the union of what `first` and `second` cover, sampling each through
    /// its own transform.
    pub fn push_merge(&mut self, first: &Snapshot, second: &Snapshot, merge: Merge) -> Snapshot {
        let region = first
            .placement
            .coverage()
            .union(&second.placement.coverage());
        let shading = self.emit.merge_pass_shading(first, second, &region, merge);
        self.push_region_pass(&region, shading)
    }

    /// `push_region_pass` is one pass over `region` into a fresh texture of
    /// exactly its pixels, which it returns as the snapshot of `region`.
    fn push_region_pass(&mut self, region: &Rect, shading: Shading) -> Snapshot {
        let area = PixelArea::over(*region);
        let target = self.pool.take_filter(area.size(), self.format);
        let quad = Rect::new(0.0, 0.0, region.width, region.height);
        self.push_pass(
            target.view.clone(),
            self.format,
            &quad,
            area.size(),
            shading,
        );
        Snapshot {
            view: target.view,
            placement: area.placement(),
        }
    }

    /// `push_pass` draws `quad`, in the pixels of `target`, a texture of
    /// `format` and `extent` texels, shaded by `shading`, as a pass of its
    /// own.
    pub fn push_pass(
        &mut self,
        target: wgpu::TextureView,
        format: wgpu::TextureFormat,
        quad: &Rect,
        extent: [u32; 2],
        shading: Shading,
    ) {
        let draw = self.emit.filter_draw(format, quad, extent, shading);
        self.plan.push_filter_pass(target, draw);
    }
}
