//! The view every filter pass goes through. [`FilterPasses`] borrows the
//! target pool, the step emitter and the plan — and no draw context, so a
//! filter pass provably cannot draw into a texture being drawn. A pass is
//! one quad into a fresh, exactly-sized 1-sample texture, appended to the
//! plan as an independent pass at the current position, and hands on a
//! [`Snapshot`] of what it wrote. The Gaussian blur's passes are in
//! `gaussian`, the filter nodes made of passes in `filters`, a pattern's
//! baked colour filter in `pattern_filter`.

use valo_dl::ColorFilter;
use valo_geometry::{Matrix, Rect};

use crate::pool::TargetPool;

use super::draw_context::PixelArea;
use super::emit::StepEmitter;
use super::filter_output::{Merge, RuntimeOutput};
use super::plan_writer::PlanWriter;
use super::shading::Shading;
use super::snapshot::{Placement, Snapshot};

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
        let into = PixelArea::over(*region).placement();
        let shading = self.emit.resample_shading(source, &into);
        self.push_region_pass(region, shading)
    }

    /// `push_scaled_resample` copies what `source` holds of `region` at
    /// `scale` of the tree's resolution, one bilinear tap per pixel: at one
    /// half, each tap lies between four texels and averages them.
    pub fn push_scaled_resample(
        &mut self,
        source: &Snapshot,
        region: &Rect,
        scale: f32,
    ) -> Snapshot {
        let scaled = Scaled::of(region, scale);
        let target = self.pool.take_filter(scaled.placement.size, self.format);
        self.push_scaled_copy(source, &scaled, target.view.clone());
        Snapshot {
            view: target.view,
            placement: scaled.placement,
        }
    }

    /// `push_pyramid` is [`FilterPasses::push_scaled_resample`] into level 0
    /// of a blur pyramid, and each level after it Core Animation's
    /// downsample of the one before, down to a texel: a runtime filter's
    /// source when its effect reads levels.
    pub fn push_pyramid(&mut self, source: &Snapshot, region: &Rect, scale: f32) -> Snapshot {
        let scaled = Scaled::of(region, scale);
        let size = scaled.placement.size;
        let pyramid = self.pool.take_pyramid(size, self.format);
        self.push_scaled_copy(source, &scaled, pyramid.levels[0].clone());
        for (level, view) in pyramid.levels.iter().enumerate().skip(1) {
            let above = level_size(size, level - 1);
            let here = level_size(size, level);
            let shading =
                self.emit
                    .pyramid_downsample_shading(&pyramid.levels[level - 1], above, here);
            let quad = Rect::new(0.0, 0.0, here[0] as f32, here[1] as f32);
            self.push_pass(view.clone(), self.format, &quad, here, shading);
        }
        Snapshot {
            view: pyramid.view,
            placement: scaled.placement,
        }
    }

    /// `push_scaled_copy` resamples `source` into `target`, the texels
    /// `scaled` places.
    fn push_scaled_copy(&mut self, source: &Snapshot, scaled: &Scaled, target: wgpu::TextureView) {
        let shading = self.emit.resample_shading(source, &scaled.placement);
        let size = scaled.placement.size;
        self.push_pass(target, self.format, &scaled.quad, size, shading);
    }

    /// `push_runtime` renders a runtime filter's `output` into a texture of
    /// its region.
    pub fn push_runtime(&mut self, output: &RuntimeOutput) -> Snapshot {
        let region = output.region;
        let varying_to_local = output
            .tree_to_local()
            .then(&Matrix::translation(region.x, region.y));
        let shading = self.emit.runtime_filter_shading(
            &output.shader,
            &output.source,
            &output.local_bounds(),
            &varying_to_local,
            &output.effect_transform,
            1.0,
        );
        self.push_region_pass(&region, shading)
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

/// `Scaled` is a region of the tree's space at a scale of its resolution:
/// the texels that hold it, and the quad of them it covers.
struct Scaled {
    placement: Placement,
    quad: Rect,
}

impl Scaled {
    fn of(region: &Rect, scale: f32) -> Self {
        let (width, height) = (region.width * scale, region.height * scale);
        Self {
            placement: Placement {
                size: [width.ceil().max(1.0) as u32, height.ceil().max(1.0) as u32],
                transform: Matrix::translation(region.x, region.y)
                    .then(&Matrix::scale(1.0 / scale, 1.0 / scale)),
                placed: false,
            },
            quad: Rect::new(0.0, 0.0, width, height),
        }
    }
}

/// `level_size` is the size of mip `level` of a texture `size` texels big.
fn level_size(size: [u32; 2], level: usize) -> [u32; 2] {
    size.map(|size| (size >> level).max(1))
}
