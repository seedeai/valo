use std::collections::HashMap;
use std::ops::Range;

use unicode_bidi::BidiInfo;
use valo_geometry::{Color, Rect};

use crate::font::{FaceSet, FontCollection, FontDemand, FontId};
use crate::shape::{shape_runs, ShapedRun};
use crate::style::{ParagraphStyle, TextDirection, TextStyle};
use crate::wrap::{place_lines, wrap_lines, Wrapped};

/// `PlacedGlyph` describes one shaped glyph positioned within a paragraph.
#[derive(Clone, Copy, Debug)]
pub struct PlacedGlyph {
    /// `id` is the font-specific glyph identifier.
    pub id: u32,
    /// `x` is the paragraph-local horizontal origin in logical pixels.
    pub x: f32,
    /// `y` is the paragraph-local baseline origin in logical pixels.
    pub y: f32,
    /// `cluster` is the UTF-8 byte offset of the glyph's text cluster.
    pub cluster: usize,
    /// `advance` is the signed cursor movement, including justification.
    pub advance: f32,
}

/// `PlacedRun` groups positioned glyphs sharing one font and paint style.
#[derive(Clone, Debug)]
pub struct PlacedRun {
    /// `font` identifies the font within the paragraph's [`FaceSet`].
    pub font: FontId,
    /// `size` is the font size in logical pixels.
    pub size: f32,
    /// `color` is the glyph fill color.
    pub color: Color,
    /// `decoration` optionally adds a line relative to the run.
    pub decoration: Option<crate::style::Decoration>,
    /// `shadows` are painted back-to-front beneath the glyphs.
    pub shadows: Vec<crate::style::Shadow>,
    /// `glyphs` contains the run's glyphs in visual order.
    pub glyphs: Vec<PlacedGlyph>,
    /// `rtl` indicates that logical text order runs from right to left.
    pub rtl: bool,
    /// `bounds` is the paragraph-local advance box used for layout geometry.
    pub bounds: Rect,
    /// `ink` conservatively bounds visible glyph pixels in paragraph coordinates.
    pub ink: Rect,
}

/// `Line` contains the visually ordered runs and metrics of one laid-out line.
#[derive(Clone, Debug)]
pub struct Line {
    /// `runs` contains the line's visually ordered glyph runs.
    pub runs: Vec<PlacedRun>,
    /// `baseline` is the paragraph-local y coordinate of the text baseline.
    pub baseline: f32,
    /// `ascent` is the maximum distance above the baseline in logical pixels.
    pub ascent: f32,
    /// `descent` is the maximum distance below the baseline in logical pixels.
    pub descent: f32,
    /// `left` is the paragraph-local x coordinate after alignment.
    pub left: f32,
    /// `width` is the signed content advance in logical pixels.
    ///
    /// Trailing whitespace contributes only when requested by [`ParagraphStyle`].
    pub width: f32,
    /// `range` is the UTF-8 byte range of paragraph text covered by the line.
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

/// `Paragraph` is the primary API for laying out and drawing text.
///
/// [`ParagraphBuilder::build`] shapes the text, selecting fonts and converting
/// characters into positioned glyph sequences. [`Self::layout`] then wraps
/// those glyphs into lines and positions them within a width. Call `layout`
/// before drawing or reading layout metrics. It can be called again at another
/// width without repeating shaping.
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
    /// `layout` prepares the paragraph for drawing within `max_width`.
    ///
    /// It wraps the shaped glyphs into lines and computes their positions and
    /// metrics. Call it before drawing the paragraph. Use `f32::INFINITY` to
    /// disable soft wrapping. Repeating the same width reuses the existing
    /// layout; shaping is never repeated.
    pub fn layout(&mut self, max_width: f32) {
        if self
            .layout
            .as_ref()
            .is_some_and(|l| l.max_width == max_width)
        {
            return;
        }
        let bidi = BidiInfo::new(&self.text, base_level(&self.style));
        let wrapped = wrap_lines(
            &self.text,
            &self.shaped,
            max_width,
            self.style.max_lines,
            self.style.preserve_trailing_whitespace,
        );
        self.layout = Some(self.place(&bidi, wrapped, max_width));
    }

    /// `update_color` changes one added span without reshaping or rewrapping.
    ///
    /// An out-of-range span index has no effect.
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
            let bidi = BidiInfo::new(&self.text, base_level(&self.style));
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

    /// `lines` returns the most recently laid-out lines.
    ///
    /// It is empty before [`Self::layout`] is called.
    pub fn lines(&self) -> &[Line] {
        self.layout.as_ref().map_or(&[], |l| &l.lines)
    }

    /// `width` returns the nonnegative width of the widest laid-out line.
    ///
    /// It is zero before [`Self::layout`] is called.
    pub fn width(&self) -> f32 {
        self.layout.as_ref().map_or(0.0, |l| l.width)
    }

    /// `advance` returns the greatest signed line advance.
    ///
    /// This is NOT [`Paragraph::width`]. A width is a layout box, so it has a
    /// floor at zero: wrapping, alignment and `bounds()` are all defined on a
    /// rectangle, and one narrower than nothing means nothing. An advance has
    /// no such floor. Letter and word spacing tighter than the glyphs are wide
    /// walks the pen backwards, and callers that report a pen position rather
    /// than a box — Canvas2D's `TextMetrics.width` — need the negative.
    pub fn advance(&self) -> f32 {
        self.lines()
            .iter()
            .map(|line| line.width)
            .reduce(f32::max)
            .unwrap_or(0.0)
    }

    /// `last_glyph_origin` returns the paragraph-local x origin of the final glyph.
    ///
    /// It returns `None` before layout or when no glyph was placed.
    pub fn last_glyph_origin(&self) -> Option<f32> {
        self.lines()
            .iter()
            .flat_map(|line| &line.runs)
            .flat_map(|run| &run.glyphs)
            .next_back()
            .map(|glyph| glyph.x)
    }

    /// `height` returns the laid-out paragraph height in logical pixels.
    ///
    /// It is zero before [`Self::layout`] is called.
    pub fn height(&self) -> f32 {
        self.layout.as_ref().map_or(0.0, |l| l.height)
    }

    /// `ink_bounds` returns tight visible glyph bounds in paragraph coordinates.
    ///
    /// It returns `None` before layout or when the paragraph has no visible
    /// glyphs. This query may rasterize color glyphs.
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

    /// `primary_font` returns the first run's font and size.
    ///
    /// For glyphless text it resolves the first styled span instead. It returns
    /// `None` when no span or font is available.
    pub fn primary_font(&self) -> Option<(&crate::font::Font, f32)> {
        if let Some(run) = self.lines().first().and_then(|line| line.runs.first()) {
            return Some((self.faces.get(run.font), run.size));
        }
        let (_, style) = self.spans.first()?;
        if self.faces.is_empty() {
            return None;
        }
        let attributes = style.font_attrs();
        let identifier = self.faces.resolve(&style.families, attributes, ' ');
        Some((self.faces.get(identifier), style.size))
    }

    /// `bounds` returns the paragraph's layout box at the origin.
    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width(), self.height())
    }

    /// `truncated` reports whether the line limit omitted content.
    pub fn truncated(&self) -> bool {
        self.layout.as_ref().is_some_and(|l| l.truncated)
    }

    /// `min_intrinsic_width` returns the widest unbreakable segment.
    ///
    /// It is zero before [`Self::layout`] is called.
    pub fn min_intrinsic_width(&self) -> f32 {
        self.layout
            .as_ref()
            .map_or(0.0, |l| l.wrapped.min_intrinsic)
    }

    /// `max_intrinsic_width` returns the width required to avoid soft wrapping.
    ///
    /// It is zero before [`Self::layout`] is called.
    pub fn max_intrinsic_width(&self) -> f32 {
        self.layout
            .as_ref()
            .map_or(0.0, |l| l.wrapped.max_intrinsic)
    }

    /// `longest_line` returns the width of the widest laid-out line.
    pub fn longest_line(&self) -> f32 {
        self.width()
    }
}

/// `ParagraphBuilder` assembles styled text into a [`Paragraph`] for layout and drawing.
///
/// Each added span can use a different [`TextStyle`]. Building selects fonts
/// from the borrowed [`FontCollection`] and shapes the text into glyphs.
pub struct ParagraphBuilder<'a> {
    fonts: &'a mut FontCollection,
    style: ParagraphStyle,
    text: String,
    spans: Vec<(Range<usize>, TextStyle)>,
}

impl<'a> ParagraphBuilder<'a> {
    /// `new` creates an empty builder with the default [`ParagraphStyle`].
    pub fn new(fonts: &'a mut FontCollection) -> Self {
        Self {
            fonts,
            style: ParagraphStyle::default(),
            text: String::new(),
            spans: Vec::new(),
        }
    }

    /// `style` replaces the paragraph-level layout style.
    pub fn style(&mut self, style: ParagraphStyle) -> &mut Self {
        self.style = style;
        self
    }

    /// `add_text` appends a UTF-8 text span with its own style.
    ///
    /// The zero-based call order defines indices accepted by
    /// [`Paragraph::update_color`].
    pub fn add_text(&mut self, text: &str, style: &TextStyle) -> &mut Self {
        let start = self.text.len();
        self.text.push_str(text);
        self.spans.push((start..self.text.len(), style.clone()));
        self
    }

    /// `build` shapes the accumulated spans and drains the builder.
    ///
    /// Font sources are consulted for missing text. The returned paragraph
    /// snapshots resolved faces and no longer borrows the collection. The empty
    /// builder can be reused afterward.
    pub fn build(&mut self) -> Paragraph {
        let bidi = BidiInfo::new(&self.text, base_level(&self.style));
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

/// `PositionWithAffinity` identifies where to place a caret in editable text.
///
/// It is returned by [`Paragraph::glyph_position_at`] when mapping a pointer
/// position back to text. At line wraps and bidirectional boundaries, one text
/// offset can have two visual caret positions. Affinity selects whether the
/// caret belongs with the text before or after that offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PositionWithAffinity {
    /// `offset` is a UTF-8 byte offset in paragraph text.
    pub offset: usize,
    /// `downstream` selects the text after the offset when true, or before it when false.
    pub downstream: bool,
}

/// `LineMetrics` describes one laid-out line for caret and selection geometry.
#[derive(Clone, Debug)]
pub struct LineMetrics {
    /// `range` is the UTF-8 byte range covered by the line.
    pub range: Range<usize>,
    /// `baseline` is the paragraph-local y coordinate of the baseline.
    pub baseline: f32,
    /// `ascent` is the logical-pixel distance above the baseline.
    pub ascent: f32,
    /// `descent` is the logical-pixel distance below the baseline.
    pub descent: f32,
    /// `left` is the paragraph-local x coordinate after alignment.
    pub left: f32,
    /// `width` is the line's signed content advance in logical pixels.
    pub width: f32,
}

impl Paragraph {
    /// `text` returns the complete UTF-8 paragraph text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// `demand` returns font requests unresolved while building this paragraph.
    ///
    /// A host can load matching fonts, register them with [`FontCollection`],
    /// and rebuild the paragraph to replace missing-glyph boxes.
    pub fn demand(&self) -> &FontDemand {
        &self.demand
    }

    /// `faces` returns the font snapshot retained for glyph lookup and drawing.
    pub fn faces(&self) -> &FaceSet {
        &self.faces
    }

    /// `line_metrics` returns measurements for every laid-out line.
    ///
    /// It is empty before [`Self::layout`] is called.
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

    /// `caret_for_offset` returns a zero-width caret rectangle for a UTF-8 offset.
    ///
    /// The offset snaps to a cluster edge. It returns [`Rect::default`] before
    /// layout or when no line exists.
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

    /// `glyph_position_at` maps a paragraph-local point to an editable text position.
    ///
    /// Use it to place a caret from a pointer press. It chooses the nearest line
    /// and glyph-cluster edge. An unlaid-out or empty paragraph returns offset
    /// zero with downstream affinity.
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

    /// `rects_for_range` returns boxes for painting a selected UTF-8 byte range.
    ///
    /// It returns one box per intersecting line and visual run, so bidirectional
    /// text may produce multiple boxes on one line.
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

    /// `word_boundary` returns the text segment selected as a word at an offset.
    ///
    /// It follows Unicode word boundaries, making it suitable for word selection
    /// from a double click. Offsets at or beyond the text end return an empty
    /// range at the end.
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

    /// `cluster_end` returns the cluster's trailing UTF-8 offset.
    ///
    /// It uses the next shaped cluster on the same
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

/// `caret_x` returns the leading edge at an offset or the nearest prior edge.
///
/// It uses the leading edge of the cluster at `offset`, else the nearest trailing edge
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

/// `base_level` returns an explicit bidi level or leaves content to choose it.
fn base_level(style: &ParagraphStyle) -> Option<unicode_bidi::Level> {
    style.direction.map(|direction| match direction {
        TextDirection::Ltr => unicode_bidi::Level::ltr(),
        TextDirection::Rtl => unicode_bidi::Level::rtl(),
    })
}

/// `empty_line_metrics` resolves glyphless line metrics from the first span.
fn empty_line_metrics(
    faces: &FaceSet,
    first_span: Option<&(std::ops::Range<usize>, TextStyle)>,
) -> Option<(f32, f32, f32)> {
    let (_, style) = first_span?;
    if faces.is_empty() {
        return None;
    }
    let attrs = style.font_attrs();
    let id = faces.resolve(&style.families, attrs, ' ');
    Some(crate::wrap::style_heights(
        faces.get(id),
        style.size,
        style.height,
    ))
}
