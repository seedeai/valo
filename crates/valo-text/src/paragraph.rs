use std::collections::HashMap;
use std::ops::Range;

use unicode_bidi::BidiInfo;
use valo_geometry::{Color, Rect};

use crate::font::{FaceSet, FontCollection, FontDemand, FontId};
use crate::shape::{shape_runs, ShapedRun};
use crate::style::{ParagraphStyle, TextStyle};
use crate::wrap::{place_lines, wrap_lines, Wrapped};

/// One positioned glyph: `x`/`y` in paragraph-local px, `y` on the baseline.
#[derive(Clone, Copy, Debug)]
pub struct PlacedGlyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    /// Byte offset of this glyph's cluster in the paragraph text.
    pub cluster: usize,
    /// The cursor delta this glyph consumed (justify stretch included).
    pub advance: f32,
}

/// A line's worth of one font+size+color — exactly what one `GlyphRun`
/// display-list op carries.
#[derive(Clone, Debug)]
pub struct PlacedRun {
    pub font: FontId,
    pub size: f32,
    pub color: Color,
    pub decoration: Option<crate::style::Decoration>,
    pub shadows: Vec<crate::style::Shadow>,
    pub glyphs: Vec<PlacedGlyph>,
    /// The run reads right-to-left: a glyph's LOGICAL start is its visual
    /// right edge (`x + advance`), its end the left (skparagraph's
    /// `Run::leftToRight()` from the bidi level).
    pub rtl: bool,
    /// Paragraph-local ADVANCE box: x from the cursor, y from ascent/
    /// descent. Decorations and selection geometry live here.
    pub bounds: Rect,
    /// Paragraph-local INK bounds: the advance box widened by the font's
    /// ink box (bearings, italic overhang, mark excursions) — what the
    /// renderer must treat as the run's pixel extent (culling, layer
    /// sizing, the opacity-elision disjointness proof).
    pub ink: Rect,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub runs: Vec<PlacedRun>,
    /// y of the baseline, paragraph-local.
    pub baseline: f32,
    /// Max ascent/descent over the line's runs (px above/below baseline).
    pub ascent: f32,
    pub descent: f32,
    /// x where content starts (alignment shift included).
    pub left: f32,
    /// Content width (trailing whitespace excluded).
    pub width: f32,
    /// Byte range of the paragraph text this line covers.
    pub range: Range<usize>,
}

/// A finished `layout(width)`: the placed lines plus what they were placed
/// against (the cache key for the re-layout tier).
#[derive(Clone, Debug)]
pub(crate) struct Layout {
    pub max_width: f32,
    pub lines: Vec<Line>,
    pub width: f32,
    pub height: f32,
    /// `max_lines` cut content off (the ellipsis case).
    pub truncated: bool,
    /// The chosen line ranges — re-placing (recolor) skips re-wrapping.
    pub wrapped: Wrapped,
}

/// The retained paragraph, with Skia's state tiers made explicit
/// (SkParagraph's kShaped/kWrapped/kFormatted ladder):
/// - `ParagraphBuilder::build()` runs the EXPENSIVE tier once — fallback
///   segmentation + harfrust shaping — and retains it.
/// - `layout(width)` re-wraps and places from the retained shaping (cheap;
///   cached when the width doesn't change).
/// - `update_color(span, color)` re-places only (no reshape, no rewrap) —
///   the text-editing hot path.
///
/// Cloning duplicates the shaped and laid-out data (no re-shaping) — a cheap
/// way for hosts to snapshot a layout before re-wrapping in place.
#[derive(Clone)]
pub struct Paragraph {
    faces: FaceSet,
    text: String,
    style: ParagraphStyle,
    spans: Vec<(Range<usize>, TextStyle)>,
    shaped: Vec<ShapedRun>,
    layout: Option<Layout>,
    /// Precomputed at build (glyphless lines measure by the first span's
    /// style) — the paragraph needs no collection afterwards.
    empty_metrics: Option<(f32, f32, f32)>,
    /// What THIS text could not resolve, even after the collection asked
    /// its sources — the per-paragraph half of the async loop.
    demand: FontDemand,
}

impl Paragraph {
    /// Wrap and place against `max_width` (`f32::INFINITY` = never wrap).
    /// Same width twice = cache hit; shaping is NEVER redone here.
    pub fn layout(&mut self, max_width: f32) {
        if self
            .layout
            .as_ref()
            .is_some_and(|l| l.max_width == max_width)
        {
            return;
        }
        let bidi = BidiInfo::new(&self.text, None);
        let wrapped = wrap_lines(
            &self.text,
            &self.shaped,
            max_width,
            self.style.max_lines,
            self.style.preserve_trailing_whitespace,
        );
        self.layout = Some(self.place(&bidi, wrapped, max_width));
    }

    /// The repaint tier: recolor one styled span and re-place — shaping and
    /// line breaks are untouched (color never moves a glyph).
    pub fn update_color(&mut self, span: usize, color: Color) {
        let Some((range, style)) = self.spans.get_mut(span) else {
            return;
        };
        style.color = color;
        let range = range.clone();
        for run in &mut self.shaped {
            if run.range.start >= range.start && run.range.end <= range.end {
                run.color = color;
            }
        }
        if let Some(prior) = self.layout.take() {
            let bidi = BidiInfo::new(&self.text, None);
            self.layout = Some(self.place(&bidi, prior.wrapped, prior.max_width));
        }
    }

    fn place(&self, bidi: &BidiInfo, wrapped: Wrapped, max_width: f32) -> Layout {
        place_lines(
            &self.faces,
            &self.text,
            &self.shaped,
            bidi,
            wrapped,
            max_width,
            &self.style,
            self.empty_line_metrics(),
        )
    }

    /// skparagraph's computeEmptyMetrics: glyphless lines (blank first line,
    /// trailing newline, empty paragraph) measure as the FIRST span's style.
    fn empty_line_metrics(&self) -> Option<(f32, f32, f32)> {
        self.empty_metrics
    }

    /// Placed lines of the most recent `layout` (empty before one).
    pub fn lines(&self) -> &[Line] {
        self.layout.as_ref().map_or(&[], |l| &l.lines)
    }

    /// Widest line's content width after `layout`.
    pub fn width(&self) -> f32 {
        self.layout.as_ref().map_or(0.0, |l| l.width)
    }

    pub fn height(&self) -> f32 {
        self.layout.as_ref().map_or(0.0, |l| l.height)
    }

    /// Tight visible ink bounds in paragraph coordinates. Vector glyphs use
    /// Bézier extrema; color/bitmap glyphs use their non-transparent pixels.
    /// This deliberate slower query path is for Canvas-style text metrics;
    /// frame recording keeps using precomputed conservative run bounds.
    pub fn ink_bounds(&self) -> Option<Rect> {
        let mut result: Option<Rect> = None;
        let mut rasterizer = crate::raster::Rasterizer::new();
        let mut color_bounds = HashMap::<(FontId, u32, u32), Option<Rect>>::new();
        for run in self.lines().iter().flat_map(|line| &line.runs) {
            let font = self.faces.get(run.font);
            for glyph in &run.glyphs {
                let key = (run.font, glyph.id, run.size.to_bits());
                let color = *color_bounds
                    .entry(key)
                    .or_insert_with(|| rasterizer.color_bounds(font, glyph.id, run.size));
                let bounds = if let Some(bounds) = color {
                    bounds
                } else if let Some(path) = crate::raster::glyph_path(font, glyph.id, run.size) {
                    path.tight_bounds()
                } else {
                    continue;
                };
                let placed = Rect::new(
                    bounds.x + glyph.x,
                    bounds.y + glyph.y,
                    bounds.width,
                    bounds.height,
                );
                result = Some(result.map_or(placed, |current| current.union(&placed)));
            }
        }
        result
    }

    /// Primary face and size even when the paragraph contains no glyphs.
    pub fn primary_font(&self) -> Option<(&crate::font::Font, f32)> {
        if let Some(run) = self.lines().first().and_then(|line| line.runs.first()) {
            return Some((self.faces.get(run.font), run.size));
        }
        let (_, style) = self.spans.first()?;
        if self.faces.is_empty() {
            return None;
        }
        let attributes = crate::font::FontAttrs {
            weight: style.weight,
            italic: style.italic,
        };
        let identifier = self.faces.resolve(&style.families, attributes, ' ');
        Some((self.faces.get(identifier), style.size))
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width(), self.height())
    }

    /// `max_lines` dropped content (what an ellipsis marks).
    pub fn truncated(&self) -> bool {
        self.layout.as_ref().is_some_and(|l| l.truncated)
    }

    /// Widest unbreakable segment — the narrowest useful layout width.
    pub fn min_intrinsic_width(&self) -> f32 {
        self.layout
            .as_ref()
            .map_or(0.0, |l| l.wrapped.min_intrinsic)
    }

    /// Width when nothing wraps (widest hard-break line).
    pub fn max_intrinsic_width(&self) -> f32 {
        self.layout
            .as_ref()
            .map_or(0.0, |l| l.wrapped.max_intrinsic)
    }

    /// Widest laid-out line's content width.
    pub fn longest_line(&self) -> f32 {
        self.width()
    }
}

/// Collects styled spans; `build()` runs fallback segmentation + shaping
/// once and hands back the retained [`Paragraph`].
pub struct ParagraphBuilder<'a> {
    fonts: &'a mut FontCollection,
    style: ParagraphStyle,
    text: String,
    spans: Vec<(Range<usize>, TextStyle)>,
}

impl<'a> ParagraphBuilder<'a> {
    /// Skia's `ParagraphBuilder::make(style, fontCollection, unicode)`:
    /// the collection comes in at construction and answers misses itself.
    pub fn new(fonts: &'a mut FontCollection) -> Self {
        Self {
            fonts,
            style: ParagraphStyle::default(),
            text: String::new(),
            spans: Vec::new(),
        }
    }

    pub fn style(&mut self, style: ParagraphStyle) -> &mut Self {
        self.style = style;
        self
    }

    pub fn add_text(&mut self, text: &str, style: &TextStyle) -> &mut Self {
        let start = self.text.len();
        self.text.push_str(text);
        self.spans.push((start..self.text.len(), style.clone()));
        self
    }

    /// The expensive tier: segment + shape, consulting the collection's
    /// sources at every miss (Skia's one `build()`; what no source could
    /// answer waits on the collection as `take_unanswered`). The paragraph
    /// keeps the faces it resolved, so it is self-contained afterwards.
    pub fn build(&mut self) -> Paragraph {
        let bidi = BidiInfo::new(&self.text, None);
        let mut demand = FontDemand::default();
        let shaped = shape_runs(self.fonts, &self.text, &self.spans, &bidi, &mut demand);
        let faces = self.fonts.faces().clone();
        let empty_metrics = empty_line_metrics(&faces, self.spans.first());
        Paragraph {
            faces,
            text: std::mem::take(&mut self.text),
            style: std::mem::take(&mut self.style),
            spans: std::mem::take(&mut self.spans),
            shaped,
            layout: None,
            empty_metrics,
            demand,
        }
    }
}

// ── the editor surface (skparagraph's Paragraph.h queries) ─────────────────

/// A byte offset plus which side of it the position leans (SkParagraph's
/// PositionWithAffinity): `downstream` = the caret belongs to the glyph
/// AFTER the offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PositionWithAffinity {
    pub offset: usize,
    pub downstream: bool,
}

/// Per-line metrics for caret/selection UIs (skparagraph's LineMetrics).
#[derive(Clone, Debug)]
pub struct LineMetrics {
    pub range: Range<usize>,
    pub baseline: f32,
    pub ascent: f32,
    pub descent: f32,
    pub left: f32,
    pub width: f32,
}

impl Paragraph {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// What this paragraph could not resolve (families and codepoints no
    /// source answered) — a host fetches them and re-registers.
    pub fn demand(&self) -> &FontDemand {
        &self.demand
    }

    /// The faces this paragraph resolved (decoration metrics, glyph
    /// lookup at record time).
    pub fn faces(&self) -> &FaceSet {
        &self.faces
    }

    pub fn line_metrics(&self) -> Vec<LineMetrics> {
        self.lines()
            .iter()
            .map(|line| LineMetrics {
                range: line.range.clone(),
                baseline: line.baseline,
                ascent: line.ascent,
                descent: line.descent,
                left: line.left,
                width: line.width,
            })
            .collect()
    }

    /// The caret rectangle (zero width) for a byte offset — the leading
    /// edge of the cluster at `offset`, or the trailing edge of the last
    /// cluster before it.
    pub fn caret_for_offset(&self, offset: usize) -> Rect {
        let Some(line) = self.line_for_offset(offset) else {
            return Rect::default();
        };
        let x = caret_x(line, offset);
        Rect::new(
            x,
            line.baseline - line.ascent,
            0.0,
            line.ascent + line.descent,
        )
    }

    /// The text position under a point (SkParagraph's
    /// getGlyphPositionAtCoordinate): nearest line by y, nearest cluster
    /// edge by x.
    pub fn glyph_position_at(&self, p: valo_geometry::Point) -> PositionWithAffinity {
        let Some(line) = self.line_at_y(p.y) else {
            return PositionWithAffinity {
                offset: 0,
                downstream: true,
            };
        };
        let mut best = PositionWithAffinity {
            offset: line.range.start,
            downstream: true,
        };
        let mut best_dx = f32::MAX;
        for run in &line.runs {
            for g in &run.glyphs {
                // An RTL cluster's LOGICAL start is its visual right edge.
                let (lead_x, trail_x) = if run.rtl {
                    (g.x + g.advance, g.x)
                } else {
                    (g.x, g.x + g.advance)
                };
                let leading = (p.x - lead_x).abs();
                if leading < best_dx {
                    best_dx = leading;
                    best = PositionWithAffinity {
                        offset: g.cluster,
                        downstream: true,
                    };
                }
                let trailing = (p.x - trail_x).abs();
                if trailing < best_dx {
                    best_dx = trailing;
                    best = PositionWithAffinity {
                        offset: self.cluster_end(g.cluster),
                        downstream: false,
                    };
                }
            }
        }
        best
    }

    /// Selection boxes for a byte range: one rect per (line, run) span —
    /// bidi ranges yield multiple boxes naturally, like SkParagraph's
    /// getRectsForRange.
    pub fn rects_for_range(&self, range: Range<usize>) -> Vec<Rect> {
        let mut out = Vec::new();
        for line in self.lines() {
            if range.end <= line.range.start || range.start >= line.range.end {
                continue;
            }
            for run in &line.runs {
                let cells: Vec<&PlacedGlyph> = run
                    .glyphs
                    .iter()
                    .filter(|g| g.cluster >= range.start && g.cluster < range.end)
                    .collect();
                let Some(first) = cells.first() else {
                    continue;
                };
                let x0 = cells.iter().map(|g| g.x).fold(first.x, f32::min);
                let x1 = cells
                    .iter()
                    .map(|g| g.x + g.advance)
                    .fold(first.x + first.advance, f32::max);
                out.push(Rect::from_ltrb(
                    x0,
                    line.baseline - line.ascent,
                    x1,
                    line.baseline + line.descent,
                ));
            }
        }
        out
    }

    /// The word containing `offset` (UAX #29 word boundaries).
    pub fn word_boundary(&self, offset: usize) -> Range<usize> {
        use unicode_segmentation::UnicodeSegmentation;
        for (start, word) in self.text.split_word_bound_indices() {
            if offset < start + word.len() {
                return start..start + word.len();
            }
        }
        self.text.len()..self.text.len()
    }

    fn line_for_offset(&self, offset: usize) -> Option<&Line> {
        let lines = self.lines();
        lines
            .iter()
            .find(|l| l.range.contains(&offset))
            .or(lines.last())
    }

    fn line_at_y(&self, y: f32) -> Option<&Line> {
        let lines = self.lines();
        lines
            .iter()
            .find(|l| y <= l.baseline + l.descent)
            .or(lines.last())
    }

    /// The cluster's trailing offset: the NEXT shaped cluster on the same
    /// line (shaping already groups combining marks — one char would land
    /// a caret INSIDE `e + U+0301`), else the next grapheme boundary.
    fn cluster_end(&self, cluster: usize) -> usize {
        let next_on_line = self
            .line_for_offset(cluster)
            .into_iter()
            .flat_map(|l| l.runs.iter())
            .flat_map(|r| r.glyphs.iter())
            .map(|g| g.cluster)
            .filter(|&c| c > cluster)
            .min();
        next_on_line.unwrap_or_else(|| self.next_grapheme(cluster))
    }

    fn next_grapheme(&self, offset: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        self.text[offset..]
            .graphemes(true)
            .next()
            .map_or(self.text.len(), |g| offset + g.len())
    }
}

/// Leading edge of the cluster at `offset`, else the nearest trailing edge
/// before it, else the line's left edge. Edges flip per run direction.
fn caret_x(line: &Line, offset: usize) -> f32 {
    let mut before: Option<(usize, f32)> = None;
    for run in &line.runs {
        for g in &run.glyphs {
            let (lead_x, trail_x) = if run.rtl {
                (g.x + g.advance, g.x)
            } else {
                (g.x, g.x + g.advance)
            };
            if g.cluster == offset {
                return lead_x;
            }
            if g.cluster < offset && before.is_none_or(|(c, _)| g.cluster > c) {
                before = Some((g.cluster, trail_x));
            }
        }
    }
    before.map_or(line.left, |(_, x)| x)
}

/// skparagraph's computeEmptyMetrics, resolved once at build: glyphless
/// lines (blank first line, trailing newline, empty paragraph) measure as
/// the FIRST span's style.
fn empty_line_metrics(
    faces: &FaceSet,
    first_span: Option<&(std::ops::Range<usize>, TextStyle)>,
) -> Option<(f32, f32, f32)> {
    let (_, style) = first_span?;
    if faces.is_empty() {
        return None;
    }
    let attrs = crate::font::FontAttrs {
        weight: style.weight,
        italic: style.italic,
    };
    let id = faces.resolve(&style.families, attrs, ' ');
    Some(crate::wrap::style_heights(
        faces.get(id),
        style.size,
        style.height,
    ))
}
