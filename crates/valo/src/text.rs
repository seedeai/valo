//! The paragraph → display-list seam: valo-text lays out, valo-dl records
//! plain `GlyphRun` ops (font id + positions — dl never sees the text stack).
//! Shadows lower to blurred offset copies UNDER the run (Flutter's
//! TextStyle.shadows), decorations to rects from the font's own metrics
//! (skparagraph's Decorations.cpp).

use std::sync::Arc;

use valo_dl::{DisplayListBuilder, GlyphPos, Paint};
use valo_geometry::{Point, Rect};
use valo_text::{DecorationKind, Line, Paragraph, PlacedRun};

/// Record a laid-out paragraph at `origin` (its top-left corner).
pub trait DrawParagraphExt {
    /// Record `paragraph` with its own per-span styles, `origin` being its
    /// top-left corner.
    fn draw_paragraph(&mut self, paragraph: &Paragraph, origin: impl Into<Point>);
}

/// Paint every run with one explicit `Paint` — gradient-filled headlines,
/// blend-mode text. Shadows/decorations still come from the styles.
pub trait DrawGlyphRunExt {
    /// Record `paragraph` with `paint` overriding every run's fill, `origin`
    /// being its top-left corner.
    fn draw_paragraph_with(
        &mut self,
        paragraph: &Paragraph,
        origin: impl Into<Point>,
        paint: &Paint,
    );
}

impl DrawGlyphRunExt for DisplayListBuilder {
    fn draw_paragraph_with(
        &mut self,
        paragraph: &Paragraph,
        origin: impl Into<Point>,
        paint: &Paint,
    ) {
        let origin = origin.into();
        for line in paragraph.lines() {
            for run in &line.runs {
                draw_shadows(self, paragraph, run, origin);
                draw_run(self, run, origin, paint);
                draw_decoration(self, paragraph, line, run, origin);
            }
        }
    }
}

impl DrawParagraphExt for DisplayListBuilder {
    fn draw_paragraph(&mut self, paragraph: &Paragraph, origin: impl Into<Point>) {
        let origin = origin.into();
        for line in paragraph.lines() {
            for run in &line.runs {
                draw_shadows(self, paragraph, run, origin);
                draw_run(self, run, origin, &Paint::from_color(run.color));
                draw_decoration(self, paragraph, line, run, origin);
            }
        }
    }
}

/// Back-to-front blurred copies beneath the sharp run.
fn draw_shadows(b: &mut DisplayListBuilder, paragraph: &Paragraph, run: &PlacedRun, origin: Point) {
    for shadow in &run.shadows {
        let paint = Paint {
            color: shadow.color,
            mask_blur: (shadow.blur > 0.0).then(|| valo_dl::MaskBlur::new(shadow.blur)),
            ..Default::default()
        };
        b.save();
        b.translate(shadow.offset.x, shadow.offset.y);
        draw_run(b, run, origin, &paint);
        b.restore();
    }
    let _ = paragraph;
}

fn draw_run(b: &mut DisplayListBuilder, run: &PlacedRun, origin: Point, paint: &Paint) {
    let glyphs: Vec<GlyphPos> = run
        .glyphs
        .iter()
        .map(|g| GlyphPos {
            id: g.id,
            x: g.x + origin.x,
            y: g.y + origin.y,
        })
        .collect();
    // INK bounds, not the advance box: the record-time oracle culls,
    // sizes layers, and proves elision disjointness with these.
    let bounds = Rect::new(
        run.ink.x + origin.x,
        run.ink.y + origin.y,
        run.ink.width,
        run.ink.height,
    );
    b.draw_glyph_run(run.font.0, run.size, paint, Arc::new(glyphs), bounds);
}

/// Underline / strike / overline as a rect over the run's x extent, placed
/// by the font's decoration metrics (post/OS2 tables, y-up offsets).
fn draw_decoration(
    b: &mut DisplayListBuilder,
    paragraph: &Paragraph,
    line: &Line,
    run: &PlacedRun,
    origin: Point,
) {
    let Some(decoration) = run.decoration else {
        return;
    };
    let font = paragraph.fonts().get(run.font);
    let (offset, thickness) = match decoration.kind {
        DecorationKind::Underline => font.underline_px(run.size),
        DecorationKind::LineThrough => font.strikeout_px(run.size),
        DecorationKind::Overline => (font.ascent_px(run.size), font.underline_px(run.size).1),
    };
    let y = line.baseline - offset;
    let thickness = thickness * decoration.thickness;
    b.draw_rect(
        Rect::new(
            run.bounds.x + origin.x,
            y - thickness * 0.5 + origin.y,
            run.bounds.width,
            thickness,
        ),
        &Paint::from_color(decoration.color.unwrap_or(run.color)),
    );
}
