//! What a filter tree hands whoever draws it, and how it is drawn. The last
//! node need not make a texture: it hands on a [`FilterOutput`] that draws
//! itself into the destination, as Impeller's `RenderFilter` returns an
//! entity, so a colour filter, a styled blur or a drop shadow at the end of
//! a tree costs no pass of its own. An inner or outer mask blur clips its
//! blur to the draw's shape, or to outside it, where it is drawn, as
//! Impeller's `ApplyClippedBlurStyle` does; a glyph run, which has no
//! shape, merges its style in the composite's fragment. The composite of
//! a filtered layer, of a whole image's filters and of a backdrop's glass is
//! that draw. A texture is made of an output only when a next node or an
//! advanced blend has to read one ([`Planner::snapshot_output`]).

use valo_dl::{BlurStyle, ClipOp, ColorFilter};
use valo_geometry::{Matrix, Rect};

use crate::pipelines::{AdvancedBlend, Blend, PipelineBlend};

use super::draw_state::DrawState;
use super::drawing::Drawing;
use super::emit::{Cover, Entity, Role};
use super::shading::Shading;
use super::snapshot::Snapshot;
use super::source::Shape;
use super::Planner;

/// `FilterOutput` is what a filter tree hands whoever draws it: Impeller's
/// `RenderFilter` result.
pub(super) enum FilterOutput<'a> {
    /// A texture, drawn as it is over what it covers.
    Snapshot(Snapshot),
    /// A colour filter drawn straight into the destination: its input's
    /// texture through the filter's fragment (Impeller's
    /// `ColorFilterContents` drawing its input snapshot).
    Recolour {
        input: Snapshot,
        filter: ColorFilter,
    },
    /// Two textures merged in the destination's fragment.
    Merge {
        first: Snapshot,
        second: Snapshot,
        merge: Merge,
    },
    /// A blur drawn only inside or only outside the draw's shape: an
    /// inner or outer mask blur (Impeller's `ApplyClippedBlurStyle`).
    Clipped { blur: Snapshot, clip: StyleClip<'a> },
    /// The sharp draw with its blur drawn over it: a solid mask blur
    /// (Impeller's `ApplyBlurStyle`).
    Solid { sharp: Snapshot, blur: Snapshot },
}

/// `StyleClip` is what a [`FilterOutput::Clipped`] blur is clipped to: the
/// draw's shape, the side of it the blur keeps, and the transform from the
/// shape's local coordinates into the tree's space.
pub(super) struct StyleClip<'a> {
    pub shape: Shape<'a>,
    pub op: ClipOp,
    pub transform: Matrix,
}

/// `Merge` is how a [`FilterOutput::Merge`] combines its two textures.
#[derive(Clone, Copy)]
pub(super) enum Merge {
    /// Skia's drop shadow: the input (second) over its moved, blurred
    /// shadow (first).
    DropShadow,
    /// A glyph run's styled mask blur: the blur (first) against the sharp
    /// run (second).
    BlurStyle(BlurStyle),
}

impl FilterOutput<'_> {
    /// `coverage` is what the output covers, in the tree's space.
    pub fn coverage(&self) -> Rect {
        match self {
            FilterOutput::Snapshot(snapshot) => snapshot.placement.coverage(),
            FilterOutput::Recolour { input, .. } => input.placement.coverage(),
            FilterOutput::Merge { first, second, .. } => first
                .placement
                .coverage()
                .union(&second.placement.coverage()),
            FilterOutput::Clipped { blur, .. } => blur.placement.coverage(),
            FilterOutput::Solid { sharp, blur } => {
                sharp.placement.coverage().union(&blur.placement.coverage())
            }
        }
    }
}

impl Planner<'_> {
    /// `composite_output` draws a filter tree's `output` into the target
    /// with `blend` at `at`, whose transform places the output's own space.
    /// The last node draws itself where the blend unit can blend it; an
    /// advanced blend makes it one texture, copies the destination and
    /// blends in the fragment.
    pub(super) fn composite_output(
        &mut self,
        output: FilterOutput<'_>,
        blend: Blend,
        at: &DrawState,
    ) {
        match blend {
            Blend::Fixed(blend) => self.drawing().draw_filter_output(&output, blend, at),
            Blend::ReadsDestination(mode) => self.composite_blended(output, mode, at),
        }
    }

    /// `composite_blended` draws `output` blended by `mode` against a copy
    /// of the destination, over all the output covers: a blur's halo reaches
    /// past the layer's own texture.
    fn composite_blended(&mut self, output: FilterOutput<'_>, mode: AdvancedBlend, at: &DrawState) {
        let quad = output.coverage();
        let (source, alpha) = self.materialize_for_blend(output, at.alpha);
        let destination = self.split_for_copy(at.z);
        let composite = DrawState { alpha, ..*at };
        self.drawing()
            .draw_blended_snapshot(&source, &quad, &destination.view, mode, &composite);
    }

    /// `snapshot_output` makes `output` a texture: a final draw rendered
    /// into a pass or a target of its own, for a node or an advanced blend
    /// that reads it.
    pub(super) fn snapshot_output(&mut self, output: FilterOutput<'_>) -> Snapshot {
        match output {
            FilterOutput::Snapshot(snapshot) => snapshot,
            FilterOutput::Recolour { input, filter } => {
                self.filter_passes().push_recolour(&input, filter, 1.0)
            }
            FilterOutput::Merge {
                first,
                second,
                merge,
            } => self.filter_passes().push_merge(&first, &second, merge),
            output @ (FilterOutput::Clipped { .. } | FilterOutput::Solid { .. }) => self
                .render_to_snapshot(&output.coverage(), |drawing, depths| {
                    let at = DrawState::in_layer(Matrix::IDENTITY, depths.first);
                    drawing.draw_filter_output(&output, PipelineBlend::SrcOver, &at);
                }),
        }
    }

    /// `materialize_for_blend` makes `output` one texture for an advanced
    /// blend's composite, and returns the alpha the composite still owes:
    /// a colour filter takes the alpha in as it renders, anything else is
    /// drawn at it.
    fn materialize_for_blend(&mut self, output: FilterOutput<'_>, alpha: f32) -> (Snapshot, f32) {
        match output {
            FilterOutput::Recolour { input, filter } => (
                self.filter_passes().push_recolour(&input, filter, alpha),
                1.0,
            ),
            output => (self.snapshot_output(output), alpha),
        }
    }
}

impl Drawing<'_, '_> {
    /// `draw_filter_output` draws `output` into the context with `blend` at
    /// `at`, whose transform places the output's own space: the composite
    /// of a filtered layer, or a backdrop's glass. A colour filter takes the
    /// alpha in before it filters, as Impeller's colour filters absorb a
    /// layer's opacity.
    pub fn draw_filter_output(
        &mut self,
        output: &FilterOutput<'_>,
        blend: PipelineBlend,
        at: &DrawState,
    ) {
        match output {
            FilterOutput::Snapshot(snapshot) => self.draw_snapshot(snapshot, blend, at),
            FilterOutput::Recolour { input, filter } => {
                let shading = self.emit.recoloured_shading(input, *filter, at.alpha);
                self.draw_output_quad(&input.placement.coverage(), shading, blend, at);
            }
            FilterOutput::Merge {
                first,
                second,
                merge,
            } => {
                let quad = first
                    .placement
                    .coverage()
                    .union(&second.placement.coverage());
                let shading = self.emit.merged_shading(first, second, *merge, at.alpha);
                self.draw_output_quad(&quad, shading, blend, at);
            }
            FilterOutput::Clipped { blur, clip } => {
                let shape_at = at.with_transform(at.transform.then(&clip.transform));
                self.clip_to_shape(&clip.shape, clip.op, &shape_at);
                self.draw_snapshot(blur, blend, at);
            }
            FilterOutput::Solid { sharp, blur } => {
                self.draw_snapshot(sharp, blend, at);
                self.draw_snapshot(blur, blend, at);
            }
        }
    }

    /// `draw_snapshot` draws a texture as it is over what it covers.
    fn draw_snapshot(&mut self, snapshot: &Snapshot, blend: PipelineBlend, at: &DrawState) {
        let shading = self.emit.texture_shading(snapshot, at.alpha);
        self.draw_output_quad(&snapshot.placement.coverage(), shading, blend, at);
    }

    /// `draw_output_quad` draws one quad over `quad`, in the output's
    /// space, shaded by `shading`.
    fn draw_output_quad(
        &mut self,
        quad: &Rect,
        shading: Shading,
        blend: PipelineBlend,
        at: &DrawState,
    ) {
        let entity = Entity {
            stencil: None,
            cover: Cover::Quad {
                transform: at.transform,
                rect: *quad,
            },
            role: Role::Fill,
            shading,
            blend,
            z: at.z,
        };
        self.emit.push(self.context, entity);
    }

    /// `draw_blended_snapshot` draws `source` over `quad`, in its own space,
    /// blended by `mode` in the fragment against `destination`, a copy of
    /// what the context held beneath it: an advanced blend's composite. The
    /// pipeline blend is SrcOver — the result replaces what the copy
    /// captured.
    pub fn draw_blended_snapshot(
        &mut self,
        source: &Snapshot,
        quad: &Rect,
        destination: &wgpu::TextureView,
        mode: AdvancedBlend,
        at: &DrawState,
    ) {
        let size = self.context.area.size();
        let shading = self
            .emit
            .blended_texture_shading(source, at.alpha, mode, destination, size);
        self.draw_output_quad(quad, shading, PipelineBlend::SrcOver, at);
    }
}
