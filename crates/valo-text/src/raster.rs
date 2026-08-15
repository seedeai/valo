use std::sync::Arc;

use skrifa::instance::Size;
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::MetadataProvider;
use valo_geometry::{Cap, Join, Path, PathBuilder, Rect};

use crate::font::Font;

/// SDF spread in texels: distance saturates ±this many pixels from the edge
/// (0.5 = on the edge). Also the raster padding so the field has room.
pub const SDF_PAD: u32 = 8;

/// The stroke a glyph raster can carry: [`valo_geometry::Stroke`] without
/// its dashes, which a fixed-size atlas key has nowhere to put. Impeller's
/// `StrokeParameters` carries the same four fields for the same reason.
/// `width` is in the raster's own pixels, like `px`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphStroke {
    pub width: f32,
    pub cap: Cap,
    pub join: Join,
    /// Miter length ÷ half-width beyond which a join bevels (SVG default 4).
    pub miter_limit: f32,
}

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
/// caches scaling state, and the stroker its segment buffers.
#[derive(Default)]
pub struct Rasterizer {
    context: swash::scale::ScaleContext,
    stroker: tiny_skia::PathStroker,
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

    /// Alpha coverage of the glyph's STROKED outline — the stroked mask
    /// tier. Stroking happens before rasterizing, which is what lets the
    /// result be an ordinary cached atlas entry (Skia's scaler strokes
    /// inside the strike for the same reason).
    ///
    /// This does NOT go through swash. swash rasterizes with zeno, and
    /// zeno's miter join short-circuits to a bevel whenever the two segment
    /// normals point apart (`stroke.rs`'s `dot < 0.0`), which caps its miter
    /// ratio at √2 and silently flattens every join sharper than a right
    /// angle — the apex of `A`, `M`, `W`, and most of what a stroked
    /// headline is made of. tiny-skia, already here for COLRv1, ports
    /// Skia's stroker and honours `miter_limit`, and it hands back a real
    /// path whose tight bounds size the atlas cell. That measurement is the
    /// point: Impeller sizes its slot the same way, by handing the stroking
    /// paint to `SkFont::getBounds`.
    pub fn stroked(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        dx: f32,
        stroke: &GlyphStroke,
    ) -> Option<GlyphImage> {
        let outline = glyph_outline(font, glyph, px, dx)?;
        let stroked = self.stroker.stroke(&outline, &skia_stroke(stroke), 1.0)?;
        mask_of(&stroked)
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

fn skia_stroke(stroke: &GlyphStroke) -> tiny_skia::Stroke {
    tiny_skia::Stroke {
        width: stroke.width,
        miter_limit: stroke.miter_limit,
        line_cap: match stroke.cap {
            Cap::Butt => tiny_skia::LineCap::Butt,
            Cap::Round => tiny_skia::LineCap::Round,
            Cap::Square => tiny_skia::LineCap::Square,
        },
        line_join: match stroke.join {
            Join::Miter => tiny_skia::LineJoin::Miter,
            Join::Round => tiny_skia::LineJoin::Round,
            Join::Bevel => tiny_skia::LineJoin::Bevel,
        },
        dash: None,
    }
}

/// The glyph as a tiny-skia path at `px`, baseline origin, y-down, shifted
/// by the subpixel x-phase — device space, so the stroke width needs no
/// further scaling.
fn glyph_outline(font: &Font, glyph: u32, px: f32, dx: f32) -> Option<tiny_skia::Path> {
    let font_ref = skrifa::FontRef::from_index(font.data(), font.face_index()).ok()?;
    let outline = font_ref.outline_glyphs().get(skrifa::GlyphId::new(glyph))?;
    let mut pen = TsPathPen::default();
    outline
        .draw(
            DrawSettings::unhinted(Size::new(px), font.variation_location()),
            &mut pen,
        )
        .ok()?;
    pen.builder
        .finish()?
        .transform(tiny_skia::Transform::from_row(1.0, 0.0, 0.0, -1.0, dx, 0.0))
}

/// A device-space path as an A8 image placed the way swash places its own:
/// `left` right of the origin, `top` above the baseline. Flooring the
/// tight bounds out to whole pixels is exactly the set of pixels the
/// antialiased fill can touch, so the cell is never short.
fn mask_of(path: &tiny_skia::Path) -> Option<GlyphImage> {
    let bounds = path.compute_tight_bounds()?;
    let (left, top) = (bounds.left().floor(), bounds.top().floor());
    let width = (bounds.right().ceil() - left) as u32;
    let height = (bounds.bottom().ceil() - top) as u32;
    let mut mask = tiny_skia::Mask::new(width, height)?;
    mask.fill_path(
        path,
        tiny_skia::FillRule::Winding,
        true,
        tiny_skia::Transform::from_translate(-left, -top),
    );
    Some(GlyphImage {
        width,
        height,
        left: left as i32,
        top: -top as i32,
        data: mask.data().to_vec(),
    })
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

/// skrifa outline pen → tiny-skia path, in whatever units the draw was
/// scaled to and y-up; the consumer's transform does the flip.
#[derive(Default)]
pub(crate) struct TsPathPen {
    pub(crate) builder: tiny_skia::PathBuilder,
}

impl OutlinePen for TsPathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(x, y);
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.builder.quad_to(cx, cy, x, y);
    }

    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        self.builder.cubic_to(c0x, c0y, c1x, c1y, x, y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
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

    /// The atlas cell is nothing but the raster's own placement, so the
    /// stroked raster has to come out already containing the miter spikes.
    /// Fira Sans `M` spikes 8.6px above its own outline at 72px with a
    /// 5px miter stroke — a cell inflated by a flat half-width (2.5px)
    /// would cut 6px off it, and nothing downstream could tell.
    #[test]
    fn stroked_raster_bounds_hold_the_miter_spikes() {
        let fonts = fira();
        let font = fonts.family("Fira Sans").unwrap();
        let mut raster = Rasterizer::new();
        let stroke = GlyphStroke {
            width: 5.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter_limit: 16.0,
        };
        let glyph = fonts.get(font).glyph_for('M').unwrap();
        let fill = raster.alpha(fonts.get(font), glyph, 72.0, 0.0).unwrap();
        let stroked = raster
            .stroked(fonts.get(font), glyph, 72.0, 0.0, &stroke)
            .unwrap();
        let reach = (stroked.top - fill.top) as f32;
        assert!(
            reach > stroke.width,
            "the stroked cell reaches only {reach}px above the fill — a \
             miter spike of 8.6px does not fit"
        );

        // A bevelled join has no spike: same stroke, and the cell shrinks
        // back to roughly the half-width. That the two DIFFER is what
        // proves the bound is measured, not assumed.
        let bevelled = raster
            .stroked(
                fonts.get(font),
                glyph,
                72.0,
                0.0,
                &GlyphStroke {
                    join: Join::Bevel,
                    ..stroke
                },
            )
            .unwrap();
        assert!(
            bevelled.top < stroked.top,
            "bevel {} vs miter {}",
            bevelled.top,
            stroked.top
        );
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
