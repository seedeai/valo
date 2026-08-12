use valo_geometry::{Color, Matrix, Point};

/// Paint's color source beyond a solid color — the per-PIXEL families.
/// Geometry is in the DRAW's local space (the same space its
/// rect/path lives in), so gradients rotate/scale with their shape.
// Serialize ONLY, now that a pattern can hold an `Image`: the dump records a
// texture's identity, and no deserializer can turn that back into a live GPU
// handle. Same call `Path` already makes.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Shader {
    Linear {
        start: Point,
        end: Point,
        stops: Vec<GradientStop>,
        spread: SpreadMode,
        /// Skia-style LOCAL MATRIX: the gradient evaluates in its own
        /// space (`p' = local⁻¹ · p`), so skewed/stretched fields — SVG's
        /// gradientTransform, elliptical radials — stay exact. IDENTITY
        /// for a plain gradient.
        local: Matrix,
    },
    Radial {
        center: Point,
        radius: f32,
        stops: Vec<GradientStop>,
        spread: SpreadMode,
        /// The gradient's START circle — Canvas2D's `(x0, y0, r0)` and
        /// SVG's focal point. `None` is the classic centred gradient, where
        /// the ramp runs from the centre out to `radius`.
        focus: Option<FocalCircle>,
        /// See `Linear::local` — this is how a radial becomes an ellipse.
        local: Matrix,
    },
    /// Full-turn sweep starting at `start_angle` (radians, clockwise from +x).
    /// Inherently periodic — no spread mode.
    Sweep {
        center: Point,
        start_angle: f32,
        stops: Vec<GradientStop>,
        /// See `Linear::local`.
        local: Matrix,
    },
    /// An image tiled across the shape — Canvas2D's `createPattern`, Skia's
    /// `SkImageShader`. `sampling` carries the per-axis tile modes and the
    /// filter, so tiling costs nothing extra: it rides the sampler's address
    /// modes exactly as an image draw's does.
    Image {
        image: crate::Image,
        sampling: crate::Sampling,
        /// See `Linear::local` — the pattern evaluates in its own space, so a
        /// rotated or scaled tiling stays exact.
        local: Matrix,
    },
}

/// What lives outside the gradient's 0..1 span (SVG `spreadMethod`, Skia's
/// shader `SkTileMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SpreadMode {
    /// Edge stops extend forever — the CSS/Skia clamp default.
    #[default]
    Pad,
    /// The ramp tiles: …0‥1 0‥1…
    Repeat,
    /// Every other tile mirrors: …0‥1 1‥0…
    Reflect,
}

/// Up to [`MAX_GRADIENT_STOPS`] uniform stops per gradient — the
/// uniform-stop family; SSBO/texture-ramp fallbacks are a later tier.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GradientStop {
    /// 0..=1 along the gradient; callers should supply sorted offsets.
    pub offset: f32,
    pub color: Color,
}

/// A two-point conical gradient's start circle. A zero `radius` is SVG's
/// focal point (`fx`/`fy`); any positive radius is the general form, which
/// is what Canvas2D's `createRadialGradient` describes.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FocalCircle {
    pub center: Point,
    pub radius: f32,
}

impl FocalCircle {
    /// SVG's focal point: a start circle with no radius.
    pub fn point(center: Point) -> Self {
        Self {
            center,
            radius: 0.0,
        }
    }
}

pub const MAX_GRADIENT_STOPS: usize = 8;

impl Shader {
    /// A gradient's stops; empty for families that have none.
    pub fn stops(&self) -> &[GradientStop] {
        match self {
            Shader::Linear { stops, .. }
            | Shader::Radial { stops, .. }
            | Shader::Sweep { stops, .. } => stops,
            Shader::Image { .. } => &[],
        }
    }

    /// Fold a colour filter into this source, Impeller's
    /// `Contents::ApplyColorFilter`: a gradient filters its STOP COLOURS,
    /// matching Impeller's gradient-source semantics (including clamping each
    /// transformed stop before interpolation). Returns false when the source
    /// cannot answer on the CPU and needs a texture snapshot instead.
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

    /// Two-color convenience: `from` at 0, `to` at 1.
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
