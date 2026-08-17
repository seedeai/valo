/// `Color` is a straight-alpha sRGB color.
///
/// Components conventionally range from zero to one but are not clamped by the
/// constructors. Valo premultiplies at the GPU boundary and blends in sRGB space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Color {
    /// `r` is the straight red component.
    pub r: f32,
    /// `g` is the straight green component.
    pub g: f32,
    /// `b` is the straight blue component.
    pub b: f32,
    /// `a` is the alpha component.
    pub a: f32,
}

impl Color {
    /// `TRANSPARENT` is transparent black.
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);
    /// `BLACK` is opaque black.
    pub const BLACK: Color = Color::rgba(0.0, 0.0, 0.0, 1.0);
    /// `WHITE` is opaque white.
    pub const WHITE: Color = Color::rgba(1.0, 1.0, 1.0, 1.0);

    /// `rgba` creates a color without clamping its components.
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// `rgb` creates an opaque color without clamping its components.
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// `from_rgba8` converts 8-bit sRGB components to floating point.
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    /// `with_alpha` replaces the alpha component without changing RGB.
    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// `components` returns straight components as `[r, g, b, a]`.
    pub fn components(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// `premultiplied` returns `[r × a, g × a, b × a, a]`.
    pub fn premultiplied(self) -> [f32; 4] {
        [self.r * self.a, self.g * self.a, self.b * self.a, self.a]
    }

    /// `is_opaque` reports whether alpha is at least one.
    pub fn is_opaque(self) -> bool {
        self.a >= 1.0
    }
}
