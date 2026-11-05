//! The second pass: a page's ops onto a krilla surface, with each region the first pass found
//! drawn as its picture.

use std::collections::HashMap;
use std::sync::Arc;

use krilla::blend::BlendMode as PdfBlend;
use krilla::color::rgb;
use krilla::geom::{Path as PdfPath, PathBuilder, Point as PdfPoint, Size, Transform};
use krilla::image::Image as PdfImage;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings as PdfPageSettings;
use krilla::paint::{
    Fill, FillRule as PdfRule, LineCap, LineJoin, LinearGradient, Paint as PdfPaint,
    RadialGradient, SpreadMethod, Stop, Stroke as PdfStroke, StrokeDash, SweepGradient,
};
use krilla::surface::Surface;
use krilla::text::{Font as PdfFont, GlyphId, KrillaGlyph};
use krilla::Document;
use valo::{
    BlendMode, Cap, ClipOp, Color, FillRule, Join, Matrix, MatrixClipState, Op, Paint, PaintStyle,
    Path, PixelBuffer, Rect, RunFont, RunText, Shader, SpreadMode,
};
use valo_codec_software::{encode, Encoding};
use valo_dl::GlyphPos;
use valo_geometry::PathElement;
use valo_text::{Font, FontUid};

use crate::raster::rasterized;
use crate::{PdfError, PdfPage, Picture};

/// `DocumentWriter` holds a document being written and the fonts its pages embed, each made once.
pub(crate) struct DocumentWriter {
    document: Document,
    fonts: HashMap<FontUid, Option<PdfFont>>,
}

impl DocumentWriter {
    pub fn new() -> Self {
        DocumentWriter {
            document: Document::new(),
            fonts: HashMap::new(),
        }
    }

    /// `page` writes `page` as the next page.
    pub fn page(&mut self, page: &PdfPage) -> Result<(), PdfError> {
        let points = page.settings.points_per_unit;
        let settings = PdfPageSettings::from_wh(page.size[0] * points, page.size[1] * points)
            .ok_or_else(|| PdfError::Write("a page has no area".to_owned()))?;
        let pictures = images_of(&page.pictures, page.settings.quality)?;
        let mut pdf_page = self.document.start_page_with(settings);
        let mut surface = pdf_page.surface();
        surface.push_transform(&Transform::from_scale(points, points));
        let mut writer = Writer {
            surface: &mut surface,
            fonts: &mut self.fonts,
            scopes: vec![0],
            pictures: pictures.into_iter(),
        };
        let mut ops = page.list.walk();
        while let Some(op) = ops.next() {
            if rasterized(op, ops.scope()) {
                writer.picture(ops.matrix_clip());
                ops.skip_scope();
            } else {
                writer.op(op);
            }
        }
        writer.close_all();
        surface.pop();
        surface.finish();
        pdf_page.finish();
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<u8>, PdfError> {
        self.document
            .finish()
            .map_err(|error| PdfError::Write(format!("{error:?}")))
    }
}

/// `PlacedImage` is a region's picture as a PDF image, and where it goes on the page.
struct PlacedImage {
    page_bounds: Rect,
    image: PdfImage,
}

/// `images_of` encodes each region's picture before the page is written, so writing can't fail.
fn images_of(
    pictures: &[Option<Picture>],
    quality: u8,
) -> Result<Vec<Option<PlacedImage>>, PdfError> {
    pictures
        .iter()
        .map(|picture| {
            picture
                .as_ref()
                .map(|picture| {
                    Ok(PlacedImage {
                        page_bounds: picture.bounds,
                        image: image_of(&picture.pixels, quality)?,
                    })
                })
                .transpose()
        })
        .collect()
}

/// `Writer` writes a page as a walk gives its ops. `scopes` counts, for each scope open on the
/// surface, the pushes it made, which its `Restore` pops.
struct Writer<'a, 'b> {
    surface: &'a mut Surface<'b>,
    fonts: &'a mut HashMap<FontUid, Option<PdfFont>>,
    scopes: Vec<usize>,
    pictures: std::vec::IntoIter<Option<PlacedImage>>,
}

impl Writer<'_, '_> {
    /// `op` writes an op PDF can hold.
    fn op(&mut self, op: &Op) {
        match op {
            Op::Save | Op::DrawDisplayList { .. } => self.open(),
            Op::SaveLayer { paint, .. } => {
                self.open();
                self.layer(paint);
            }
            Op::Restore => self.close(),
            Op::Transform(matrix) => {
                self.surface.push_transform(&transform_of(matrix));
                self.pushed();
            }
            Op::ClipPath {
                path,
                fill_rule,
                op,
                ..
            } => self.clip(path, *fill_rule, *op),
            Op::DrawRect { rect, paint, .. } => {
                let mut pb = PathBuilder::new();
                if let Some(rect) =
                    krilla::geom::Rect::from_xywh(rect.x, rect.y, rect.width, rect.height)
                {
                    pb.push_rect(rect);
                }
                if let Some(path) = pb.finish() {
                    self.draw(&path, FillRule::NonZero, paint);
                }
            }
            Op::DrawPath {
                path,
                fill_rule,
                paint,
                ..
            } => {
                if let Some(path) = path_of(path) {
                    self.draw(&path, *fill_rule, paint);
                }
            }
            Op::GlyphRun {
                font,
                paint,
                glyphs,
                text,
                ..
            } => self.glyph_run(font, paint, glyphs, text),
            Op::RRectBlur { .. } | Op::DrawImage { .. } => {
                unreachable!("blurs and images are rasterized")
            }
        }
    }

    fn open(&mut self) {
        self.scopes.push(0);
    }

    fn close(&mut self) {
        if let Some(pushes) = self.scopes.pop() {
            (0..pushes).for_each(|_| self.surface.pop());
        }
    }

    /// `close_all` pops what the page's own scope pushed and any scope a list left open.
    fn close_all(&mut self) {
        while !self.scopes.is_empty() {
            self.close();
        }
    }

    /// `pushed` counts a push the open scope's `Restore` pops.
    fn pushed(&mut self) {
        *self.scopes.last_mut().expect("the page's scope is open") += 1;
    }

    /// `picture` draws the next region's picture where it was drawn on the page, from where
    /// `state` places what is drawn now.
    fn picture(&mut self, state: &MatrixClipState) {
        let Some(Some(picture)) = self.pictures.next() else {
            return;
        };
        let bounds = picture.page_bounds;
        let (Some(back), Some(size)) = (
            state.transform().invert(),
            Size::from_wh(bounds.width, bounds.height),
        ) else {
            return;
        };
        self.surface.push_transform(&transform_of(&back));
        self.surface
            .push_transform(&Transform::from_translate(bounds.x, bounds.y));
        self.surface.draw_image(picture.image, size);
        self.surface.pop();
        self.surface.pop();
    }

    /// `layer` opens a layer as a transparency group with its opacity and blend mode.
    fn layer(&mut self, paint: &Paint) {
        if let Some(blend) = blend_of(paint.blend_mode) {
            self.surface.push_blend_mode(blend);
            self.pushed();
        }
        self.surface.push_isolated();
        self.pushed();
        if paint.color.a < 1.0 {
            self.surface.push_opacity(normalized(paint.color.a));
            self.pushed();
        }
    }

    fn clip(&mut self, path: &Path, rule: FillRule, op: ClipOp) {
        let clip = match op {
            ClipOp::Intersect => path_of(path).map(|path| (path, rule_of(rule))),
            // PDF only intersects clips; outside a shape is a huge rect with the shape cut out,
            // exact when the shape's even-odd and non-zero insides agree, as an outline's do.
            ClipOp::Difference => {
                let mut pb = PathBuilder::new();
                if let Some(everywhere) = krilla::geom::Rect::from_xywh(-1e6, -1e6, 2e6, 2e6) {
                    pb.push_rect(everywhere);
                }
                append(&mut pb, path);
                pb.finish().map(|path| (path, PdfRule::EvenOdd))
            }
        };
        if let Some((path, rule)) = clip {
            self.surface.push_clip_path(&path, &rule);
            self.pushed();
        }
    }

    fn draw(&mut self, path: &PdfPath, rule: FillRule, paint: &Paint) {
        let Some(pdf_paint) = paint_of(paint) else {
            return;
        };
        let blended = self.push_blend(paint.blend_mode);
        match &paint.style {
            PaintStyle::Fill => {
                self.surface.set_stroke(None);
                self.surface.set_fill(Some(Fill {
                    paint: pdf_paint,
                    opacity: normalized(paint.color.a),
                    rule: rule_of(rule),
                }));
            }
            PaintStyle::Stroke(stroke) => {
                self.surface.set_fill(None);
                self.surface
                    .set_stroke(Some(stroke_of(stroke, pdf_paint, paint.color.a)));
            }
        }
        self.surface.draw_path(path);
        if blended {
            self.surface.pop();
        }
    }

    fn push_blend(&mut self, mode: BlendMode) -> bool {
        match blend_of(mode) {
            Some(blend) => {
                self.surface.push_blend_mode(blend);
                true
            }
            None => false,
        }
    }

    /// `glyph_run` writes glyphs as text in their embedded, subset font, each with the text it
    /// shows, so the text can be selected and searched.
    fn glyph_run(
        &mut self,
        font: &RunFont,
        paint: &Paint,
        glyphs: &[GlyphPos],
        text: &Option<RunText>,
    ) {
        let RunFont {
            face,
            size,
            synthesis,
        } = font;
        let (size, synthesis) = (*size, *synthesis);
        let (Some(origin), Some(pdf_paint), Some(pdf_font)) =
            (glyphs.first().copied(), paint_of(paint), self.font(face))
        else {
            return;
        };
        // A run without its text still draws; its glyphs just stand for no text.
        let (text, ranges) = match text {
            Some(run_text) => (&*run_text.text, text_ranges(run_text)),
            None => ("", vec![0..0; glyphs.len()]),
        };
        let pdf_glyphs: Vec<KrillaGlyph> = glyphs
            .iter()
            .zip(ranges)
            .enumerate()
            .map(|(index, (glyph, range))| {
                let advance = glyphs
                    .get(index + 1)
                    .map_or(0.0, |next| (next.x - glyph.x) / size);
                KrillaGlyph::new(
                    GlyphId::new(glyph.id),
                    advance,
                    0.0,
                    (origin.y - glyph.y) / size,
                    0.0,
                    range,
                    None,
                )
            })
            .collect();
        let blended = self.push_blend(paint.blend_mode);
        // Fake italic slants about the baseline, as `FontSynthesis::skew` says.
        let slanted = synthesis.skew != 0.0;
        if slanted {
            self.surface.push_transform(&Transform::from_row(
                1.0,
                0.0,
                synthesis.skew,
                1.0,
                -synthesis.skew * origin.y,
                0.0,
            ));
        }
        let opacity = normalized(paint.color.a);
        match &paint.style {
            PaintStyle::Fill => {
                self.surface.set_fill(Some(Fill {
                    paint: pdf_paint.clone(),
                    opacity,
                    rule: PdfRule::NonZero,
                }));
                // Fake bold strokes the outline as it fills it, as Skia does.
                self.surface
                    .set_stroke((synthesis.embolden > 0.0).then(|| PdfStroke {
                        paint: pdf_paint,
                        width: synthesis.embolden * size,
                        opacity,
                        line_join: LineJoin::Round,
                        ..Default::default()
                    }));
            }
            PaintStyle::Stroke(stroke) => {
                self.surface.set_fill(None);
                self.surface
                    .set_stroke(Some(stroke_of(stroke, pdf_paint, paint.color.a)));
            }
        }
        self.surface.draw_glyphs(
            PdfPoint::from_xy(origin.x, origin.y),
            &pdf_glyphs,
            pdf_font,
            text,
            size,
            false,
        );
        self.surface.set_stroke(None);
        if slanted {
            self.surface.pop();
        }
        if blended {
            self.surface.pop();
        }
    }

    /// `font` is the PDF font for a valo font, made once per document; `None` for one krilla
    /// can't read.
    fn font(&mut self, font: &Arc<Font>) -> Option<PdfFont> {
        self.fonts
            .entry(font.uid())
            .or_insert_with(|| {
                let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(FontBytes(font.clone()));
                let coordinates: Vec<(krilla::text::Tag, f32)> = font
                    .variation_coordinates()
                    .iter()
                    .map(|(tag, value)| (krilla::text::Tag::new(tag), *value))
                    .collect();
                PdfFont::new_variable(data.into(), font.face_index(), &coordinates)
            })
            .clone()
    }
}

/// `text_ranges` is the text each glyph shows: from its cluster to the next cluster in the text,
/// the same range for glyphs sharing one, so a ligature reads as all its letters.
fn text_ranges(run_text: &RunText) -> Vec<std::ops::Range<usize>> {
    let text = &run_text.text;
    let mut starts: Vec<usize> = run_text
        .clusters
        .iter()
        .map(|cluster| (*cluster as usize).min(text.len()))
        .collect();
    starts.sort_unstable();
    starts.dedup();
    run_text
        .clusters
        .iter()
        .map(|cluster| {
            let start = (*cluster as usize).min(text.len());
            let end = starts
                .iter()
                .find(|next| **next > start)
                .copied()
                .unwrap_or(text.len());
            start..end
        })
        .collect()
}

/// `image_of` is a picture as a PDF image: a JPEG when every pixel is opaque, else lossless with
/// its transparency.
fn image_of(pixels: &PixelBuffer, quality: u8) -> Result<PdfImage, PdfError> {
    let [width, height] = pixels.layout().size;
    let opaque = pixels
        .bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| pixel[3] == 255);
    if !opaque {
        return Ok(PdfImage::from_rgba8(pixels.bytes().to_vec(), width, height));
    }
    let jpeg = encode(
        pixels,
        Encoding::Jpeg {
            quality,
            background: Color::WHITE,
        },
    )
    .map_err(|error| PdfError::Encode(error.to_string()))?;
    PdfImage::from_jpeg(jpeg.into(), true).map_err(PdfError::Encode)
}

fn paint_of(paint: &Paint) -> Option<PdfPaint> {
    let Some(shader) = &paint.shader else {
        return Some(color_of(paint.color).into());
    };
    Some(match shader {
        Shader::Linear {
            start,
            end,
            stops,
            spread,
            local,
        } => LinearGradient {
            x1: start.x,
            y1: start.y,
            x2: end.x,
            y2: end.y,
            transform: transform_of(local),
            spread_method: spread_of(*spread),
            stops: stops_of(stops),
            anti_alias: false,
        }
        .into(),
        Shader::Radial {
            center,
            radius,
            stops,
            spread,
            focus,
            local,
        } => {
            let (fx, fy, fr) = focus.map_or((center.x, center.y, 0.0), |focus| {
                (focus.center.x, focus.center.y, focus.radius)
            });
            RadialGradient {
                fx,
                fy,
                fr,
                cx: center.x,
                cy: center.y,
                cr: *radius,
                transform: transform_of(local),
                spread_method: spread_of(*spread),
                stops: stops_of(stops),
                anti_alias: false,
            }
            .into()
        }
        Shader::Sweep {
            center,
            start_angle,
            stops,
            local,
        } => {
            let start = start_angle.to_degrees();
            SweepGradient {
                cx: center.x,
                cy: center.y,
                start_angle: start,
                end_angle: start + 360.0,
                transform: transform_of(local),
                spread_method: SpreadMethod::Pad,
                stops: stops_of(stops),
                anti_alias: false,
            }
            .into()
        }
        Shader::Image { .. } | Shader::Runtime(_) => return None,
    })
}

/// `FontBytes` lends krilla a valo font's file without copying it.
struct FontBytes(Arc<Font>);

impl AsRef<[u8]> for FontBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.data()
    }
}

fn transform_of(m: &Matrix) -> Transform {
    let [a, b, c, d, tx, ty] = m.to_affine();
    Transform::from_row(a, b, c, d, tx, ty)
}

fn path_of(path: &Path) -> Option<PdfPath> {
    let mut pb = PathBuilder::new();
    append(&mut pb, path);
    pb.finish()
}

/// `append` adds a valo path to a krilla one. PDF has no conics: each becomes the cubic with the
/// same ends and tangents, exact for the quarter circles of round corners to within 0.03%.
fn append(pb: &mut PathBuilder, path: &Path) {
    let mut last = valo::Point::new(0.0, 0.0);
    for element in path.elements() {
        match element {
            PathElement::MoveTo(p) => {
                pb.move_to(p.x, p.y);
                last = p;
            }
            PathElement::LineTo(p) => {
                pb.line_to(p.x, p.y);
                last = p;
            }
            PathElement::QuadTo(c, p) => {
                pb.quad_to(c.x, c.y, p.x, p.y);
                last = p;
            }
            PathElement::ConicTo(c, p, w) => {
                let k = 4.0 * w / (3.0 * (1.0 + w));
                pb.cubic_to(
                    last.x + (c.x - last.x) * k,
                    last.y + (c.y - last.y) * k,
                    p.x + (c.x - p.x) * k,
                    p.y + (c.y - p.y) * k,
                    p.x,
                    p.y,
                );
                last = p;
            }
            PathElement::CubicTo(c1, c2, p) => {
                pb.cubic_to(c1.x, c1.y, c2.x, c2.y, p.x, p.y);
                last = p;
            }
            PathElement::Close => pb.close(),
        }
    }
}

fn rule_of(rule: FillRule) -> PdfRule {
    match rule {
        FillRule::NonZero => PdfRule::NonZero,
        FillRule::EvenOdd => PdfRule::EvenOdd,
    }
}

fn stroke_of(stroke: &valo::Stroke, paint: PdfPaint, alpha: f32) -> PdfStroke {
    PdfStroke {
        paint,
        width: stroke.width,
        miter_limit: stroke.miter_limit,
        line_cap: match stroke.cap {
            Cap::Butt => LineCap::Butt,
            Cap::Round => LineCap::Round,
            Cap::Square => LineCap::Square,
        },
        line_join: match stroke.join {
            Join::Miter => LineJoin::Miter,
            Join::Round => LineJoin::Round,
            Join::Bevel => LineJoin::Bevel,
        },
        opacity: normalized(alpha),
        dash: stroke.dash.as_ref().map(|dash| StrokeDash {
            array: dash.intervals.clone(),
            offset: dash.phase,
        }),
    }
}

fn color_of(color: Color) -> rgb::Color {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    rgb::Color::new(byte(color.r), byte(color.g), byte(color.b))
}

fn normalized(v: f32) -> NormalizedF32 {
    NormalizedF32::new(v.clamp(0.0, 1.0)).expect("clamped")
}

fn stops_of(stops: &[valo::GradientStop]) -> Vec<Stop> {
    stops
        .iter()
        .map(|stop| Stop {
            offset: normalized(stop.offset),
            color: color_of(stop.color).into(),
            opacity: normalized(stop.color.a),
        })
        .collect()
}

fn spread_of(spread: SpreadMode) -> SpreadMethod {
    match spread {
        SpreadMode::Pad => SpreadMethod::Pad,
        SpreadMode::Repeat => SpreadMethod::Repeat,
        SpreadMode::Reflect => SpreadMethod::Reflect,
    }
}

/// `blend_of` is PDF's blend mode for valo's; `None` for source-over, and for Porter-Duff modes
/// PDF has no word for.
fn blend_of(mode: BlendMode) -> Option<PdfBlend> {
    Some(match mode {
        BlendMode::Multiply => PdfBlend::Multiply,
        BlendMode::Screen => PdfBlend::Screen,
        BlendMode::Overlay => PdfBlend::Overlay,
        BlendMode::Darken => PdfBlend::Darken,
        BlendMode::Lighten => PdfBlend::Lighten,
        BlendMode::ColorDodge => PdfBlend::ColorDodge,
        BlendMode::ColorBurn => PdfBlend::ColorBurn,
        BlendMode::HardLight => PdfBlend::HardLight,
        BlendMode::SoftLight => PdfBlend::SoftLight,
        BlendMode::Difference => PdfBlend::Difference,
        BlendMode::Exclusion => PdfBlend::Exclusion,
        BlendMode::Hue => PdfBlend::Hue,
        BlendMode::Saturation => PdfBlend::Saturation,
        BlendMode::Color => PdfBlend::Color,
        BlendMode::Luminosity => PdfBlend::Luminosity,
        _ => return None,
    })
}
