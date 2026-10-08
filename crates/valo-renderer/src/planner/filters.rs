//! Filter rendering: a filter tree (`filter_tree`) walked from its source
//! out, each node over what the node beneath it made — Gaussian blurs
//! (planned by `gaussian`), colour filters, drop shadows, a mask blur and
//! its fill. A node made of passes goes through [`FilterPasses`], which
//! touch no target; a fill is a target of its own (`render_to_snapshot`),
//! Impeller's blend subpass. A node hands on a [`Snapshot`]: a texture of
//! exactly what it made, and where that goes in the tree's space (a
//! target's replay coordinates, or a draw's source space). A node's pass
//! covers what its input's snapshot covers, or the union of its inputs': a
//! blur's halo lies outside the layer it blurs, in the blur's own gutter. A
//! blur's σ is in the tree's pixels, so over an input whose texels are not
//! it is divided by their size. The tree's last node hands on a
//! [`FilterOutput`] that draws itself (`filter_output`).
//!
//! The coverage hint goes down the tree as in Impeller: a composite's is
//! the target it draws into when its picture lies where it is drawn (the
//! picture is placed, [`Placement::placed`]), a blur hands its input its
//! own grown by its padding, any other node hands on none. A blur reading
//! the placed picture itself may cut it to that hint; whatever a node makes
//! is not placed.

use valo_dl::{BlendMode, BlurStyle, ClipOp, ColorFilter, Image, Paint, TileMode};

use crate::pipelines::{Blend, PipelineBlend};
use valo_geometry::{Color, Matrix, Rect};

use super::draw_state::DrawState;
use super::filter_output::{FilterOutput, Merge, StyleClip};
use super::filter_passes::FilterPasses;
use super::filter_tree::{device_offset, Fill, Filter, FilterInput, FilterTree, StyleShape};
use super::gaussian::{BlurBounds, BlurPlan};
use super::layers::SourceSpace;
use super::snapshot::{Placement, Snapshot};
use super::source::Shape;
use super::Planner;

impl Planner<'_> {
    /// `composite_picture` draws `picture`, a layer the planner rendered,
    /// into the target with `blend` at `at`, through `filter` when the paint
    /// has one.
    pub(super) fn composite_picture(
        &mut self,
        picture: Snapshot,
        filter: Option<&FilterTree<'_>>,
        blend: Blend,
        at: &DrawState,
    ) {
        let output = match filter {
            None => FilterOutput::Snapshot(picture),
            Some(filter) => match self.render_filters(filter, &picture, &at.transform) {
                Some(output) => output,
                None => return,
            },
        };
        self.composite_output(output, blend, at);
    }

    /// `plan_filtered_image` runs a draw's filter tree straight over an
    /// image drawn whole: Impeller's `TextureContents` hands its texture
    /// through as the filters' snapshot, the image's own texels placed at
    /// `dst` in the draw's source space `space`, with no layer in between. So
    /// a blur that clamps, as one on an image draw does unless told
    /// otherwise, reads the image's own edge; it blurs in source pixels
    /// however many of them a texel spans. The image is never placed: it is
    /// not a layer the composite draws where it lies. `None` when the
    /// filtered image misses the target.
    #[expect(
        clippy::too_many_arguments,
        reason = "the image's draw, its route's decisions and the state"
    )]
    pub(super) fn plan_filtered_image(
        &mut self,
        image: &Image,
        dst: &Rect,
        paint: &Paint,
        filter: &FilterTree<'_>,
        space: &SourceSpace,
        at: &DrawState,
        blend: Blend,
    ) -> Option<()> {
        let texture = Snapshot::of_image(image, dst, &space.source);
        let output = self.render_filters(filter, &texture, &space.remainder)?;
        let composite = at.with_transform(space.remainder).faded(paint.color.a);
        self.composite_output(output, blend, &composite);
        Some(())
    }

    /// `render_filters` renders `filter`'s tree over `picture` for a
    /// composite whose transform is `placement`. `None` when the result
    /// covers nothing of the target, as Impeller's
    /// `FilterContents::GetEntity` skips a filter whose coverage misses its
    /// pass.
    fn render_filters<'t>(
        &mut self,
        filter: &FilterTree<'t>,
        picture: &Snapshot,
        placement: &Matrix,
    ) -> Option<FilterOutput<'t>> {
        let area = self.contexts.top().area;
        let coverage = placement.map_rect(&filter.coverage(&picture.placement.coverage()));
        if area.culls(&coverage) {
            return None;
        }
        let hint = picture.placement.placed.then(|| area.rect());
        Some(self.render_filter_tree(&filter.root, picture, &filter.effect_transform, hint))
    }

    /// `render_filter_tree` renders `tree` over `source`, from the node
    /// nearest the source out, each node reading a texture of what the one
    /// beneath it made, or drawing it, for a fill; its filters' parameters
    /// are under `effect_transform`. `hint` is the node's coverage hint, the
    /// region its output has to cover; a fill hands its mask none.
    pub(super) fn render_filter_tree<'t>(
        &mut self,
        tree: &FilterInput<'t>,
        source: &Snapshot,
        effect_transform: &Matrix,
        hint: Option<Rect>,
    ) -> FilterOutput<'t> {
        match tree {
            FilterInput::Source => FilterOutput::Snapshot(source.clone()),
            FilterInput::FillIn { mask, fill } => {
                let mask = self.render_filter_tree(mask, source, effect_transform, None);
                self.render_fill_in(&mask, fill, effect_transform, hint)
            }
            FilterInput::Filter(node) => {
                let input_hint = node.filter.input_hint(hint, effect_transform);
                let input =
                    self.render_filter_tree(&node.input, source, effect_transform, input_hint);
                let input = self.snapshot_output(input);
                self.render_filter(&node.filter, input, effect_transform, input_hint)
            }
        }
    }

    /// `render_filter` is one node over its input's texture, its parameters
    /// under `effect_transform`. A blur cuts a placed input to `input_hint`
    /// when the input holds it.
    fn render_filter<'t>(
        &mut self,
        filter: &Filter<'t>,
        input: Snapshot,
        effect_transform: &Matrix,
        input_hint: Option<Rect>,
    ) -> FilterOutput<'t> {
        let plan = |placement: &Placement, tile_mode, bounds| {
            let blur = filter.blur(effect_transform)?;
            BlurPlan::new(&blur, placement, input_hint.as_ref(), tile_mode, bounds)
        };
        match filter {
            Filter::Blur {
                bounds, tile_mode, ..
            } => {
                let bounds = bounds.as_ref().map(|rect| BlurBounds {
                    rect,
                    local_to_tree: effect_transform,
                });
                let blurred = match plan(&input.placement, *tile_mode, bounds) {
                    Some(plan) => self.filter_passes().push_blur(&plan, &input.view),
                    None => input,
                };
                FilterOutput::Snapshot(blurred)
            }
            Filter::MaskBlur { blur, style_shape } => {
                // Decal, as `CreateMaskBlur` makes it. A negligible σ leaves
                // the draw as it is, style and all, as Impeller's blur
                // returns its input before it applies a style.
                let Some(plan) = plan(&input.placement, TileMode::Decal, None) else {
                    return FilterOutput::Snapshot(input);
                };
                let blurred = self.filter_passes().push_blur(&plan, &input.view);
                let style = MaskStyle {
                    style: blur.style,
                    shape: *style_shape,
                    transform: effect_transform,
                };
                style.apply(blurred, input)
            }
            Filter::Color(filter) => FilterOutput::Recolour {
                input,
                filter: *filter,
            },
            Filter::DropShadow { offset, color, .. } => {
                let tinted = self.filter_passes().push_shadow_tint(&input, *color);
                let shadow = match plan(&tinted.placement, TileMode::Decal, None) {
                    Some(plan) => self.filter_passes().push_blur(&plan, &tinted.view),
                    None => tinted,
                };
                drop_shadow(input, shadow, device_offset(effect_transform, *offset))
            }
        }
    }

    /// `render_fill_in` is `CreateMaskBlur`'s blend, Impeller's
    /// `PipelineBlend` with `SrcIn`: a subpass over what the blurred mask
    /// and the fill cover, within the node's coverage hint, where the mask
    /// is drawn and the fill is drawn `SrcIn` over it. The mask draws itself
    /// as a final draw, clip and all, where Impeller first makes a texture
    /// of it; on a cleared subpass the two are the same.
    fn render_fill_in(
        &mut self,
        mask: &FilterOutput<'_>,
        fill: &Fill<'_>,
        effect_transform: &Matrix,
        hint: Option<Rect>,
    ) -> FilterOutput<'static> {
        let transform = *effect_transform;
        let coverage = mask
            .coverage()
            .union(&transform.map_rect(&fill.local_bounds()));
        // A hint the coverage misses cuts nothing: the composite culls such
        // a tree before it renders, so this is never more than a fallback.
        let region = hint
            .and_then(|hint| coverage.intersect(&hint))
            .unwrap_or(coverage);
        let snapshot = self.render_to_snapshot(&region, |drawing, depths| {
            let mask_at = DrawState::in_layer(Matrix::IDENTITY, depths.first);
            drawing.draw_filter_output(mask, PipelineBlend::SrcOver, &mask_at);
            let (contents, paint) = fill.as_draw();
            let fill_at = DrawState::in_layer(transform, depths.second);
            drawing.draw(contents, paint, &fill_at, PipelineBlend::SrcIn);
        });
        FilterOutput::Snapshot(snapshot)
    }
}

impl FilterPasses<'_, '_> {
    /// `push_shadow_tint` is the first stage of Skia's
    /// `SkImageFilters::DropShadow` graph: the input's alpha tinted with the
    /// shadow `color`. The blur runs on this tinted copy rather than the
    /// input so a translucent shadow colour spreads at its own alpha; a blur
    /// is linear, so the order of the two is otherwise Skia's.
    fn push_shadow_tint(&mut self, source: &Snapshot, color: Color) -> Snapshot {
        let tint = ColorFilter::Blend(color, BlendMode::SrcIn);
        self.push_recolour(source, tint, 1.0)
    }
}

/// `drop_shadow` is the end of Skia's drop-shadow graph: the blurred,
/// tinted `shadow` moved by `offset` (in the tree's pixels), and the
/// untouched `source` merged on top. The move only changes where the
/// shadow's texture goes.
fn drop_shadow(source: Snapshot, mut shadow: Snapshot, offset: [f32; 2]) -> FilterOutput<'static> {
    let [x, y] = offset;
    shadow.placement.transform = Matrix::translation(x, y).then(&shadow.placement.transform);
    FilterOutput::Merge {
        first: shadow,
        second: source,
        merge: Merge::DropShadow,
    }
}

/// `MaskStyle` is how a mask blur keeps its blur: its style, and what it
/// keeps the blur to, with the transform from the draw's local coordinates
/// into the tree's space.
struct MaskStyle<'m, 't> {
    style: BlurStyle,
    shape: StyleShape<'t>,
    transform: &'m Matrix,
}

impl<'t> MaskStyle<'_, 't> {
    /// `apply` is Impeller's `ApplyBlurStyle`: a normal blur is the blur; an
    /// inner or outer one is the blur clipped to the draw's shape or to
    /// outside it (`ApplyClippedBlurStyle`); a solid one is the sharp draw
    /// with the blur over it. A glyph run, with no shape, merges its style
    /// in the composite's fragment instead.
    fn apply(self, blurred: Snapshot, sharp: Snapshot) -> FilterOutput<'t> {
        match (self.style, self.shape) {
            (BlurStyle::Normal, _) => FilterOutput::Snapshot(blurred),
            (style, StyleShape::GlyphRun) => FilterOutput::Merge {
                first: blurred,
                second: sharp,
                merge: Merge::BlurStyle(style),
            },
            (BlurStyle::Solid, StyleShape::Shape(_)) => FilterOutput::Solid {
                sharp,
                blur: blurred,
            },
            (BlurStyle::Inner, StyleShape::Shape(shape)) => {
                self.clipped(blurred, shape, ClipOp::Intersect)
            }
            (BlurStyle::Outer, StyleShape::Shape(shape)) => {
                self.clipped(blurred, shape, ClipOp::Difference)
            }
        }
    }

    /// `clipped` is the blur clipped by `op` to the draw's shape.
    fn clipped(&self, blurred: Snapshot, shape: Shape<'t>, op: ClipOp) -> FilterOutput<'t> {
        FilterOutput::Clipped {
            blur: blurred,
            clip: StyleClip {
                shape,
                op,
                transform: *self.transform,
            },
        }
    }
}
