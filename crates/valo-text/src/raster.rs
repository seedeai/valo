use std::sync::Arc;

use skrifa::outline::OutlinePen;
use valo_geometry::{Cap, Join, Path, PathBuilder, Rect};

use crate::font::Font;
use crate::synthesis::FontSynthesis;

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

/// `GlyphRaster` turns one glyph of a font into pixels or an outline.
///
/// It is the renderer's only way to a glyph's shape: everything else in text
/// (shaping, layout, the glyph cache) reads a font's metric and shaping tables,
/// which a font without outline tables still has. [`Rasterizer`] is the
/// implementation over the font's own bytes; a host whose glyphs are rendered
/// elsewhere, such as a program running behind a boundary, supplies another.
/// All sizes are in raster pixels, and the results are as [`Rasterizer`]'s
/// methods document them. Every monochrome shape takes the run's
/// [`FontSynthesis`], the bold and italic it fakes; colour glyphs are drawn
/// as they are.
pub trait GlyphRaster: Send {
    /// See [`Rasterizer::alpha`].
    fn alpha(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
        dx: f32,
    ) -> Option<GlyphImage>;

    /// See [`Rasterizer::stroked`].
    fn stroked(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
        dx: f32,
        stroke: &GlyphStroke,
    ) -> Option<GlyphImage>;

    /// See [`Rasterizer::sdf`].
    fn sdf(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
    ) -> Option<GlyphImage>;

    /// See [`Rasterizer::color`].
    fn color(&mut self, font: &Font, glyph: u32, px: f32) -> Option<GlyphImage>;

    /// See [`Rasterizer::path`].
    fn path(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
    ) -> Option<Arc<Path>>;
}

impl GlyphRaster for Rasterizer {
    fn alpha(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
        dx: f32,
    ) -> Option<GlyphImage> {
        Rasterizer::alpha(self, font, glyph, px, synthesis, dx)
    }

    fn stroked(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
        dx: f32,
        stroke: &GlyphStroke,
    ) -> Option<GlyphImage> {
        Rasterizer::stroked(self, font, glyph, px, synthesis, dx, stroke)
    }

    fn sdf(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
    ) -> Option<GlyphImage> {
        Rasterizer::sdf(self, font, glyph, px, synthesis)
    }

    fn color(&mut self, font: &Font, glyph: u32, px: f32) -> Option<GlyphImage> {
        Rasterizer::color(self, font, glyph, px)
    }

    fn path(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
    ) -> Option<Arc<Path>> {
        Rasterizer::path(self, font, glyph, px, synthesis)
    }
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
    /// `px` is the font size in raster pixels. `synthesis` is the bold and
    /// italic the glyph fakes, [`FontSynthesis::NONE`] for the face as it is.
    /// `dx` shifts the outline horizontally for subpixel positioning and is
    /// usually one of `0.0`, `0.25`, `0.5`, or `0.75`. It returns `None` when
    /// the glyph has no rasterizable monochrome outline.
    pub fn alpha(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
        dx: f32,
    ) -> Option<GlyphImage> {
        let image = self.render(font, glyph, px, synthesis, dx)?;
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
    /// `px`, `dx`, and stroke dimensions are in raster pixels. The stroke
    /// follows the outline `synthesis` makes, as the outline tier strokes
    /// [`Self::path`]'s. It returns `None` when the glyph has no outline or
    /// the stroked path cannot be built.
    pub fn stroked(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
        dx: f32,
        stroke: &GlyphStroke,
    ) -> Option<GlyphImage> {
        let outline = self.skia_outline(font, glyph, px, synthesis, dx)?;
        let stroked = self.stroker.stroke(&outline, &skia_stroke(stroke), 1.0)?;
        mask_of(&stroked)
    }

    /// `sdf` rasterizes a glyph into a one-byte signed distance field.
    ///
    /// `px` is the font size in raster pixels, and `synthesis` is as
    /// [`Self::alpha`] takes it. A value near 128 marks the edge; larger values
    /// are inside and smaller values are outside. The image is padded by
    /// [`SDF_PAD`] pixels. It returns `None` when no monochrome outline can be
    /// rasterized.
    pub fn sdf(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
    ) -> Option<GlyphImage> {
        let alpha = self.render(font, glyph, px, synthesis, 0.0)?;
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
        let mut scaler = self.scaler(font, px)?;
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
        for px in data.as_chunks_mut::<4>().0 {
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

    /// `path` returns a glyph outline as a baseline-relative Valo path.
    ///
    /// `px` is the font size in logical pixels, and `synthesis` is as
    /// [`Self::alpha`] takes it. The path uses Valo's y-down coordinates with
    /// its origin on the baseline. It returns `None` when the glyph has no
    /// nonempty monochrome outline.
    pub fn path(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
    ) -> Option<Arc<Path>> {
        let outline = self.outline(font, glyph, px, synthesis)?;
        let mut pen = PathPen {
            builder: PathBuilder::new(),
        };
        draw_outline(&outline, &mut pen);
        let path = pen.builder.build();
        // COLR fonts carry EMPTY classic outlines for their color glyphs — an
        // empty path IS "no outline", so callers take their color fallback
        // instead of stencil-filling nothing (the >path_min emoji vanish bug).
        (!path.is_empty()).then_some(path)
    }

    /// The glyph's coverage at `px`, synthesized, shifted right by `dx`.
    fn render(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
        dx: f32,
    ) -> Option<swash::scale::image::Image> {
        let mut scaler = self.scaler(font, px)?;
        swash::scale::Render::new(&[swash::scale::Source::Outline])
            .embolden(synthesis.edge_growth(px))
            .transform(synthesis_transform(synthesis, px))
            .offset(swash::zeno::Vector::new(dx, 0.0))
            .render(&mut scaler, glyph as swash::GlyphId)
    }

    /// The glyph's outline at `px` with `synthesis` applied as [`Self::render`]
    /// applies it: swash's, y-up around the baseline origin.
    fn outline(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
    ) -> Option<swash::scale::outline::Outline> {
        let mut outline = self
            .scaler(font, px)?
            .scale_outline(glyph as swash::GlyphId)?;
        let growth = synthesis.edge_growth(px);
        if growth != 0.0 {
            outline.embolden(growth, growth);
        }
        if let Some(transform) = synthesis_transform(synthesis, px) {
            outline.transform(&transform);
        }
        Some(outline)
    }

    /// [`Self::outline`] as a tiny-skia path in device space: y-down, shifted
    /// right by the subpixel `dx`, so a stroke width needs no further scaling.
    fn skia_outline(
        &mut self,
        font: &Font,
        glyph: u32,
        px: f32,
        synthesis: FontSynthesis,
        dx: f32,
    ) -> Option<tiny_skia::Path> {
        let outline = self.outline(font, glyph, px, synthesis)?;
        let mut pen = TsPathPen::default();
        draw_outline(&outline, &mut pen);
        pen.builder
            .finish()?
            .transform(tiny_skia::Transform::from_row(1.0, 0.0, 0.0, -1.0, dx, 0.0))
    }

    /// A swash scaler for `font` at `px`, unhinted, at the font's variation
    /// coordinates.
    fn scaler<'a>(&'a mut self, font: &'a Font, px: f32) -> Option<swash::scale::Scaler<'a>> {
        let font_ref = swash::FontRef::from_index(font.data(), font.face_index() as usize)?;
        Some(
            self.context
                .builder(font_ref)
                .size(px)
                .hint(false)
                .variations(swash_variations(font))
                .build(),
        )
    }
}

/// The shear and shift `synthesis` puts on a glyph at `px`, in swash's y-up
/// outline space, applied after its embolden; `None` when there is neither.
///
/// swash's embolden moves every edge out by its strength and then the whole
/// outline right and up by the same, so a glyph grows from its bottom-left
/// corner; shifting back by one strength centres the growth, as Skia's stroke
/// grows a glyph evenly on every side. y-up flips the shear's sign: `-skew`
/// leans the top right.
fn synthesis_transform(synthesis: FontSynthesis, px: f32) -> Option<swash::zeno::Transform> {
    if synthesis.is_none() {
        return None;
    }
    let growth = synthesis.edge_growth(px);
    let shear = swash::zeno::Transform::new(1.0, 0.0, -synthesis.skew, 1.0, 0.0, 0.0);
    Some(swash::zeno::Transform::translation(-growth, -growth).then(&shear))
}

/// `draw_outline` replays a swash outline into a skrifa pen, point for point.
fn draw_outline(outline: &swash::scale::outline::Outline, pen: &mut impl OutlinePen) {
    use swash::zeno::Verb;
    let mut points = outline.points().iter();
    let mut next = || points.next().copied().unwrap_or_default();
    for verb in outline.verbs() {
        match verb {
            Verb::MoveTo => {
                let to = next();
                pen.move_to(to.x, to.y);
            }
            Verb::LineTo => {
                let to = next();
                pen.line_to(to.x, to.y);
            }
            Verb::QuadTo => {
                let (control, to) = (next(), next());
                pen.quad_to(control.x, control.y, to.x, to.y);
            }
            Verb::CurveTo => {
                let (first, second, to) = (next(), next(), next());
                pen.curve_to(first.x, first.y, second.x, second.y, to.x, to.y);
            }
            Verb::Close => pen.close(),
        }
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
/// It is [`Rasterizer::path`] on a fresh rasterizer; reuse a [`Rasterizer`]
/// to outline many glyphs.
pub fn glyph_path(font: &Font, glyph: u32, px: f32, synthesis: FontSynthesis) -> Option<Arc<Path>> {
    Rasterizer::new().path(font, glyph, px, synthesis)
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
        let fill = raster
            .alpha(fonts.get(font), glyph, 72.0, FontSynthesis::NONE, 0.0)
            .unwrap();
        let stroked = raster
            .stroked(
                fonts.get(font),
                glyph,
                72.0,
                FontSynthesis::NONE,
                0.0,
                &stroke,
            )
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
                FontSynthesis::NONE,
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

    const FAKE_BOLD: FontSynthesis = FontSynthesis {
        embolden: 1.0 / 32.0,
        skew: 0.0,
    };

    const FAKE_ITALIC: FontSynthesis = FontSynthesis {
        embolden: 0.0,
        skew: -0.25,
    };

    /// The mean x of `image`'s ink in rows `rows` (counted from its top),
    /// relative to the glyph origin.
    fn ink_centre(image: &GlyphImage, rows: std::ops::Range<u32>) -> f32 {
        let (mut sum, mut moment) = (0.0, 0.0);
        for y in rows {
            for x in 0..image.width {
                let coverage = image.data[(y * image.width + x) as usize] as f32 / 255.0;
                sum += coverage;
                moment += coverage * (image.left as f32 + x as f32 + 0.5);
            }
        }
        moment / sum
    }

    /// How much ink `image` has on the row `height` pixels above the baseline.
    fn row_coverage(image: &GlyphImage, height: i32) -> f32 {
        let y = (image.top - height) as u32;
        (0..image.width)
            .map(|x| image.data[(y * image.width + x) as usize] as f32 / 255.0)
            .sum()
    }

    /// Skia's fake italic shears by -1/4 in y-down space, which must lean the
    /// top of a glyph RIGHT. swash's outlines are y-up, so the shear's sign
    /// flips on the way in; a wrong flip leans the glyph left. The upright
    /// stem of `I` shows it: its top band's ink sits right of its bottom
    /// band's by a quarter of the height between them, in the mask and in the
    /// outline-tier path alike.
    #[test]
    fn fake_italic_leans_the_top_of_a_glyph_right() {
        let fonts = fira();
        let font = fonts.get(fonts.family("Fira Sans").unwrap());
        let glyph = font.glyph_for('I').unwrap();
        let mut raster = Rasterizer::new();
        let lean = |image: &GlyphImage| {
            let band = image.height / 4;
            let top = ink_centre(image, 0..band);
            let bottom = ink_centre(image, image.height - band..image.height);
            (top - bottom) / (image.height - band) as f32
        };
        let plain = raster
            .alpha(font, glyph, 64.0, FontSynthesis::NONE, 0.0)
            .unwrap();
        let italic = raster.alpha(font, glyph, 64.0, FAKE_ITALIC, 0.0).unwrap();
        assert!(lean(&plain).abs() < 0.02, "upright: {}", lean(&plain));
        assert!(
            (lean(&italic) - 0.25).abs() < 0.03,
            "the top leans right by a quarter: {}",
            lean(&italic)
        );

        // The path is y-down: its top (the cap height) moves right, its foot stays.
        let plain = raster
            .path(font, glyph, 64.0, FontSynthesis::NONE)
            .unwrap()
            .tight_bounds();
        let italic = raster
            .path(font, glyph, 64.0, FAKE_ITALIC)
            .unwrap()
            .tight_bounds();
        let cap_height = -plain.y;
        assert!((italic.x - plain.x).abs() < 0.01, "{italic:?} vs {plain:?}");
        let reach = italic.right() - plain.right();
        assert!(
            (reach - cap_height / 4.0).abs() < 0.01,
            "the top moved {reach} for a cap height of {cap_height}"
        );
    }

    /// Skia fakes bold by stroking and filling the outline with a stroke of
    /// `size × embolden`, which moves every edge out by half of it. swash's
    /// embolden moves every edge out by its WHOLE strength and then shifts the
    /// glyph right and up by the same, so the rasterizer passes half the stroke
    /// and shifts back. Measured against Skia's construction, done with
    /// tiny-skia: the stroked outline's box is where the fake bold glyph's ink
    /// must land, and a stem must thicken by the full stroke.
    #[test]
    fn fake_bold_grows_a_glyph_as_skias_stroke_and_fill_does() {
        let fonts = fira();
        let font = fonts.get(fonts.family("Fira Sans").unwrap());
        let glyph = font.glyph_for('I').unwrap();
        let px = 64.0;
        let width = px * FAKE_BOLD.embolden;
        let mut raster = Rasterizer::new();
        let outline = raster
            .skia_outline(font, glyph, px, FontSynthesis::NONE, 0.0)
            .unwrap();
        // Skia's `SkPaint` defaults: a miter join with a limit of 4.
        let skia = raster
            .stroker
            .stroke(
                &outline,
                &tiny_skia::Stroke {
                    width,
                    miter_limit: 4.0,
                    ..Default::default()
                },
                1.0,
            )
            .unwrap()
            .compute_tight_bounds()
            .unwrap();
        let bold = raster.alpha(font, glyph, px, FAKE_BOLD, 0.0).unwrap();
        let ink = (
            bold.left as f32,
            -bold.top as f32,
            (bold.left + bold.width as i32) as f32,
            (bold.height as i32 - bold.top) as f32,
        );
        let expected = (
            skia.left().floor(),
            skia.top().floor(),
            skia.right().ceil(),
            skia.bottom().ceil(),
        );
        assert_eq!(ink, expected, "fake bold's cell vs Skia's stroked outline");

        let plain = raster
            .alpha(font, glyph, px, FontSynthesis::NONE, 0.0)
            .unwrap();
        let gain = row_coverage(&bold, 20) - row_coverage(&plain, 20);
        assert!(
            (gain - width).abs() < 0.1,
            "the stem thickens by {gain}, the stroke is {width}"
        );
        assert!(
            (ink_centre(&bold, 0..bold.height) - ink_centre(&plain, 0..plain.height)).abs() < 0.05,
            "the glyph grows evenly about its own centre"
        );
    }

    /// A run's ink box grows a glyph box by what synthesis adds, so the
    /// grown box of each glyph's own outline must hold that glyph synthesized
    /// — the run's box is the same growth over the font-wide box.
    #[test]
    fn synthesized_glyphs_stay_inside_their_grown_ink_box() {
        let fonts = fira();
        let font = fonts.get(fonts.family("Fira Sans").unwrap());
        let mut raster = Rasterizer::new();
        let both = FontSynthesis {
            embolden: 1.0 / 24.0,
            skew: -0.25,
        };
        for synthesis in [FAKE_BOLD, FAKE_ITALIC, both] {
            for ch in ['W', 'A', 'l', 'g', 'v', 'Q'] {
                let glyph = font.glyph_for(ch).unwrap();
                let plain = raster.path(font, glyph, 48.0, FontSynthesis::NONE).unwrap();
                let drawn = raster.path(font, glyph, 48.0, synthesis).unwrap();
                let (plain, drawn) = (plain.tight_bounds(), drawn.tight_bounds());
                // y-up boxes, as fonts and `ink_box` give them.
                let (x_min, y_min, x_max, y_max) =
                    synthesis.ink_box((plain.x, -plain.bottom(), plain.right(), -plain.y), 48.0);
                let epsilon = 1e-3;
                assert!(
                    x_min <= drawn.x + epsilon
                        && x_max >= drawn.right() - epsilon
                        && y_min <= -drawn.bottom() + epsilon
                        && y_max >= -drawn.y - epsilon,
                    "'{ch}' {synthesis:?}: box {:?} misses {drawn:?}",
                    (x_min, y_min, x_max, y_max)
                );
            }
        }
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
            let alpha = raster
                .alpha(fonts.get(font), glyph, 64.0, FontSynthesis::NONE, 0.0)
                .unwrap();
            let sdf = raster
                .sdf(fonts.get(font), glyph, 64.0, FontSynthesis::NONE)
                .unwrap();
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
