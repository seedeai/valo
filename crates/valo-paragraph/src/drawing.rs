//! Paragraph recording lowers the current layout into independent display-list
//! operations. The list owns the resolved fonts, positions, and paints rather
//! than retaining the paragraph, so later paragraph changes cannot mutate it.
//!
//! Each line is recorded in skparagraph's order (`TextLine::paint`): every
//! background, then every shadow, then the glyphs, then the decorations, so
//! the backgrounds and shadows of a line lie beneath all of its glyphs.

use std::sync::Arc;

use valo_dl::{DisplayListBuilder, GlyphPos, Paint};
use valo_geometry::{Point, Rect};
use valo_text::FaceSet;

use crate::paragraph::{Line, Paragraph, PlacedRun};
use crate::style::DecorationKind;
use crate::wrap::style_heights;

/// `DrawParagraphExt` adds paragraph recording to [`DisplayListBuilder`].
pub trait DrawParagraphExt {
    /// `draw_paragraph` records the paragraph's current layout and span styles.
    ///
    /// The paragraph is lowered into backgrounds, shadows, glyph runs, and
    /// decorations at its top-left `origin`. Later layout or style changes do
    /// not affect the recorded display list.
    fn draw_paragraph(&mut self, paragraph: &Paragraph, origin: impl Into<Point>);
}

/// `DrawGlyphRunExt` adds paragraph recording with an explicit glyph paint.
///
/// Use it for gradient or blended text. Backgrounds, shadows and decorations
/// continue to use the paragraph's span styles.
pub trait DrawGlyphRunExt {
    /// `draw_paragraph_with` records the current layout with one glyph paint.
    ///
    /// `paint` replaces every span's foreground and color, while backgrounds,
    /// shadows and decorations retain their span styles. Later paragraph
    /// changes do not affect the display list.
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
        draw_lines(self, paragraph, origin.into(), Some(paint));
    }
}

impl DrawParagraphExt for DisplayListBuilder {
    fn draw_paragraph(&mut self, paragraph: &Paragraph, origin: impl Into<Point>) {
        draw_lines(self, paragraph, origin.into(), None);
    }
}

/// `draw_lines` records every line in skparagraph's order. `glyph_paint`
/// replaces each span's own glyph paint when set.
fn draw_lines(
    b: &mut DisplayListBuilder,
    paragraph: &Paragraph,
    origin: Point,
    glyph_paint: Option<&Paint>,
) {
    for line in paragraph.lines() {
        draw_backgrounds(b, paragraph, line, origin);
        draw_line_shadows(b, paragraph, line, origin);
        draw_line_glyphs(b, paragraph, line, origin, glyph_paint);
        draw_line_decorations(b, paragraph, line, origin);
    }
}

/// `draw_line_shadows` records the shadows of every run on `line`.
fn draw_line_shadows(
    b: &mut DisplayListBuilder,
    paragraph: &Paragraph,
    line: &Line,
    origin: Point,
) {
    for run in text_runs(line) {
        draw_shadows(b, paragraph, run, origin);
    }
}

/// `draw_line_glyphs` records the glyphs of every run on `line`.
fn draw_line_glyphs(
    b: &mut DisplayListBuilder,
    paragraph: &Paragraph,
    line: &Line,
    origin: Point,
    glyph_paint: Option<&Paint>,
) {
    for run in text_runs(line) {
        draw_glyphs(b, paragraph.faces(), run, origin, glyph_paint);
    }
}

/// `draw_line_decorations` records the decorations of every run on `line`.
fn draw_line_decorations(
    b: &mut DisplayListBuilder,
    paragraph: &Paragraph,
    line: &Line,
    origin: Point,
) {
    for run in text_runs(line) {
        draw_decoration(b, paragraph, line, run, origin);
    }
}

/// `text_runs` returns the line's runs that draw glyphs, leaving out
/// placeholders, whose content the host draws.
fn text_runs(line: &Line) -> impl Iterator<Item = &PlacedRun> {
    line.runs.iter().filter(|run| run.placeholder.is_none())
}

/// `draw_backgrounds` fills the line's background boxes in visual order.
fn draw_backgrounds(b: &mut DisplayListBuilder, paragraph: &Paragraph, line: &Line, origin: Point) {
    for (bounds, paint) in background_boxes(paragraph, line) {
        let bounds = Rect::new(
            bounds.x + origin.x,
            bounds.y + origin.y,
            bounds.width,
            bounds.height,
        );
        b.draw_rect(bounds, paint);
    }
}

/// `background_boxes` pairs each background on `line` with the box it fills,
/// in paragraph coordinates and visual order.
///
/// Neighbouring runs that skparagraph shapes as one run (same face, size,
/// synthesis, height and direction) and whose backgrounds are equal share one
/// box, as skparagraph merges the style blocks of a run whose backgrounds
/// match (`TextLine::iterateThroughSingleRunByStyles`). A box with no height
/// is skipped, as it is there.
fn background_boxes<'a>(paragraph: &Paragraph, line: &'a Line) -> Vec<(Rect, &'a Paint)> {
    let mut boxes: Vec<(Rect, &Paint)> = Vec::new();
    let mut previous: Option<&PlacedRun> = None;
    for run in &line.runs {
        if let Some(paint) = &run.background {
            let run_box = background_box(paragraph, line, run);
            match boxes.last_mut() {
                Some((merged, _)) if previous.is_some_and(|before| shares_box(before, run)) => {
                    *merged = merged.union(&run_box);
                }
                _ => boxes.push((run_box, paint)),
            }
        }
        previous = Some(run);
    }
    boxes.retain(|(bounds, _)| bounds.height > 0.0);
    boxes
}

/// `background_box` is the box a run's background fills: skparagraph's run
/// box (`TextLine::measureTextInsideOneRun`), across the run's advances and
/// down from its ascent to its descent at the style's height multiplier
/// (`Run::calculateHeight` at `InternalLineMetrics::runTop`). A
/// placeholder's box is its own.
fn background_box(paragraph: &Paragraph, line: &Line, run: &PlacedRun) -> Rect {
    if run.placeholder.is_some() {
        return run.bounds;
    }
    let font = paragraph.faces().get(run.font);
    let (ascent, descent, _) = style_heights(font, run.size, run.height);
    Rect::from_ltrb(
        run.bounds.x,
        line.baseline - ascent,
        run.bounds.right(),
        line.baseline + descent,
    )
}

/// `shares_box` reports whether `run` continues the background box of
/// `before`, the run left of it: both are text from what skparagraph shapes
/// as one run, with equal backgrounds.
fn shares_box(before: &PlacedRun, run: &PlacedRun) -> bool {
    before.placeholder.is_none()
        && run.placeholder.is_none()
        && before.background == run.background
        && before.font == run.font
        && before.size == run.size
        && before.synthesis == run.synthesis
        && before.height == run.height
        && before.rtl == run.rtl
}

/// `draw_glyphs` records a run's glyphs in `glyph_paint` when set, else in
/// the span's foreground, else in its color: skparagraph's text paint
/// (`TextLine::buildTextBlob`).
fn draw_glyphs(
    b: &mut DisplayListBuilder,
    fonts: &FaceSet,
    run: &PlacedRun,
    origin: Point,
    glyph_paint: Option<&Paint>,
) {
    match (glyph_paint, &run.foreground) {
        (Some(paint), _) | (None, Some(paint)) => draw_run(b, fonts, run, origin, paint),
        (None, None) => draw_run(b, fonts, run, origin, &Paint::from_color(run.color)),
    }
}

/// `draw_shadows` records styled shadows beneath their source glyph run.
///
/// Recording them first preserves the paragraph's back-to-front shadow order.
fn draw_shadows(b: &mut DisplayListBuilder, paragraph: &Paragraph, run: &PlacedRun, origin: Point) {
    for shadow in &run.shadows {
        let paint = Paint {
            color: shadow.color,
            mask_blur: (shadow.blur > 0.0).then(|| valo_dl::MaskBlur::new(shadow.blur)),
            ..Default::default()
        };
        b.save();
        b.translate(shadow.offset.x, shadow.offset.y);
        draw_run(b, paragraph.faces(), run, origin, &paint);
        b.restore();
    }
}

/// `draw_run` snapshots a placed glyph run into the display list.
///
/// It copies glyph positions and retains the resolved font so future paragraph
/// layout cannot affect the recorded run.
fn draw_run(
    b: &mut DisplayListBuilder,
    fonts: &FaceSet,
    run: &PlacedRun,
    origin: Point,
    paint: &Paint,
) {
    let glyphs: Vec<GlyphPos> = run
        .glyphs
        .iter()
        .map(|g| GlyphPos {
            id: g.id,
            x: g.x + origin.x,
            y: g.y + origin.y,
        })
        .collect();
    // Ink bounds, not the advance box: the record-time oracle culls,
    // sizes layers, and proves elision disjointness with these.
    let bounds = Rect::new(
        run.ink.x + origin.x,
        run.ink.y + origin.y,
        run.ink.width,
        run.ink.height,
    );
    b.draw_glyph_run(
        fonts.get_arc(run.font),
        run.size,
        run.synthesis,
        paint,
        Arc::new(glyphs),
        bounds,
    );
}

/// `draw_decoration` records a run's decoration as geometry.
///
/// Glyph runs contain no decoration data, so font metrics position the geometry
/// against the resolved face and line baseline.
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
    let font = paragraph.faces().get(run.font);
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
