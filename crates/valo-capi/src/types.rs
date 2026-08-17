//! The by-value C vocabulary — `#[repr(C)]` structs and integer enums —
//! and the ONE place they convert into valo's types. Every enum keeps an
//! explicit, header-documented numbering; unknown values fall back to the
//! valo default rather than trapping (a C embedder passing garbage gets a
//! deterministic frame, not UB).

use valo::{BlendMode, BlurStyle, Color, MaskBlur, Matrix, Paint, PaintStyle, Rect, Stroke};

/// `ValoColor` is a straight-alpha sRGB color passed by value.
///
/// Components conventionally range from 0 to 1 and are not clamped here.
/// Valo premultiplies at the GPU boundary and blends in sRGB.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoColor {
    /// `red` is the straight red component.
    pub red: f32,
    /// `green` is the straight green component.
    pub green: f32,
    /// `blue` is the straight blue component.
    pub blue: f32,
    /// `alpha` is the opacity.
    pub alpha: f32,
}

/// `ValoRect` is an axis-aligned rectangle in Valo's y-down coordinates, passed by value.
///
/// Origin is the top-left; `width` and `height` are extents in logical pixels.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoRect {
    /// `x` is the left edge in logical pixels.
    pub x: f32,
    /// `y` is the top edge in logical pixels.
    pub y: f32,
    /// `width` is the horizontal extent in logical pixels.
    pub width: f32,
    /// `height` is the vertical extent in logical pixels.
    pub height: f32,
}

/// `ValoPoint` is a 2D position or vector in logical pixels, passed by value.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoPoint {
    /// `x` is the horizontal component.
    pub x: f32,
    /// `y` is the vertical component (down is positive).
    pub y: f32,
}

/// `ValoTransform` is a row-major 2×3 affine transform (the CSS/Skia 6-tuple).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoTransform {
    /// `a` is the horizontal scaling component (`xx`).
    pub a: f32,
    /// `b` is the vertical skew component (`xy`).
    pub b: f32,
    /// `c` is the horizontal skew component (`yx`).
    pub c: f32,
    /// `d` is the vertical scaling component (`yy`).
    pub d: f32,
    /// `translate_x` is the horizontal translation in logical pixels.
    pub translate_x: f32,
    /// `translate_y` is the vertical translation in logical pixels.
    pub translate_y: f32,
}

/// `ValoCornerRadii` holds per-corner elliptical radii, clockwise from top-left.
///
/// Circular corners are `x == y`. Radii that would overlap are reduced
/// proportionally when the rounded rect is built.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoCornerRadii {
    /// `top_left_x` is the top-left horizontal radius.
    pub top_left_x: f32,
    /// `top_left_y` is the top-left vertical radius.
    pub top_left_y: f32,
    /// `top_right_x` is the top-right horizontal radius.
    pub top_right_x: f32,
    /// `top_right_y` is the top-right vertical radius.
    pub top_right_y: f32,
    /// `bottom_right_x` is the bottom-right horizontal radius.
    pub bottom_right_x: f32,
    /// `bottom_right_y` is the bottom-right vertical radius.
    pub bottom_right_y: f32,
    /// `bottom_left_x` is the bottom-left horizontal radius.
    pub bottom_left_x: f32,
    /// `bottom_left_y` is the bottom-left vertical radius.
    pub bottom_left_y: f32,
}

/// `ValoPaint` describes one draw's paint, passed by value.
///
/// There is no retained paint object and no shader field — C paints are
/// solid color, stroke, blend, mask blur, and an optional borrowed color
/// filter. Unknown integer enums take the valo default (fill, srcOver,
/// butt, miter, normal blur) rather than trapping. The 29 blend modes
/// are numbered in `include/valo.h`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ValoPaint {
    /// `color` is the solid-draw color.
    ///
    /// Shader and image draws use only its alpha and ignore its RGB channels.
    pub color: ValoColor,
    /// `blend_mode` indexes the 29 Skia modes: 0 clear … 3 srcOver (default) … 28 luminosity.
    pub blend_mode: i32,
    /// `style` is 0 fill (default) or 1 stroke.
    pub style: i32,
    /// `stroke_width` is the stroke thickness in local units.
    ///
    /// Zero is a hairline (one device pixel); only a negative width draws nothing.
    pub stroke_width: f32,
    /// `stroke_cap` is 0 butt (default), 1 round, or 2 square.
    pub stroke_cap: i32,
    /// `stroke_join` is 0 miter (default), 1 round, or 2 bevel.
    pub stroke_join: i32,
    /// `stroke_miter_limit` caps miter joins; values `<= 0` become 4.
    pub stroke_miter_limit: f32,
    /// `mask_blur_style` is 0 normal (default), 1 solid, 2 inner, or 3 outer.
    pub mask_blur_style: i32,
    /// `mask_blur_sigma` is the Gaussian sigma in local units; `<= 0` means no mask blur.
    pub mask_blur_sigma: f32,
    /// `color_filter` is a borrowed [`crate::ValoColorFilter`], or null.
    ///
    /// Recolours what this paint drew, before mask blur spreads it. The handle
    /// only has to outlive the call — the draw copies what it needs.
    pub color_filter: *const crate::ValoColorFilter,
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

/// # Safety
/// `p.color_filter` must be null or a live [`crate::ValoColorFilter`] handle.
pub(crate) unsafe fn paint_of(p: ValoPaint) -> Paint {
    Paint {
        color: p.color.into(),
        blend_mode: blend_mode(p.blend_mode),
        shader: None,
        mask_blur: mask_blur(p.mask_blur_style, p.mask_blur_sigma),
        color_filter: unsafe { crate::color_filter_of(p.color_filter) },
        image_filter: None,
        style: paint_style(&p),
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
pub(crate) fn blend_mode(value: i32) -> BlendMode {
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
