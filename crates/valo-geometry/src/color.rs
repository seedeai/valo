/// Straight (unpremultiplied) sRGB color, components 0..=1.
///
/// Premultiplication happens at the GPU boundary (`premultiplied()`), and valo
/// blends in sRGB space — the CSS/Skia-compatible look. Linear-light blending
/// and wide gamut are deliberately deferred: when they land, this type stays and
/// the conversion moves into the uniform-fill path.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Color = Color::rgba(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Color = Color::rgba(1.0, 1.0, 1.0, 1.0);

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// From 8-bit sRGB (the CSS `#rrggbbaa` layout).
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Straight components in draw order `[r, g, b, a]`.
    pub fn components(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Alpha-premultiplied components in draw order `[r, g, b, a]` — what the
    /// uniform fill hands the blender.
    pub fn premultiplied(self) -> [f32; 4] {
        [self.r * self.a, self.g * self.a, self.b * self.a, self.a]
    }

    pub fn is_opaque(self) -> bool {
        self.a >= 1.0
    }
}
