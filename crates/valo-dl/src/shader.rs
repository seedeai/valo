use valo_geometry::{Color, Matrix, Point};

/// Paint's color source beyond a solid color — the per-PIXEL families.
/// Geometry is in the DRAW's local space (the same space its
/// rect/path lives in), so gradients rotate/scale with their shape.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
        /// SVG's focal point (`fx`/`fy`, radius-0 inner circle): stops
        /// emanate from here instead of the center. Must lie INSIDE the
        /// circle; `None` = the classic centered gradient. This is the
        /// r0=0 two-point conical — the general form stays deferred.
        focus: Option<Point>,
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

pub const MAX_GRADIENT_STOPS: usize = 8;

impl Shader {
    pub fn stops(&self) -> &[GradientStop] {
        match self {
            Shader::Linear { stops, .. }
            | Shader::Radial { stops, .. }
            | Shader::Sweep { stops, .. } => stops,
        }
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
