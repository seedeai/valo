use std::sync::Arc;

use valo_geometry::{Color, Matrix, Point, Rect, Stroke};

use crate::{Bounds, TileMode};

/// `BlendMode` controls how source pixels combine with destination pixels.
///
/// [`BlendMode::SrcOver`] is the default. Advanced modes that read destination
/// pixels may require an additional render-pass break and snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BlendMode {
    Clear,
    Src,
    Dst,
    #[default]
    SrcOver,
    DstOver,
    SrcIn,
    DstIn,
    SrcOut,
    DstOut,
    SrcAtop,
    DstAtop,
    Xor,
    Plus,
    Modulate,
    Screen,
    // Destination-reading advanced modes require a target snapshot.
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Multiply,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    /// `is_destructive` reports whether transparent source pixels can change
    /// destination pixels outside the source ink.
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            BlendMode::Clear
                | BlendMode::Src
                | BlendMode::SrcIn
                | BlendMode::DstIn
                | BlendMode::SrcOut
                | BlendMode::DstOut
                | BlendMode::DstAtop
                | BlendMode::Xor
                | BlendMode::Modulate
        )
    }
}

/// `BlurStyle` controls where blurred coverage appears relative to a shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BlurStyle {
    /// `Normal` blurs coverage inside and outside the shape.
    #[default]
    Normal,
    /// `Solid` keeps a sharp interior and blurs outside.
    Solid,
    /// `Inner` blurs inside and leaves the exterior empty.
    Inner,
    /// `Outer` blurs outside and leaves the interior empty.
    Outer,
}

/// `MaskBlur` applies a Gaussian blur to a draw's coverage mask.
///
/// Sigma is measured in local units and follows the draw's transform.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct MaskBlur {
    /// `sigma` is the nonnegative Gaussian standard deviation in local units.
    pub sigma: f32,
    /// `style` controls which side of the original coverage remains visible.
    pub style: BlurStyle,
}

impl MaskBlur {
    /// `new` creates a normal mask blur.
    pub fn new(sigma: f32) -> Self {
        Self::styled(sigma, BlurStyle::Normal)
    }

    /// `solid` creates a blur with a sharp interior.
    pub fn solid(sigma: f32) -> Self {
        Self::styled(sigma, BlurStyle::Solid)
    }

    /// `inner` creates a blur visible only inside the shape.
    pub fn inner(sigma: f32) -> Self {
        Self::styled(sigma, BlurStyle::Inner)
    }

    /// `outer` creates a blur visible only outside the shape.
    pub fn outer(sigma: f32) -> Self {
        Self::styled(sigma, BlurStyle::Outer)
    }

    /// `styled` clamps sigma to keep effect bounds from shrinking.
    fn styled(sigma: f32, style: BlurStyle) -> Self {
        Self {
            sigma: sigma.max(0.0),
            style,
        }
    }
}

/// `ColorFilter` transforms the pixels produced by a draw or layer.
///
/// Color filters run before mask blur, so the blur spreads the filtered result.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ColorFilter {
    /// `Matrix` is a row-major 4×5 transform over straight color in 0..1.
    ///
    /// Each output
    /// channel is `row · [r, g, b, a, 1]`, clamped. Skia's `SkColorMatrix`
    /// convention.
    ///
    /// Flutter's `ColorFilter.matrix` hands the translation column in
    /// unnormalized 0..255 space instead, so a Flutter matrix needs entries
    /// 4, 9, 14 and 19 divided by 255 before it arrives here. Getting that
    /// wrong still produces a plausible-looking image, which is why it is
    /// called out rather than absorbed.
    Matrix([f32; 20]),
    /// `Blend` composites a constant source color over each produced pixel.
    Blend(Color, BlendMode),
    /// `LinearToSrgbGamma` encodes linear-light values with the sRGB transfer
    /// curve.
    ///
    /// The curve runs on the red, green and blue values as they are stored,
    /// on straight colour, and leaves alpha alone; no texture changes colour
    /// space. Flutter's `ColorFilter.linearToSrgbGamma`.
    LinearToSrgbGamma,
    /// `SrgbToLinearGamma` decodes sRGB-encoded values to linear light, the
    /// inverse of [`ColorFilter::LinearToSrgbGamma`].
    ///
    /// Decoded values stored in an 8-bit target keep little precision in the
    /// shadows, so a decode, blur, encode chain bands in dark areas.
    /// Flutter's `ColorFilter.srgbToLinearGamma`.
    SrgbToLinearGamma,
}

impl ColorFilter {
    /// `folded_into` applies this filter to one solid color on the CPU.
    pub fn folded_into(&self, color: Color) -> Color {
        crate::color_filter::apply(*self, color)
    }

    /// `modifies_transparent_black` reports whether this filter can create
    /// visible output from a transparent input pixel.
    pub fn modifies_transparent_black(&self) -> bool {
        self.folded_into(Color::TRANSPARENT).a > 0.0
    }
}

/// `ImageFilter` transforms a rasterized draw or layer.
///
/// In a composition, the inner filter runs first and feeds the outer filter.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ImageFilter {
    /// `Blur` applies a Gaussian blur in local x and y units.
    Blur {
        /// `sigma_x` is the horizontal standard deviation.
        sigma_x: f32,
        /// `sigma_y` is the vertical standard deviation.
        sigma_y: f32,
        /// `bounds` limits what the blur reads to a rectangle in local units,
        /// placed by the transform where the filter applies, so a rotation
        /// turns it with the content.
        ///
        /// Pixels outside it count as transparent, and each result is divided
        /// by its own alpha: content near the edge stays opaque instead of
        /// fading or taking in colour from beyond the edge, and translucent
        /// content comes out opaque. The output is not clipped; it spreads
        /// past the bounds as far as an unbounded blur's would, so pair it
        /// with a clip. `None` blurs everything. Flutter's
        /// `ImageFilter.blur(bounds:)`.
        bounds: Option<Rect>,
        /// `tile_mode` is how the blur reads past the edge of what it blurs:
        /// transparent ([`TileMode::Decal`]), the edge pixels extended, or
        /// the picture mirrored or repeated.
        ///
        /// `None` leaves it to where the filter is used, as dart:ui does for
        /// an `ImageFilter.blur` without a `tileMode`: decal on a layer and
        /// on most draws, clamp on an image draw and inside a composition,
        /// mirror for a backdrop.
        tile_mode: Option<TileMode>,
    },
    /// `Color` applies a color filter after rasterization.
    Color(ColorFilter),
    /// `DropShadow` composites the input over a blurred, colored copy of its alpha.
    DropShadow {
        /// `offset` moves the shadow in local coordinates.
        offset: Point,
        /// `sigma_x` is the horizontal standard deviation.
        sigma_x: f32,
        /// `sigma_y` is the vertical standard deviation.
        sigma_y: f32,
        /// `color` colors the shadow.
        color: Color,
    },
    /// `Compose` applies `inner` and then `outer`.
    Compose {
        /// `outer` receives the filtered result of `inner`.
        outer: Arc<ImageFilter>,
        /// `inner` receives the original input.
        inner: Arc<ImageFilter>,
    },
}

impl ImageFilter {
    /// `blur` creates a Gaussian image filter with nonnegative sigmas, its
    /// tile mode left to where it is used.
    pub fn blur(sigma_x: f32, sigma_y: f32) -> Self {
        Self::Blur {
            sigma_x: sigma_x.max(0.0),
            sigma_y: sigma_y.max(0.0),
            bounds: None,
            tile_mode: None,
        }
    }

    /// `bounded_blur` creates a Gaussian image filter that reads only inside
    /// `bounds`, a rectangle in local units.
    ///
    /// It is the frosted-glass blur: colour from beyond the bounds does not
    /// bleed in at the edges. See the `bounds` field of [`ImageFilter::Blur`].
    pub fn bounded_blur(sigma_x: f32, sigma_y: f32, bounds: Rect) -> Self {
        Self::Blur {
            sigma_x: sigma_x.max(0.0),
            sigma_y: sigma_y.max(0.0),
            bounds: Some(bounds),
            tile_mode: None,
        }
    }

    /// `with_tile_mode` sets a blur's tile mode, the way it reads past the
    /// edge of what it blurs; any other filter is returned as it is. See the
    /// `tile_mode` field of [`ImageFilter::Blur`].
    pub fn with_tile_mode(self, tile_mode: TileMode) -> Self {
        match self {
            Self::Blur {
                sigma_x,
                sigma_y,
                bounds,
                ..
            } => Self::Blur {
                sigma_x,
                sigma_y,
                bounds,
                tile_mode: Some(tile_mode),
            },
            other => other,
        }
    }

    /// `color` creates an image filter from a color filter.
    pub fn color(filter: ColorFilter) -> Self {
        Self::Color(filter)
    }

    /// `compose` applies `inner` first and `outer` second.
    pub fn compose(outer: ImageFilter, inner: ImageFilter) -> Self {
        Self::Compose {
            outer: Arc::new(outer),
            inner: Arc::new(inner),
        }
    }

    /// `drop_shadow` creates a shadow that retains the original input.
    pub fn drop_shadow(offset: Point, sigma_x: f32, sigma_y: f32, color: Color) -> Self {
        Self::DropShadow {
            offset,
            sigma_x: sigma_x.max(0.0),
            sigma_y: sigma_y.max(0.0),
            color,
        }
    }

    /// `is_nop` reports whether the filter leaves every input pixel unchanged.
    pub fn is_nop(&self) -> bool {
        match self {
            Self::Blur {
                sigma_x, sigma_y, ..
            } => *sigma_x <= 0.0 && *sigma_y <= 0.0,
            Self::Color(_) => false,
            // An invisible shadow leaves the input exactly as it found it.
            Self::DropShadow { color, .. } => color.a <= 0.0,
            Self::Compose { outer, inner } => outer.is_nop() && inner.is_nop(),
        }
    }

    /// `padding` returns conservative local x and y expansion for this filter.
    pub fn padding(&self) -> [f32; 2] {
        match self {
            // The bounds limit what the blur reads, not how far it spreads.
            Self::Blur {
                sigma_x, sigma_y, ..
            } => [(sigma_x * 3.0).ceil(), (sigma_y * 3.0).ceil()],
            Self::Color(_) => [0.0; 2],
            // Padding is symmetric, so a one-sided offset has to be paid on
            // both sides — the shadow is free to land on either.
            Self::DropShadow {
                offset,
                sigma_x,
                sigma_y,
                ..
            } => [
                (sigma_x * 3.0).ceil() + offset.x.abs(),
                (sigma_y * 3.0).ceil() + offset.y.abs(),
            ],
            Self::Compose { outer, inner } => {
                let outer = outer.padding();
                let inner = inner.padding();
                [outer[0] + inner[0], outer[1] + inner[1]]
            }
        }
    }

    /// `modifies_transparent_black` reports whether this filter can create
    /// visible output from a transparent input pixel.
    pub fn modifies_transparent_black(&self) -> bool {
        match self {
            Self::Blur { .. } => false,
            Self::Color(filter) => filter.modifies_transparent_black(),
            // The shadow is the input's own alpha recoloured, so transparent
            // input stays transparent however opaque the shadow colour is.
            Self::DropShadow { .. } => false,
            Self::Compose { outer, inner } => {
                outer.modifies_transparent_black() || inner.modifies_transparent_black()
            }
        }
    }
}

/// `PaintStyle` selects filled geometry or a stroked outline.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum PaintStyle {
    /// `Fill` covers the geometry's interior.
    #[default]
    Fill,
    /// `Stroke` draws the geometry's outline with the supplied stroke parameters.
    Stroke(Stroke),
}

/// `Paint` describes how a drawing operation produces and composites pixels.
///
/// The default is an opaque black fill using [`BlendMode::SrcOver`].
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Paint {
    /// `color` supplies solid-draw color and the alpha for shader or image draws.
    ///
    /// Shader and image draws ignore its RGB channels.
    pub color: Color,
    /// `blend_mode` controls compositing with destination pixels.
    pub blend_mode: BlendMode,
    /// `shader` replaces the solid color with a per-pixel source.
    pub shader: Option<crate::Shader>,
    /// `mask_blur` softens the draw's coverage. A save layer's paint takes
    /// none: the layer is recorded without it.
    pub mask_blur: Option<MaskBlur>,
    /// `color_filter` transforms produced colors before mask blur.
    pub color_filter: Option<ColorFilter>,
    /// `image_filter` transforms the rasterized draw or layer.
    pub image_filter: Option<ImageFilter>,
    /// `style` selects fill or stroke rendering.
    pub style: PaintStyle,
}

impl Default for Paint {
    fn default() -> Self {
        Self {
            color: Color::BLACK,
            blend_mode: BlendMode::SrcOver,
            shader: None,
            mask_blur: None,
            color_filter: None,
            image_filter: None,
            style: PaintStyle::Fill,
        }
    }
}

impl Paint {
    /// `from_color` creates a solid-color fill paint.
    pub fn from_color(color: Color) -> Self {
        Self {
            color,
            ..Default::default()
        }
    }

    /// `from_shader` creates a fill paint using a per-pixel shader.
    pub fn from_shader(shader: crate::Shader) -> Self {
        Self {
            color: Color::WHITE,
            shader: Some(shader),
            ..Default::default()
        }
    }

    /// `is_nop` reports whether this paint can produce no visible change.
    pub fn is_nop(&self) -> bool {
        // Width ZERO is a hairline, not an empty stroke — Skia and Impeller
        // both draw it one device pixel wide, and the renderer's hairline
        // floor is what realises that. Only a negative width draws nothing.
        let empty_stroke = matches!(&self.style, PaintStyle::Stroke(s) if s.width < 0.0);
        self.is_invisible() || empty_stroke
    }

    /// `is_invisible` reports whether what this paint composites shows
    /// nowhere, whatever it draws: a transparent colour blended `SrcOver`
    /// with no filter to colour it. A save layer with such a paint records
    /// nothing inside (Flutter's `PaintResult`).
    pub(crate) fn is_invisible(&self) -> bool {
        self.color.a <= 0.0 && self.blend_mode == BlendMode::SrcOver && !self.reveals_transparent()
    }

    /// `takes_group_opacity` reports whether an enclosing group's opacity
    /// can be applied to what this paint draws instead of to the group. It
    /// needs the paint to blend `SrcOver`, and no colour or image filter,
    /// which would take the alpha in before it filters. Flutter's display
    /// list asks the same of every op in a group before the group's opacity
    /// may ride its children (`UpdateCurrentOpacityCompatibility`).
    pub(crate) fn takes_group_opacity(&self) -> bool {
        self.blend_mode == BlendMode::SrcOver
            && self.color_filter.is_none()
            && self.effective_image_filter().is_none()
    }

    /// `effective_image_filter` returns the image filter when it changes pixels.
    pub fn effective_image_filter(&self) -> Option<&ImageFilter> {
        self.image_filter.as_ref().filter(|f| !f.is_nop())
    }

    /// `mask_padding` returns conservative local padding for the mask blur.
    pub fn mask_padding(&self) -> f32 {
        self.mask_blur.map_or(0.0, |blur| (blur.sigma * 3.0).ceil())
    }

    /// `effect_padding_axes` returns local x and y padding for raster effects.
    pub fn effect_padding_axes(&self) -> [f32; 2] {
        let image = self
            .image_filter
            .as_ref()
            .map_or([0.0; 2], ImageFilter::padding);
        let mask = self.mask_padding();
        [image[0] + mask, image[1] + mask]
    }

    /// `effect_padding` returns the largest local-axis padding for raster effects.
    pub fn effect_padding(&self) -> f32 {
        let axes = self.effect_padding_axes();
        axes[0].max(axes[1])
    }

    /// `device_effect_padding` returns effect padding in device pixels.
    ///
    /// It maps both local padding axes through `transform`, preserving a
    /// conservative bound under rotation and shear.
    pub fn device_effect_padding(&self, transform: &Matrix) -> f32 {
        let [x, y] = self.effect_padding_axes();
        if x <= 0.0 && y <= 0.0 {
            return 0.0;
        }
        // The half-extent of an axis-aligned box under a linear map is the
        // component-wise absolute matrix applied to the half-extent.
        let [a, b, c, d, ..] = transform.to_affine();
        let device_x = (x * a).abs() + (y * c).abs();
        let device_y = (x * b).abs() + (y * d).abs();
        device_x.max(device_y)
    }

    /// `effect_bounds` returns where a draw of `bounds` with this paint may
    /// show, in local coordinates, its effects included.
    ///
    /// A draw whose paint fills its clip ([`Paint::draw_fills_clip`]) is
    /// unbounded: only its clip bounds it.
    pub fn effect_bounds(&self, bounds: Rect) -> Bounds {
        if self.draw_fills_clip() {
            Bounds::Unbounded
        } else {
            Bounds::of(bounds.expand(self.effect_padding()))
        }
    }

    /// `draw_fills_clip` reports whether a draw with this paint covers its
    /// whole clip, whatever its shape: its image filter colours transparent
    /// pixels, which then show everywhere the clip lets them, as Skia draws
    /// it.
    ///
    /// A colour filter does not: it stays inside the shape it colours
    /// (Flutter's `AdjustBoundsForPaint`). On a save layer either filter, or
    /// a destructive blend, fills the clip instead (`Op::SaveLayer`'s
    /// `floods_clip`).
    pub fn draw_fills_clip(&self) -> bool {
        self.image_filter
            .as_ref()
            .is_some_and(ImageFilter::modifies_transparent_black)
    }

    /// `reveals_transparent` reports whether the paint's colour or image
    /// filter colours transparent pixels, so a transparent colour still
    /// paints something.
    pub(crate) fn reveals_transparent(&self) -> bool {
        self.color_filter
            .is_some_and(|filter| filter.modifies_transparent_black())
            || self.draw_fills_clip()
    }

    /// `stroke_padding` returns conservative stroke expansion at unit scale.
    pub fn stroke_padding(&self) -> f32 {
        self.stroke_padding_at_scale(1.0)
    }

    /// `stroke_padding_at_scale` returns stroke expansion at a device scale.
    ///
    /// Hairlines and minified strokes retain at least one device pixel.
    pub fn stroke_padding_at_scale(&self, scale: f32) -> f32 {
        match &self.style {
            PaintStyle::Fill => 0.0,
            PaintStyle::Stroke(s) => {
                let spike = match s.join {
                    valo_geometry::Join::Miter => s.miter_limit.max(1.5),
                    _ => 1.5,
                };
                let effective_width = s.width.max(1.0 / scale.max(1e-3));
                effective_width * 0.5 * spike
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ColorFilter, ImageFilter, MaskBlur, Paint, PaintStyle};
    use valo_geometry::Stroke;

    #[test]
    fn hairline_padding_stays_large_enough_when_minified() {
        let paint = Paint {
            style: PaintStyle::Stroke(Stroke::new(0.0)),
            ..Paint::default()
        };
        let scale = 0.1;
        let device_padding = paint.stroke_padding_at_scale(scale) * scale;
        assert!(device_padding >= 0.5);
    }

    #[test]
    fn composed_image_filters_accumulate_blur_coverage() {
        let filter = ImageFilter::compose(
            ImageFilter::blur(3.0, 4.0),
            ImageFilter::compose(
                ImageFilter::color(ColorFilter::Matrix([0.0; 20])),
                ImageFilter::blur(2.0, 1.0),
            ),
        );
        assert_eq!(filter.padding(), [15.0, 15.0]);
    }

    #[test]
    fn drop_shadow_padding_covers_the_offset_on_both_sides() {
        let filter = ImageFilter::drop_shadow(
            valo_geometry::Point::new(4.0, -6.0),
            2.0,
            1.0,
            valo_geometry::Color::BLACK,
        );
        assert_eq!(filter.padding(), [10.0, 9.0]);
    }

    // A rotation reaches further than any axis length reports: `max_scale`
    // is 1 for a pure rotation, so scalar padding would clip this shadow.
    #[test]
    fn device_padding_bounds_a_rotated_effect() {
        use valo_geometry::Matrix;
        let paint = Paint {
            image_filter: Some(ImageFilter::drop_shadow(
                valo_geometry::Point::new(10.0, 10.0),
                0.0,
                0.0,
                valo_geometry::Color::BLACK,
            )),
            ..Paint::default()
        };
        assert_eq!(paint.effect_padding(), 10.0);

        let quarter_turn = Matrix::rotation(std::f32::consts::FRAC_PI_4);
        let padding = paint.device_effect_padding(&quarter_turn);
        assert!(
            (padding - 14.142136).abs() < 1e-3,
            "a 45° rotation maps the (10, 10) padding box to 14.14, got {padding}"
        );
        assert!(
            padding > paint.effect_padding() * quarter_turn.max_scale(),
            "the scalar bound is exactly what this has to beat"
        );
    }

    #[test]
    fn device_padding_matches_the_scalar_bound_under_a_plain_scale() {
        use valo_geometry::Matrix;
        let paint = Paint {
            mask_blur: Some(MaskBlur::new(2.0)),
            ..Paint::default()
        };
        let scale = Matrix::scale(3.0, 3.0);
        assert_eq!(paint.effect_padding(), 6.0);
        assert!((paint.device_effect_padding(&scale) - 18.0).abs() < 1e-4);
    }

    /// Flutter's rule: a group's opacity rides a `SrcOver` paint with no
    /// colour filter. valo also keeps it off an image filter, whose colour
    /// filter would take it in first; a shader or a mask blur takes it after.
    #[test]
    fn only_a_source_over_paint_without_filters_takes_group_opacity() {
        use crate::BlendMode;
        let takes = |paint: Paint| paint.takes_group_opacity();
        assert!(takes(Paint::default()));
        assert!(takes(Paint {
            mask_blur: Some(MaskBlur::new(2.0)),
            ..Paint::default()
        }));
        assert!(takes(Paint::from_shader(crate::Shader::linear(
            valo_geometry::Point::new(0.0, 0.0),
            valo_geometry::Point::new(1.0, 0.0),
            valo_geometry::Color::BLACK,
            valo_geometry::Color::WHITE,
        ))));
        assert!(!takes(Paint {
            blend_mode: BlendMode::Plus,
            ..Paint::default()
        }));
        assert!(!takes(Paint {
            color_filter: Some(ColorFilter::Matrix([0.0; 20])),
            ..Paint::default()
        }));
        assert!(!takes(Paint {
            image_filter: Some(ImageFilter::blur(2.0, 2.0)),
            ..Paint::default()
        }));
    }

    /// A draw fills its clip only for an image filter that colours
    /// transparent pixels; a colour filter that does still makes a
    /// transparent paint paint something.
    #[test]
    fn a_draw_fills_its_clip_for_an_image_filter_that_colours_transparent_pixels() {
        use crate::BlendMode;
        let fill_red = ColorFilter::Blend(valo_geometry::Color::rgb(1.0, 0.0, 0.0), BlendMode::Src);
        let transparent = Paint::from_color(valo_geometry::Color::TRANSPARENT);
        let colour_filtered = Paint {
            color_filter: Some(fill_red),
            ..transparent.clone()
        };
        let image_filtered = Paint {
            image_filter: Some(ImageFilter::color(fill_red)),
            ..transparent.clone()
        };
        assert!(transparent.is_nop());
        assert!(!colour_filtered.draw_fills_clip() && !colour_filtered.is_nop());
        assert!(image_filtered.draw_fills_clip() && !image_filtered.is_nop());
        let blurred = Paint {
            image_filter: Some(ImageFilter::blur(2.0, 2.0)),
            ..Paint::default()
        };
        assert!(!blurred.draw_fills_clip());
    }

    #[test]
    fn an_invisible_drop_shadow_is_a_nop() {
        let filter = ImageFilter::drop_shadow(
            valo_geometry::Point::new(4.0, 4.0),
            2.0,
            2.0,
            valo_geometry::Color::TRANSPARENT,
        );
        assert!(filter.is_nop());
        assert!(!filter.modifies_transparent_black());
    }
}
