//! The by-value C vocabulary — `#[repr(C)]` structs and integer enums —
//! and the ONE place they convert into valo's types. Every enum keeps an
//! explicit, header-documented numbering; unknown values fall back to the
//! valo default rather than trapping (a C embedder passing garbage gets a
//! deterministic frame, not UB).

use valo::{BlendMode, BlurStyle, Color, MaskBlur, Matrix, Paint, PaintStyle, Rect, Stroke};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoColor {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoPoint {
    pub x: f32,
    pub y: f32,
}

/// Row-major 2×3 affine transform (the CSS/Skia 6-tuple).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoTransform {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub translate_x: f32,
    pub translate_y: f32,
}

/// Per-corner elliptical radii, clockwise from top-left — the full
/// CSS/Flutter rounded rect. Circular corners are `x == y`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoCornerRadii {
    pub top_left_x: f32,
    pub top_left_y: f32,
    pub top_right_x: f32,
    pub top_right_y: f32,
    pub bottom_right_x: f32,
    pub bottom_right_y: f32,
    pub bottom_left_x: f32,
    pub bottom_left_y: f32,
}

/// One draw's paint, by value. `style`: 0 fill, 1 stroke. `blend_mode`:
/// the 29 Skia modes in valo's order (header lists them). `mask_blur_sigma
/// <= 0` means no mask blur; `mask_blur_style`: 0 normal, 1 solid,
/// 2 inner, 3 outer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoPaint {
    pub color: ValoColor,
    pub blend_mode: i32,
    pub style: i32,
    pub stroke_width: f32,
    /// 0 butt, 1 round, 2 square.
    pub stroke_cap: i32,
    /// 0 miter, 1 round, 2 bevel.
    pub stroke_join: i32,
    pub stroke_miter_limit: f32,
    pub mask_blur_style: i32,
    pub mask_blur_sigma: f32,
}

impl From<ValoColor> for Color {
    fn from(c: ValoColor) -> Color {
        Color::rgba(c.red, c.green, c.blue, c.alpha)
    }
}

impl From<ValoRect> for Rect {
    fn from(r: ValoRect) -> Rect {
        Rect::new(r.x, r.y, r.width, r.height)
    }
}

impl From<ValoPoint> for valo::Point {
    fn from(p: ValoPoint) -> valo::Point {
        valo::Point::new(p.x, p.y)
    }
}

impl From<ValoTransform> for Matrix {
    fn from(t: ValoTransform) -> Matrix {
        Matrix::from_affine(t.a, t.b, t.c, t.d, t.translate_x, t.translate_y)
    }
}

impl ValoCornerRadii {
    pub(crate) fn to_elliptical(self) -> [[f32; 2]; 4] {
        [
            [self.top_left_x, self.top_left_y],
            [self.top_right_x, self.top_right_y],
            [self.bottom_right_x, self.bottom_right_y],
            [self.bottom_left_x, self.bottom_left_y],
        ]
    }
}

impl From<ValoPaint> for Paint {
    fn from(p: ValoPaint) -> Paint {
        Paint {
            color: p.color.into(),
            blend_mode: blend_mode(p.blend_mode),
            shader: None,
            mask_blur: mask_blur(p.mask_blur_style, p.mask_blur_sigma),
            style: paint_style(&p),
        }
    }
}

fn paint_style(p: &ValoPaint) -> PaintStyle {
    if p.style != 1 {
        return PaintStyle::Fill;
    }
    PaintStyle::Stroke(Stroke {
        width: p.stroke_width,
        cap: match p.stroke_cap {
            1 => valo::Cap::Round,
            2 => valo::Cap::Square,
            _ => valo::Cap::Butt,
        },
        join: match p.stroke_join {
            1 => valo::Join::Round,
            2 => valo::Join::Bevel,
            _ => valo::Join::Miter,
        },
        miter_limit: if p.stroke_miter_limit > 0.0 {
            p.stroke_miter_limit
        } else {
            4.0
        },
        dash: None,
    })
}

fn mask_blur(style: i32, sigma: f32) -> Option<MaskBlur> {
    if sigma <= 0.0 {
        return None;
    }
    let mut blur = MaskBlur::new(sigma);
    blur.style = match style {
        1 => BlurStyle::Solid,
        2 => BlurStyle::Inner,
        3 => BlurStyle::Outer,
        _ => BlurStyle::Normal,
    };
    Some(blur)
}

/// The 29 modes in valo-dl's declaration order — the header's enum table.
fn blend_mode(value: i32) -> BlendMode {
    use BlendMode::*;
    match value {
        0 => Clear,
        1 => Src,
        2 => Dst,
        3 => SrcOver,
        4 => DstOver,
        5 => SrcIn,
        6 => DstIn,
        7 => SrcOut,
        8 => DstOut,
        9 => SrcAtop,
        10 => DstAtop,
        11 => Xor,
        12 => Plus,
        13 => Modulate,
        14 => Screen,
        15 => Overlay,
        16 => Darken,
        17 => Lighten,
        18 => ColorDodge,
        19 => ColorBurn,
        20 => HardLight,
        21 => SoftLight,
        22 => Difference,
        23 => Exclusion,
        24 => Multiply,
        25 => Hue,
        26 => Saturation,
        27 => Color,
        28 => Luminosity,
        _ => SrcOver,
    }
}

/// 0 non-zero, 1 even-odd — the two fill rules.
pub(crate) fn fill_rule(value: i32) -> valo::FillRule {
    match value {
        1 => valo::FillRule::EvenOdd,
        _ => valo::FillRule::NonZero,
    }
}

/// 0 linear (mipmapped), 1 nearest — image sampling (tiling clamps; the
/// C surface grows tile modes when an embedder needs them).
pub(crate) fn sampling(value: i32) -> valo::Sampling {
    valo::Sampling {
        filter: match value {
            1 => valo::Filter::Nearest,
            _ => valo::Filter::Linear,
        },
        ..valo::Sampling::default()
    }
}

/// 0 intersect, 1 difference.
pub(crate) fn clip_op(value: i32) -> valo::ClipOp {
    match value {
        1 => valo::ClipOp::Difference,
        _ => valo::ClipOp::Intersect,
    }
}
