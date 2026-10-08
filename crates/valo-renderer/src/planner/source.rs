//! What a draw draws: its [`Shape`] (Impeller's `Geometry`) or another
//! source — an image, the analytic blurred rrect, a glyph run — and the
//! paints its parts are drawn with when a layer takes its effects.
//!
//! A shape is one value for everything that needs a draw's geometry: the
//! draw fills it, the white mask a mask blur blurs is it, and an inner or
//! outer blur style clips to it. Whether a path is stroked is decided once,
//! when the shape is made from the recorded op and its paint, and an
//! image's shape is its destination rect.

use std::sync::Arc;

use valo_dl::{ColorFilter, GlyphPos, Image, MaskBlur, Paint, PaintStyle, Sampling, TileMode};
use valo_geometry::{Color, FillRule, Matrix, Path, Rect, Stroke};
use valo_text::Font;

use super::filter_tree::{DrawMaskBlur, Fill, MaskedDraw};

/// `Shape` is a draw's geometry in its local coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Shape<'a> {
    /// A rectangle, one quad.
    Rect(Rect),
    /// A path filled by its rule: stencil-then-cover.
    Path { path: &'a Arc<Path>, rule: FillRule },
    /// A path's outline: a triangle strip along it.
    Stroke {
        path: &'a Arc<Path>,
        stroke: &'a Stroke,
    },
}

impl<'a> Shape<'a> {
    /// `of_path` is `path` drawn the way `style` says: filled by `rule`, or
    /// stroked.
    pub fn of_path(path: &'a Arc<Path>, rule: FillRule, style: &'a PaintStyle) -> Self {
        match style {
            PaintStyle::Fill => Shape::Path { path, rule },
            PaintStyle::Stroke(stroke) => Shape::Stroke { path, stroke },
        }
    }
}

/// `DrawSource` is what one draw draws, decoupled from the decision above
/// it.
#[derive(Clone, Copy)]
pub(super) enum DrawSource<'a> {
    /// A shape painted with the paint's colour or shader. `ink` is its
    /// recorded ink, local coordinates, a stroke included.
    Shape {
        shape: Shape<'a>,
        ink: Rect,
    },
    /// An image's `src` texels drawn over `dst`.
    Image {
        image: &'a Image,
        src: Rect,
        dst: Rect,
        sampling: Sampling,
    },
    /// The recorded fast path for a solid blurred (r)rect — coverage is
    /// analytic in the fragment, so its mask blur never opens a layer.
    RRectBlur {
        rect: Rect,
        radii: [f32; 4],
        blur: MaskBlur,
    },
    Glyphs(GlyphRun<'a>),
}

/// `GlyphRun` is one placed run of glyphs: the font instance, the size it
/// was laid out at, the positioned glyphs, and its recorded bounds. Those
/// ride along because glyph extents are not derivable at plan time — a
/// layer that has to enclose the run sizes itself from them.
#[derive(Clone, Copy)]
pub(super) struct GlyphRun<'a> {
    pub font: &'a Arc<Font>,
    pub size: f32,
    pub glyphs: &'a Arc<Vec<GlyphPos>>,
    /// The run's ink in local coordinates, stroke included, before effects.
    pub content_bounds: Rect,
    /// The run's recorded bounds mapped into target coords: its ink with the
    /// paint's effects, cropped by the clip.
    pub device_bounds: Rect,
    /// The colour filter the run's glyphs are coloured through, taken from
    /// a solid paint before the draw is routed: each glyph's colour passes
    /// through it before its coverage, as Skia filters a paint's colour, so
    /// it never reaches past the glyphs. A shader-painted run's folds into
    /// its shader instead.
    pub colour_filter: Option<ColorFilter>,
}

impl<'a> DrawSource<'a> {
    /// `blur_tile_mode` is how an image filter's blur that left its tile mode
    /// unspecified reads past this draw's edge, as dart:ui picks it by the
    /// canvas call: clamp for an image, decal for every other draw here.
    pub fn blur_tile_mode(&self) -> TileMode {
        match self {
            DrawSource::Image { .. } => TileMode::Clamp,
            _ => TileMode::Decal,
        }
    }

    /// `local_bounds` is the draw's ink in local coordinates, before any
    /// effect: what a layer for its effects has to hold.
    pub fn local_bounds(&self) -> Rect {
        match self {
            DrawSource::Shape { ink, .. } => *ink,
            DrawSource::Image { dst, .. } => *dst,
            DrawSource::RRectBlur { rect, .. } => *rect,
            DrawSource::Glyphs(run) => run.content_bounds,
        }
    }

    /// `mask_blur` is what `blur` needs of this draw painted with `paint`,
    /// by Impeller's `CreateMaskBlur`: a solid shape blurs its colours, a
    /// shader paint or an image blurs the white mask of its shape and fills
    /// it, a glyph run blurs its colours. `None` for the analytic blurred
    /// rrect, which blurs in its own fragment.
    pub fn mask_blur(&self, paint: &Paint, blur: MaskBlur) -> Option<DrawMaskBlur<'a>> {
        let masked = match *self {
            DrawSource::Shape { shape, ink } if paint.shader.is_some() => MaskedDraw::Filled {
                shape,
                fill: Box::new(Fill::shader(paint, &ink, blur)),
            },
            DrawSource::Shape { shape, .. } => MaskedDraw::Colours(shape),
            DrawSource::Image {
                image,
                src,
                dst,
                sampling,
            } => MaskedDraw::Filled {
                shape: Shape::Rect(dst),
                fill: Box::new(Fill::image(image, &src, &dst, sampling, paint, blur)),
            },
            DrawSource::Glyphs(_) => MaskedDraw::GlyphRun,
            DrawSource::RRectBlur { .. } => return None,
        };
        Some(DrawMaskBlur { blur, masked })
    }

    /// `in_effect_layer` is what this draw's effect layer holds and the
    /// paint it is drawn with: the draw plain, its effects left to the layer
    /// and its blend to the composite; or, when its mask blur fills a mask,
    /// the white mask of its shape.
    pub fn in_effect_layer(
        self,
        paint: &Paint,
        mask_blur: Option<&DrawMaskBlur<'a>>,
    ) -> (Self, Paint) {
        match mask_blur.map(|mask_blur| &mask_blur.masked) {
            Some(MaskedDraw::Filled { shape, .. }) => {
                let mask = DrawSource::Shape {
                    shape: *shape,
                    ink: self.local_bounds(),
                };
                (mask, white_mask(paint))
            }
            _ => (self, plain(paint)),
        }
    }

    /// `whole_image` is the image and where it is drawn, when this draw
    /// shows the whole of an image: the case Impeller's `TextureContents`
    /// hands its texture through un-rendered.
    pub fn whole_image(&self) -> Option<(&'a Image, Rect)> {
        match *self {
            DrawSource::Image {
                image, src, dst, ..
            } if src == Rect::new(0.0, 0.0, image.width(), image.height()) => Some((image, dst)),
            _ => None,
        }
    }

    /// `device_bounds` is where this draw can show under `transform`: what
    /// an implicit layer for it has to hold.
    pub fn device_bounds(&self, transform: &Matrix) -> Rect {
        let local = match self {
            DrawSource::RRectBlur { rect, blur, .. } => rect.expand((blur.sigma * 3.0).ceil()),
            DrawSource::Glyphs(run) => return run.device_bounds,
            source => source.local_bounds(),
        };
        transform.map_rect(&local)
    }
}

/// `white_mask` is the paint of a draw's coverage mask: opaque white, drawn
/// as the paint draws (a glyph run's stroke stays a stroke).
pub(super) fn white_mask(paint: &Paint) -> Paint {
    Paint {
        style: paint.style.clone(),
        ..Paint::from_color(Color::WHITE)
    }
}

/// `fill_paint` is the paint that fills a mask with `paint`'s contents,
/// drawn `SrcIn`: its shader, or the image drawn with it, at its alpha and
/// through its colour filter.
pub(super) fn fill_paint(paint: &Paint) -> Paint {
    Paint {
        shader: paint.shader.clone(),
        color: paint.color,
        color_filter: paint.color_filter,
        ..Paint::default()
    }
}

/// `plain` is the paint an inner draw of an implicit/effect layer uses:
/// the effects moved to the layer, whose composite blends.
pub(super) fn plain(paint: &Paint) -> Paint {
    Paint {
        mask_blur: None,
        color_filter: None,
        image_filter: None,
        ..paint.clone()
    }
}
