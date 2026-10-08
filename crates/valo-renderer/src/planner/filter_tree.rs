//! The filter tree: Impeller's `FilterContents` over its `FilterInput`s. A
//! paint's effects become one tree in the order Impeller builds it — for a
//! save layer, the image filter under the colour filter
//! (`Paint::WithFiltersForSubpassTarget`); for a draw, the colour filter
//! under the image filter (`Paint::WithFilters`) — and a composed image
//! filter nests, its inner filter under its outer one (`WrapInput`'s
//! `kCompose`). The order of effects is the shape of the tree, not a recipe
//! beside it.
//!
//! A draw's mask blur is Impeller's `CreateMaskBlur`: a solid colour or a
//! glyph run blurs as it is drawn; a shader paint or an image blurs a white
//! mask of its shape and fills it `SrcIn` with its contents, the colour
//! filter on them — the fill is the blend's second input, as in Impeller's
//! `MakeBlend(kSrcIn, {mask, contents})`. Its node sits on the draw, under
//! the image filter, for every kind of draw. That is a departure: Impeller's
//! three draw paths disagree and each drops an effect (geometry ignores the
//! image filter after a mask blur, text ignores the mask blur beside a
//! colour or image filter), so valo takes its image path's order, which
//! drops nothing. A save layer's mask blur is dropped when it is recorded,
//! as Flutter's display list drops it.

use valo_dl::{ColorFilter, Image, ImageFilter, MaskBlur, Paint, Sampling, TileMode};
use valo_geometry::{Color, Matrix, Point, Rect};

use super::gaussian::{
    basis_of, blur_radius, blur_source_coverage, expanded, scale_sigma, skia_sigma, BlurInfo,
};
use super::layer_coverage::SourceCoverage;
use super::source::{fill_paint, DrawSource, Shape};

/// `FilterInput` is what a filter reads: Impeller's `FilterInput`.
///
/// Every tree has one leaf, [`FilterInput::Source`], and the texture it
/// stands for is handed over when the tree renders. So a tree is plain data,
/// built from a paint, and a draw's mask blur, wherever the planner needs it.
pub(super) enum FilterInput<'a> {
    /// What the tree filters: a layer's texture, or a copy of the parent
    /// beneath a backdrop (Impeller's `TextureFilterInput`).
    Source,
    /// Another filter's result (Impeller's `FilterContentsFilterInput`).
    Filter(Box<FilterNode<'a>>),
    /// `CreateMaskBlur`'s blend: the paint's contents, `fill`, drawn `SrcIn`
    /// over `mask`, the blurred white mask of the draw's shape (Impeller's
    /// `MakeBlend(kSrcIn, {mask, contents})`).
    FillIn {
        mask: Box<FilterInput<'a>>,
        fill: Box<Fill<'a>>,
    },
}

/// `FilterNode` is one filter and what it reads: Impeller's
/// `FilterContents` with its one input.
pub(super) struct FilterNode<'a> {
    pub filter: Filter<'a>,
    pub input: FilterInput<'a>,
}

/// `Filter` is what one node does to its input.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Filter<'a> {
    /// An image filter's Gaussian blur, σ in local units
    /// (Impeller's `GaussianBlurFilterContents`).
    Blur {
        sigma_x: f32,
        sigma_y: f32,
        /// What the blur reads, a rectangle in local units: see
        /// [`ImageFilter::Blur`].
        bounds: Option<Rect>,
        /// How the blur reads past the edge of its input, resolved from
        /// where the filter is used when the paint left it unspecified.
        tile_mode: TileMode,
    },
    /// A draw's mask blur with its style, σ in local units: Impeller's
    /// `GaussianBlurFilterContents` as `CreateMaskBlur` makes it, decal.
    MaskBlur {
        blur: MaskBlur,
        /// What a solid, inner or outer style keeps the blur to.
        style_shape: StyleShape<'a>,
    },
    /// A colour filter (Impeller's `ColorFilterContents`).
    Color(ColorFilter),
    /// Skia's drop shadow: the input over a blurred copy of its alpha in
    /// `color`, moved by `offset`.
    DropShadow {
        offset: Point,
        sigma_x: f32,
        sigma_y: f32,
        color: Color,
    },
}

/// `StyleShape` is what a mask blur's style keeps the blur to: the draw's
/// shape, which an inner or outer style clips the blur to (Impeller's
/// `ApplyClippedBlurStyle`), or a glyph run's own coverage, which the style
/// merges with in the composite's fragment, a run having no shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum StyleShape<'a> {
    Shape(Shape<'a>),
    GlyphRun,
}

/// `Fill` is what fills a draw's blurred mask `SrcIn`: the paint's
/// contents, which keep their own colours inside the soft edge, with the
/// paint's colour filter on them.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Fill<'a> {
    /// The paint's shader at its colour's alpha over `rect`, local
    /// coordinates.
    Shader { rect: Rect, paint: Paint },
    /// The image drawn from `src` over `dst` with `paint`'s alpha and colour
    /// filter.
    Image {
        image: &'a Image,
        src: Rect,
        dst: Rect,
        sampling: Sampling,
        paint: Paint,
    },
}

impl<'a> Fill<'a> {
    /// `shader` is `CreateMaskBlur`'s fill for a shader paint: the shader
    /// over the blurred mask's coverage, the draw's `local_bounds` grown by
    /// the blur's padding as an untransformed blur grows them
    /// (`expanded_local_bounds`).
    pub fn shader(paint: &Paint, local_bounds: &Rect, blur: MaskBlur) -> Self {
        let padding = blur_of_mask_blur(blur.sigma, &Matrix::IDENTITY).local_padding();
        Fill::Shader {
            paint: fill_paint(paint),
            rect: expanded(local_bounds, padding),
        }
    }

    /// `image` is `CreateMaskBlur`'s fill for an image: the image's source
    /// and destination each grown by the blur's radius, so the image's edge,
    /// read past as its sampler says, reaches into the halo.
    pub fn image(
        image: &'a Image,
        src: &Rect,
        dst: &Rect,
        sampling: Sampling,
        paint: &Paint,
        blur: MaskBlur,
    ) -> Self {
        let radius = blur_radius(scale_sigma(blur.sigma));
        Fill::Image {
            image,
            src: src.expand(radius),
            dst: dst.expand(radius),
            sampling,
            paint: fill_paint(paint),
        }
    }

    /// `local_bounds` is what the fill covers, local coordinates.
    pub fn local_bounds(&self) -> Rect {
        match self {
            Fill::Shader { rect, .. } => *rect,
            Fill::Image { dst, .. } => *dst,
        }
    }

    /// `as_draw` is the fill as a draw and the paint it is drawn with.
    pub fn as_draw(&self) -> (DrawSource<'a>, &Paint) {
        match self {
            Fill::Shader { rect, paint } => {
                let shape = Shape::Rect(*rect);
                (DrawSource::Shape { shape, ink: *rect }, paint)
            }
            Fill::Image {
                image,
                src,
                dst,
                sampling,
                paint,
            } => {
                let image = DrawSource::Image {
                    image,
                    src: *src,
                    dst: *dst,
                    sampling: *sampling,
                };
                (image, paint)
            }
        }
    }
}

/// `DrawMaskBlur` is a draw's mask blur and what it needs of the draw
/// (Impeller's `CreateMaskBlur`).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DrawMaskBlur<'a> {
    pub blur: MaskBlur,
    pub masked: MaskedDraw<'a>,
}

/// `MaskedDraw` is how a draw makes the mask its blur blurs.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum MaskedDraw<'a> {
    /// A solid shape blurs its own colours, its colour filter taken first;
    /// a style keeps the blur to the shape.
    Colours(Shape<'a>),
    /// A shader paint or an image blurs a white mask of its shape, which a
    /// style keeps the blur to, and its contents fill the blurred mask.
    Filled {
        shape: Shape<'a>,
        fill: Box<Fill<'a>>,
    },
    /// A glyph run blurs its own colours, its colour filter taken first; a
    /// style merges the blur with the sharp run.
    GlyphRun,
}

/// `FilterTree` is what a paint does to a picture as it is drawn: the
/// paint's filters as a tree over the picture (a save layer's texture, a
/// draw's effect layer, an image handed through), and the transform the
/// filters' parameters are in.
pub(super) struct FilterTree<'a> {
    /// The tree's last node; the picture is its one leaf.
    pub root: FilterInput<'a>,
    /// Maps the paint's local coordinates into the tree's space: the
    /// transform where the paint applies (Impeller's effect transform). A
    /// blur's σ comes from its 2×2 basis, used whole rather than as two axis
    /// lengths: σ is a VECTOR, and under rotation its axes move with the
    /// matrix.
    pub effect_transform: Matrix,
}

impl<'a> FilterTree<'a> {
    /// `coverage` is what the filtered picture covers, in the tree's space,
    /// when the picture covers `source` (Impeller's `GetCoverage`).
    pub fn coverage(&self, source: &Rect) -> Rect {
        self.root.coverage(source, &self.effect_transform)
    }

    /// `for_draw` is the filters of a draw, in a draw's order, with the
    /// draw's mask blur; an unspecified blur tile mode is `tile_mode`, which
    /// dart:ui picks by the draw (clamp for an image, decal for the rest).
    /// `None` when the paint does nothing to its pixels.
    pub fn for_draw(
        paint: &Paint,
        mask_blur: Option<&DrawMaskBlur<'a>>,
        effect_transform: Matrix,
        tile_mode: TileMode,
    ) -> Option<Self> {
        Self::of(
            FilterInput::Source.with_filters(paint, mask_blur, tile_mode),
            effect_transform,
        )
    }

    /// `for_save_layer` is the filters of a recorded save layer, in a save
    /// layer's order. `None` when the paint does nothing to the layer.
    pub fn for_save_layer(paint: &Paint, effect_transform: Matrix) -> Option<Self> {
        Self::of(
            FilterInput::Source.with_filters_for_subpass_target(paint),
            effect_transform,
        )
    }

    fn of(root: FilterInput<'a>, effect_transform: Matrix) -> Option<Self> {
        match root {
            FilterInput::Source => None,
            root => Some(Self {
                root,
                effect_transform,
            }),
        }
    }
}

impl FilterTree<'_> {
    /// `source_coverage` is what the picture has to cover for the tree to
    /// produce everything inside `output_limit`, in the tree's space.
    pub fn source_coverage(&self, output_limit: &Rect) -> Rect {
        self.root
            .source_coverage(&self.effect_transform, output_limit)
    }

    /// `reads_past_its_edge` reports whether a blur in the tree reads past
    /// the edge of the picture it filters as something other than
    /// transparent: its resolved tile mode clamps, mirrors or repeats. Such
    /// a blur's result depends on where that edge is.
    pub fn reads_past_its_edge(&self) -> bool {
        self.root.reads_past_its_edge()
    }
}

impl SourceCoverage for FilterTree<'_> {
    fn source_coverage(&self, output_limit: &Rect) -> Option<Rect> {
        Some(FilterTree::source_coverage(self, output_limit))
    }
}

impl<'a> FilterInput<'a> {
    /// `reads_past_its_edge` is [`FilterTree::reads_past_its_edge`] for the
    /// tree this input roots.
    fn reads_past_its_edge(&self) -> bool {
        match self {
            FilterInput::Source => false,
            FilterInput::Filter(node) => {
                let blur_tiles = matches!(
                    node.filter,
                    Filter::Blur { tile_mode, .. } if tile_mode != TileMode::Decal
                );
                blur_tiles || node.input.reads_past_its_edge()
            }
            FilterInput::FillIn { mask, .. } => mask.reads_past_its_edge(),
        }
    }

    /// `coverage` is Impeller's `FilterContents::GetCoverage`: what the tree's
    /// output covers when its source covers `source`, each filter growing
    /// what the one beneath it covers. A fill covers its mask and itself, as
    /// a blend covers all of its inputs.
    pub fn coverage(&self, source: &Rect, effect_transform: &Matrix) -> Rect {
        match self {
            FilterInput::Source => *source,
            FilterInput::Filter(node) => node.filter.coverage(
                &node.input.coverage(source, effect_transform),
                effect_transform,
            ),
            FilterInput::FillIn { mask, fill } => mask
                .coverage(source, effect_transform)
                .union(&effect_transform.map_rect(&fill.local_bounds())),
        }
    }

    /// `source_coverage` is Impeller's `FilterContents::GetSourceCoverage`:
    /// what the source has to cover for the tree to produce everything
    /// inside `output_limit`, each filter asking of the one beneath it.
    /// `None` when no source can produce it.
    pub fn source_coverage(&self, effect_transform: &Matrix, output_limit: &Rect) -> Rect {
        match self {
            FilterInput::Source => *output_limit,
            FilterInput::Filter(node) => {
                let input_limit = node.filter.source_coverage(effect_transform, output_limit);
                node.input.source_coverage(effect_transform, &input_limit)
            }
            FilterInput::FillIn { mask, .. } => {
                mask.source_coverage(effect_transform, output_limit)
            }
        }
    }

    /// `with_filters` is a draw's order, one for every kind of draw: the
    /// draw's mask blur with the colour filter on what it fills, as
    /// `CreateMaskBlur` puts it, then the image filter over the result
    /// (Impeller's `Paint::WithFilters` after the mask blur, as its image
    /// path runs them). `tile_mode` is an unspecified blur's.
    pub fn with_filters(
        self,
        paint: &Paint,
        mask_blur: Option<&DrawMaskBlur<'a>>,
        tile_mode: TileMode,
    ) -> Self {
        let drawn = match mask_blur {
            Some(mask_blur) => self.with_mask_blur(paint.color_filter, mask_blur),
            None => self.with_color_filter(paint.color_filter),
        };
        drawn.with_image_filter(paint.effective_image_filter(), tile_mode)
    }

    /// `with_filters_for_subpass_target` is Impeller's
    /// `Paint::WithFiltersForSubpassTarget`, a save layer's order: the image
    /// filter, then the colour filter, so a translating or clamping colour
    /// matrix acts on the halo's fractional alpha the way Flutter's does. An
    /// unspecified blur on a layer is decal, as dart:ui gives `saveLayer` and
    /// `pushImageFilter`.
    pub fn with_filters_for_subpass_target(self, paint: &Paint) -> Self {
        self.with_image_filter(paint.effective_image_filter(), TileMode::Decal)
            .with_color_filter(paint.color_filter)
    }

    /// `with_mask_blur` is the draw under its mask blur (`CreateMaskBlur`):
    /// a draw that fills its blurred mask carries its colour filter in the
    /// fill; one that blurs its own colours takes it before the blur.
    fn with_mask_blur(
        self,
        color_filter: Option<ColorFilter>,
        mask_blur: &DrawMaskBlur<'a>,
    ) -> Self {
        let blur = |style_shape| Filter::MaskBlur {
            blur: mask_blur.blur,
            style_shape,
        };
        match &mask_blur.masked {
            MaskedDraw::Colours(shape) => self
                .with_color_filter(color_filter)
                .with(blur(StyleShape::Shape(*shape))),
            MaskedDraw::GlyphRun => self
                .with_color_filter(color_filter)
                .with(blur(StyleShape::GlyphRun)),
            MaskedDraw::Filled { shape, fill } => FilterInput::FillIn {
                mask: Box::new(self.with(blur(StyleShape::Shape(*shape)))),
                fill: fill.clone(),
            },
        }
    }

    /// `with_image_filter` is Impeller's `WrapInput`: the image filter over
    /// this input, a composition's inner filter first. A blur that left its
    /// tile mode unspecified takes `tile_mode`, except inside a composition,
    /// which dart:ui resolves to clamp on both sides
    /// (`ImageFilter::initComposeFilter`).
    pub fn with_image_filter(self, filter: Option<&ImageFilter>, tile_mode: TileMode) -> Self {
        let Some(filter) = filter else {
            return self;
        };
        match filter {
            ImageFilter::Blur {
                sigma_x,
                sigma_y,
                bounds,
                tile_mode: specified,
            } => self.with(Filter::Blur {
                sigma_x: *sigma_x,
                sigma_y: *sigma_y,
                bounds: *bounds,
                tile_mode: specified.unwrap_or(tile_mode),
            }),
            ImageFilter::Color(color_filter) => self.with(Filter::Color(*color_filter)),
            ImageFilter::DropShadow {
                offset,
                sigma_x,
                sigma_y,
                color,
            } => self.with(Filter::DropShadow {
                offset: *offset,
                sigma_x: *sigma_x,
                sigma_y: *sigma_y,
                color: *color,
            }),
            ImageFilter::Compose { outer, inner } => self
                .with_image_filter(Some(inner), TileMode::Clamp)
                .with_image_filter(Some(outer), TileMode::Clamp),
        }
    }

    fn with_color_filter(self, filter: Option<ColorFilter>) -> Self {
        match filter {
            Some(filter) => self.with(Filter::Color(filter)),
            None => self,
        }
    }

    fn with(self, filter: Filter<'a>) -> Self {
        FilterInput::Filter(Box::new(FilterNode {
            filter,
            input: self,
        }))
    }
}

impl Filter<'_> {
    /// `blur` is the Gaussian blur this filter runs under
    /// `effect_transform`; `None` for a filter that does not blur.
    pub fn blur(&self, effect_transform: &Matrix) -> Option<BlurInfo> {
        match self {
            Filter::Blur {
                sigma_x, sigma_y, ..
            } => Some(blur_of_image_filter([*sigma_x, *sigma_y], effect_transform)),
            Filter::MaskBlur { blur, .. } => Some(blur_of_mask_blur(blur.sigma, effect_transform)),
            Filter::DropShadow {
                sigma_x, sigma_y, ..
            } => Some(blur_of_drop_shadow([*sigma_x, *sigma_y], effect_transform)),
            Filter::Color(_) => None,
        }
    }

    /// `input_hint` is the coverage hint this filter hands its input when
    /// its own is `hint`: a blur's grown by its padding, as Impeller's blurs
    /// hand theirs on; none from a colour filter or a drop shadow.
    pub fn input_hint(&self, hint: Option<Rect>, effect_transform: &Matrix) -> Option<Rect> {
        match self {
            Filter::Blur { .. } | Filter::MaskBlur { .. } => {
                let blur = self.blur(effect_transform)?;
                Some(expanded(&hint?, blur.local_padding()))
            }
            Filter::Color(_) | Filter::DropShadow { .. } => None,
        }
    }

    /// `coverage` is this filter's `GetFilterCoverage`: what it covers when
    /// its input covers `input`. A blur grows its input by its halo; a drop
    /// shadow covers its input and the halo moved by its offset.
    fn coverage(&self, input: &Rect, effect_transform: &Matrix) -> Rect {
        let Some(blur) = self.blur(effect_transform) else {
            return *input;
        };
        let halo = expanded(input, blur.local_padding());
        match self {
            Filter::DropShadow { offset, .. } => {
                input.union(&shifted(&halo, device_offset(effect_transform, *offset)))
            }
            _ => halo,
        }
    }

    /// `source_coverage` is this filter's `GetFilterSourceCoverage`: the
    /// input that an output limit needs. An image filter's blur needs the
    /// limit grown by its radius, as Impeller computes it; a mask blur's and
    /// a drop shadow's blur reach as far as the σ rule they run by takes
    /// them, and a drop shadow also needs the limit moved back by its offset.
    fn source_coverage(&self, effect_transform: &Matrix, output_limit: &Rect) -> Rect {
        let reach = self
            .blur(effect_transform)
            .map_or([0.0; 2], |blur| blur.radius());
        match self {
            Filter::Blur {
                sigma_x, sigma_y, ..
            } => blur_source_coverage(effect_transform, [*sigma_x, *sigma_y], output_limit),
            Filter::MaskBlur { .. } => expanded(output_limit, reach),
            Filter::Color(_) => *output_limit,
            Filter::DropShadow { offset, .. } => {
                let [x, y] = device_offset(effect_transform, *offset);
                output_limit.union(&expanded(&shifted(output_limit, [-x, -y]), reach))
            }
        }
    }
}

/// `blur_of_image_filter` is an image filter's blur of local σ under
/// `effect_transform`: Impeller's `CalculateBlurInfo` for a blur in a layer,
/// its entity of scale one and the effect transform taking σ as a vector.
fn blur_of_image_filter(sigma: [f32; 2], effect_transform: &Matrix) -> BlurInfo {
    BlurInfo::calculate(&Matrix::IDENTITY, effect_transform, sigma)
}

/// `blur_of_mask_blur` is a draw's mask blur of local σ under `transform`:
/// `CalculateBlurInfo` as Impeller runs a draw's, the transform as its
/// entity's, so σ follows each axis's length.
fn blur_of_mask_blur(sigma: f32, transform: &Matrix) -> BlurInfo {
    BlurInfo::calculate(transform, &Matrix::IDENTITY, [sigma; 2])
}

/// `blur_of_drop_shadow` is a drop shadow's blur of local σ under
/// `effect_transform`, by Skia's σ rule.
fn blur_of_drop_shadow(sigma: [f32; 2], effect_transform: &Matrix) -> BlurInfo {
    BlurInfo::of(skia_sigma(
        basis_of(effect_transform),
        scale_sigma(sigma[0]),
        scale_sigma(sigma[1]),
    ))
}

/// `device_offset` maps a local-space filter offset onto the device axes by
/// the effect transform's basis. Sign-preserving, unlike a σ — a shadow that
/// leans right has to keep leaning right after the basis mirrors or rotates
/// it.
pub(super) fn device_offset(effect_transform: &Matrix, offset: Point) -> [f32; 2] {
    let [a, b, c, d] = basis_of(effect_transform);
    [offset.x * a + offset.y * c, offset.x * b + offset.y * d]
}

/// `shifted` is `rect` moved by `offset`.
fn shifted(rect: &Rect, offset: [f32; 2]) -> Rect {
    Rect::new(
        rect.x + offset[0],
        rect.y + offset[1],
        rect.width,
        rect.height,
    )
}

#[cfg(test)]
mod tests {
    use super::super::gaussian::sigma_for_blur_radius;
    use super::*;
    use valo_dl::BlendMode;

    /// A tree of one blur, σ local and the same on both axes.
    fn blur(sigma: f32) -> FilterInput<'static> {
        FilterInput::Source
            .with_image_filter(Some(&ImageFilter::blur(sigma, sigma)), TileMode::Decal)
    }

    fn ltrb(left: f32, top: f32, right: f32, bottom: f32) -> Rect {
        Rect::from_ltrb(left, top, right, bottom)
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

    // Impeller's `gaussian_blur_filter_contents_unittests.cc`. Its entity
    // transform places the input; valo's sources come in replay
    // coordinates, so the entity's translation is in the source's coverage.

    #[test]
    fn coverage_simple() {
        let coverage = blur(0.0).coverage(&ltrb(10.0, 10.0, 110.0, 110.0), &Matrix::IDENTITY);
        assert_eq!(coverage, ltrb(10.0, 10.0, 110.0, 110.0));
    }

    #[test]
    fn coverage_with_sigma() {
        let sigma = sigma_for_blur_radius(1.0, &Matrix::IDENTITY);
        let coverage = blur(sigma).coverage(&ltrb(100.0, 100.0, 200.0, 200.0), &Matrix::IDENTITY);
        assert_rect_near(coverage, ltrb(99.0, 99.0, 201.0, 201.0));
    }

    /// A 100×100 texture under an entity that moves it to (100, 100).
    #[test]
    fn coverage_with_texture() {
        let sigma = sigma_for_blur_radius(1.0, &Matrix::IDENTITY);
        let texture = Matrix::translation(100.0, 100.0).map_rect(&ltrb(0.0, 0.0, 100.0, 100.0));
        let coverage = blur(sigma).coverage(&texture, &Matrix::IDENTITY);
        assert_rect_near(coverage, ltrb(99.0, 99.0, 201.0, 201.0));
    }

    #[test]
    fn coverage_with_effect_transform() {
        let effect_transform = Matrix::scale(2.0, 2.0);
        let sigma = sigma_for_blur_radius(1.0, &effect_transform);
        let texture = Matrix::translation(100.0, 100.0).map_rect(&ltrb(0.0, 0.0, 100.0, 100.0));
        let coverage = blur(sigma).coverage(&texture, &effect_transform);
        assert_rect_near(coverage, ltrb(99.0, 99.0, 201.0, 201.0));
    }

    #[test]
    fn filter_source_coverage() {
        let sigma = sigma_for_blur_radius(1.0, &Matrix::IDENTITY);
        let coverage = blur(sigma)
            .source_coverage(&Matrix::scale(2.0, 2.0), &ltrb(100.0, 100.0, 200.0, 200.0));
        assert_rect_near(coverage, ltrb(98.0, 98.0, 202.0, 202.0));
    }

    /// A negative scale still grows the limit.
    #[test]
    fn filter_source_coverage_negative_scale() {
        let sigma = sigma_for_blur_radius(1.0, &Matrix::IDENTITY);
        let coverage = blur(sigma)
            .source_coverage(&Matrix::scale(-2.0, 2.0), &ltrb(100.0, 100.0, 200.0, 200.0));
        assert_rect_near(coverage, ltrb(98.0, 98.0, 202.0, 202.0));
    }

    /// Not one of Impeller's: what a colour filter over a blur covers and
    /// needs is the blur's, since a colour filter covers its input.
    #[test]
    fn a_colour_filter_covers_and_needs_what_its_input_does() {
        let sigma = sigma_for_blur_radius(1.0, &Matrix::IDENTITY);
        let tree = blur(sigma).with_color_filter(Some(TINT));
        let source = ltrb(100.0, 100.0, 200.0, 200.0);
        assert_rect_near(
            tree.coverage(&source, &Matrix::IDENTITY),
            ltrb(99.0, 99.0, 201.0, 201.0),
        );
        let needed = tree.source_coverage(&Matrix::IDENTITY, &source);
        assert_rect_near(needed, ltrb(99.0, 99.0, 201.0, 201.0));
    }

    /// Not one of Impeller's: a drop shadow covers its input and its halo
    /// moved by the offset, and needs the limit moved back.
    #[test]
    fn a_drop_shadow_covers_its_input_and_its_moved_halo() {
        let shadow = FilterInput::Source.with_image_filter(
            Some(&ImageFilter::drop_shadow(
                Point::new(10.0, 0.0),
                0.0,
                0.0,
                Color::BLACK,
            )),
            TileMode::Decal,
        );
        let source = ltrb(0.0, 0.0, 20.0, 20.0);
        assert_eq!(
            shadow.coverage(&source, &Matrix::scale(2.0, 2.0)),
            ltrb(0.0, 0.0, 40.0, 20.0),
            "the offset is 20 device pixels"
        );
        assert_eq!(
            shadow.source_coverage(&Matrix::IDENTITY, &source),
            ltrb(-10.0, 0.0, 20.0, 20.0)
        );
    }

    /// The filters of a tree, from the one nearest the source outward,
    /// through a fill's mask.
    fn filters_from_the_input_out<'a>(tree: &FilterInput<'a>) -> Vec<Filter<'a>> {
        match tree {
            FilterInput::Source => Vec::new(),
            FilterInput::Filter(node) => {
                let mut filters = filters_from_the_input_out(&node.input);
                filters.push(node.filter.clone());
                filters
            }
            FilterInput::FillIn { mask, .. } => filters_from_the_input_out(mask),
        }
    }

    const TINT: ColorFilter = ColorFilter::Blend(Color::WHITE, BlendMode::Modulate);

    #[test]
    fn a_draw_filters_colour_under_the_image_filter() {
        let paint = Paint {
            color_filter: Some(TINT),
            image_filter: Some(ImageFilter::blur(2.0, 3.0)),
            ..Paint::default()
        };
        let tree = FilterInput::Source.with_filters(&paint, None, TileMode::Decal);
        assert_eq!(
            filters_from_the_input_out(&tree),
            [
                Filter::Color(TINT),
                Filter::Blur {
                    sigma_x: 2.0,
                    sigma_y: 3.0,
                    bounds: None,
                    tile_mode: TileMode::Decal,
                },
            ]
        );
    }

    #[test]
    fn a_save_layer_filters_colour_over_the_image_filter() {
        let paint = Paint {
            color_filter: Some(TINT),
            image_filter: Some(ImageFilter::blur(2.0, 3.0)),
            ..Paint::default()
        };
        let tree = FilterInput::Source.with_filters_for_subpass_target(&paint);
        assert_eq!(
            filters_from_the_input_out(&tree),
            [
                Filter::Blur {
                    sigma_x: 2.0,
                    sigma_y: 3.0,
                    bounds: None,
                    tile_mode: TileMode::Decal,
                },
                Filter::Color(TINT),
            ]
        );
    }

    /// Two blurs composed under a 45° rotation stay two filters, each taking
    /// its own σ onto the device axes (Impeller's per-filter
    /// `CalculateBlurInfo`), so together they blur both device axes alike.
    /// Combining their σ in local space first and rotating the total would
    /// collapse x to nothing and pile everything onto y.
    #[test]
    fn composed_blurs_transform_before_they_combine() {
        let rotation = Matrix::rotation(std::f32::consts::FRAC_PI_4);
        let composed =
            ImageFilter::compose(ImageFilter::blur(0.0, 10.0), ImageFilter::blur(10.0, 0.0));
        let tree = FilterInput::Source.with_image_filter(Some(&composed), TileMode::Decal);
        let stages: Vec<[f32; 2]> = filters_from_the_input_out(&tree)
            .iter()
            .map(|filter| match filter {
                Filter::Blur {
                    sigma_x, sigma_y, ..
                } => blur_of_image_filter([*sigma_x, *sigma_y], &rotation).scaled_sigma,
                other => panic!("a composition of blurs holds only blurs, not {other:?}"),
            })
            .collect();
        assert_eq!(stages.len(), 2, "each blur runs on its own");
        let together = [0, 1].map(|axis| {
            stages
                .iter()
                .map(|stage| stage[axis] * stage[axis])
                .sum::<f32>()
                .sqrt()
        });
        assert!(
            (together[0] - together[1]).abs() < 1e-3 && together[0] > 5.0,
            "both device axes blur alike: {together:?}"
        );
        let combined_first = blur_of_image_filter([10.0, 10.0], &rotation).scaled_sigma;
        assert!(
            combined_first[0] < 1e-3 && combined_first[1] > together[1],
            "combining first is the wrong answer: {combined_first:?}"
        );
    }

    /// `WrapInput`'s `kCompose`: the outer filter reads what the inner one
    /// made.
    #[test]
    fn a_composition_nests_its_inner_filter_under_its_outer_one() {
        let filter = ImageFilter::compose(ImageFilter::color(TINT), ImageFilter::blur(4.0, 4.0));
        let tree = FilterInput::Source.with_image_filter(Some(&filter), TileMode::Decal);
        assert_eq!(
            filters_from_the_input_out(&tree),
            [
                Filter::Blur {
                    sigma_x: 4.0,
                    sigma_y: 4.0,
                    bounds: None,
                    tile_mode: TileMode::Clamp,
                },
                Filter::Color(TINT),
            ]
        );
    }

    /// The tile mode of a blur's node: the paint's when it gave one,
    /// otherwise where the filter is used, as dart:ui resolves it.
    fn tile_modes(tree: &FilterInput) -> Vec<TileMode> {
        filters_from_the_input_out(tree)
            .into_iter()
            .filter_map(|filter| match filter {
                Filter::Blur { tile_mode, .. } => Some(tile_mode),
                _ => None,
            })
            .collect()
    }

    fn blurred(filter: ImageFilter) -> Paint {
        Paint {
            image_filter: Some(filter),
            ..Paint::default()
        }
    }

    /// dart:ui: decal for a layer, what the canvas call picks for a draw
    /// (clamp for an image), mirror for a backdrop.
    #[test]
    fn an_unspecified_blur_takes_the_tile_mode_of_where_it_is_used() {
        let paint = blurred(ImageFilter::blur(2.0, 2.0));
        let layer = FilterInput::Source.with_filters_for_subpass_target(&paint);
        let image = FilterInput::Source.with_filters(&paint, None, TileMode::Clamp);
        let backdrop =
            FilterInput::Source.with_image_filter(paint.image_filter.as_ref(), TileMode::Mirror);
        assert_eq!(tile_modes(&layer), [TileMode::Decal]);
        assert_eq!(tile_modes(&image), [TileMode::Clamp]);
        assert_eq!(tile_modes(&backdrop), [TileMode::Mirror]);
    }

    #[test]
    fn a_given_tile_mode_is_kept_wherever_the_blur_is_used() {
        let paint = blurred(ImageFilter::blur(2.0, 2.0).with_tile_mode(TileMode::Repeat));
        let layer = FilterInput::Source.with_filters_for_subpass_target(&paint);
        assert_eq!(tile_modes(&layer), [TileMode::Repeat]);
    }

    /// dart:ui's `initComposeFilter` resolves both sides of a composition
    /// to clamp; a side that gave its own keeps it.
    #[test]
    fn a_composition_clamps_its_unspecified_blurs() {
        let paint = blurred(ImageFilter::compose(
            ImageFilter::blur(2.0, 2.0),
            ImageFilter::blur(3.0, 3.0).with_tile_mode(TileMode::Mirror),
        ));
        let layer = FilterInput::Source.with_filters_for_subpass_target(&paint);
        assert_eq!(tile_modes(&layer), [TileMode::Mirror, TileMode::Clamp]);
    }

    const SHAPE: Shape = Shape::Rect(Rect {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    });

    fn masked<'a>(paint: &Paint, masked: MaskedDraw<'a>) -> DrawMaskBlur<'a> {
        DrawMaskBlur {
            blur: paint.mask_blur.expect("a mask-blurred paint"),
            masked,
        }
    }

    const STYLED_BLUR: Filter = Filter::MaskBlur {
        blur: MaskBlur {
            sigma: 5.0,
            style: valo_dl::BlurStyle::Inner,
        },
        style_shape: StyleShape::Shape(SHAPE),
    };

    fn mask_blurred(paint: Paint) -> Paint {
        Paint {
            color_filter: Some(TINT),
            image_filter: Some(ImageFilter::blur(2.0, 2.0)),
            mask_blur: Some(MaskBlur::inner(5.0)),
            ..paint
        }
    }

    const BLUR: Filter = Filter::Blur {
        sigma_x: 2.0,
        sigma_y: 2.0,
        bounds: None,
        tile_mode: TileMode::Decal,
    };

    /// A draw that blurs its own colours takes its colour filter, then its
    /// mask blur, then its image filter over the result.
    #[test]
    fn a_draw_blurs_its_mask_under_its_image_filter() {
        let paint = mask_blurred(Paint::default());
        let mask_blur = masked(&paint, MaskedDraw::Colours(SHAPE));
        let draw = FilterInput::Source.with_filters(&paint, Some(&mask_blur), TileMode::Decal);
        assert_eq!(
            filters_from_the_input_out(&draw),
            [Filter::Color(TINT), STYLED_BLUR, BLUR]
        );
    }

    /// A glyph run's style merges with the sharp run, having no shape.
    #[test]
    fn a_glyph_runs_mask_blur_has_no_shape_to_keep_its_style_to() {
        let paint = mask_blurred(Paint::default());
        let mask_blur = masked(&paint, MaskedDraw::GlyphRun);
        let draw = FilterInput::Source.with_filters(&paint, Some(&mask_blur), TileMode::Decal);
        let blur = Filter::MaskBlur {
            blur: MaskBlur::inner(5.0),
            style_shape: StyleShape::GlyphRun,
        };
        assert_eq!(
            filters_from_the_input_out(&draw),
            [Filter::Color(TINT), blur, BLUR]
        );
    }

    /// `CreateMaskBlur` for a shader paint: the blurred mask, then the fill
    /// over it as the blend's second input, carrying the colour filter; then
    /// the image filter.
    #[test]
    fn a_filled_mask_blur_carries_its_colour_filter_in_the_fill() {
        let paint = mask_blurred(Paint::default());
        let fill = Fill::Shader {
            paint: Paint::default(),
            rect: ltrb(0.0, 0.0, 10.0, 10.0),
        };
        let mask_blur = masked(
            &paint,
            MaskedDraw::Filled {
                shape: SHAPE,
                fill: Box::new(fill.clone()),
            },
        );
        let draw = FilterInput::Source.with_filters(&paint, Some(&mask_blur), TileMode::Decal);
        let FilterInput::Filter(image_filter) = &draw else {
            panic!("the image filter is the tree's last node");
        };
        assert_eq!(image_filter.filter, BLUR);
        let FilterInput::FillIn { mask, fill: filled } = &image_filter.input else {
            panic!("the image filter reads the filled mask");
        };
        assert_eq!(**filled, fill);
        assert_eq!(filters_from_the_input_out(mask), [STYLED_BLUR]);
    }

    /// `CreateMaskBlur`'s shader fill covers the mask's bounds grown by the
    /// padding of the blur untransformed: σ 5 reaches ceil(7.65) = 8.
    #[test]
    fn a_shader_fill_covers_the_blurred_mask_untransformed() {
        let paint = Paint::from_color(Color::WHITE);
        let fill = Fill::shader(&paint, &ltrb(10.0, 10.0, 20.0, 20.0), MaskBlur::new(5.0));
        assert_eq!(fill.local_bounds(), ltrb(2.0, 2.0, 28.0, 28.0));
    }

    /// A fill covers what its mask covers and what it covers itself, in the
    /// tree's space, and needs of its mask what its output needs.
    #[test]
    fn a_fill_covers_its_mask_and_itself() {
        let fill = FilterInput::FillIn {
            mask: Box::new(FilterInput::Source),
            fill: Box::new(Fill::Shader {
                paint: Paint::default(),
                rect: ltrb(0.0, 0.0, 10.0, 10.0),
            }),
        };
        let effect_transform = Matrix::scale(2.0, 2.0);
        assert_eq!(
            fill.coverage(&ltrb(5.0, 5.0, 30.0, 15.0), &effect_transform),
            ltrb(0.0, 0.0, 30.0, 20.0)
        );
        let limit = ltrb(1.0, 2.0, 3.0, 4.0);
        assert_eq!(fill.source_coverage(&effect_transform, &limit), limit);
    }

    /// A save layer has no mask blur: what its paint does is its image
    /// filter and colour filter.
    #[test]
    fn a_save_layer_filters_without_a_mask_blur() {
        let paint = mask_blurred(Paint::default());
        let layer = FilterInput::Source.with_filters_for_subpass_target(&paint);
        assert_eq!(
            filters_from_the_input_out(&layer),
            [BLUR, Filter::Color(TINT)]
        );
    }
}
