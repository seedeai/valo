//! What a filter tree hands whoever draws it, and how it is drawn. The last
//! node need not make a texture: it hands on a [`FilterOutput`] that draws
//! itself into the destination, as Impeller's `RenderFilter` returns an
//! entity, so a colour filter, a styled blur, a drop shadow or a matrix at
//! the end of a tree costs no pass of its own. An inner or outer mask blur
//! clips its blur to the draw's shape, or to outside it, where it is drawn,
//! as Impeller's `ApplyClippedBlurStyle` does; a glyph run, which has no
//! shape, merges its style in the composite's fragment. The composite of
//! a filtered layer, of a whole image's filters and of a backdrop's glass is
//! that draw. A texture is made of an output only when a next node or an
//! advanced blend has to read one ([`Planner::snapshot_output`]).

use valo_dl::{BlurStyle, ClipOp, ColorFilter, Filter, RuntimeShader, Sampling};
use valo_geometry::{Matrix, MatrixKind, Rect};

use crate::pipelines::{AdvancedBlend, Blend, PipelineBlend};

use super::draw_state::DrawState;
use super::drawing::Drawing;
use super::emit::{Cover, Entity, Role};
use super::shader_filter::ShaderFilterOutput;
use super::shading::Shading;
use super::snapshot::{Placement, Snapshot};
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
    /// A runtime filter drawn straight into the destination.
    Runtime(RuntimeOutput),
    /// A paint shader's effect drawn as a filter, by Flutter's convention,
    /// straight into the destination.
    ShaderFilter(ShaderFilterOutput),
    /// A matrix filter's input moved, drawn straight into the destination.
    Moved(MovedOutput),
}

/// `MovedOutput` is what a matrix filter draws: Impeller's
/// `MatrixFilterContents` hands its input's snapshot on with the matrix in
/// its transform and its own sampler, as an entity drawn as it is. `input`
/// goes where `transform`, in the tree's space, moves it, read as
/// `sampling` filters it; `limit` is the region the output has to cover,
/// when the tree knows one.
pub(super) struct MovedOutput {
    pub input: Snapshot,
    pub transform: Matrix,
    pub sampling: Sampling,
    pub limit: Option<Rect>,
}

impl MovedOutput {
    /// `texel_transform` maps the input's texels to where they are drawn in
    /// the tree's space.
    pub fn texel_transform(&self) -> Matrix {
        self.transform.then(&self.input.placement.transform)
    }

    /// `texel_quad` is the input's texels, all of them.
    pub fn texel_quad(&self) -> Rect {
        let [width, height] = self.input.placement.size.map(|side| side as f32);
        Rect::new(0.0, 0.0, width, height)
    }

    /// `coverage` is what the moved input covers in the tree's space, cut
    /// to the limit when there is one.
    pub fn coverage(&self) -> Rect {
        let moved = self.texel_transform().map_rect(&self.texel_quad());
        self.limit
            .and_then(|limit| limit.intersect(&moved))
            .unwrap_or(moved)
    }

    /// `as_snapshot` is the moved input as a snapshot that needs no pass of
    /// its own, when one can say it: moved by a positive scale and a
    /// translation only, as every snapshot's placement is, and read
    /// linearly, as every reader reads a snapshot.
    pub fn as_snapshot(&self) -> Option<Snapshot> {
        let transform = self.texel_transform();
        let placeable =
            transform.kind() == MatrixKind::AxisAligned && self.sampling.filter == Filter::Linear;
        placeable.then(|| Snapshot {
            view: self.input.view.clone(),
            placement: Placement {
                size: self.input.placement.size,
                transform,
                placed: false,
            },
        })
    }
}

/// `RuntimeOutput` is what a runtime filter draws: its effect's `shade`
/// over `region` of the tree's space, reading `source`, `effect_transform`
/// taking the effect's local space into the tree's.
pub(super) struct RuntimeOutput {
    pub shader: RuntimeShader,
    pub source: Snapshot,
    pub region: Rect,
    pub effect_transform: Matrix,
}

impl RuntimeOutput {
    /// `tree_to_local` maps the tree's space into the effect's local space;
    /// a degenerate effect transform maps everything to its origin.
    pub fn tree_to_local(&self) -> Matrix {
        self.effect_transform
            .invert()
            .unwrap_or(Matrix::from_affine(0.0, 0.0, 0.0, 0.0, 0.0, 0.0))
    }

    /// `local_bounds` is what the filter shades, in the effect's local
    /// space: the bounds of its region taken there.
    pub fn local_bounds(&self) -> Rect {
        self.tree_to_local().map_rect(&self.region)
    }
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
            FilterOutput::Runtime(output) => output.region,
            FilterOutput::ShaderFilter(output) => output.region,
            FilterOutput::Moved(output) => output.coverage(),
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
        let destination = self.split_for_read(at.z);
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
            FilterOutput::Runtime(output) => self.filter_passes().push_runtime(&output),
            FilterOutput::ShaderFilter(output) => self.filter_passes().push_shader_filter(&output),
            FilterOutput::Moved(moved) => match moved.as_snapshot() {
                Some(snapshot) => snapshot,
                None => {
                    let region = self.moved_region(&moved);
                    self.render_output_to_snapshot(&FilterOutput::Moved(moved), &region)
                }
            },
            output @ (FilterOutput::Clipped { .. } | FilterOutput::Solid { .. }) => {
                let region = output.coverage();
                self.render_output_to_snapshot(&output, &region)
            }
        }
    }

    /// `render_output_to_snapshot` draws `output` into a texture of its own
    /// over `region` of the tree's space.
    fn render_output_to_snapshot(&mut self, output: &FilterOutput<'_>, region: &Rect) -> Snapshot {
        self.render_to_snapshot(region, |drawing, depths| {
            let at = DrawState::in_layer(Matrix::IDENTITY, depths.first);
            drawing.draw_filter_output(output, PipelineBlend::SrcOver, &at);
        })
    }

    /// `moved_region` is the region a texture of `moved` covers: its
    /// coverage, or, when perspective takes the input past the eye plane
    /// and leaves its coverage everything with no limit to cut it, the
    /// target the planner is drawing, which is all of the output that can
    /// show when the tree's space is the target's.
    fn moved_region(&self, moved: &MovedOutput) -> Rect {
        let coverage = moved.coverage();
        if coverage == Rect::EVERYTHING {
            self.contexts.top().area.rect()
        } else {
            coverage
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
                let coverage = self.clip_to_shape(&clip.shape, clip.op, &shape_at);
                self.draw_snapshot(blur, blend, &at.clipped_to(coverage));
            }
            FilterOutput::Solid { sharp, blur } => {
                self.draw_snapshot(sharp, blend, at);
                self.draw_snapshot(blur, blend, at);
            }
            FilterOutput::Runtime(output) => {
                let shading = self.emit.runtime_filter_shading(
                    &output.shader,
                    &output.source,
                    &output.local_bounds(),
                    &output.tree_to_local(),
                    &output.effect_transform,
                    at.alpha,
                );
                self.draw_output_quad(&output.region, shading, blend, at);
            }
            FilterOutput::ShaderFilter(output) => {
                let shading =
                    self.emit
                        .shader_filter_shading(&output.shader, &output.input.view, at.alpha);
                let texels_at =
                    at.with_transform(at.transform.then(&output.input.placement.transform));
                self.draw_output_quad(&output.texel_quad(), shading, blend, &texels_at);
            }
            FilterOutput::Moved(output) => {
                let shading =
                    self.emit
                        .moved_texture_shading(&output.input, output.sampling, at.alpha);
                let texels_at = at.with_transform(at.transform.then(&output.texel_transform()));
                self.draw_output_quad(&output.texel_quad(), shading, blend, &texels_at);
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
            clip: at.clip,
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
