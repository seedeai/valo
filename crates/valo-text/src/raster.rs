use std::sync::Arc;

use skrifa::instance::Size;
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::MetadataProvider;
use valo_geometry::{Path, PathBuilder, Rect};

use crate::font::Font;

/// SDF spread in texels: distance saturates ±this many pixels from the edge
/// (0.5 = on the edge). Also the raster padding so the field has room.
pub const SDF_PAD: u32 = 8;

/// A rasterized glyph: A8 coverage (or normalized distance for SDF), plus
/// the placement of the bitmap's top-left relative to the glyph origin
/// (`left` right of origin, `top` above the baseline — swash conventions).
pub struct GlyphImage {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub data: Vec<u8>,
}

/// CPU glyph rasterization, on swash. One per renderer — swash's context
/// caches scaling state.
#[derive(Default)]
pub struct Rasterizer {
    context: swash::scale::ScaleContext,
}

impl Rasterizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Plain alpha coverage at `px` — the mask tier. `dx` is the subpixel
    /// x-phase (0/¼/½/¾ px) baked into the raster, Skia/Impeller's
    /// quarter-pixel positioning.
    pub fn alpha(&mut self, font: &Font, glyph: u32, px: f32, dx: f32) -> Option<GlyphImage> {
        let image = self.render(font, glyph, px, dx)?;
        Some(GlyphImage {
            width: image.placement.width,
            height: image.placement.height,
            left: image.placement.left,
            top: image.placement.top,
            data: image.data,
        })
    }

    /// Signed distance field at `px`: the 1× AA coverage seeds the exact
    /// EDT directly (mapbox TinySDF's shape — partial alpha carries the
    /// sub-pixel edge, so no supersample; ~7× the old
    /// 2×-8SSEDT pipeline). 128 = edge, ±[`SDF_PAD`] px span the range.
    pub fn sdf(&mut self, font: &Font, glyph: u32, px: f32) -> Option<GlyphImage> {
        let alpha = self.render(font, glyph, px, 0.0)?;
        let pad = SDF_PAD;
        let w = alpha.placement.width + 2 * pad;
        let h = alpha.placement.height + 2 * pad;
        let mut coverage = vec![0u8; (w * h) as usize];
        for y in 0..alpha.placement.height {
            for x in 0..alpha.placement.width {
                coverage[((y + pad) * w + x + pad) as usize] =
                    alpha.data[(y * alpha.placement.width + x) as usize];
            }
        }
        let field = crate::sdf::signed_distances(&coverage, w as usize, h as usize);
        Some(GlyphImage {
            width: w,
            height: h,
            left: alpha.placement.left - pad as i32,
            top: alpha.placement.top + pad as i32,
            data: crate::sdf::encode(&field, SDF_PAD as f32),
        })
    }

    /// Color glyph (COLR outlines / CBDT-sbix bitmaps) at `px`: premultiplied
    /// RGBA, or `None` when the glyph has no color form — the caller falls
    /// back to the mask tiers. Mini rendered emoji through Canvas2D; swash
    /// is the native replacement.
    pub fn color(&mut self, font: &Font, glyph: u32, px: f32) -> Option<GlyphImage> {
        let font_ref = swash::FontRef::from_index(font.data(), font.face_index() as usize)?;
        let mut scaler = self
            .context
            .builder(font_ref)
            .size(px)
            .hint(false)
            .variations(swash_variations(font))
            .build();
        let image = swash::scale::Render::new(&[
            swash::scale::Source::ColorOutline(0),
            swash::scale::Source::ColorBitmap(swash::scale::StrikeWith::BestFit),
        ])
        .render(&mut scaler, glyph as swash::GlyphId);
        let Some(image) = image else {
            // swash covers CBDT bitmaps and COLRv0 layers; COLRv1 paint
            // graphs raster through the skrifa painter.
            return crate::colr::raster(font, glyph, px);
        };
        if image.content != swash::scale::image::Content::Color {
            return crate::colr::raster(font, glyph, px);
        }
        let mut data = image.data;
        for px in data.chunks_exact_mut(4) {
            let a = px[3] as u32;
            // Round half up — truncation biases emoji a hair dark and
            // breaks the round-trip with export's unpremultiply.
            px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
            px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
            px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
        }
        Some(GlyphImage {
            width: image.placement.width,
            height: image.placement.height,
            left: image.placement.left,
            top: image.placement.top,
            data,
        })
    }

    /// Tight non-transparent pixel bounds for a bitmap/color glyph, relative
    /// to its baseline origin in Valo's y-down coordinates. Metrics query this
    /// before a monochrome outline because rendering also prefers COLR/CBDT.
    pub(crate) fn color_bounds(&mut self, font: &Font, glyph: u32, px: f32) -> Option<Rect> {
        let image = self.color(font, glyph, px)?;
        let mut left = image.width;
        let mut top = image.height;
        let mut right = 0;
        let mut bottom = 0;
        for y in 0..image.height {
            for x in 0..image.width {
                let alpha = image.data[((y * image.width + x) * 4 + 3) as usize];
                if alpha == 0 {
                    continue;
                }
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
        (left < right && top < bottom).then(|| {
            Rect::new(
                image.left as f32 + left as f32,
                -image.top as f32 + top as f32,
                (right - left) as f32,
                (bottom - top) as f32,
            )
        })
    }

    fn render(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        dx: f32,
    ) -> Option<swash::scale::image::Image> {
        let font_ref = swash::FontRef::from_index(font.data(), font.face_index() as usize)?;
        let mut scaler = self
            .context
            .builder(font_ref)
            .size(px)
            .hint(false)
            .variations(swash_variations(font))
            .build();
        swash::scale::Render::new(&[swash::scale::Source::Outline])
            .offset(swash::zeno::Vector::new(dx, 0.0))
            .render(&mut scaler, glyph as swash::GlyphId)
    }
}

/// The glyph as a valo `Path` at `px`, baseline-origin, y-down — the huge-
/// text tier: stencil-then-cover handles it like any shape.
pub fn glyph_path(font: &Font, glyph: u32, px: f32) -> Option<Arc<Path>> {
    let font_ref = skrifa::FontRef::from_index(font.data(), font.face_index()).ok()?;
    let outline = font_ref.outline_glyphs().get(skrifa::GlyphId::new(glyph))?;
    let mut pen = PathPen {
        builder: PathBuilder::new(),
    };
    outline
        .draw(
            DrawSettings::unhinted(Size::new(px), font.variation_location()),
            &mut pen,
        )
        .ok()?;
    let path = pen.builder.build();
    // COLR fonts carry EMPTY classic outlines for their color glyphs — an
    // empty path IS "no outline", so callers take their color fallback
    // instead of stencil-filling nothing (the >path_min emoji vanish bug).
    (!path.is_empty()).then_some(path)
}

/// skrifa pen → PathBuilder, flipping y (fonts are y-up, canvases y-down).
struct PathPen {
    builder: PathBuilder,
}

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to((x, -y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to((x, -y));
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.builder.quad_to((cx, -cy), (x, -y));
    }

    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        self.builder.cubic_to((c0x, -c0y), (c1x, -c1y), (x, -y));
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

/// A font's variation coordinates in swash's setting form (named
/// instances rasterize at their own axis positions).
fn swash_variations(font: &Font) -> impl Iterator<Item = (&str, f32)> + '_ {
    font.variation_coordinates()
        .iter()
        .filter_map(|(tag, value)| Some((std::str::from_utf8(tag).ok()?, *value)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FaceSet;

    fn fira() -> FaceSet {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/fira_sans.ttf"
        );
        let mut c = FaceSet::default();
        c.register("Fira Sans", std::fs::read(path).unwrap())
            .unwrap();
        c
    }

    /// Tier continuity (005-B7): the SDF raster places its glyph where the
    /// mask raster does — minus the SDF pad, within the 2×-downsample's
    /// half-pixel. An extra bias here makes text POP vertically when zoom
    /// crosses the mask→SDF threshold.
    #[test]
    fn sdf_and_mask_tiers_agree_on_placement() {
        let fonts = fira();
        let font = fonts.family("Fira Sans").unwrap();
        let mut raster = Rasterizer::new();
        for ch in ['H', 'g', 'x', 'Q'] {
            let glyph = fonts.get(font).glyph_for(ch).unwrap();
            let alpha = raster.alpha(fonts.get(font), glyph, 64.0, 0.0).unwrap();
            let sdf = raster.sdf(fonts.get(font), glyph, 64.0).unwrap();
            let pad = SDF_PAD as i32;
            for (axis, a, s) in [
                ("top", alpha.top, sdf.top - pad),
                ("left", alpha.left, sdf.left + pad),
            ] {
                assert!(
                    (a - s).abs() <= 1,
                    "'{ch}' {axis}: mask {a} vs sdf-adjusted {s}"
                );
            }
        }
    }
}
