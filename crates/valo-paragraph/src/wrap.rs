use std::ops::Range;
use std::sync::Arc;

use unicode_bidi::BidiInfo;
use valo_text::{FaceSet, Font};

use crate::paragraph::{Layout, Line, PlacedGlyph, PlacedRun};
use crate::shape::{shape_ellipsis, ShapedPlaceholder, ShapedRun};
use crate::style::{LeadingDistribution, ParagraphStyle, TextAlign};

/// The wrap tier's output: chosen line ranges, truncation, and the
/// intrinsic widths accumulated during the walk (Skia's TextWrapper carries
/// the same trio). Retained on the layout so re-placing skips re-wrapping.
#[derive(Clone, Debug)]
pub struct Wrapped {
    pub ranges: Vec<Range<usize>>,
    pub truncated: bool,
    /// Widest unbreakable segment (auto-sizing's lower bound).
    pub min_intrinsic: f32,
    /// Widest hard-break line when nothing wraps (the upper bound).
    pub max_intrinsic: f32,
}

/// Cluster widths + prefix sums, built once per layout: every width query
/// is a binary search instead of a scan over all glyphs — the wrapper walk
/// is O(n log n) instead of O(n²) (Skia wraps over a cluster table too).
pub(crate) struct Measure {
    bytes: Vec<usize>,
    prefix: Vec<f32>,
}

impl Measure {
    pub(crate) fn new(runs: &[ShapedRun]) -> Self {
        let mut widths: Vec<(usize, f32)> = runs
            .iter()
            .flat_map(|run| run.glyphs.iter())
            .map(|g| (g.cluster, g.x_advance))
            .collect();
        widths.sort_by_key(|&(cluster, _)| cluster);
        let mut bytes = Vec::with_capacity(widths.len());
        let mut prefix = vec![0.0f32];
        for (cluster, advance) in widths {
            if bytes.last() == Some(&cluster) {
                *prefix.last_mut().expect("nonempty") += advance;
            } else {
                bytes.push(cluster);
                let total = prefix.last().copied().expect("nonempty") + advance;
                prefix.push(total);
            }
        }
        Self { bytes, prefix }
    }

    /// Total advance of clusters starting in `[start, end)`.
    pub(crate) fn width(&self, start: usize, end: usize) -> f32 {
        let a = self.bytes.partition_point(|&b| b < start);
        let b = self.bytes.partition_point(|&b| b < end);
        self.prefix[b] - self.prefix[a]
    }
}

/// Greedy UAX #14 wrapping over the endless-line shaping: walk
/// break opportunities, commit a line at the last one that still fit.
/// Trailing whitespace rides the line and normally does not count toward its
/// width. Canvas-style callers can preserve its advance.
/// Min/max intrinsic widths ride the same walk, which goes on over the text
/// the line limit cut off: skparagraph's TextWrapper scans that rest for them too,
/// so a truncated paragraph still reports the width its whole text asks for.
pub fn wrap_lines(
    text: &str,
    runs: &[ShapedRun],
    max_width: f32,
    limit: LineLimit,
    preserve_trailing_whitespace: bool,
) -> Wrapped {
    let measure = Measure::new(runs);
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut committed = 0usize; // last opportunity that fit
    let mut full = false;
    let mut intrinsics = Intrinsics::default();
    for (at, kind) in unicode_linebreak::linebreaks(text) {
        intrinsics.segment(
            text,
            &measure,
            committed,
            at,
            kind,
            preserve_trailing_whitespace,
        );
        if !full {
            if measured_width(text, &measure, start..at, preserve_trailing_whitespace) > max_width
                && committed > start
            {
                ranges.push(start..committed);
                start = committed;
                full = limit.is_reached(ranges.len(), LineEnd::Soft);
            }
            // Re-check the limit: the overflow push above may have just reached it
            // (one iteration can otherwise push TWICE and overshoot max_lines).
            if kind == unicode_linebreak::BreakOpportunity::Mandatory && !full {
                ranges.push(start..at);
                start = at;
                full = limit.is_reached(ranges.len(), LineEnd::Hard);
            }
        }
        committed = at;
    }
    if start < text.len() && !full {
        ranges.push(start..text.len());
    }
    // A trailing hard break opens one more, EMPTY line (skparagraph emits
    // it with fEmptyMetrics) — the caret after "hi\n" lives there.
    if !full
        && text.chars().last().is_some_and(is_hard_break)
        && ranges.last().is_some_and(|r| r.end == text.len())
    {
        ranges.push(text.len()..text.len());
    }
    // An empty paragraph still has ONE line — a zero-height caret is useless.
    if ranges.is_empty() {
        ranges.push(0..0);
    }
    let (min_intrinsic, max_intrinsic) = intrinsics.reported(limit);
    Wrapped {
        truncated: full && start < text.len(),
        ranges,
        min_intrinsic,
        max_intrinsic,
    }
}

/// `LineLimit` is where wrapping stops laying out lines, as skparagraph's TextWrapper
/// decides it: after `max_lines` lines, or, for a paragraph with an ellipsis and no
/// `max_lines`, after the first line that overflows the width (Flutter's `ellipsis` contract).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineLimit {
    /// At most this many lines; `usize::MAX` for no limit.
    Lines(usize),
    /// Lines a hard break ends go on; the first one a soft break ends is the last.
    FirstOverflow,
}

/// How a committed line ended: at a hard break, or wrapped at a soft one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineEnd {
    Hard,
    Soft,
}

impl LineLimit {
    /// `of` is the limit `style` sets. An empty ellipsis ellipsizes nothing, as
    /// skparagraph's `ParagraphStyle::ellipsized` says.
    pub fn of(style: &ParagraphStyle) -> Self {
        let ellipsized = style.ellipsis.as_deref().is_some_and(|e| !e.is_empty());
        match style.max_lines {
            Some(lines) => Self::Lines(lines.max(1) as usize),
            None if ellipsized => Self::FirstOverflow,
            None => Self::Lines(usize::MAX),
        }
    }

    fn is_reached(self, lines: usize, end: LineEnd) -> bool {
        match self {
            Self::Lines(limit) => lines >= limit,
            Self::FirstOverflow => end == LineEnd::Soft,
        }
    }

    /// `never_soft_wraps` is whether a soft break always ends the paragraph rather than
    /// starting a line, as on a single line or at an ellipsis without `max_lines`.
    fn never_soft_wraps(self) -> bool {
        matches!(self, Self::Lines(1) | Self::FirstOverflow)
    }
}

/// Accumulates skparagraph's intrinsic widths during the wrap walk: min =
/// the widest unbreakable segment, max = the widest hard-break line.
#[derive(Default)]
struct Intrinsics {
    min: f32,
    max: f32,
    line: f32,
    line_start: usize,
}

impl Intrinsics {
    fn segment(
        &mut self,
        text: &str,
        measure: &Measure,
        from: usize,
        to: usize,
        kind: unicode_linebreak::BreakOpportunity,
        preserve_trailing_whitespace: bool,
    ) {
        self.min = self.min.max(measured_width(
            text,
            measure,
            from..to,
            preserve_trailing_whitespace,
        ));
        if kind == unicode_linebreak::BreakOpportunity::Mandatory {
            self.line = measured_width(
                text,
                measure,
                self.line_start..to,
                preserve_trailing_whitespace,
            );
            self.max = self.max.max(self.line);
            self.line_start = to;
        }
    }

    /// `reported` is the (min, max) pair a paragraph under `limit` reports, as
    /// skparagraph's `ParagraphImpl::layout` settles the walk's: a paragraph that never
    /// soft-wraps (one line, or an ellipsis without `max_lines`) has its max as its min
    /// (Flutter's rule), and max is never below min.
    fn reported(&self, limit: LineLimit) -> (f32, f32) {
        let min = if limit.never_soft_wraps() {
            self.max
        } else {
            self.min
        };
        (min, self.max.max(min))
    }
}

/// Width of a byte range, trailing whitespace excluded — the number wrapping
/// and alignment both reason about.
fn measured_width(
    text: &str,
    measure: &Measure,
    range: Range<usize>,
    preserve_trailing_whitespace: bool,
) -> f32 {
    let content_end = content_end(text, &range, preserve_trailing_whitespace);
    measure.width(range.start, content_end)
}

fn content_end(text: &str, range: &Range<usize>, preserve_trailing_whitespace: bool) -> usize {
    if preserve_trailing_whitespace {
        range.end
    } else {
        trimmed_end(text, range)
    }
}

fn trimmed_end(text: &str, range: &Range<usize>) -> usize {
    text[range.clone()]
        .trim_end()
        .len()
        .saturating_add(range.start)
}

/// Turn wrapped ranges into positioned lines: per-line UAX #9 visual reorder,
/// then a left-to-right cursor over the visual runs, glyphs sliced
/// from the endless shaping by cluster membership. Justify stretches word
/// gaps; a truncated last line gets the style's ellipsis spliced in.
#[allow(clippy::too_many_arguments)] // the paragraph → layout seam, one call site
pub fn place_lines(
    collection: &FaceSet,
    text: &str,
    runs: &[ShapedRun],
    bidi: &BidiInfo,
    wrapped: Wrapped,
    max_width: f32,
    style: &ParagraphStyle,
    empty_metrics: Option<LineHeights>,
) -> Layout {
    let mut lines = Vec::new();
    let mut y = 0.0f32;
    let mut para_width = 0.0f32;
    // Lines with no glyphs (blank first line, trailing newline, empty
    // paragraph) take the paragraph style's metrics — skparagraph's
    // computeEmptyMetrics — not invented constants.
    let mut last_heights = empty_metrics.unwrap_or(LineHeights::FALLBACK);
    let count = wrapped.ranges.len();
    for (index, range) in wrapped.ranges.iter().cloned().enumerate() {
        let overlapping: Vec<&ShapedRun> = runs
            .iter()
            .filter(|r| r.range.start < range.end && r.range.end > range.start)
            .collect();
        let text_runs: Vec<&ShapedRun> = overlapping
            .iter()
            .filter(|run| run.placeholder.is_none())
            .copied()
            .collect();
        let text_heights = line_heights(collection, &text_runs);
        if let Some(h) = text_heights {
            last_heights = h;
        }
        let text_descent = text_heights.map_or(0.0, |heights| heights.descent);
        let cap_height = text_runs
            .iter()
            .map(|run| collection.get(run.font).cap_height_px(run.size))
            .fold(0.0, f32::max);
        let heights =
            fit_placeholders(text_heights, &overlapping, text_descent).unwrap_or(last_heights);
        let heights = style
            .text_height_behavior
            .applied(heights, index == 0, index + 1 == count);
        let baseline = y + heights.ascent;
        let ellipsis = ellipsis_for(
            collection,
            style,
            &overlapping,
            wrapped.truncated,
            index,
            count,
        );
        let line = LineSpec {
            range,
            baseline,
            text_descent,
            max_width,
            is_last: index + 1 == count,
            align: style.align,
            preserve_trailing_whitespace: style.preserve_trailing_whitespace,
        };
        let placed = place_line(collection, text, bidi, &overlapping, &line, ellipsis);
        para_width = para_width.max(placed.width);
        lines.push(Line {
            runs: placed.runs,
            baseline,
            ascent: heights.ascent,
            descent: heights.descent,
            cap_height,
            left: placed.left,
            width: placed.width,
            range: line.range,
        });
        y += heights.height();
    }
    Layout {
        max_width,
        width: para_width,
        height: y,
        truncated: wrapped.truncated,
        wrapped,
        lines,
    }
}

struct LineSpec {
    range: Range<usize>,
    baseline: f32,
    /// How far the line's text, without its placeholders, reaches below the baseline.
    text_descent: f32,
    max_width: f32,
    is_last: bool,
    align: TextAlign,
    preserve_trailing_whitespace: bool,
}

struct PlacedLine {
    runs: Vec<PlacedRun>,
    width: f32,
    left: f32,
}

/// The ellipsis run for a truncated final line, shaped in the line's
/// trailing style (font/size/color of its last shaped run).
fn ellipsis_for(
    collection: &FaceSet,
    style: &ParagraphStyle,
    overlapping: &[&ShapedRun],
    truncated: bool,
    index: usize,
    count: usize,
) -> Option<ShapedRun> {
    if !truncated || index + 1 != count {
        return None;
    }
    let text = style.ellipsis.as_deref()?;
    let tail = overlapping
        .iter()
        .rev()
        .find(|run| run.placeholder.is_none())?;
    Some(shape_ellipsis(collection, tail, text))
}

fn place_line(
    collection: &FaceSet,
    text: &str,
    bidi: &BidiInfo,
    runs: &[&ShapedRun],
    line: &LineSpec,
    ellipsis: Option<ShapedRun>,
) -> PlacedLine {
    let Some(para) = bidi
        .paragraphs
        .iter()
        .find(|p| p.range.contains(&line.range.start))
    else {
        return PlacedLine {
            runs: Vec::new(),
            width: 0.0,
            left: 0.0,
        };
    };
    let mut end = content_end(
        text,
        &(line.range.start..line.range.end.min(para.range.end)),
        line.preserve_trailing_whitespace,
    );
    let ellipsis_width: f32 = ellipsis
        .as_ref()
        .map(|e| e.glyphs.iter().map(|g| g.x_advance).sum())
        .unwrap_or(0.0);
    if ellipsis.is_some() {
        end = fit_for_ellipsis(runs, line, end, ellipsis_width);
    }
    let content = line.range.start..end;
    let width = advance_between_refs(runs, &content) + ellipsis_width;
    let extra_per_space = justify_extra(text, line, para, &content, width);
    let x0 = align_shift(line, width);
    let rtl_base = para.level.is_rtl();

    let mut x = x0;
    let mut placed = Vec::new();
    if rtl_base {
        if let Some(e) = &ellipsis {
            place_isolated(collection, e, line.baseline, end, &mut x, &mut placed);
        }
    }
    if !content.is_empty() {
        place_visual_runs(
            collection,
            text,
            bidi,
            runs,
            para,
            &content,
            (line.baseline, line.text_descent),
            extra_per_space,
            &mut x,
            &mut placed,
        );
    }
    if !rtl_base {
        if let Some(e) = &ellipsis {
            place_isolated(collection, e, line.baseline, end, &mut x, &mut placed);
        }
    }
    PlacedLine {
        runs: placed,
        width,
        left: x0,
    }
}

fn advance_between_refs(runs: &[&ShapedRun], range: &Range<usize>) -> f32 {
    runs.iter()
        .flat_map(|run| run.glyphs.iter())
        .filter(|g| g.cluster >= range.start && g.cluster < range.end)
        .map(|g| g.x_advance)
        .sum()
}

/// Drop clusters off the logical end until the ellipsis fits the width.
fn fit_for_ellipsis(
    runs: &[&ShapedRun],
    line: &LineSpec,
    end: usize,
    ellipsis_width: f32,
) -> usize {
    if !line.max_width.is_finite() {
        return end;
    }
    let budget = line.max_width - ellipsis_width;
    let mut clusters: Vec<(usize, f32)> = runs
        .iter()
        .flat_map(|run| run.glyphs.iter())
        .filter(|g| g.cluster >= line.range.start && g.cluster < end)
        .map(|g| (g.cluster, g.x_advance))
        .collect();
    clusters.sort_by_key(|&(cluster, _)| cluster);
    let mut total = 0.0;
    for (cluster, advance) in clusters {
        if total + advance > budget {
            return cluster;
        }
        total += advance;
    }
    end
}

/// CSS-style justify: stretch U+0020 gaps on every line except paragraph-
/// final ones (hard breaks and the overall last line stay ragged).
fn justify_extra(
    text: &str,
    line: &LineSpec,
    para: &unicode_bidi::ParagraphInfo,
    content: &Range<usize>,
    width: f32,
) -> f32 {
    if line.align != TextAlign::Justify || !line.max_width.is_finite() {
        return 0.0;
    }
    let para_final = line.is_last || line.range.end >= para.range.end;
    if para_final || width >= line.max_width {
        return 0.0;
    }
    let spaces = text[content.clone()].bytes().filter(|&b| b == b' ').count();
    if spaces == 0 {
        return 0.0;
    }
    (line.max_width - width) / spaces as f32
}

fn align_shift(line: &LineSpec, width: f32) -> f32 {
    if !line.max_width.is_finite() {
        return 0.0;
    }
    match line.align {
        TextAlign::Left | TextAlign::Justify => 0.0,
        TextAlign::Center => (line.max_width - width) * 0.5,
        TextAlign::Right => line.max_width - width,
    }
}

#[allow(clippy::too_many_arguments)] // the line-placement kernel, called once
fn place_visual_runs(
    collection: &FaceSet,
    text: &str,
    bidi: &BidiInfo,
    runs: &[&ShapedRun],
    para: &unicode_bidi::ParagraphInfo,
    content: &Range<usize>,
    (baseline, text_descent): (f32, f32),
    extra_per_space: f32,
    x: &mut f32,
    placed: &mut Vec<PlacedRun>,
) {
    let (levels, visual) = bidi.visual_runs(para, content.clone());
    for segment in visual {
        // Shaped runs (font splits) inside ONE visual segment: an RTL
        // segment lays its runs out in REVERSE logical order — the segment
        // is one right-to-left stretch, fonts don't change that.
        let mut overlapping: Vec<&&ShapedRun> = runs
            .iter()
            .filter(|run| run.range.start < segment.end && run.range.end > segment.start)
            .collect();
        let rtl = levels[segment.start].is_rtl();
        if rtl {
            overlapping.reverse();
        }
        for run in overlapping {
            let slice = Slice {
                range: segment.start.max(content.start)..segment.end.min(content.end),
                baseline,
                text_descent,
                extra_per_space,
                rtl,
            };
            if let Some(p) = place_slice(collection, text, run, &slice, x) {
                placed.push(p);
            }
        }
    }
}

struct Slice {
    range: Range<usize>,
    baseline: f32,
    text_descent: f32,
    extra_per_space: f32,
    rtl: bool,
}

/// One shaped run's glyphs inside a visual segment, laid out along the
/// cursor. Glyph order within the run is already visual (RTL shaped
/// backwards) — slicing preserves it.
fn place_slice(
    collection: &FaceSet,
    text: &str,
    run: &ShapedRun,
    slice: &Slice,
    x: &mut f32,
) -> Option<PlacedRun> {
    if let Some(placeholder) = &run.placeholder {
        return place_placeholder(run, placeholder, slice, x);
    }
    let font = collection.get(run.font);
    let start_x = *x;
    let mut last_origin = *x;
    let mut glyphs = Vec::new();
    for g in &run.glyphs {
        if g.cluster < slice.range.start || g.cluster >= slice.range.end {
            continue;
        }
        let mut advance = g.x_advance;
        if text.as_bytes().get(g.cluster) == Some(&b' ') {
            advance += slice.extra_per_space;
        }
        last_origin = *x;
        glyphs.push(PlacedGlyph {
            id: g.id,
            x: *x + g.x_offset,
            y: slice.baseline - g.y_offset,
            cluster: g.cluster,
            advance,
        });
        *x += advance;
    }
    if glyphs.is_empty() {
        return None;
    }
    let shown = run.range.start.max(slice.range.start)..run.range.end.min(slice.range.end);
    let bounds = valo_geometry::Rect::from_ltrb(
        start_x,
        slice.baseline - font.ascent_px(run.size),
        *x,
        slice.baseline + font.descent_px(run.size),
    );
    Some(PlacedRun {
        font: run.font,
        size: run.size,
        synthesis: run.synthesis,
        color: run.color,
        foreground: run.foreground.clone(),
        background: run.background.clone(),
        height: run.height,
        leading_distribution: run.leading_distribution,
        decoration: run.decoration,
        shadows: run.shadows.clone(),
        glyphs,
        text: Arc::from(&text[shown]),
        rtl: slice.rtl,
        bounds,
        ink: ink_bounds(font, run, start_x, last_origin, *x, slice.baseline),
        placeholder: None,
    })
}

/// A placeholder's box at the cursor, when its cluster is in the slice: one glyph for carets
/// and hit testing, and its box as both bounds and ink.
fn place_placeholder(
    run: &ShapedRun,
    placeholder: &ShapedPlaceholder,
    slice: &Slice,
    x: &mut f32,
) -> Option<PlacedRun> {
    if !slice.range.contains(&run.range.start) {
        return None;
    }
    let (ascent, descent) = placeholder.extent(slice.text_descent);
    let start_x = *x;
    *x += placeholder.style.width;
    let rect = valo_geometry::Rect::from_ltrb(
        start_x,
        slice.baseline - ascent,
        *x,
        slice.baseline + descent,
    );
    Some(PlacedRun {
        font: run.font,
        size: run.size,
        synthesis: run.synthesis,
        color: run.color,
        foreground: run.foreground.clone(),
        background: run.background.clone(),
        height: run.height,
        leading_distribution: run.leading_distribution,
        decoration: None,
        shadows: Vec::new(),
        glyphs: vec![PlacedGlyph {
            id: 0,
            x: start_x,
            y: slice.baseline,
            cluster: run.range.start,
            advance: placeholder.style.width,
        }],
        text: Arc::from(""),
        rtl: slice.rtl,
        bounds: rect,
        ink: rect,
        placeholder: Some(placeholder.index),
    })
}

/// The run's INK box, not its advance box: the font-wide extremes bound
/// bearings, italic overhang, and mark excursions (Skia's font bounds), grown
/// by what the run's synthesis adds to every glyph. A font without bounds
/// takes its ascent and descent over the advances. Under-reporting here
/// mis-culls and lets the opacity-elision proof call overlapping runs
/// "disjoint" (double-blend).
fn ink_bounds(
    font: &Font,
    run: &ShapedRun,
    start_x: f32,
    last_origin: f32,
    end_x: f32,
    baseline: f32,
) -> valo_geometry::Rect {
    let (ascent, descent) = (font.ascent_px(run.size), font.descent_px(run.size));
    let glyph_box = font
        .ink_box_px(run.size)
        .unwrap_or((0.0, -descent, 0.0, ascent));
    let (x_min, y_min, x_max, y_max) = run.synthesis.ink_box(glyph_box, run.size);
    valo_geometry::Rect::from_ltrb(
        start_x + x_min.min(0.0),
        baseline - ascent.max(y_max),
        end_x.max(last_origin + x_max),
        baseline + descent.max(-y_min),
    )
}

/// An isolated pre-shaped run (the ellipsis) dropped at the cursor.
/// `cluster` stamps every glyph with the TRUNCATION offset — hit-testing
/// the "…" then resolves where the cut happened, not paragraph start.
fn place_isolated(
    collection: &FaceSet,
    run: &ShapedRun,
    baseline: f32,
    cluster: usize,
    x: &mut f32,
    placed: &mut Vec<PlacedRun>,
) {
    let font = collection.get(run.font);
    let start_x = *x;
    let mut last_origin = *x;
    let mut glyphs = Vec::new();
    for g in &run.glyphs {
        last_origin = *x;
        glyphs.push(PlacedGlyph {
            id: g.id,
            x: *x + g.x_offset,
            y: baseline - g.y_offset,
            cluster,
            advance: g.x_advance,
        });
        *x += g.x_advance;
    }
    if glyphs.is_empty() {
        return;
    }
    placed.push(PlacedRun {
        font: run.font,
        size: run.size,
        synthesis: run.synthesis,
        color: run.color,
        foreground: run.foreground.clone(),
        background: run.background.clone(),
        height: run.height,
        leading_distribution: run.leading_distribution,
        decoration: run.decoration,
        shadows: run.shadows.clone(),
        glyphs,
        text: Arc::from(""),
        rtl: false,
        bounds: valo_geometry::Rect::from_ltrb(
            start_x,
            baseline - font.ascent_px(run.size),
            *x,
            baseline + font.descent_px(run.size),
        ),
        ink: ink_bounds(font, run, start_x, last_origin, *x, baseline),
        placeholder: None,
    });
}

/// A line's room grown to hold its placeholders: the text's, each side raised to the tallest
/// placeholder's, or the placeholders' alone on a line with no text, as Skia's line metrics add
/// placeholder runs to both their sets. `None` when the line has neither.
fn fit_placeholders(
    text: Option<LineHeights>,
    runs: &[&ShapedRun],
    text_descent: f32,
) -> Option<LineHeights> {
    runs.iter()
        .filter_map(|run| run.placeholder.as_ref())
        .map(|placeholder| {
            let (ascent, descent) = placeholder.extent(text_descent);
            LineHeights {
                ascent,
                descent,
                raw_ascent: ascent,
                raw_descent: descent,
            }
        })
        .chain(text)
        .reduce(LineHeights::max)
}

/// Max-over-runs line metrics (the strut-free rule).
fn line_heights(collection: &FaceSet, runs: &[&ShapedRun]) -> Option<LineHeights> {
    runs.iter()
        .map(|run| {
            let font = collection.get(run.font);
            style_heights(font, run.size, run.height, run.leading_distribution)
        })
        .reduce(LineHeights::max)
}

/// `LineHeights` is a line's room above and below its baseline, as its text asks for it
/// (`ascent`, `descent`) and as its fonts alone have it, at no height multiplier and with no
/// line gap (`raw_ascent`, `raw_descent`): skparagraph's `InternalLineMetrics`, its correct and
/// raw sets. A line is `ascent + descent` tall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LineHeights {
    pub ascent: f32,
    pub descent: f32,
    pub raw_ascent: f32,
    pub raw_descent: f32,
}

impl LineHeights {
    /// `FALLBACK` is a glyphless line's room when no font is there to measure: a 24 px line.
    const FALLBACK: LineHeights = LineHeights {
        ascent: 18.0,
        descent: 6.0,
        raw_ascent: 16.0,
        raw_descent: 4.0,
    };

    /// `height` is how tall the line is.
    pub fn height(&self) -> f32 {
        self.ascent + self.descent
    }

    /// `max` is room for both: each side the larger of the two, as a line takes its runs'.
    fn max(self, other: LineHeights) -> LineHeights {
        LineHeights {
            ascent: self.ascent.max(other.ascent),
            descent: self.descent.max(other.descent),
            raw_ascent: self.raw_ascent.max(other.raw_ascent),
            raw_descent: self.raw_descent.max(other.raw_descent),
        }
    }
}

/// One style's room above and below the baseline at `size`: skparagraph's
/// `Run::calculateMetrics`. The font's line gap is shared half above and half
/// below. A `height` multiplier makes the line `height × size` tall (its
/// heightOverride), sharing the difference from the font's own height as
/// `distribution` says: scaling the room above and below alike, or adding half
/// of it to each (its half leading, where the line gap cancels out). Also seeds
/// empty-line metrics from the paragraph style.
pub(crate) fn style_heights(
    font: &Font,
    size: f32,
    height: Option<f32>,
    distribution: LeadingDistribution,
) -> LineHeights {
    let (raw_ascent, raw_descent) = (font.ascent_px(size), font.descent_px(size));
    let half_gap = (font.line_height_px(size) - raw_ascent - raw_descent) / 2.0;
    let (ascent, descent) = (raw_ascent + half_gap, raw_descent + half_gap);
    let (ascent, descent) = match height.filter(|multiplier| multiplier.abs() > f32::EPSILON) {
        None => (ascent, descent),
        Some(multiplier) => {
            let target = multiplier * size;
            match distribution {
                LeadingDistribution::Proportional => {
                    let k = target / (ascent + descent).max(1e-3);
                    (ascent * k, descent * k)
                }
                LeadingDistribution::Even => {
                    let half_leading = (target - (ascent + descent)) / 2.0;
                    (ascent + half_leading, descent + half_leading)
                }
            }
        }
    };
    LineHeights {
        ascent,
        descent,
        raw_ascent,
        raw_descent,
    }
}

/// Hard-break characters (UAX #14 BK/CR/LF/NL) — a paragraph ending in one
/// opens a final empty line.
fn is_hard_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}
