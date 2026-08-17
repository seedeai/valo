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
    /// `Sweep` interpolates colors around one full clockwise turn.
    Sweep {
        /// `center` is the sweep origin in local coordinates.
        center: Point,
        /// `start_angle` is the zero-offset angle in radians clockwise from +x.
        start_angle: f32,
        /// `stops` defines the colors around the sweep.
        stops: Vec<GradientStop>,
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
            Shader::Image { .. } => &[],
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
            Shader::Image { .. } => return false,
        };
        let mut folded = Vec::with_capacity(stops.len());
        for stop in stops.iter() {
            match filter.folded_into(stop.color) {
                Some(color) => folded.push(GradientStop { color, ..*stop }),
                None => return false,
            }
        }
        *stops = folded;
        true
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
