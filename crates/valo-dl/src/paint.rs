use std::sync::Arc;

use valo_geometry::{Color, Matrix, Point, Rect, Stroke};

use crate::{Bounds, Sampling, TileMode};

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
    /// `Plus` adds source and destination, clamped at one: Core Graphics'
    /// `kCGBlendModePlusLighter` and Core Animation's `plusL` filter.
    ///
    /// Over premultiplied colour it is `R = min(1, S + D)`, alpha included
    /// (`CGContext.h`); Core Image's `_plusLighter` kernel is `src + dst`, so
    /// the clamp is the target's own and a float target keeps the sum, as
    /// the blend unit does here.
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
    /// `PlusDarker` adds source and destination toward black: Core
    /// Graphics' `kCGBlendModePlusDarker`, Core Animation's `plusD` filter
    /// and Core Image's `CIPlusDarkerCompositing`. It is not one of
    /// Flutter's modes.
    ///
    /// Over premultiplied colour it is Core Image's `_plusDarker` kernel, as
    /// CoreImage ships it in macOS 27: `Ra = min(1, Sa + Da)` and
    /// `Rc = Ra − min(1, (Sa − Sc) + (Da − Dc))`, which for opaque colours
    /// is `R = max(0, 1 − ((1 − D) + (1 − S)))` (the comment in
    /// `CGContext.h` drops the outer `1 −`; Core Graphics draws the kernel's
    /// values). A transparent source leaves the destination as it is, and
    /// over a transparent destination the source shows unchanged. It reads
    /// the destination, so like `Overlay` it breaks the render pass.
    PlusDarker,
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
    /// `Runtime` shades what the input covers with a runtime effect's `shade`.
    ///
    /// An effect made by [`RuntimeEffect::filter`](crate::RuntimeEffect::filter)
    /// reads the input as `source`, prepared as its
    /// [`FilterInputs`](crate::FilterInputs) ask, at local positions. One made
    /// by [`RuntimeEffect::shader`](crate::RuntimeEffect::shader) follows
    /// Flutter's `ImageFilter.shader`: the input is its first image, the
    /// input's size in pixels its first uniform, and `position` is in the
    /// input's pixels. See [`ImageFilter::runtime`].
    Runtime(crate::RuntimeShader),
    /// `Matrix` draws the input moved by `matrix`, in local coordinates:
    /// dart:ui's `ImageFilter.matrix`, Flutter's `DlMatrixImageFilter` and
    /// Skia's `SkImageFilters::MatrixTransform`. See [`ImageFilter::matrix`].
    Matrix {
        /// `matrix` maps the input's local coordinates to where it shows,
        /// taken through the transform where the filter applies, so a scale
        /// about a point of the content stays about that point however the
        /// content is drawn.
        matrix: Matrix,
        /// `sampling` is how the moved input is read: its `filter` and
        /// `mipmap`, as dart:ui's `filterQuality` picks them. The filter
        /// draws the input's own pixels and nothing past them, so the tile
        /// modes go unused.
        sampling: Sampling,
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

    /// `runtime` creates a filter from a runtime effect with one draw's
    /// inputs.
    ///
    /// A filter's effect reads its input as `source`. A paint shader's effect
    /// is a filter by Flutter's convention, as `ImageFilter.shader` is: its
    /// first uniform is a `vec2<f32>` the renderer sets to the input's size in
    /// pixels, its first image is left unset (`EffectImage::image` is `None`)
    /// for the renderer to bind the input, sampled as that slot's `sampling`
    /// says, and `shade` is called at positions in the input's pixels. The
    /// same effect can then also fill shapes, with its first image set. Each
    /// kind refuses inputs that break its convention, as dart:ui refuses them.
    pub fn runtime(shader: crate::RuntimeShader) -> Result<Self, crate::EffectError> {
        match shader.effect.kind() {
            crate::EffectKind::Filter(_) => shader.check_images_set()?,
            crate::EffectKind::Shader => shader.check_filter_input()?,
        }
        Ok(Self::Runtime(shader))
    }

    /// `matrix` creates a filter that draws its input moved by `matrix`, read
    /// as `sampling` says: dart:ui's `ImageFilter.matrix(matrix4,
    /// filterQuality:)`.
    ///
    /// It is how Flutter's `Transform` with a `filterQuality` and its
    /// magnifier draw: the content is drawn first, then its pixels are moved,
    /// so a scale enlarges pixels rather than redrawing at the new size. An
    /// identity matrix, or one that is not finite, leaves the input as it
    /// is, as Flutter's `DlMatrixImageFilter::Make` makes no filter of it.
    pub fn matrix(matrix: Matrix, sampling: Sampling) -> Self {
        Self::Matrix { matrix, sampling }
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
            Self::Runtime(_) => false,
            Self::Matrix { matrix, .. } => {
                *matrix == Matrix::IDENTITY || !matrix.to_mat4().is_finite()
            }
        }
    }

    /// `map_local_bounds` is where the filter's output may show when its
    /// input shows within `bounds`, both in local coordinates: Flutter's
    /// `DlImageFilter::map_local_bounds`. A blur or a shadow grows them by
    /// how far it spreads, a matrix moves them, and a composition maps them
    /// through its inner filter and then its outer one.
    pub fn map_local_bounds(&self, bounds: Rect) -> Rect {
        match self {
            Self::Blur { .. } | Self::DropShadow { .. } => {
                let [x, y] = self.halo();
                Rect::from_ltrb(
                    bounds.x - x,
                    bounds.y - y,
                    bounds.right() + x,
                    bounds.bottom() + y,
                )
            }
            // It shades what its input covers; its margin is what it reads.
            // A matrix that moves nothing is no filter at all.
            Self::Color(_) | Self::Runtime(_) => bounds,
            Self::Matrix { .. } if self.is_nop() => bounds,
            Self::Compose { outer, inner } => {
                outer.map_local_bounds(inner.map_local_bounds(bounds))
            }
            Self::Matrix { matrix, .. } => matrix.map_rect(&bounds),
        }
    }

    /// `map_device_bounds` is where the filter's output may show when its
    /// input shows within `bounds`, both in device space, the filter
    /// applying where `transform` maps its local coordinates: Flutter's
    /// `DlImageFilter::map_device_bounds`. A matrix under a transform that
    /// cannot be inverted may show anywhere.
    pub fn map_device_bounds(&self, bounds: &Bounds, transform: &Matrix) -> Bounds {
        match self {
            Self::Blur { .. } | Self::DropShadow { .. } => {
                bounds.expand(device_reach(self.halo(), transform))
            }
            Self::Color(_) | Self::Runtime(_) => *bounds,
            Self::Matrix { .. } if self.is_nop() => *bounds,
            Self::Compose { outer, inner } => {
                outer.map_device_bounds(&inner.map_device_bounds(bounds, transform), transform)
            }
            Self::Matrix { matrix, .. } => match transform.invert() {
                Some(inverse) => bounds.map(&transform.then(matrix).then(&inverse)),
                None => Bounds::Unbounded,
            },
        }
    }

    /// `input_device_bounds` is what the filter's input has to cover for
    /// its output to cover `output`, both in device space, the filter
    /// applying where `transform` maps its local coordinates: Flutter's
    /// `DlImageFilter::get_input_device_bounds`. A matrix that cannot be
    /// inverted with the transform may need everything.
    pub fn input_device_bounds(&self, output: &Bounds, transform: &Matrix) -> Bounds {
        match self {
            Self::Blur { .. } | Self::DropShadow { .. } => {
                output.expand(device_reach(self.halo(), transform))
            }
            Self::Color(_) | Self::Runtime(_) => *output,
            Self::Matrix { .. } if self.is_nop() => *output,
            Self::Compose { outer, inner } => {
                inner.input_device_bounds(&outer.input_device_bounds(output, transform), transform)
            }
            Self::Matrix { matrix, .. } => match transform.then(matrix).invert() {
                Some(inverse) => output.map(&transform.then(&inverse)),
                None => Bounds::Unbounded,
            },
        }
    }

    /// `halo` is how far a blur or a drop shadow spreads its input, in local
    /// x and y units: three σ, and a shadow's offset; nothing for any other
    /// filter.
    fn halo(&self) -> [f32; 2] {
        match self {
            // The bounds limit what the blur reads, not how far it spreads.
            Self::Blur {
                sigma_x, sigma_y, ..
            } => [(sigma_x * 3.0).ceil(), (sigma_y * 3.0).ceil()],
            // The halo is symmetric, so a one-sided offset has to be paid on
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
            Self::Color(_) | Self::Compose { .. } | Self::Runtime(_) | Self::Matrix { .. } => {
                [0.0; 2]
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
            // Flutter's `DlRuntimeEffectImageFilter`: it shades only what its
            // input covers.
            Self::Runtime(_) => false,
            // Flutter's `DlMatrixImageFilter`: transparent pixels moved stay
            // transparent.
            Self::Matrix { .. } => false,
        }
    }
}

/// `device_reach` is how far a halo of local x and y units reaches in
/// device pixels under `transform`, on every side. The half-extent of an
/// axis-aligned box under a linear map is the component-wise absolute matrix
/// applied to the half-extent, so the bound holds under rotation and shear.
fn device_reach(halo: [f32; 2], transform: &Matrix) -> f32 {
    let [x, y] = halo;
    if x <= 0.0 && y <= 0.0 {
        return 0.0;
    }
    let [a, b, c, d, ..] = transform.to_affine();
    let device_x = (x * a).abs() + (y * c).abs();
    let device_y = (x * b).abs() + (y * d).abs();
    device_x.max(device_y)
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

    /// `effect_bounds` returns where a draw of `bounds` with this paint may
    /// show, in local coordinates, its effects included: its mask blur's
    /// spread, then its image filter's mapping (Flutter's
    /// `AdjustBoundsForPaint`).
    ///
    /// A draw whose paint fills its clip ([`Paint::draw_fills_clip`]) is
    /// unbounded: only its clip bounds it.
    pub fn effect_bounds(&self, bounds: Rect) -> Bounds {
        if self.draw_fills_clip() {
            return Bounds::Unbounded;
        }
        let blurred = bounds.expand(self.mask_padding());
        Bounds::of(match self.effective_image_filter() {
            Some(filter) => filter.map_local_bounds(blurred),
            None => blurred,
        })
    }

    /// `device_effect_bounds` is where a save layer composited with this
    /// paint shows when its content shows within `content`, device space:
    /// the content through the image filter applying under `transform`.
    pub(crate) fn device_effect_bounds(&self, content: &Bounds, transform: &Matrix) -> Bounds {
        match self.effective_image_filter() {
            Some(filter) => filter.map_device_bounds(content, transform),
            None => *content,
        }
    }

    /// `device_effect_input_bounds` is what a save layer composited with
    /// this paint has to hold for its composite to cover `output`, device
    /// space: what its image filter, applying under `transform`, reads.
    pub(crate) fn device_effect_input_bounds(&self, output: &Bounds, transform: &Matrix) -> Bounds {
        match self.effective_image_filter() {
            Some(filter) => filter.input_device_bounds(output, transform),
            None => *output,
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

    fn ltrb(left: f32, top: f32, right: f32, bottom: f32) -> valo_geometry::Rect {
        valo_geometry::Rect::from_ltrb(left, top, right, bottom)
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
        assert_eq!(
            filter.map_local_bounds(ltrb(0.0, 0.0, 10.0, 10.0)),
            ltrb(-15.0, -15.0, 25.0, 25.0)
        );
    }

    #[test]
    fn drop_shadow_bounds_cover_the_offset_on_both_sides() {
        let filter = ImageFilter::drop_shadow(
            valo_geometry::Point::new(4.0, -6.0),
            2.0,
            1.0,
            valo_geometry::Color::BLACK,
        );
        assert_eq!(
            filter.map_local_bounds(ltrb(0.0, 0.0, 10.0, 10.0)),
            ltrb(-10.0, -9.0, 20.0, 19.0)
        );
    }

    // A rotation reaches further than any axis length reports: `max_scale`
    // is 1 for a pure rotation, so scalar padding would clip this shadow.
    #[test]
    fn device_bounds_hold_a_rotated_effect() {
        use crate::Bounds;
        use valo_geometry::Matrix;
        let shadow = ImageFilter::drop_shadow(
            valo_geometry::Point::new(10.0, 10.0),
            0.0,
            0.0,
            valo_geometry::Color::BLACK,
        );
        let quarter_turn = Matrix::rotation(std::f32::consts::FRAC_PI_4);
        let content = Bounds::of(ltrb(0.0, 0.0, 10.0, 10.0));
        let reach = 14.142136;
        let Some(mapped) = shadow.map_device_bounds(&content, &quarter_turn).rect() else {
            panic!("a shadow of bounded content is bounded");
        };
        assert!(
            (mapped.x + reach).abs() < 1e-3 && (mapped.right() - 10.0 - reach).abs() < 1e-3,
            "a 45° rotation maps the (10, 10) halo to 14.14 on every side: {mapped:?}"
        );
        assert_eq!(
            shadow.input_device_bounds(&content, &quarter_turn),
            shadow.map_device_bounds(&content, &quarter_turn),
            "a blur reads as far as it spreads"
        );
    }

    #[test]
    fn a_mask_blur_grows_a_draw_by_three_sigma() {
        let paint = Paint {
            mask_blur: Some(MaskBlur::new(2.0)),
            ..Paint::default()
        };
        assert_eq!(paint.mask_padding(), 6.0);
        assert_eq!(
            paint.effect_bounds(ltrb(0.0, 0.0, 10.0, 10.0)),
            crate::Bounds::of(ltrb(-6.0, -6.0, 16.0, 16.0))
        );
    }

    /// Flutter's `DlMatrixImageFilter`: local bounds go through the matrix;
    /// device bounds through the matrix taken into device space,
    /// `ctm · matrix · ctm⁻¹`; what it reads is the output taken back,
    /// `ctm · (ctm · matrix)⁻¹`.
    #[test]
    fn a_matrix_filter_moves_its_bounds() {
        use crate::{Bounds, Sampling};
        use valo_geometry::Matrix;
        let scale_about_ten = Matrix::translation(10.0, 10.0)
            .then(&Matrix::scale(2.0, 2.0))
            .then(&Matrix::translation(-10.0, -10.0));
        let filter = ImageFilter::matrix(scale_about_ten, Sampling::default());
        assert_eq!(
            filter.map_local_bounds(ltrb(0.0, 0.0, 20.0, 20.0)),
            ltrb(-10.0, -10.0, 30.0, 30.0)
        );
        let ctm = Matrix::translation(100.0, 0.0).then(&Matrix::scale(3.0, 3.0));
        let device = Bounds::of(ctm.map_rect(&ltrb(0.0, 0.0, 20.0, 20.0)));
        assert_eq!(
            filter.map_device_bounds(&device, &ctm),
            Bounds::of(ctm.map_rect(&ltrb(-10.0, -10.0, 30.0, 30.0)))
        );
        assert_eq!(
            filter.input_device_bounds(
                &Bounds::of(ctm.map_rect(&ltrb(-10.0, -10.0, 30.0, 30.0))),
                &ctm
            ),
            device
        );
        assert!(!filter.modifies_transparent_black());
        let flat = ImageFilter::matrix(Matrix::scale(0.0, 1.0), Sampling::default());
        assert_eq!(flat.input_device_bounds(&device, &ctm), Bounds::Unbounded);
    }

    /// `DlMatrixImageFilter::Make` makes no filter of the identity or of a
    /// matrix that is not finite.
    #[test]
    fn an_identity_or_unfinite_matrix_filter_is_a_nop() {
        use crate::Sampling;
        use valo_geometry::Matrix;
        let nop = |matrix| ImageFilter::matrix(matrix, Sampling::default()).is_nop();
        assert!(nop(Matrix::IDENTITY));
        assert!(nop(Matrix::scale(f32::NAN, 1.0)));
        assert!(!nop(Matrix::scale(2.0, 2.0)));
        let composed = ImageFilter::compose(
            ImageFilter::blur(1.0, 1.0),
            ImageFilter::matrix(Matrix::scale(f32::NAN, 1.0), Sampling::default()),
        );
        assert_eq!(
            composed.map_local_bounds(ltrb(0.0, 0.0, 10.0, 10.0)),
            ltrb(-3.0, -3.0, 13.0, 13.0),
            "a composition takes only the side that is a filter"
        );
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
