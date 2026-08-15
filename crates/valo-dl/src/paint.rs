use std::sync::Arc;

use valo_geometry::{Color, Matrix, Point, Rect, Stroke};

/// Porter–Duff + advanced blend modes — the full Skia/Flutter vocabulary, declared
/// up front so the recorded format never changes. The renderer implements the
/// pipeline-blendable subset first (M1); the dst-reading advanced modes arrive with
/// the pass-break machinery (M4) — until then they fall back to `SrcOver` with a
/// debug warning, never a panic.
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
    // ── dst-reading "advanced" modes (need a target copy; M4) ──
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

    /// A rotation reaches further than any axis length reports: `max_scale`
    /// is 1 for a pure rotation, so a scalar padding × max_scale bound would
    /// leave a diagonal drop shadow short by 4.14 px and the combine pass
    /// would cut the remainder away as transparent.
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
    /// Transparent source pixels may change destination pixels outside the
    /// source ink. Impeller uses this to flood save-layer output coverage to
    /// the active clip.
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

    /// Expressible as fixed-function pipeline blending (no dst read).
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

/// Where a mask blur shows relative to the sharp shape (Skia's SkBlurStyle).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BlurStyle {
    /// Blurred inside and outside — a shadow.
    #[default]
    Normal,
    /// Sharp inside, blurred outside — the shape sitting on its own glow.
    Solid,
    /// Blurred inside, nothing outside — an inset/pressed look.
    Inner,
    /// Nothing inside, blurred outside — a halo.
    Outer,
}

/// Gaussian mask blur: σ in LOCAL units (rides the transform) plus a style.
/// Solid-paint rects/rrects render it in closed form (one quad); everything
/// else takes the layer + filter-pass route.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct MaskBlur {
    pub sigma: f32,
    pub style: BlurStyle,
}

impl MaskBlur {
    pub fn new(sigma: f32) -> Self {
        Self::styled(sigma, BlurStyle::Normal)
    }

    pub fn solid(sigma: f32) -> Self {
        Self::styled(sigma, BlurStyle::Solid)
    }

    pub fn inner(sigma: f32) -> Self {
        Self::styled(sigma, BlurStyle::Inner)
    }

    pub fn outer(sigma: f32) -> Self {
        Self::styled(sigma, BlurStyle::Outer)
    }

    /// σ is clamped non-negative: a negative value would DEFLATE the
    /// record-time bounds padding and wrongly cull the draw.
    fn styled(sigma: f32, style: BlurStyle) -> Self {
        Self {
            sigma: sigma.max(0.0),
            style,
        }
    }
}

/// A per-pixel colour transform over what a draw or layer produced —
/// Flutter's `ColorFilter`, Skia's `SkColorFilter`. Applied BEFORE
/// [`MaskBlur`], matching Impeller: the filter runs on the shape's own
/// pixels and the blur spreads the filtered result.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ColorFilter {
    /// Row-major 4×5 over UNPREMULTIPLIED colour in 0..1: each output
    /// channel is `row · [r, g, b, a, 1]`, clamped. Skia's `SkColorMatrix`
    /// convention.
    ///
    /// Flutter's `ColorFilter.matrix` hands the translation column in
    /// unnormalized 0..255 space instead, so a Flutter matrix needs entries
    /// 4, 9, 14 and 19 divided by 255 before it arrives here. Getting that
    /// wrong still produces a plausible-looking image, which is why it is
    /// called out rather than absorbed.
    Matrix([f32; 20]),
    /// Blend a constant colour AS THE SOURCE over what was drawn — Flutter's
    /// `ColorFilter.mode`, the tint behind every coloured icon.
    Blend(Color, BlendMode),
}

impl ColorFilter {
    /// A solid paint's colour after this filter — the CPU fold that skips
    /// the layer and the filter pass entirely (Impeller folds on the CPU
    /// first for the same reason).
    ///
    /// `None` when the filter needs the drawn pixels as its destination, so
    /// only the GPU can answer it.
    pub fn folded_into(&self, color: Color) -> Option<Color> {
        Some(crate::color_filter::apply(*self, color))
    }

    /// Whether this filter can turn an untouched transparent pixel into a
    /// visible one. Layer coverage must include the full filter scope when
    /// this is true (Flutter's `modifies_transparent_black`).
    pub fn modifies_transparent_black(&self) -> bool {
        self.folded_into(Color::TRANSPARENT)
            .is_some_and(|color| color.a > 0.0)
    }
}

/// A post-raster image filter. Composition follows Flutter/Impeller naming:
/// the inner filter runs first and its result becomes the outer filter's
/// input.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ImageFilter {
    /// Gaussian blur in local x/y units.
    Blur { sigma_x: f32, sigma_y: f32 },
    /// Run a color filter as a texture-stage image filter.
    Color(ColorFilter),
    /// The input composited over a blurred, recoloured, offset copy of its own
    /// alpha — Skia's `SkImageFilters::DropShadow` and what CSS
    /// `filter: drop-shadow()` lowers to. `offset` and the sigmas are in local
    /// units and ride the effect transform, like every other filter here.
    ///
    /// The plain shadow, not `DropShadowOnly`: the source survives in the
    /// output. A caller that wants only the shadow composes a colour filter.
    DropShadow {
        offset: Point,
        sigma_x: f32,
        sigma_y: f32,
        color: Color,
    },
    /// `outer(inner(input))`.
    Compose {
        outer: Arc<ImageFilter>,
        inner: Arc<ImageFilter>,
    },
}

impl ImageFilter {
    pub fn blur(sigma_x: f32, sigma_y: f32) -> Self {
        Self::Blur {
            sigma_x: sigma_x.max(0.0),
            sigma_y: sigma_y.max(0.0),
        }
    }

    pub fn color(filter: ColorFilter) -> Self {
        Self::Color(filter)
    }

    pub fn compose(outer: ImageFilter, inner: ImageFilter) -> Self {
        Self::Compose {
            outer: Arc::new(outer),
            inner: Arc::new(inner),
        }
    }

    pub fn drop_shadow(offset: Point, sigma_x: f32, sigma_y: f32, color: Color) -> Self {
        Self::DropShadow {
            offset,
            sigma_x: sigma_x.max(0.0),
            sigma_y: sigma_y.max(0.0),
            color,
        }
    }

    /// Whether the chain leaves every pixel exactly as it found it. A
    /// zero-sigma blur is the case that matters: Flutter rejects one outright,
    /// and letting it through here would buy a layer and two full-size
    /// resamples to reproduce the input.
    ///
    /// A colour filter is never a no-op — even an identity matrix clamps.
    pub fn is_nop(&self) -> bool {
        match self {
            Self::Blur { sigma_x, sigma_y } => *sigma_x <= 0.0 && *sigma_y <= 0.0,
            Self::Color(_) => false,
            // An invisible shadow leaves the input exactly as it found it.
            Self::DropShadow { color, .. } => color.a <= 0.0,
            Self::Compose { outer, inner } => outer.is_nop() && inner.is_nop(),
        }
    }

    /// Conservative local-space x/y expansion required by the complete chain.
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

/// Fill the shape's interior, or stroke its outline.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum PaintStyle {
    #[default]
    Fill,
    Stroke(Stroke),
}

/// How to fill what's drawn. Grows fields as features land — additions,
/// never reshapes, so recorded lists stay stable.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Paint {
    pub color: Color,
    pub blend_mode: BlendMode,
    /// Per-pixel color source (gradients). When set, `color` acts as an
    /// opacity/tint multiplier — leave it WHITE for a plain gradient.
    pub shader: Option<crate::Shader>,
    /// Soft coverage for shadows, glows, and insets.
    pub mask_blur: Option<MaskBlur>,
    /// Recolour what this paint produced, before any blur spreads it.
    pub color_filter: Option<ColorFilter>,
    /// Texture-stage filter over the rasterized draw or save-layer result.
    pub image_filter: Option<ImageFilter>,
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
    pub fn from_color(color: Color) -> Self {
        Self {
            color,
            ..Default::default()
        }
    }

    pub fn from_shader(shader: crate::Shader) -> Self {
        Self {
            color: Color::WHITE,
            shader: Some(shader),
            ..Default::default()
        }
    }

    /// Fully transparent + `SrcOver` (or a negative-width stroke) draws
    /// nothing — the recorder drops them.
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

    /// A plain-alpha composite (what an elidable saveLayer needs): SrcOver,
    /// no shader — only `color.a` matters.
    pub fn is_opacity_only(&self) -> bool {
        self.blend_mode == BlendMode::SrcOver
            && self.shader.is_none()
            && self.mask_blur.is_none()
            && self.color_filter.is_none()
            && self.effective_image_filter().is_none()
    }

    /// The image filter only when it would actually change pixels. Every
    /// decision about whether this paint needs a layer goes through here, so a
    /// no-op filter never costs a target.
    pub fn effective_image_filter(&self) -> Option<&ImageFilter> {
        self.image_filter.as_ref().filter(|f| !f.is_nop())
    }

    /// Record-time bounds padding: ±3σ holds >99.7% of a gaussian's spread.
    /// (Inner style never spreads, but padding is conservative-correct.)
    pub fn mask_padding(&self) -> f32 {
        self.mask_blur.map_or(0.0, |blur| (blur.sigma * 3.0).ceil())
    }

    /// Padding for every raster-stage effect applied to this paint, in LOCAL
    /// units and per axis. Callers working in local space (a draw's own effect
    /// layer) can expand before mapping and are done; callers that already
    /// hold device-space bounds want [`Self::device_effect_padding`].
    pub fn effect_padding_axes(&self) -> [f32; 2] {
        let image = self
            .image_filter
            .as_ref()
            .map_or([0.0; 2], ImageFilter::padding);
        let mask = self.mask_padding();
        [image[0] + mask, image[1] + mask]
    }

    /// Padding for every raster-stage effect applied to this paint.
    pub fn effect_padding(&self) -> f32 {
        let axes = self.effect_padding_axes();
        axes[0].max(axes[1])
    }

    /// The same padding expressed in DEVICE pixels under `transform` — what a
    /// scope whose bounds are already device-space has to expand by.
    ///
    /// The local padding box is mapped through the transform's linear part,
    /// which is the only thing that bounds a rotated or sheared effect.
    /// `max(local padding) × max_scale` does not: a 45° rotation sends a
    /// (10, 10) drop-shadow offset to 14.14 device px while `max_scale` stays
    /// 1, and the combine pass cuts the missing 4.14 px away as transparent.
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

    /// Local bounds needed to evaluate this paint's post-raster effects.
    /// Filters that create pixels from transparent black cover the eventual
    /// clip rather than only the source ink.
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

    /// Half the stroke width, times the miter's worst-case spike (and √2
    /// for square-cap corners) — how far ink can reach past the geometry.
    pub fn stroke_padding(&self) -> f32 {
        self.stroke_padding_at_scale(1.0)
    }

    /// Transform-aware stroke padding. The renderer floors every stroke to
    /// one device pixel, so bounds must use that same effective width under
    /// minification or a layer/cull edge can trim the widened geometry.
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
