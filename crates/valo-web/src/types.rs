use valo::{
    BlendMode, BlurStyle, Cap, ClipOp, FillRule, Filter, Join, Matrix, MipmapMode, Sampling,
    SpreadMode, TileMode,
};
use wasm_bindgen::prelude::*;

pub(crate) fn matrix(values: &[f32]) -> Result<Matrix, JsValue> {
    match values {
        [a, b, c, d, translate_x, translate_y] => Ok(Matrix::from_affine(
            *a,
            *b,
            *c,
            *d,
            *translate_x,
            *translate_y,
        )),
        values if values.len() == 16 => {
            let values: [f32; 16] = values.try_into().expect("length checked");
            Ok(Matrix::from_flutter_array(&values))
        }
        _ => Err(JsValue::from_str(
            "a transform needs 6 affine or 16 matrix values",
        )),
    }
}

pub(crate) fn blend_mode(value: u32) -> BlendMode {
    use BlendMode::*;
    [
        Clear, Src, Dst, SrcOver, DstOver, SrcIn, DstIn, SrcOut, DstOut, SrcAtop, DstAtop, Xor,
        Plus, Modulate, Screen, Overlay, Darken, Lighten, ColorDodge, ColorBurn, HardLight,
        SoftLight, Difference, Exclusion, Multiply, Hue, Saturation, Color, Luminosity,
    ]
    .get(value as usize)
    .copied()
    .unwrap_or(SrcOver)
}

pub(crate) fn blur_style(value: u32) -> BlurStyle {
    [
        BlurStyle::Normal,
        BlurStyle::Solid,
        BlurStyle::Inner,
        BlurStyle::Outer,
    ]
    .get(value as usize)
    .copied()
    .unwrap_or_default()
}

pub(crate) fn cap(value: u32) -> Cap {
    [Cap::Butt, Cap::Round, Cap::Square]
        .get(value as usize)
        .copied()
        .unwrap_or_default()
}

pub(crate) fn join(value: u32) -> Join {
    [Join::Miter, Join::Round, Join::Bevel]
        .get(value as usize)
        .copied()
        .unwrap_or_default()
}

pub(crate) fn fill_rule(value: u32) -> FillRule {
    if value == 1 {
        FillRule::EvenOdd
    } else {
        FillRule::NonZero
    }
}

pub(crate) fn clip_op(value: u32) -> ClipOp {
    if value == 1 {
        ClipOp::Difference
    } else {
        ClipOp::Intersect
    }
}

pub(crate) fn spread_mode(value: u32) -> SpreadMode {
    [SpreadMode::Pad, SpreadMode::Repeat, SpreadMode::Reflect]
        .get(value as usize)
        .copied()
        .unwrap_or_default()
}

pub(crate) fn sampling(filter: u32, mipmap: u32, tile_x: u32, tile_y: u32) -> Sampling {
    Sampling {
        filter: if filter == 1 {
            Filter::Nearest
        } else {
            Filter::Linear
        },
        mipmap: mipmap_mode(mipmap),
        tile_x: tile_mode(tile_x),
        tile_y: tile_mode(tile_y),
    }
}

fn mipmap_mode(value: u32) -> MipmapMode {
    [MipmapMode::None, MipmapMode::Nearest, MipmapMode::Linear]
        .get(value as usize)
        .copied()
        .unwrap_or_default()
}

fn tile_mode(value: u32) -> TileMode {
    [
        TileMode::Clamp,
        TileMode::Repeat,
        TileMode::Mirror,
        TileMode::Decal,
    ]
    .get(value as usize)
    .copied()
    .unwrap_or_default()
}
