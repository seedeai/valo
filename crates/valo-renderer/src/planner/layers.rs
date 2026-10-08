//! The layer lifecycle — open (or elide, or skip), fill, close, composite —
//! for save layers, the implicit and effect layers a draw opens for itself,
//! and snapshots. Each opens a draw context on the stack; closing it
//! returns its picture, a [`Snapshot`] of what it holds, and whoever opened
//! it decides what to do with that: a save layer's composite rides its
//! scope entry until the restore (Impeller's `SaveLayerState`), an implicit
//! or effect layer composites at once, a snapshot is handed on.
//!
//! What becomes of a save layer is decided first, as a value
//! ([`LayerPlan`]), and only then acted on. Replay state never lives here:
//! the group alpha and a save layer's composite ride the scope entries in
//! `replay`, so a context is purely a texture being drawn.

use rustc_hash::FxHashMap;
use valo_dl::{Bounds, DisplayList, MaskKind, Op, Paint};
use valo_geometry::{Matrix, Rect};

use crate::pipelines::{Blend, PipelineBlend};
use crate::raster::FillTarget;

use super::backdrop::{BackdropRequest, SharedBackdrop};
use super::draw_context::{DepthRange, DrawContext, PixelArea};
use super::draw_state::{DrawState, ONE_DRAW_LAYER};
use super::drawing::Drawing;
use super::emit::{Cover, Entity, Role};
use super::filter_tree::FilterTree;
use super::gaussian::extract_scale;
use super::layer_coverage::{compute_save_layer_coverage, SourceCoverage};
use super::replay::ReplayState;
use super::shading::Shading;
use super::snapshot::Snapshot;
use super::source::DrawSource;
use super::Planner;

/// `SNAPSHOT_DEPTH` is a snapshot's depth line: a first draw, a style's
/// clip half a slot above it, and a draw over both.
const SNAPSHOT_DEPTH: DepthRange = DepthRange::new(0, 3);

/// `SnapshotDepths` are the depths on a snapshot's line that its draws are
/// handed.
#[derive(Clone, Copy, Debug)]
pub(super) struct SnapshotDepths {
    /// The first draw's. A style clip it carries writes its ceiling half a
    /// slot above it (`Drawing::clip_to_shape`), below `second`.
    pub first: f32,
    /// What is drawn over the first draw, past its clip.
    pub second: f32,
}

/// `LayerComposite` is how a save layer is drawn into its parent once its
/// restore closes it, decided when it opens.
pub(super) struct LayerComposite {
    pub blend: Blend,
    /// Set = the layer's texture is coverage, not content.
    pub mask: Option<MaskKind>,
    /// The paint's filter tree over the layer; `None` composites the
    /// texture as is.
    pub filter: Option<FilterTree<'static>>,
    /// Where the composite draws in the parent: at the identity, the layer
    /// being in the parent's replay coordinates, at the depth taken while
    /// the PARENT's slot base was still active, and at the paint's alpha
    /// times the group alpha around the layer.
    pub at: DrawState,
}

/// `ResolvedLayer` is one `SaveLayer` op pinned to one replay: its rects in
/// the target's coordinates and its slots on the target's depth line.
///
/// The op is written once and replayed anywhere — nested inside another
/// list, or into a cache texture, at whatever depth the walk has reached —
/// so it carries list-root rects, slot numbers rather than depths, and a
/// backdrop key it has no standing to validate. The walk resolves them all
/// at its boundary, so deciding what becomes of the layer reads them as
/// they are. Skia passes the same set as `SkCanvas::SaveLayerRec`.
pub(super) struct ResolvedLayer<'a> {
    /// Configures the composite draw that puts the finished layer into its
    /// parent — blend mode and filters treat the layer as one image.
    ///
    /// Read at OPEN, not at close: the composite takes the parent's group
    /// alpha and depth, which the children's scope replaces.
    pub paint: &'a Paint,
    /// Set = the layer's texture is coverage, not content.
    ///
    /// The composite turns its pixels into a multiplier on the parent
    /// (DstIn over the whole enclosing extent), so everything the mask
    /// does not cover disappears.
    pub mask: Option<MaskKind>,
    /// The layer's content: the children's union, cropped by the clips
    /// inside the layer and the bounds hint but not by the clips around it.
    pub content: Bounds,
    /// The clip where the layer opens, cropped by the bounds hint. With the
    /// target, the coverage limit.
    pub clip: Bounds,
    /// The slots the layer hosts: its children's, from the one where its
    /// scope opened, up to its composite's.
    pub depth: DepthRange,
    /// The composite paints the layer's whole clip, so the texture covers it.
    pub floods_clip: bool,
    /// Where the layer opens, taken while the PARENT's scope was still open
    /// — the composite draws in the parent, not in the layer: the save
    /// point's transform (the paint's effect transform), the composite
    /// slot's depth, and the group alpha the composite takes.
    pub at: DrawState,
    /// The children turned out alpha-linear and pairwise disjoint, so
    /// nothing overlaps for the group's alpha to blend twice: it can ride
    /// each child's own tint and the texture disappears entirely.
    pub can_elide: bool,
    /// Set = the layer opens pre-filled with the blurred scene beneath it.
    pub backdrop: Option<BackdropRequest>,
}

impl<'a> ResolvedLayer<'a> {
    /// `of` pins `op`, a recorded `SaveLayer` of `list`, to the replay
    /// `state`: `at` is where its composite draws in the parent.
    pub fn of(op: &'a Op, list: &DisplayList, state: &ReplayState, at: DrawState) -> Self {
        let Op::SaveLayer {
            paint,
            mask_composite,
            scope_bounds,
            clip_bounds,
            base_slot,
            composite_slot,
            floods_clip,
            can_elide,
            backdrop,
            ..
        } = op
        else {
            unreachable!("replay resolves only SaveLayer ops as layers, not {op:?}");
        };
        Self {
            paint,
            mask: *mask_composite,
            content: scope_bounds.map(state.base()),
            clip: clip_bounds.map(state.base()),
            depth: DepthRange::new(state.absolute(*base_slot), composite_slot - base_slot),
            floods_clip: *floods_clip,
            at,
            can_elide: *can_elide,
            backdrop: backdrop
                .as_ref()
                .map(|backdrop| BackdropRequest::of(backdrop, list, state.base())),
        }
    }

    /// `coverage` is where the layer's texture goes in `target`, whole
    /// pixels round it: Impeller's `ComputeSaveLayerCoverage` of its content
    /// within the clip and the target (`GetLocalCoverageLimit`), sized for
    /// `filter`. `None` when the layer shows nothing.
    ///
    /// A layer that shows and whose filter reads past its edge (a blur that
    /// clamps, mirrors or repeats), and does not flood, covers that limit
    /// itself, its clip cut by its bounds hint: the blur's edge is the layer's own extent, as
    /// Skia takes it (`SkBlurImageFilter`'s legacy tiling crops at the
    /// layer's bounds, which are the caller's bounds, a hard clip on the
    /// layer, or the clip's), not the edge of its content. Impeller sizes
    /// such a layer to its content and clamps there; valo follows Skia.
    fn coverage(&self, target: &PixelArea, filter: Option<&FilterTree<'_>>) -> Option<Rect> {
        let limit = self.clip.intersect(&Bounds::of(target.rect())).rect()?;
        let coverage = compute_save_layer_coverage(
            &self.content,
            &Matrix::IDENTITY,
            &limit,
            filter.map(|filter| filter as &dyn SourceCoverage),
            self.floods_clip,
        )?;
        let reads_past_its_edge = filter.is_some_and(FilterTree::reads_past_its_edge);
        let coverage = if reads_past_its_edge && !self.floods_clip {
            limit
        } else {
            coverage
        };
        (!coverage.is_empty()).then(|| round_out(&coverage))
    }
}

/// `LayerPlan` is what becomes of one save layer, decided from its recorded
/// facts and the target it opens in before anything is drawn.
pub(super) enum LayerPlan {
    /// Nothing visible: its scope is skipped. A mask is not nothing: its
    /// coverage is 0 everywhere, so it erases the target beneath it first.
    Skip { erase: bool },
    /// The opacity peephole: the children draw in the parent at their own
    /// slots, at the group alpha times this alpha. Depth does not change,
    /// which is what makes it safe.
    Elide(f32),
    /// A texture of its own over `area`, closed at the restore and drawn
    /// into the parent as `composite` says.
    Open {
        area: PixelArea,
        composite: Box<LayerComposite>,
    },
}

impl LayerPlan {
    /// `of` is what becomes of `layer`, opening in `target`.
    pub fn of(layer: &ResolvedLayer<'_>, target: &PixelArea) -> Self {
        // The layer paint's σ is local at the SAVE POINT, so the save-point
        // transform scales it to device. (The list base alone would leave a
        // `scale(4); save_layer(blur σ5)` halo four times too narrow.)
        let filter = FilterTree::for_save_layer(layer.paint, layer.at.transform);
        let Some(rect) = layer.coverage(target, filter.as_ref()) else {
            return LayerPlan::Skip {
                erase: layer.mask.is_some(),
            };
        };
        if layer.can_elide {
            return LayerPlan::Elide(layer.paint.color.a);
        }
        LayerPlan::Open {
            area: PixelArea::over(rect),
            composite: Box::new(LayerComposite {
                blend: Blend::of(layer.paint.blend_mode),
                mask: layer.mask,
                filter,
                at: layer
                    .at
                    .with_transform(Matrix::IDENTITY)
                    .faded(layer.paint.color.a),
            }),
        }
    }
}

/// `Opened` is what the walk does with a save layer's scope once the layer
/// is opened.
pub(super) enum Opened {
    /// Nothing visible: skip the scope's ops entirely.
    Skip,
    /// The opacity shortcut: children draw in the parent at the group alpha
    /// times this alpha, on their tints.
    Elided(f32),
    /// A real offscreen, the top context now: the restore closes it and
    /// draws it into the parent as this says.
    Layer(LayerComposite),
}

impl Planner<'_> {
    /// `open_layer` acts on what becomes of a recorded `SaveLayer`: skip it
    /// (erasing first, for a mask), elide it into the parent, or open a
    /// context of its own, seeded with its backdrop.
    pub(super) fn open_layer(
        &mut self,
        layer: ResolvedLayer<'_>,
        shared_backdrops: &mut FxHashMap<u64, SharedBackdrop>,
    ) -> Opened {
        match LayerPlan::of(&layer, &self.contexts.top().area) {
            LayerPlan::Skip { erase } => {
                if erase {
                    self.drawing().erase_alpha(layer.at.z);
                }
                Opened::Skip
            }
            LayerPlan::Elide(alpha) => {
                self.plan.stats.layers_elided += 1;
                Opened::Elided(alpha)
            }
            LayerPlan::Open { area, composite } => {
                // The blurred parent is sampled BEFORE the layer's texture
                // opens — this ordering is the whole point of
                // backdrop-as-a-layer-property: the glass shows the real
                // scene, not a fresh offscreen.
                let seed = layer.backdrop.and_then(|request| {
                    self.render_backdrop_seed(&layer.at, &area.rect(), request, shared_backdrops)
                });
                self.open_target(area, layer.depth);
                if let Some(seed) = seed {
                    self.draw_backdrop_seed(&seed);
                }
                Opened::Layer(*composite)
            }
        }
    }

    /// `close_layer` closes a save layer's context at its restore and draws
    /// its picture into the parent as `composite` says: a mask multiplies
    /// the parent by its coverage; anything else goes through the paint's
    /// filters, if any, with the paint's blend. The picture lies where the
    /// composite draws it.
    pub(super) fn close_layer(&mut self, composite: LayerComposite) {
        let picture = self.close_target().placed_where_drawn(true);
        match composite.mask {
            Some(kind) => self
                .drawing()
                .draw_mask_composite(&picture, kind, &composite.at),
            None => self.composite_picture(
                picture,
                composite.filter.as_ref(),
                composite.blend,
                &composite.at,
            ),
        }
    }

    /// `render_to_snapshot` renders what `draw` draws into a texture of its
    /// own over `region`, whole pixels round it: Impeller's
    /// `Contents::RenderToSnapshot` for a final draw that a next node reads,
    /// and its blend subpass. The context has depth and stencil, so a
    /// style's clip works in it; `draw` is handed the depths of its line's
    /// two draws ([`SnapshotDepths`]). Its draws take no group alpha
    /// ([`DrawState::in_layer`]): whoever draws the snapshot does.
    pub(super) fn render_to_snapshot(
        &mut self,
        region: &Rect,
        draw: impl FnOnce(&mut Drawing, SnapshotDepths),
    ) -> Snapshot {
        self.open_target(PixelArea::over(round_out(region)), SNAPSHOT_DEPTH);
        let depths = SnapshotDepths {
            first: SNAPSHOT_DEPTH.z(1),
            second: SNAPSHOT_DEPTH.z(2),
        };
        draw(&mut self.drawing(), depths);
        self.close_target()
    }

    /// `plan_via_implicit_layer` renders one draw into its own layer and
    /// composites it by `blend` — the desugar for a destination-reading
    /// paint on anything but a solid shape, and for shader-painted text.
    /// `inner` draws the draw at the state it is handed
    /// ([`DrawState::alone_in_layer`] under the draw's transform): never
    /// slots, so the replay slot base stays untouched, and alpha 1, because
    /// the composite takes the draw's.
    /// `None` when the draw misses the target.
    pub(super) fn plan_via_implicit_layer(
        &mut self,
        device_bounds: Rect,
        at: &DrawState,
        blend: Blend,
        inner: impl FnOnce(&mut Drawing, &DrawState),
    ) -> Option<()> {
        let rect = device_bounds.intersect(&self.contexts.top().area.rect())?;
        self.open_target(PixelArea::over(rect), ONE_DRAW_LAYER);
        inner(
            &mut self.drawing(),
            &DrawState::alone_in_layer(at.transform),
        );
        let picture = self.close_target();
        self.composite_picture(picture, None, blend, &at.with_transform(Matrix::IDENTITY));
        Some(())
    }

    /// `render_effect_layer` renders one draw into a layer of its own, in
    /// its source space `space` (its transform without rotation or skew,
    /// where `filter` runs along its own axes), and returns the layer's
    /// picture; `None` when the filter needs nothing of the draw. The layer
    /// holds `content` drawn at `paint`, whose ink is `local_bounds`, a texel
    /// wider all round (as Impeller pads a draw's snapshot so a sampler past
    /// its edge reads transparent), cut to what the filter needs of the
    /// target: not cut at the clip, which the composite meets instead. With
    /// no ink given, the draw fills its clip: the layer is all the filter
    /// needs of the target. The picture lies where the composite draws it
    /// when source space is the target's.
    pub(super) fn render_effect_layer(
        &mut self,
        content: DrawSource<'_>,
        paint: &Paint,
        local_bounds: Option<&Rect>,
        filter: &FilterTree<'_>,
        space: &SourceSpace,
    ) -> Option<Snapshot> {
        let target = self.contexts.top().area.rect();
        let limit = filter.source_coverage(&space.to_source(&target));
        let region = match local_bounds {
            Some(local_bounds) => {
                let ink = space.source.map_rect(local_bounds).expand(1.0);
                ink.intersect(&limit)?
            }
            None => limit,
        };
        self.open_target(PixelArea::over(round_out(&region)), ONE_DRAW_LAYER);
        self.drawing()
            .draw_alone(content, paint, &DrawState::alone_in_layer(space.source));
        let placed = space.remainder == Matrix::IDENTITY;
        Some(self.close_target().placed_where_drawn(placed))
    }

    /// `open_target` opens a pooled offscreen over `area`, hosting `depth`,
    /// as the context draws go to, and counts it as a rendered layer: `area`
    /// is in the space of the context beneath it, replay coords or a draw's
    /// source space.
    fn open_target(&mut self, area: PixelArea, depth: DepthRange) {
        self.plan.stats.layers_rendered += 1;
        let layer = self.tools.pool.take_layer(area.size(), self.tools.format);
        self.contexts
            .push(DrawContext::offscreen(layer, area, depth));
    }

    /// `open_raster_target` opens a list-raster cache texture as the context
    /// draws go to, its depth line `list`'s own. Closing it composites
    /// nothing: the quad that samples the finished texture is an ordinary
    /// draw the caller makes in the parent.
    pub(super) fn open_raster_target(&mut self, fill: &FillTarget, list: &DisplayList) {
        let attachments = self
            .tools
            .pool
            .take_attachments(fill.size, self.tools.format);
        self.contexts.push(DrawContext::raster(
            attachments,
            fill,
            DepthRange::of_list(list),
        ));
    }

    /// `close_target` emits the top context's last segment, pops it, and
    /// returns its picture.
    pub(super) fn close_target(&mut self) -> Snapshot {
        self.emit_segment();
        self.contexts.pop().picture()
    }
}

impl Drawing<'_, '_> {
    /// `draw_mask_composite` samples the `mask` layer across the WHOLE
    /// enclosing context and multiplies it in via DstIn — outside the
    /// mask's rect the fragment forces coverage 0, which is what erases
    /// unmasked content.
    fn draw_mask_composite(&mut self, mask: &Snapshot, kind: MaskKind, at: &DrawState) {
        let shading = self.emit.mask_composite_shading(mask, kind, at.alpha);
        self.erase_by(shading, at.z);
    }

    /// `erase_alpha` composites DstIn with zero source alpha over the whole
    /// context — the "mask never rendered" result (coverage 0 everywhere).
    fn erase_alpha(&mut self, z: f32) {
        self.erase_by(Shading::solid([0.0; 4]), z);
    }

    /// `erase_by` multiplies the whole context by `shading`'s alpha, DstIn.
    fn erase_by(&mut self, shading: Shading, z: f32) {
        let entity = Entity {
            stencil: None,
            cover: Cover::Quad {
                transform: Matrix::IDENTITY,
                rect: self.context.area.rect(),
            },
            role: Role::Fill,
            shading,
            blend: PipelineBlend::DstIn,
            z,
        };
        self.emit.push(self.context, entity);
    }
}

/// `SourceSpace` is where a draw's effects run: Impeller's blur source
/// space, the draw's transform without its rotation or skew — its
/// translation and the length of each axis (`CalculateBlurInfo`'s
/// `source_space_offset` and `source_space_scalar`). The rest of the
/// transform turns the result where it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SourceSpace {
    /// Local coordinates into source space.
    pub source: Matrix,
    /// Source space into the parent's replay coordinates.
    pub remainder: Matrix,
}

impl SourceSpace {
    /// `of` splits `transform`; `None` when an axis has no length, so
    /// nothing it draws shows.
    pub fn of(transform: &Matrix) -> Option<Self> {
        let [scale_x, scale_y] = extract_scale(transform);
        let [.., offset_x, offset_y] = transform.to_affine();
        let source = Matrix::translation(offset_x, offset_y).then(&Matrix::scale(scale_x, scale_y));
        let remainder = transform.then(&source.invert()?);
        Some(Self { source, remainder })
    }

    /// `to_source` maps a region of the parent's replay coordinates into
    /// source space, its bounds there.
    pub fn to_source(&self, region: &Rect) -> Rect {
        self.remainder
            .invert()
            .map_or(Rect::EVERYTHING, |inverse| inverse.map_rect(region))
    }
}

/// `round_out` is the smallest rect of whole pixels holding `rect`:
/// Impeller's `IRect::RoundOut` for a save layer's subpass. A layer on whole
/// pixels renders its children on the parent's pixel grid, and its
/// composite copies texels 1:1 instead of resampling them.
pub(super) fn round_out(rect: &Rect) -> Rect {
    Rect::from_ltrb(
        rect.x.floor(),
        rect.y.floor(),
        rect.right().ceil(),
        rect.bottom().ceil(),
    )
}

/// Impeller's `gaussian_blur_filter_contents_unittests.cc` for a draw: the
/// blurred draw's coverage, worked out in its source space and turned back
/// by the rest of its transform, is what Impeller's `GetCoverage` says.
#[cfg(test)]
mod tests {
    use super::*;

    /// A layer over `content`, opening in an unclipped 100×100 target at
    /// slot 4 with three slots of children, as `paint` says.
    fn layer(paint: &Paint, content: Rect) -> ResolvedLayer<'_> {
        ResolvedLayer {
            paint,
            mask: None,
            content: Bounds::of(content),
            clip: Bounds::Unbounded,
            depth: DepthRange::new(4, 4),
            floods_clip: false,
            at: DrawState::in_layer(Matrix::IDENTITY, 0.5),
            can_elide: false,
            backdrop: None,
        }
    }

    fn target() -> PixelArea {
        PixelArea::of_size([100, 100])
    }

    fn opened_area(plan: LayerPlan) -> Rect {
        match plan {
            LayerPlan::Open { area, .. } => area.rect(),
            _ => panic!("the layer opens"),
        }
    }

    /// A layer opens over its content in whole pixels; a layer that misses
    /// the target is skipped, and a mask that does erases instead.
    #[test]
    fn a_layer_opens_over_its_content_or_is_skipped() {
        let paint = Paint::default();
        let content = Rect::new(10.5, 20.0, 30.0, 10.25);
        assert_eq!(
            opened_area(LayerPlan::of(&layer(&paint, content), &target())),
            Rect::from_ltrb(10.0, 20.0, 41.0, 31.0)
        );
        let outside = layer(&paint, Rect::new(200.0, 0.0, 10.0, 10.0));
        assert!(matches!(
            LayerPlan::of(&outside, &target()),
            LayerPlan::Skip { erase: false }
        ));
        let mask = ResolvedLayer {
            mask: Some(MaskKind::Alpha),
            ..layer(&paint, Rect::new(200.0, 0.0, 10.0, 10.0))
        };
        assert!(matches!(
            LayerPlan::of(&mask, &target()),
            LayerPlan::Skip { erase: true }
        ));
    }

    /// An elidable layer hands its alpha to its children; it still shows
    /// somewhere, or it would be skipped.
    #[test]
    fn an_elidable_layer_hands_its_alpha_on() {
        let paint = Paint::from_color(valo_geometry::Color::rgba(0.0, 0.0, 0.0, 0.25));
        let elidable = ResolvedLayer {
            can_elide: true,
            ..layer(&paint, Rect::new(10.0, 10.0, 10.0, 10.0))
        };
        assert!(matches!(
            LayerPlan::of(&elidable, &target()),
            LayerPlan::Elide(alpha) if alpha == 0.25
        ));
    }

    /// A layer that floods its clip covers the clip within the target,
    /// whatever its content; its composite draws at the identity with the
    /// paint's alpha.
    #[test]
    fn a_flooding_layer_covers_its_clip() {
        let paint = Paint::from_color(valo_geometry::Color::rgba(0.0, 0.0, 0.0, 0.5));
        let flooding = ResolvedLayer {
            floods_clip: true,
            clip: Bounds::Bounded(Rect::new(-10.0, 30.0, 50.0, 20.0)),
            at: DrawState {
                transform: Matrix::translation(3.0, 4.0),
                z: 0.5,
                alpha: 0.5,
            },
            ..layer(&paint, Rect::new(12.0, 34.0, 2.0, 2.0))
        };
        let LayerPlan::Open { area, composite } = LayerPlan::of(&flooding, &target()) else {
            panic!("the layer opens");
        };
        assert_eq!(area.rect(), Rect::new(0.0, 30.0, 40.0, 20.0));
        assert_eq!(composite.at.transform, Matrix::IDENTITY);
        assert_eq!(composite.at.alpha, 0.25);
        assert_eq!(composite.at.z, 0.5);
    }

    /// A layer whose blur reads transparent past its edge (decal) keeps its
    /// tight texture round its content, wherever the clip is; one whose blur
    /// clamps covers its clip, cut by its bounds hint, the edge it clamps at.
    #[test]
    fn a_blur_that_reads_past_its_edge_covers_the_clip_and_a_decal_one_its_content() {
        use valo_dl::{ImageFilter, TileMode};
        let blurred = |tile_mode: Option<TileMode>| {
            let blur = ImageFilter::blur(4.0, 4.0);
            Paint {
                image_filter: Some(match tile_mode {
                    Some(tile_mode) => blur.with_tile_mode(tile_mode),
                    None => blur,
                }),
                ..Paint::default()
            }
        };
        let content = Rect::new(40.0, 40.0, 20.0, 20.0);
        let clip = Bounds::Bounded(Rect::new(10.0, 10.0, 80.0, 80.0));
        let area = |paint: &Paint, clip: Bounds| {
            let layer = ResolvedLayer {
                clip,
                ..layer(paint, content)
            };
            opened_area(LayerPlan::of(&layer, &target()))
        };
        let decal = blurred(None);
        assert_eq!(area(&decal, clip), content);
        assert_eq!(area(&decal, Bounds::Unbounded), content);
        let clamp = blurred(Some(TileMode::Clamp));
        assert_eq!(area(&clamp, clip), Rect::new(10.0, 10.0, 80.0, 80.0));
        assert_eq!(
            area(&clamp, Bounds::Unbounded),
            Rect::new(0.0, 0.0, 100.0, 100.0),
            "unclipped, the target is the limit"
        );
    }

    use crate::planner::filter_tree::FilterInput;
    use crate::planner::gaussian::sigma_for_blur_radius;
    use valo_dl::{ImageFilter, TileMode};

    /// What a draw of `local` blurred by σ under `entity` covers.
    fn blurred_draw_coverage(entity: &Matrix, local: &Rect, sigma: f32) -> Rect {
        let space = SourceSpace::of(entity).expect("an invertible transform");
        let tree = FilterInput::Source
            .with_image_filter(Some(&ImageFilter::blur(sigma, sigma)), TileMode::Decal);
        let source = space.source.map_rect(local);
        space
            .remainder
            .map_rect(&tree.coverage(&source, &space.source))
    }

    fn assert_rect_near(actual: Rect, expected: Rect) {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(
            near(actual.x, expected.x)
                && near(actual.y, expected.y)
                && near(actual.right(), expected.right())
                && near(actual.bottom(), expected.bottom()),
            "{actual:?} != {expected:?}"
        );
    }

    /// A 400×300 draw turned a quarter about its top left, then moved to
    /// (400, 100).
    #[test]
    fn render_coverage_matches_get_coverage_rotated() {
        let sigma = sigma_for_blur_radius(1.0, &Matrix::IDENTITY);
        let entity =
            Matrix::translation(400.0, 100.0).then(&Matrix::rotation(std::f32::consts::FRAC_PI_2));
        let coverage = blurred_draw_coverage(&entity, &Rect::new(0.0, 0.0, 400.0, 300.0), sigma);
        assert_rect_near(coverage, Rect::from_ltrb(99.0, 99.0, 401.0, 501.0));
    }

    #[test]
    fn texture_contents_with_destination_rect() {
        let sigma = sigma_for_blur_radius(1.0, &Matrix::IDENTITY);
        let coverage = blurred_draw_coverage(
            &Matrix::IDENTITY,
            &Rect::new(50.0, 40.0, 100.0, 100.0),
            sigma,
        );
        assert_rect_near(coverage, Rect::from_ltrb(49.0, 39.0, 151.0, 141.0));
    }

    /// Impeller expects 94, 74, 212, 212 here: it passes the texture through
    /// without rendering it, so its texels are two source pixels wide and a σ
    /// computed for source pixels blurs twice as far, and its
    /// `local_padding` scales the padding by the entity once more. valo
    /// departs from it: a draw renders into its effect layer at source
    /// resolution, and a texture passed through blurs with σ divided by its
    /// texel size (`BlurInfo::in_texels`), so the σ of 2.16 source pixels
    /// reaches 3 of them each side either way.
    #[test]
    fn texture_contents_with_destination_rect_scaled() {
        let sigma = sigma_for_blur_radius(1.0, &Matrix::IDENTITY);
        let coverage = blurred_draw_coverage(
            &Matrix::scale(2.0, 2.0),
            &Rect::new(50.0, 40.0, 100.0, 100.0),
            sigma,
        );
        assert_rect_near(coverage, Rect::from_ltrb(97.0, 77.0, 303.0, 283.0));
    }

    /// The effect transform of a layer: the blur's σ is taken through it,
    /// and what it covers stays in the layer's space.
    #[test]
    fn texture_contents_with_effect_transform() {
        let effect_transform = Matrix::scale(2.0, 2.0);
        let sigma = sigma_for_blur_radius(1.0, &effect_transform);
        let tree = FilterInput::Source
            .with_image_filter(Some(&ImageFilter::blur(sigma, sigma)), TileMode::Decal);
        let coverage = tree.coverage(&Rect::new(50.0, 40.0, 100.0, 100.0), &effect_transform);
        assert_rect_near(coverage, Rect::from_ltrb(49.0, 39.0, 151.0, 141.0));
    }

    /// A mirror is the remainder's to undo: source space keeps the axes'
    /// lengths, and the draw lands back where the transform puts it.
    #[test]
    fn source_space_leaves_a_mirror_to_the_remainder() {
        let entity = Matrix::translation(100.0, 0.0).then(&Matrix::scale(-2.0, 3.0));
        let space = SourceSpace::of(&entity).expect("an invertible transform");
        assert_eq!(extract_scale(&space.source), [2.0, 3.0]);
        let point = valo_geometry::Point::new(5.0, 7.0);
        let placed = space.remainder.map_point(space.source.map_point(point));
        assert_eq!(placed, entity.map_point(point));
    }
}
