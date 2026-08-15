use std::ops::Range;

use unicode_bidi::BidiInfo;

use crate::font::FaceSet;
use crate::paragraph::{Layout, Line, PlacedGlyph, PlacedRun};
use crate::shape::{shape_isolated, ShapedRun};
use crate::style::{ParagraphStyle, TextAlign};

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
/// Min/max intrinsic widths ride the same walk.
pub fn wrap_lines(
    text: &str,
    runs: &[ShapedRun],
    max_width: f32,
    max_lines: Option<u32>,
    preserve_trailing_whitespace: bool,
) -> Wrapped {
    let measure = Measure::new(runs);
    let cap = max_lines.map_or(usize::MAX, |n| n.max(1) as usize);
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut committed = 0usize; // last opportunity that fit
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
        if ranges.len() >= cap {
            return Wrapped {
                ranges,
                truncated: true,
                min_intrinsic: intrinsics.min,
                max_intrinsic: intrinsics.max,
            };
        }
        if measured_width(text, &measure, start..at, preserve_trailing_whitespace) > max_width
            && committed > start
        {
            ranges.push(start..committed);
            start = committed;
        }
        committed = at;
        // Re-check the cap: the overflow push above may have just filled it
        // (one iteration can otherwise push TWICE and overshoot max_lines).
        if kind == unicode_linebreak::BreakOpportunity::Mandatory && ranges.len() < cap {
            ranges.push(start..at);
            start = at;
        }
    }
    if start < text.len() && ranges.len() < cap {
        ranges.push(start..text.len());
    }
    // A trailing hard break opens one more, EMPTY line (skparagraph emits
    // it with fEmptyMetrics) — the caret after "hi\n" lives there.
    if ranges.len() < cap
        && text.chars().last().is_some_and(is_hard_break)
        && ranges.last().is_some_and(|r| r.end == text.len())
    {
        ranges.push(text.len()..text.len());
    }
    // An empty paragraph still has ONE line — a zero-height caret is useless.
    if ranges.is_empty() {
        ranges.push(0..0);
    }
    Wrapped {
        truncated: start < text.len()
            && ranges.len() >= cap
            && ranges.last().is_none_or(|l| l.end < text.len()),
        ranges,
        min_intrinsic: intrinsics.min,
        max_intrinsic: intrinsics.max,
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
    empty_metrics: Option<(f32, f32, f32)>,
) -> Layout {
    let mut lines = Vec::new();
    let mut y = 0.0f32;
    let mut para_width = 0.0f32;
    // Lines with no glyphs (blank first line, trailing newline, empty
    // paragraph) take the paragraph style's metrics — skparagraph's
    // computeEmptyMetrics — not invented constants.
    let mut last_heights = empty_metrics.unwrap_or((16.0f32, 4.0, 24.0));
    let count = wrapped.ranges.len();
    for (index, range) in wrapped.ranges.iter().cloned().enumerate() {
        let overlapping: Vec<&ShapedRun> = runs
            .iter()
            .filter(|r| r.range.start < range.end && r.range.end > range.start)
            .collect();
        if let Some(h) = line_heights(collection, &overlapping) {
            last_heights = h;
        }
        let (ascent, _descent, height) = last_heights;
        let baseline = y + ascent;
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
            ascent,
            descent: _descent,
            left: placed.left,
            width: placed.width,
            range: line.range,
        });
        y += height;
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
    let tail = overlapping.last()?;
    Some(shape_isolated(
        collection, tail.font, tail.size, tail.color, text,
    ))
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
            line.baseline,
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
    baseline: f32,
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
    let bounds = valo_geometry::Rect::from_ltrb(
        start_x,
        slice.baseline - font.ascent_px(run.size),
        *x,
        slice.baseline + font.descent_px(run.size),
    );
    Some(PlacedRun {
        font: run.font,
        size: run.size,
        color: run.color,
        decoration: run.decoration,
        shadows: run.shadows.clone(),
        glyphs,
        rtl: slice.rtl,
        bounds,
        ink: ink_bounds(font, run.size, start_x, last_origin, *x, slice.baseline),
    })
}

/// The run's INK box, not its advance box: the font-wide extremes bound
/// bearings, italic overhang, and mark excursions (Skia's font bounds).
/// Under-reporting here mis-culls and lets the opacity-elision proof call
/// overlapping runs "disjoint" (double-blend).
fn ink_bounds(
    font: &crate::font::Font,
    size: f32,
    start_x: f32,
    last_origin: f32,
    end_x: f32,
    baseline: f32,
) -> valo_geometry::Rect {
    let (ascent, descent) = (font.ascent_px(size), font.descent_px(size));
    match font.ink_box_px(size) {
        Some((x_min, y_min, x_max, y_max)) => valo_geometry::Rect::from_ltrb(
            start_x + x_min.min(0.0),
            baseline - ascent.max(y_max),
            end_x.max(last_origin + x_max),
            baseline + descent.max(-y_min),
        ),
        None => {
            valo_geometry::Rect::from_ltrb(start_x, baseline - ascent, end_x, baseline + descent)
        }
    }
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
        color: run.color,
        decoration: run.decoration,
        shadows: run.shadows.clone(),
        glyphs,
        rtl: false,
        bounds: valo_geometry::Rect::from_ltrb(
            start_x,
            baseline - font.ascent_px(run.size),
            *x,
            baseline + font.descent_px(run.size),
        ),
        ink: ink_bounds(font, run.size, start_x, last_origin, *x, baseline),
    });
}

/// Max-over-runs line metrics (the strut-free rule).
fn line_heights(collection: &FaceSet, runs: &[&ShapedRun]) -> Option<(f32, f32, f32)> {
    let mut out: Option<(f32, f32, f32)> = None;
    for run in runs {
        let font = collection.get(run.font);
        let candidate = style_heights(font, run.size, run.height);
        out = Some(match out {
            None => candidate,
            Some(cur) => (
                cur.0.max(candidate.0),
                cur.1.max(candidate.1),
                cur.2.max(candidate.2),
            ),
        });
    }
    out
}

/// One style's (ascent, descent, height) at `size`. A `height` multiplier
/// rescales to `height × size`, ascent/descent proportional (skparagraph's
/// heightOverride). Also seeds empty-line metrics from the paragraph style.
pub(crate) fn style_heights(
    font: &crate::font::Font,
    size: f32,
    height: Option<f32>,
) -> (f32, f32, f32) {
    let mut candidate = (
        font.ascent_px(size),
        font.descent_px(size),
        font.line_height_px(size),
    );
    if let Some(multiplier) = height {
        let target = multiplier * size;
        let k = target / (candidate.0 + candidate.1).max(1e-3);
        candidate = (candidate.0 * k, candidate.1 * k, target);
    }
    candidate
}

/// Hard-break characters (UAX #14 BK/CR/LF/NL) — a paragraph ending in one
/// opens a final empty line.
fn is_hard_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}
