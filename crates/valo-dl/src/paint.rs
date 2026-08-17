use std::sync::Arc;

use valo_geometry::{Color, Matrix, Point, Rect, Stroke};

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

    /// `is_pipeline_blendable` reports whether fixed-function blending is sufficient.
    pub fn is_pipeline_blendable(self) -> bool {
        !matches!(
            self,
            BlendMode::Overlay
                | BlendMode::Darken
                | BlendMode::Lighten
                | BlendMode::ColorDodge
                | BlendMode::ColorBurn
                | BlendMode::HardLight
                | BlendMode::SoftLight
                | BlendMode::Difference
                | BlendMode::Exclusion
                | BlendMode::Multiply
                | BlendMode::Hue
                | BlendMode::Saturation
                | BlendMode::Color
                | BlendMode::Luminosity
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
}

impl ColorFilter {
    /// `folded_into` applies this filter to one solid color on the CPU.
    pub fn folded_into(&self, color: Color) -> Option<Color> {
        Some(crate::color_filter::apply(*self, color))
    }

    /// `modifies_transparent_black` reports whether this filter can create
    /// visible output from a transparent input pixel.
    pub fn modifies_transparent_black(&self) -> bool {
        self.folded_into(Color::TRANSPARENT)
            .is_some_and(|color| color.a > 0.0)
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
    /// `blur` creates a Gaussian image filter with nonnegative sigmas.
    pub fn blur(sigma_x: f32, sigma_y: f32) -> Self {
        Self::Blur {
            sigma_x: sigma_x.max(0.0),
            sigma_y: sigma_y.max(0.0),
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
            Self::Blur { sigma_x, sigma_y } => *sigma_x <= 0.0 && *sigma_y <= 0.0,
            Self::Color(_) => false,
            // An invisible shadow leaves the input exactly as it found it.
            Self::DropShadow { color, .. } => color.a <= 0.0,
            Self::Compose { outer, inner } => outer.is_nop() && inner.is_nop(),
        }
    }

    /// `padding` returns conservative local x and y expansion for this filter.
    pub fn padding(&self) -> [f32; 2] {
        match self {
            Self::Blur { sigma_x, sigma_y } => [(sigma_x * 3.0).ceil(), (sigma_y * 3.0).ceil()],
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
    /// `mask_blur` softens the draw's coverage.
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
        let filter_keeps_transparent = self
            .color_filter
            .is_none_or(|filter| !filter.modifies_transparent_black())
            && self
                .image_filter
                .as_ref()
                .is_none_or(|filter| !filter.modifies_transparent_black());
        let invisible = self.color.a <= 0.0
            && self.blend_mode == BlendMode::SrcOver
            && filter_keeps_transparent;
        // Width ZERO is a hairline, not an empty stroke — Skia and Impeller
        // both draw it one device pixel wide, and the renderer's hairline
        // floor is what realises that. Only a negative width draws nothing.
        let empty_stroke = matches!(&self.style, PaintStyle::Stroke(s) if s.width < 0.0);
        invisible || empty_stroke
    }

    /// `is_opacity_only` reports whether this paint is only a SrcOver alpha.
    pub fn is_opacity_only(&self) -> bool {
        self.blend_mode == BlendMode::SrcOver
            && self.shader.is_none()
            && self.mask_blur.is_none()
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

    /// `effect_bounds` returns local bounds required by this paint's effects.
    ///
    /// Filters that create visible pixels from transparency return unbounded
    /// coverage for the caller to intersect with its active clip.
    pub fn effect_bounds(&self, bounds: Rect) -> Rect {
        let floods = self
            .color_filter
            .is_some_and(|filter| filter.modifies_transparent_black())
            || self
                .image_filter
                .as_ref()
                .is_some_and(|filter| filter.modifies_transparent_black());
        if floods {
            Rect::EVERYTHING
        } else {
            bounds.expand(self.effect_padding())
        }
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
