use std::sync::Arc;

use skrifa::instance::Size;
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::MetadataProvider;
use valo_geometry::{Cap, Join, Path, PathBuilder, Rect};

use crate::font::Font;

/// `SDF_PAD` is the padding and maximum encoded distance around an SDF glyph.
///
/// Distances saturate this many pixels inside or outside the glyph edge.
pub const SDF_PAD: u32 = 8;

/// `GlyphStroke` describes an outline applied before rasterizing a glyph.
///
/// It is equivalent to [`valo_geometry::Stroke`] without a dash pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphStroke {
    /// `width` is the full stroke width in raster pixels.
    pub width: f32,
    /// `cap` controls the ends of open outline contours.
    pub cap: Cap,
    /// `join` controls how consecutive outline segments meet.
    pub join: Join,
    /// `miter_limit` is the maximum miter length divided by half the stroke width.
    pub miter_limit: f32,
}

/// `GlyphImage` contains rasterized glyph pixels and baseline-relative placement.
///
/// Alpha and SDF images store one byte per pixel. Color images store
/// premultiplied RGBA8. To place the bitmap at glyph origin `(x, y)`, draw its
/// top-left at `(x + left, y - top)`.
pub struct GlyphImage {
    /// `width` is the bitmap width in pixels.
    pub width: u32,
    /// `height` is the bitmap height in pixels.
    pub height: u32,
    /// `left` is the bitmap's horizontal offset from the glyph origin.
    pub left: i32,
    /// `top` is the bitmap's upward offset from the baseline.
    pub top: i32,
    /// `data` contains tightly packed rows in the format produced by the raster method.
    pub data: Vec<u8>,
}

/// `Rasterizer` converts font glyphs into CPU bitmap or distance-field images.
///
/// Valo's renderer owns one internally. Hosts need this type only when building
/// a custom glyph cache or text renderer. Reuse an instance to retain scaling
/// and stroking scratch state; its methods require mutable access.
#[derive(Default)]
pub struct Rasterizer {
    context: swash::scale::ScaleContext,
    stroker: tiny_skia::PathStroker,
}

impl Rasterizer {
    /// `new` creates an empty CPU glyph rasterizer.
    pub fn new() -> Self {
        Self::default()
    }

    /// `alpha` rasterizes a glyph into one-byte alpha coverage.
    ///
    /// `px` is the font size in raster pixels. `dx` shifts the outline
    /// horizontally for subpixel positioning and is usually one of
    /// `0.0`, `0.25`, `0.5`, or `0.75`. It returns `None` when the glyph has no
    /// rasterizable monochrome outline.
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

    /// `stroked` rasterizes a stroked glyph outline into one-byte alpha coverage.
    ///
    /// `px`, `dx`, and stroke dimensions are in raster pixels. It returns
    /// `None` when the glyph has no outline or the stroked path cannot be built.
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

    /// `sdf` rasterizes a glyph into a one-byte signed distance field.
    ///
    /// `px` is the font size in raster pixels. A value near 128 marks the edge;
    /// larger values are inside and smaller values are outside. The image is
    /// padded by [`SDF_PAD`] pixels. It returns `None` when no monochrome
    /// outline can be rasterized.
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

    /// `color` rasterizes a color glyph into premultiplied RGBA8 pixels.
    ///
    /// `px` is the font size in raster pixels. It supports color outlines and
    /// embedded color bitmaps. It returns `None` when the glyph has no supported
    /// color representation, allowing callers to fall back to alpha rendering.
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

/// `glyph_path` returns a glyph outline as a baseline-relative Valo path.
///
/// `px` is the font size in logical pixels. The path uses Valo's y-down
/// coordinates with its origin on the baseline. It returns `None` when the
/// glyph has no nonempty monochrome outline.
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
