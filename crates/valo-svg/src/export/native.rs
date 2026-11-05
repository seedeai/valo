//! What an SVG document draws itself, as far as [`NativeEffects`] lets it: the rule both passes
//! walk a list by, and the filters SVG says effects with.

use valo::{
    Backdrop, BlendMode, BlurStyle, Color, ColorFilter, ImageFilter, Op, Paint, Shader, TileMode,
};

use super::NativeEffects;

/// `Filter` is an effect as an SVG filter says it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Filter {
    /// `Blur` blurs what it filters by a sigma for each axis.
    Blur([f32; 2]),
    /// `Shadow` blurs what it filters and fills it with one colour: a group's or a text's
    /// shadow.
    Shadow([f32; 2], Color),
    /// `Matrix` recolours by a row-major 4×5 colour matrix over straight colour.
    Matrix([f32; 20]),
    /// `Flood` fills what it filters with one colour.
    Flood(Color),
}

impl Filter {
    /// `reach` is how far the filter spreads what it filters.
    pub fn reach(&self) -> f32 {
        match self {
            Filter::Blur([x, y]) | Filter::Shadow([x, y], _) => 3.0 * x.max(*y),
            Filter::Matrix(_) | Filter::Flood(_) => 0.0,
        }
    }
}

/// `rasterized` is whether a document drawn with `native` leaves `op` and the `scope` it opens to
/// a picture: an image, a sweep gradient or a shader SVG has no word for, a blend mode or filter
/// SVG can't say, or an effect `native` leaves out.
pub(super) fn rasterized(op: &Op, scope: &[Op], native: NativeEffects) -> bool {
    match op {
        Op::SaveLayer {
            mask_composite: Some(_),
            ..
        } => !native.masks,
        Op::SaveLayer {
            paint, backdrop, ..
        } => {
            (holds_a_mask(scope) && !native.masks)
                || backdrop
                    .as_ref()
                    .is_some_and(|backdrop| backdrop_blur(backdrop, native).is_none())
                || blend_name(paint.blend_mode).is_none()
                || layer_filter(paint, native).is_none()
        }
        Op::RRectBlur { paint, .. }
        | Op::DrawRect { paint, .. }
        | Op::DrawPath { paint, .. }
        | Op::GlyphRun { paint, .. } => !paints_natively(paint, native),
        Op::DrawImage { .. } => true,
        Op::Save
        | Op::Restore
        | Op::Transform(_)
        | Op::ClipPath { .. }
        | Op::DrawDisplayList { .. } => false,
    }
}

/// `holds_a_mask` is whether a mask layer opens directly inside the layer `scope` opens: a mask
/// applies to the layer it is in.
pub(super) fn holds_a_mask(scope: &[Op]) -> bool {
    let mut depth = 0usize;
    for op in scope {
        match op {
            Op::SaveLayer {
                mask_composite: Some(_),
                ..
            } if depth == 1 => return true,
            Op::Save | Op::SaveLayer { .. } => depth += 1,
            Op::Restore => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

fn paints_natively(paint: &Paint, native: NativeEffects) -> bool {
    blend_name(paint.blend_mode).is_some()
        && matches!(
            paint.shader,
            None | Some(Shader::Linear { .. } | Shader::Radial { .. })
        )
        && draw_filter(paint, native).is_some()
}

/// `draw_filter` is the filter a draw's paint needs: `Some(None)` for none, `None` when SVG can't
/// say it or `native` leaves it to a picture.
pub(super) fn draw_filter(paint: &Paint, native: NativeEffects) -> Option<Option<Filter>> {
    match (
        paint.mask_blur,
        paint.effective_image_filter(),
        &paint.color_filter,
    ) {
        (None, None, None) => Some(None),
        (Some(blur), None, None) => (native.shadows && blur.style == BlurStyle::Normal)
            .then_some(Some(Filter::Blur([blur.sigma; 2]))),
        (None, Some(filter), None) => native
            .layer_blurs
            .then(|| plain_blur(filter))
            .flatten()
            .map(|sigma| Some(Filter::Blur(sigma))),
        (None, None, Some(color)) => native
            .color_filters
            .then(|| recolor(color))
            .flatten()
            .map(Some),
        _ => None,
    }
}

/// `layer_filter` is the filter a layer's paint needs: `Some(None)` for none, `None` when SVG
/// can't say it or `native` leaves it to a picture. A blur with everything in one colour is a
/// shadow.
pub(super) fn layer_filter(paint: &Paint, native: NativeEffects) -> Option<Option<Filter>> {
    match (paint.effective_image_filter(), &paint.color_filter) {
        (None, None) => Some(None),
        (Some(filter), None) => native
            .layer_blurs
            .then(|| plain_blur(filter))
            .flatten()
            .map(|sigma| Some(Filter::Blur(sigma))),
        (Some(filter), Some(ColorFilter::Blend(color, BlendMode::SrcIn))) => native
            .shadows
            .then(|| plain_blur(filter))
            .flatten()
            .map(|sigma| Some(Filter::Shadow(sigma, *color))),
        (None, Some(color)) => native
            .color_filters
            .then(|| recolor(color))
            .flatten()
            .map(Some),
        (Some(_), Some(_)) => None,
    }
}

/// `backdrop_blur` is the sigma of a backdrop CSS's `backdrop-filter` can blur, when `native`
/// lets it.
pub(super) fn backdrop_blur(backdrop: &Backdrop, native: NativeEffects) -> Option<f32> {
    match backdrop.filter {
        ImageFilter::Blur {
            sigma_x,
            sigma_y,
            bounds: None,
            ..
        } if native.backdrop_blurs && sigma_x == sigma_y => Some(sigma_x),
        _ => None,
    }
}

/// `blend_name` is CSS's name for a blend mode; `None` for a Porter-Duff mode CSS has no word
/// for.
pub(super) fn blend_name(mode: BlendMode) -> Option<&'static str> {
    Some(match mode {
        BlendMode::SrcOver => "normal",
        BlendMode::Multiply => "multiply",
        BlendMode::Screen => "screen",
        BlendMode::Overlay => "overlay",
        BlendMode::Darken => "darken",
        BlendMode::Lighten => "lighten",
        BlendMode::ColorDodge => "color-dodge",
        BlendMode::ColorBurn => "color-burn",
        BlendMode::HardLight => "hard-light",
        BlendMode::SoftLight => "soft-light",
        BlendMode::Difference => "difference",
        BlendMode::Exclusion => "exclusion",
        BlendMode::Hue => "hue",
        BlendMode::Saturation => "saturation",
        BlendMode::Color => "color",
        BlendMode::Luminosity => "luminosity",
        _ => return None,
    })
}

/// `plain_blur` is the sigmas of a blur that reads everything and leaves transparent what lies
/// past its edge, as SVG's does.
fn plain_blur(filter: &ImageFilter) -> Option<[f32; 2]> {
    match filter {
        ImageFilter::Blur {
            sigma_x,
            sigma_y,
            bounds: None,
            tile_mode: None | Some(TileMode::Decal),
        } => Some([*sigma_x, *sigma_y]),
        _ => None,
    }
}

fn recolor(color: &ColorFilter) -> Option<Filter> {
    match color {
        ColorFilter::Matrix(matrix) => Some(Filter::Matrix(*matrix)),
        ColorFilter::Blend(color, BlendMode::SrcIn) => Some(Filter::Flood(*color)),
        _ => None,
    }
}
