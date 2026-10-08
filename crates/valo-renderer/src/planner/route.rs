//! The one per-draw decision: given a draw request (what it draws + paint +
//! transform), pick which execution pattern realizes it — a direct step, an
//! effect layer, or a destination read. Every primitive shares the
//! identical decision, only "draw yourself plain" differs, so the decision
//! lives here once, as a value ([`Route`]) worked out before anything is
//! drawn, and primitives only know their geometry (Skia's
//! `AutoLayerForImageFilter`, Impeller's
//! `AddRenderEntityWithFiltersToCurrentPass`).
//!
//! A paint's colour filter acts on the draw's colour before its coverage,
//! as Skia's paint does, so it is taken in before routing wherever it can
//! be: folded into a solid's colour or a gradient's stops, baked into a
//! pattern, or handed to a solid glyph run, which colours its glyphs
//! through it. Impeller instead filters a finished text run, colouring its
//! box; valo follows Skia.
//!
//! Three sources carry deliberate policy differences: an image applies a
//! colour filter on its sampled pixel in the same draw (never a layer), the
//! analytic rrect blur handles its own mask blur in the fragment, and a
//! shader-painted glyph run paints THROUGH its glyphs instead of sampling
//! the shader in their fragments. A layer that holds one draw alone (an
//! effect layer, an implicit layer) holds it as the draw would be drawn
//! without its effects and its blend (`Drawing::draw_alone`), so a
//! shader-painted run keeps its shader under any of them, as Skia draws a
//! draw whole into the layer its image filter runs on
//! (`AutoLayerForImageFilter`).
//!
//! A mask blur is Impeller's `CreateMaskBlur` for each kind of draw: a
//! solid colour renders as it is and blurs; a shader paint or an image
//! renders a white mask of its shape, which is blurred and then filled with
//! the paint's contents; a glyph run blurs its colours (Impeller's
//! blurred-text path).

use std::sync::Arc;

use valo_dl::{Image, Paint};
use valo_geometry::{FillRule, Matrix, Path, Rect};

use crate::pipelines::{AdvancedBlend, Blend, PipelineBlend};

use super::draw_state::DrawState;
use super::filter_tree::{DrawMaskBlur, FilterTree};
use super::layers::SourceSpace;
use super::primitives::BlendedSolid;
use super::source::{DrawSource, Shape};
use super::Planner;

/// `Route` is how one draw is drawn, decided from what it draws, its paint
/// and its transform alone. The blend is decided here too, once: on the
/// blend unit, or in a fragment against a copy of the destination.
pub(super) enum Route<'a> {
    /// One draw, as it is, blended by the blend unit.
    Direct(PipelineBlend),
    /// Its effects need its finished pixels: it is drawn plain, or as the
    /// white mask its mask blur fills, into a layer in its source space
    /// `space`, and the layer is drawn through `filter`, blended by
    /// `blend`.
    EffectLayer {
        space: SourceSpace,
        filter: FilterTree<'a>,
        mask_blur: Option<DrawMaskBlur<'a>>,
        blend: Blend,
    },
    /// An image drawn whole whose filters read its own texture, with no
    /// layer between (Impeller's `TextureContents`).
    FilteredImage {
        space: SourceSpace,
        filter: FilterTree<'a>,
        image: &'a Image,
        dst: Rect,
        blend: Blend,
    },
    /// A solid rect whose fragment blends `mode` against a copy of the
    /// destination.
    BlendedRect { rect: Rect, mode: AdvancedBlend },
    /// A solid filled path whose cover blends `mode` against a copy of the
    /// destination.
    BlendedPath {
        path: &'a Arc<Path>,
        rule: FillRule,
        mode: AdvancedBlend,
    },
    /// The draw alone in an implicit layer whose composite blends by
    /// `blend`: a destination-reading blend on anything but a solid shape,
    /// or shader-painted text, whose run and shader meet in a layer since
    /// glyph coverage lives in an atlas a paint's fragment cannot sample
    /// alongside its own colour source.
    InImplicitLayer(Blend),
    /// Nothing shows: an axis of the transform has no length.
    Nothing,
}

impl<'a> Route<'a> {
    /// `of` is the route of `source` drawn with `paint` under `transform`:
    ///
    /// - the paint carries a blur or filter that must see the draw's
    ///   FINISHED pixels → render plain into an effect layer, run the
    ///   filters, composite;
    /// - the blend mode is beyond the fixed-function blend unit → make the
    ///   destination readable and blend in the shader;
    /// - neither → one direct draw.
    ///
    /// A draw whose image filter colours transparent pixels fills its clip,
    /// so even an image drawn whole takes an effect layer then.
    pub fn of(source: &DrawSource<'a>, paint: &Paint, transform: &Matrix) -> Self {
        let blend = Blend::of(paint.blend_mode);
        if let DrawSource::RRectBlur { .. } = source {
            // Its mask blur is the fragment's.
            return Route::blended_whole(blend);
        }
        if needs_effect_layer(source, paint) {
            let Some(space) = SourceSpace::of(transform) else {
                return Route::Nothing;
            };
            if let Some(route) = Route::through_filters(source, paint, space, blend) {
                return route;
            }
        }
        match (blend, *source) {
            (Blend::ReadsDestination(mode), source) => Route::blended(&source, paint, mode),
            (Blend::Fixed(_), DrawSource::Glyphs(_)) if paint.shader.is_some() => {
                Route::InImplicitLayer(blend)
            }
            (Blend::Fixed(blend), _) => Route::Direct(blend),
        }
    }

    /// `through_filters` is the route of a draw through its paint's filter
    /// tree in source space `space`; `None` when the tree does nothing.
    fn through_filters(
        source: &DrawSource<'a>,
        paint: &Paint,
        space: SourceSpace,
        blend: Blend,
    ) -> Option<Self> {
        let mask_blur = paint
            .mask_blur
            .and_then(|blur| source.mask_blur(paint, blur));
        let filter = FilterTree::for_draw(
            paint,
            mask_blur.as_ref(),
            space.source,
            source.blur_tile_mode(),
        )?;
        let whole_image = source
            .whole_image()
            .filter(|_| mask_blur.is_none() && !paint.draw_fills_clip());
        Some(match whole_image {
            Some((image, dst)) => Route::FilteredImage {
                space,
                filter,
                image,
                dst,
                blend,
            },
            None => Route::EffectLayer {
                space,
                filter,
                mask_blur,
                blend,
            },
        })
    }

    /// `blended` is the route of a draw whose blend `mode` reads the
    /// destination: a solid rect or filled path blends in its own fragment;
    /// anything else first materializes in an implicit layer.
    fn blended(source: &DrawSource<'a>, paint: &Paint, mode: AdvancedBlend) -> Self {
        let in_layer = Route::InImplicitLayer(Blend::ReadsDestination(mode));
        match *source {
            DrawSource::Shape { shape, .. } if paint.shader.is_none() => match shape {
                Shape::Rect(rect) => Route::BlendedRect { rect, mode },
                Shape::Path { path, rule } => Route::BlendedPath { path, rule, mode },
                Shape::Stroke { .. } => in_layer,
            },
            _ => in_layer,
        }
    }

    /// `blended_whole` is the route of a draw that is drawn whole whatever
    /// its blend: directly, or in an implicit layer for a blend that reads
    /// the destination.
    fn blended_whole(blend: Blend) -> Self {
        match blend {
            Blend::Fixed(blend) => Route::Direct(blend),
            Blend::ReadsDestination(_) => Route::InImplicitLayer(blend),
        }
    }
}

impl Planner<'_> {
    /// `plan_routed` draws one draw request the way its [`Route`] says,
    /// counting it as drawn or, when nothing of it shows, culled. A colour
    /// filter is cheaper than a layer whenever it can fold: into a solid's
    /// colour or a gradient's stops on the CPU, or into an image draw's
    /// sampling fragment.
    pub(super) fn plan_routed(
        &mut self,
        mut source: DrawSource<'_>,
        paint: &Paint,
        at: &DrawState,
    ) {
        let prepared = self.prepare_paint(&mut source, paint);
        let paint = prepared.as_ref().unwrap_or(paint);
        let shown = match Route::of(&source, paint, &at.transform) {
            Route::Direct(blend) => {
                self.drawing().draw(source, paint, at, blend);
                Some(())
            }
            Route::EffectLayer {
                space,
                filter,
                mask_blur,
                blend,
            } => {
                let layer = EffectLayer {
                    space: &space,
                    filter: &filter,
                    mask_blur: mask_blur.as_ref(),
                    blend,
                };
                self.plan_effect_layer(source, paint, at, layer)
            }
            Route::FilteredImage {
                space,
                filter,
                image,
                dst,
                blend,
            } => self.plan_filtered_image(image, &dst, paint, &filter, &space, at, blend),
            Route::BlendedRect { rect, mode } => {
                self.plan_blended_solid(BlendedSolid::Rect(rect), paint, at, mode);
                Some(())
            }
            Route::BlendedPath { path, rule, mode } => {
                self.plan_blended_path(path, rule, paint, at, mode)
            }
            Route::InImplicitLayer(blend) => self.plan_in_implicit_layer(source, paint, at, blend),
            Route::Nothing => None,
        };
        match shown {
            Some(()) => self.plan.stats.draws += 1,
            None => self.plan.stats.culled += 1,
        }
    }

    /// `prepare_paint` is `paint` with its colour filter taken in by
    /// `source`, when `source` takes one before routing: folded into a
    /// solid's colour or a gradient's stops, baked into a pattern's image,
    /// or handed to a solid glyph run, whose glyphs are coloured through it
    /// (a coverage glyph's colour folds it, a colour glyph's pixels run it
    /// in their fragment). An image applies its own to the SAMPLED pixel in
    /// the fragment, not to the (alpha-only) paint colour.
    fn prepare_paint(&mut self, source: &mut DrawSource<'_>, paint: &Paint) -> Option<Paint> {
        match source {
            DrawSource::Image { .. } => None,
            DrawSource::Glyphs(run) if paint.shader.is_none() => {
                run.colour_filter = paint.color_filter;
                run.colour_filter.map(|_| Paint {
                    color_filter: None,
                    ..paint.clone()
                })
            }
            _ => folded_paint(paint).or_else(|| self.bake_pattern_colour_filter(paint)),
        }
    }

    /// `plan_effect_layer` renders the draw into its own layer over its
    /// ink, in the draw's source space — plain, or as the white mask its
    /// mask blur fills — then the composite runs the layer's filter over
    /// that texture. A draw whose image filter colours transparent pixels
    /// fills its clip, so its layer covers the target.
    fn plan_effect_layer(
        &mut self,
        source: DrawSource<'_>,
        paint: &Paint,
        at: &DrawState,
        layer: EffectLayer<'_, '_>,
    ) -> Option<()> {
        let content_bounds = (!paint.draw_fills_clip()).then(|| source.local_bounds());
        let (content, content_paint) = source.in_effect_layer(paint, layer.mask_blur);
        let picture = self.render_effect_layer(
            content,
            &content_paint,
            content_bounds.as_ref(),
            layer.filter,
            layer.space,
        )?;
        let composite = at.with_transform(layer.space.remainder);
        self.composite_picture(picture, Some(layer.filter), layer.blend, &composite);
        Some(())
    }

    /// `plan_blended_solid` lowers a solid shape's destination-reading
    /// blend: it copies the destination and blends in one fragment.
    fn plan_blended_solid(
        &mut self,
        solid: BlendedSolid,
        paint: &Paint,
        at: &DrawState,
        mode: AdvancedBlend,
    ) {
        let destination = self.split_for_copy(at.z);
        self.drawing()
            .draw_blended_solid(solid, paint, at, mode, &destination.view);
    }

    /// `plan_blended_path` lowers a solid filled path's destination-reading
    /// blend: stencil-then-cover, with the cover doing the blend against a
    /// snapshot of the destination; `None` when the path has no area.
    fn plan_blended_path(
        &mut self,
        path: &Arc<Path>,
        rule: FillRule,
        paint: &Paint,
        at: &DrawState,
        mode: AdvancedBlend,
    ) -> Option<()> {
        let fan = self.drawing().fan_mesh(path, &at.transform)?;
        let solid = BlendedSolid::Path {
            bounds: path.bounds(),
            fan,
            rule,
        };
        self.plan_blended_solid(solid, paint, at, mode);
        Some(())
    }

    /// `plan_in_implicit_layer` draws `source` alone into an implicit
    /// layer whose composite blends by `blend`.
    fn plan_in_implicit_layer(
        &mut self,
        source: DrawSource<'_>,
        paint: &Paint,
        at: &DrawState,
        blend: Blend,
    ) -> Option<()> {
        let device_bounds = source.device_bounds(&at.transform);
        self.plan_via_implicit_layer(device_bounds, at, blend, |drawing, inner| {
            drawing.draw_alone(source, paint, inner);
        })
    }
}

/// `EffectLayer` is what an effect layer's route decided: the source space
/// it is drawn in, the filter over it, the draw's mask blur, and the
/// composite's blend.
struct EffectLayer<'r, 'a> {
    space: &'r SourceSpace,
    filter: &'r FilterTree<'a>,
    mask_blur: Option<&'r DrawMaskBlur<'a>>,
    blend: Blend,
}

/// `needs_effect_layer` decides whether the paint's remaining effects need
/// the draw's finished pixels. Images are the exception: their colour
/// filter runs inline on the sampled pixel, so only blur-family effects
/// force a layer.
fn needs_effect_layer(source: &DrawSource<'_>, paint: &Paint) -> bool {
    let blur_family = paint.mask_blur.is_some() || paint.effective_image_filter().is_some();
    match source {
        DrawSource::Image { .. } => blur_family,
        _ => blur_family || paint.color_filter.is_some(),
    }
}

/// `folded_paint` absorbs a colour filter on the CPU, matching Impeller's
/// `Contents::ApplyColorFilter`: a gradient folds it into its stops, a
/// solid into the colour itself. Image patterns return `None` and become
/// cached filtered-source textures instead.
fn folded_paint(paint: &Paint) -> Option<Paint> {
    let filter = paint.color_filter?;
    let mut folded = paint.clone();
    match &mut folded.shader {
        Some(shader) => {
            if !shader.fold_color_filter(&filter) {
                return None;
            }
        }
        None => folded.color = filter.folded_into(paint.color),
    }
    folded.color_filter = None;
    Some(folded)
}

/// The route of each kind of draw, decided without a GPU.
#[cfg(test)]
mod tests {
    use super::*;
    use valo_dl::{BlendMode, ImageFilter, MaskBlur, Shader};
    use valo_geometry::{Color, PathBuilder, Point, Stroke};

    const RECT: Rect = Rect {
        x: 10.0,
        y: 10.0,
        width: 20.0,
        height: 20.0,
    };

    fn rect() -> DrawSource<'static> {
        DrawSource::Shape {
            shape: Shape::Rect(RECT),
            ink: RECT,
        }
    }

    fn triangle() -> Arc<Path> {
        let mut path = PathBuilder::new();
        path.move_to((0.0, 0.0))
            .line_to((10.0, 0.0))
            .line_to((0.0, 10.0))
            .close();
        path.build()
    }

    fn multiply() -> Paint {
        Paint {
            blend_mode: BlendMode::Multiply,
            ..Paint::default()
        }
    }

    fn blurred() -> Paint {
        Paint {
            image_filter: Some(ImageFilter::blur(2.0, 2.0)),
            ..Paint::default()
        }
    }

    fn route_name(route: Route<'_>) -> &'static str {
        match route {
            Route::Direct(_) => "direct",
            Route::EffectLayer { .. } => "effect layer",
            Route::FilteredImage { .. } => "filtered image",
            Route::BlendedRect { .. } => "blended rect",
            Route::BlendedPath { .. } => "blended path",
            Route::InImplicitLayer(_) => "in implicit layer",
            Route::Nothing => "nothing",
        }
    }

    fn route(source: &DrawSource<'_>, paint: &Paint) -> &'static str {
        route_name(Route::of(source, paint, &Matrix::IDENTITY))
    }

    #[test]
    fn a_plain_draw_is_direct_and_one_with_filters_takes_a_layer() {
        assert_eq!(route(&rect(), &Paint::default()), "direct");
        assert_eq!(route(&rect(), &blurred()), "effect layer");
        let masked = Paint {
            mask_blur: Some(MaskBlur::new(3.0)),
            ..Paint::default()
        };
        assert_eq!(route(&rect(), &masked), "effect layer");
    }

    /// A filter that changes nothing needs no layer.
    #[test]
    fn a_draw_whose_filters_do_nothing_is_direct() {
        let nothing = Paint {
            image_filter: Some(ImageFilter::blur(0.0, 0.0)),
            ..Paint::default()
        };
        assert_eq!(route(&rect(), &nothing), "direct");
    }

    /// Source space needs both axes: a draw squashed flat shows nothing.
    #[test]
    fn a_filtered_draw_under_a_flat_transform_shows_nothing() {
        let flat = Matrix::scale(1.0, 0.0);
        assert_eq!(route_name(Route::of(&rect(), &blurred(), &flat)), "nothing");
    }

    #[test]
    fn a_solid_shape_blends_in_its_own_fragment_and_anything_else_in_a_layer() {
        let path = triangle();
        let filled = DrawSource::Shape {
            shape: Shape::Path {
                path: &path,
                rule: FillRule::NonZero,
            },
            ink: path.bounds(),
        };
        let stroke = Stroke::new(2.0);
        let stroked = DrawSource::Shape {
            shape: Shape::Stroke {
                path: &path,
                stroke: &stroke,
            },
            ink: path.bounds(),
        };
        let gradient = Paint {
            shader: Some(Shader::linear(
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Color::BLACK,
                Color::WHITE,
            )),
            ..multiply()
        };
        assert_eq!(route(&rect(), &multiply()), "blended rect");
        assert_eq!(route(&filled, &multiply()), "blended path");
        assert_eq!(route(&stroked, &multiply()), "in implicit layer");
        assert_eq!(route(&rect(), &gradient), "in implicit layer");
        let rrect_blur = DrawSource::RRectBlur {
            rect: RECT,
            radii: [2.0; 4],
            blur: MaskBlur::new(2.0),
        };
        assert_eq!(route(&rrect_blur, &multiply()), "in implicit layer");
        assert_eq!(route(&rrect_blur, &Paint::default()), "direct");
    }

    /// Effects come before the blend: a blurred Multiply rect blends at its
    /// layer's composite.
    #[test]
    fn a_draw_with_filters_and_a_destination_reading_blend_takes_its_effect_layer() {
        let paint = Paint {
            blend_mode: BlendMode::Multiply,
            ..blurred()
        };
        assert_eq!(route(&rect(), &paint), "effect layer");
    }
}
