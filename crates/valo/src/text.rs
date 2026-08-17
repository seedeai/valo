//! Paragraph recording lowers the current layout into independent display-list
//! operations. The list owns the resolved fonts, positions, and paints rather
//! than retaining the paragraph, so later paragraph changes cannot mutate it.

use std::sync::Arc;

use valo_dl::{DisplayListBuilder, GlyphPos, Paint};
use valo_geometry::{Point, Rect};
use valo_text::{DecorationKind, FaceSet, Line, Paragraph, PlacedRun};

/// `DrawParagraphExt` adds paragraph recording to [`DisplayListBuilder`].
pub trait DrawParagraphExt {
    /// `draw_paragraph` records the paragraph's current layout and span styles.
    ///
    /// The paragraph is lowered into glyph runs, shadows, and decorations at
    /// its top-left `origin`. Later layout or style changes do not affect the
    /// recorded display list.
    fn draw_paragraph(&mut self, paragraph: &Paragraph, origin: impl Into<Point>);
}

/// `DrawGlyphRunExt` adds paragraph recording with an explicit fill paint.
///
/// Use it for gradient or blended text. Shadows and decorations continue to
/// use the paragraph's span styles.
pub trait DrawGlyphRunExt {
    /// `draw_paragraph_with` records the current layout with one fill paint.
    ///
    /// `paint` replaces every span's fill, while shadows and decorations retain
    /// their span styles. Later paragraph changes do not affect the display list.
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
                draw_run(self, paragraph.faces(), run, origin, paint);
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
                draw_run(
                    self,
                    paragraph.faces(),
                    run,
                    origin,
                    &Paint::from_color(run.color),
                );
                draw_decoration(self, paragraph, line, run, origin);
            }
        }
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
