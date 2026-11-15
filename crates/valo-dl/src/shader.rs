use valo_geometry::{Color, Matrix, Point};

/// `Shader` determines the color painted at each point of a drawing operation.
///
/// Shader coordinates begin in the draw's local coordinate space, so shaders
/// follow the same transforms as their geometry.
// Serialize ONLY, now that a pattern can hold an `Image`: the dump records a
// texture's identity, and no deserializer can turn that back into a live GPU
// handle. Same call `Path` already makes.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Shader {
    /// `Linear` interpolates colors along the line from `start` to `end`.
    Linear {
        /// `start` is the zero-offset point in local coordinates.
        start: Point,
        /// `end` is the one-offset point in local coordinates.
        end: Point,
        /// `stops` defines the colors along the gradient.
        stops: Vec<GradientStop>,
        /// `spread` controls colors outside the zero-to-one span.
        spread: SpreadMode,
        /// `local` transforms the shader independently of the drawn geometry.
        ///
        /// Use [`Matrix::IDENTITY`] when no additional transform is needed.
        local: Matrix,
    },
    /// `Radial` interpolates between a start circle and an end circle.
    Radial {
        /// `center` is the end circle's center in local coordinates.
        center: Point,
        /// `radius` is the end circle's radius in local coordinates.
        radius: f32,
        /// `stops` defines the colors along the gradient.
        stops: Vec<GradientStop>,
        /// `spread` controls colors outside the zero-to-one span.
        spread: SpreadMode,
        /// `focus` is the optional start circle.
        ///
        /// `None` starts at a zero-radius circle centered on `center`.
        focus: Option<FocalCircle>,
        /// `local` transforms the shader independently of the drawn geometry.
        local: Matrix,
    },
    /// `Sweep` interpolates colors around `center`, from `start_angle` to
    /// `end_angle`: Skia's `SkGradientShader::MakeSweep` and Flutter's
    /// `Gradient.sweep`.
    ///
    /// The angle of a point is measured from +x, clockwise in Valo's y-down
    /// coordinates, from 0 up to one full turn; it is not wrapped round to
    /// the start angle. So the offset at a point is how far its angle lies
    /// from `start_angle` toward `end_angle`, and points whose angle lies
    /// outside the two take their colour from `spread`. A full turn from
    /// zero (`0.0` to [`std::f32::consts::TAU`]) is the whole circle;
    /// a full turn from any other start with [`SpreadMode::Repeat`] is the
    /// whole circle rotated to begin at it, which is Canvas2D's
    /// `createConicGradient`.
    Sweep {
        /// `center` is the sweep origin in local coordinates.
        center: Point,
        /// `start_angle` is the angle of offset zero, in radians.
        start_angle: f32,
        /// `end_angle` is the angle of offset one, in radians; it must be
        /// greater than `start_angle`, as Skia and dart:ui require.
        end_angle: f32,
        /// `stops` defines the colors around the sweep.
        stops: Vec<GradientStop>,
        /// `spread` controls colors at angles outside the two.
        spread: SpreadMode,
        /// `local` transforms the shader independently of the drawn geometry.
        local: Matrix,
    },
    /// `Image` samples an image across the drawn geometry.
    Image {
        /// `image` supplies the sampled pixels.
        image: crate::Image,
        /// `sampling` controls filtering, mipmaps, and tiling.
        sampling: crate::Sampling,
        /// `local` transforms the image pattern independently of the geometry.
        local: Matrix,
    },
    /// `Runtime` colours each pixel with a runtime effect's `shade` function.
    /// The effect is made by
    /// [`RuntimeEffect::shader`](crate::RuntimeEffect::shader); see
    /// [`Shader::runtime`].
    Runtime(crate::RuntimeShader),
}

/// `SpreadMode` controls a gradient outside its zero-to-one span.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SpreadMode {
    /// `Pad` extends the nearest edge color.
    #[default]
    Pad,
    /// `Repeat` repeats the gradient in the same direction.
    Repeat,
    /// `Reflect` repeats the gradient with alternating direction.
    Reflect,
    /// `Decal` paints nothing outside the span: Skia's `SkTileMode::kDecal`
    /// and dart:ui's `TileMode.decal`. A decal gradient leaves transparent
    /// whatever its span does not reach, however opaque its stops.
    Decal,
}

/// `GradientStop` assigns a color to one position along a gradient.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GradientStop {
    /// `offset` is the position from zero to one.
    ///
    /// Supply stops in nondecreasing offset order.
    pub offset: f32,
    /// `color` is the straight-alpha sRGB color at this offset.
    pub color: Color,
}

/// `FocalCircle` defines the start circle of a radial gradient.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FocalCircle {
    /// `center` is the start circle's center in local coordinates.
    pub center: Point,
    /// `radius` is the start circle's radius.
    ///
    /// Zero represents a focal point.
    pub radius: f32,
}

impl FocalCircle {
    /// `point` creates a zero-radius focal circle.
    pub fn point(center: Point) -> Self {
        Self {
            center,
            radius: 0.0,
        }
    }
}

/// `MAX_GRADIENT_STOPS` is the largest gradient stored directly in uniforms.
///
/// Gradients with more stops use a cached texture ramp.
pub const MAX_GRADIENT_STOPS: usize = 8;

impl Shader {
    /// `stops` returns this gradient's stops or an empty slice for image shaders.
    pub fn stops(&self) -> &[GradientStop] {
        match self {
            Shader::Linear { stops, .. }
            | Shader::Radial { stops, .. }
            | Shader::Sweep { stops, .. } => stops,
            Shader::Image { .. } | Shader::Runtime(_) => &[],
        }
    }

    /// `spread` returns how this gradient spreads past its span, or `None`
    /// for image and runtime shaders.
    pub fn spread(&self) -> Option<SpreadMode> {
        match self {
            Shader::Linear { spread, .. }
            | Shader::Radial { spread, .. }
            | Shader::Sweep { spread, .. } => Some(*spread),
            Shader::Image { .. } | Shader::Runtime(_) => None,
        }
    }

    /// `fold_color_filter` applies a color filter directly to gradient stops.
    ///
    /// It returns `false` for image shaders, whose colors must be filtered
    /// during sampling.
    pub fn fold_color_filter(&mut self, filter: &crate::ColorFilter) -> bool {
        let stops = match self {
            Shader::Linear { stops, .. }
            | Shader::Radial { stops, .. }
            | Shader::Sweep { stops, .. } => stops,
            // A pattern's colours live in texels; the image fragment applies
            // the filter as it samples.
            Shader::Image { .. } | Shader::Runtime(_) => return false,
        };
        for stop in stops.iter_mut() {
            stop.color = filter.folded_into(stop.color);
        }
        true
    }

    /// `runtime` creates a shader from a runtime paint shader effect with one
    /// draw's inputs; a filter's effect, or an image left unset, is refused.
    pub fn runtime(shader: crate::RuntimeShader) -> Result<Self, crate::EffectError> {
        match shader.effect.kind() {
            crate::EffectKind::Shader => {
                shader.check_images_set()?;
                Ok(Shader::Runtime(shader))
            }
            crate::EffectKind::Filter(_) => Err(crate::EffectError::Contract(
                "a paint shader's effect is made by `RuntimeEffect::shader`".to_owned(),
            )),
        }
    }

    /// `linear` creates a two-color padded linear gradient.
    pub fn linear(start: Point, end: Point, from: Color, to: Color) -> Self {
        Shader::Linear {
            start,
            end,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: from,
                },
                GradientStop {
                    offset: 1.0,
                    color: to,
                },
            ],
            spread: SpreadMode::Pad,
            local: Matrix::IDENTITY,
        }
    }
}
