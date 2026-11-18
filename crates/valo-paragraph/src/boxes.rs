//! Selection boxes: where a UTF-8 range of a laid-out paragraph is, in every
//! height and width style a host can ask for — skparagraph's
//! `ParagraphImpl::getRectsForRange` and `TextLine::getRectsForRange`.
//!
//! A box is one visual run's stretch of one line. Its height is the run's own
//! room, the line's, or the paragraph's strut, as the [`BoxHeightStyle`] says;
//! its width is the run's, or as wide as the paragraph's widest line for a
//! [`BoxWidthStyle::Max`] box. Runs whose boxes touch and sit on the same
//! vertical room join into one box, as skparagraph merges them, and every box
//! reports the direction of the text in it ([`TextBox`]).
//!
//! Skia rounds every box coordinate to a hundredth when a paragraph style asks
//! for its rounding hack (`applyRoundingHack`, `littleRound`). Flutter turns the
//! hack off, and Valo leaves coordinates exact, as it does everywhere else.

use std::ops::Range;

use valo_geometry::Rect;
use valo_text::FaceSet;

use crate::paragraph::{Layout, Line, PlacedRun};
use crate::strut::StrutBox;
use crate::style::{ParagraphStyle, TextDirection, TextHeightBehavior};
use crate::wrap::{style_heights, LineHeights};

/// `BoxHeightStyle` is how a selection box takes its height: Flutter's
/// `BoxHeightStyle`, Skia's `RectHeightStyle`.
///
/// The line-spacing styles give every box a line's whole room, where `Tight`
/// gives it the room of the run it covers: a host painting a selection over
/// lines at a height multiplier picks one of them, and a text field's caret
/// wants [`Self::Strut`] when the paragraph has a strut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxHeightStyle {
    /// `Tight` fits each run's box to its font's ascent and descent about the
    /// line's baseline: a run with a height multiplier takes its font's own
    /// room, not the multiplier's, as skparagraph's `kTight` case does.
    Tight,
    /// `Max` gives every box on a line the line's whole height, rounded to a
    /// whole pixel as the line is.
    Max,
    /// `IncludeLineSpacingMiddle` gives every box its line's whole height moved
    /// down by half the room a height multiplier or strut added above the
    /// line's text, so the room between two lines' text is shared between their
    /// boxes, which touch. The first line's box starts where its fonts' own
    /// ascent does, and the last line's ends at the line's bottom.
    IncludeLineSpacingMiddle,
    /// `IncludeLineSpacingTop` gives every box its line's whole height, the room
    /// a height multiplier or strut added above the line's text included,
    /// except on the first line, whose box starts where its fonts' own ascent
    /// does.
    IncludeLineSpacingTop,
    /// `IncludeLineSpacingBottom` gives every box its line's whole height moved
    /// down by the room a height multiplier or strut added above the line's
    /// text, so a box takes the room above the next line's text. The first
    /// line's box starts where its fonts' own ascent does, and the last line's
    /// ends at the line's bottom.
    IncludeLineSpacingBottom,
    /// `Strut` gives every box the paragraph's strut, its leading left out:
    /// the line a text field's caret keeps whatever the text's own fonts are.
    Strut,
}

/// `BoxWidthStyle` is how wide a selection box is: Flutter's `BoxWidthStyle`,
/// Skia's `RectWidthStyle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxWidthStyle {
    /// `Tight` fits each box to the run it covers.
    Tight,
    /// `Max` extends a line's last box to the paragraph's widest line, trailing
    /// whitespace included, so a selection covers the line it highlights. It
    /// leaves the paragraph's last line alone, as skparagraph does.
    Max,
}

/// `TextBox` is a selection rectangle and the direction of the text in it:
/// Flutter's `TextBox`, skparagraph's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextBox {
    /// `rect` is the box in paragraph coordinates.
    pub rect: Rect,
    /// `direction` is the direction of the text the box covers; a line's
    /// trailing whitespace takes the paragraph's direction, as Skia gives it
    /// the paragraph's.
    pub direction: TextDirection,
}

/// `Selection` is a laid-out paragraph as one selection query sees it: the
/// lines and their metrics, the faces the runs' fonts are measured from, and
/// the strut the strut height style draws.
pub(crate) struct Selection<'a> {
    /// `faces` are the paragraph's fonts, which a run's room is measured from.
    pub faces: &'a FaceSet,
    /// `layout` holds the lines, their metrics and the widths the styles use.
    pub layout: &'a Layout,
    /// `strut` is the paragraph's strut box, when it has one to show.
    pub strut: Option<StrutBox>,
    /// `text` is the paragraph's text, whose emptiness has a box of its own.
    pub text: &'a str,
    /// `style` is the paragraph's style, whose height behavior decides which
    /// lines take their runs' typographic room.
    pub style: &'a ParagraphStyle,
}

impl Selection<'_> {
    /// `boxes` is every box `range` covers, in `height_style` and
    /// `width_style`: skparagraph's `ParagraphImpl::getRectsForRange`.
    ///
    /// An empty range, a range past the text and an unlaid-out paragraph have
    /// no boxes. An empty paragraph has one zero-width box of its whole height
    /// for a range that starts at zero, which is where a caret sits in it:
    /// skparagraph's implied last line break.
    pub(crate) fn boxes(
        &self,
        range: Range<usize>,
        height_style: BoxHeightStyle,
        width_style: BoxWidthStyle,
    ) -> Vec<TextBox> {
        let mut boxes = Vec::new();
        if self.layout.lines.is_empty() {
            return boxes;
        }
        if self.text.is_empty() {
            if range.start == 0 && range.end > 0 {
                boxes.push(TextBox {
                    rect: Rect::new(0.0, 0.0, 0.0, self.layout.height),
                    direction: self.layout.direction,
                });
            }
            return boxes;
        }
        if range.start >= range.end || range.start > self.text.len() || range.end == 0 {
            return boxes;
        }
        let count = self.layout.lines.len();
        for (index, line) in self.layout.lines.iter().enumerate() {
            if range.end <= line.range.start || range.start >= line.range.end {
                continue;
            }
            let start = boxes.len();
            let edges = LineEdges::of(
                line,
                &self.layout.heights[index],
                self.style.text_height_behavior,
                index == 0,
                index + 1 == count,
            );
            self.line_boxes(line, &edges, &range, height_style, &mut boxes);
            if width_style == BoxWidthStyle::Max && index + 1 < count {
                self.extend_to_widest(line, start, &mut boxes);
            }
        }
        boxes
    }

    /// `line_boxes` is one line's boxes, in visual order, the touching ones
    /// merged.
    fn line_boxes(
        &self,
        line: &Line,
        edges: &LineEdges,
        range: &Range<usize>,
        height_style: BoxHeightStyle,
        boxes: &mut Vec<TextBox>,
    ) {
        let mut previous: Option<&PlacedRun> = None;
        for run in &line.runs {
            let Some((left, right)) = run_extent(run, range) else {
                continue;
            };
            let (top, bottom) = self.run_edges(line, edges, run, height_style);
            let direction = self.box_direction(line, run);
            let rect = Rect::from_ltrb(left, top, right, bottom);
            match (previous, boxes.last_mut()) {
                (Some(before), Some(last)) if joins(before, run, last, &rect, direction) => {
                    *last = TextBox {
                        rect: joined(last.rect, rect),
                        direction,
                    };
                }
                _ => boxes.push(TextBox { rect, direction }),
            }
            previous = Some(run);
        }
    }

    /// `run_edges` is where a run's box starts and ends down the paragraph:
    /// skparagraph's height styles, each measured on the line and the run it
    /// covers.
    fn run_edges(
        &self,
        line: &Line,
        edges: &LineEdges,
        run: &PlacedRun,
        style: BoxHeightStyle,
    ) -> (f32, f32) {
        let (first, last, shift) = (edges.first, edges.last, edges.shift);
        match style {
            BoxHeightStyle::Tight => self.tight_edges(line, edges, run),
            BoxHeightStyle::Max => (edges.top + edges.delta, edges.top + edges.height),
            BoxHeightStyle::IncludeLineSpacingTop => (
                edges.top + edges.delta + if first { shift } else { 0.0 },
                edges.top + edges.height,
            ),
            BoxHeightStyle::IncludeLineSpacingMiddle => (
                edges.top + edges.delta + shift / 2.0 + if first { shift / 2.0 } else { 0.0 },
                edges.top + edges.height + shift / 2.0 - if last { shift / 2.0 } else { 0.0 },
            ),
            BoxHeightStyle::IncludeLineSpacingBottom => (
                edges.top + edges.delta + shift,
                edges.top + edges.height + shift - if last { shift } else { 0.0 },
            ),
            BoxHeightStyle::Strut => match self.strut {
                Some(strut) => (line.baseline - strut.ascent, line.baseline + strut.descent),
                // A strut without a size, or none at all, leaves the measured
                // run box, as skparagraph's case falls through.
                None => self.measured_edges(line, edges, run),
            },
        }
    }

    /// `tight_edges` is the room of the run a box covers: a run with a height
    /// multiplier takes its font's own ascent and descent around the line's
    /// baseline, as skparagraph's `kTight` case does, and any other run takes
    /// the room it was measured with.
    fn tight_edges(&self, line: &Line, edges: &LineEdges, run: &PlacedRun) -> (f32, f32) {
        if run.placeholder.is_some() {
            return self.measured_edges(line, edges, run);
        }
        if run.height.is_some_and(|height| height > 0.0) {
            let font = self.faces.get(run.font);
            let baseline = line.baseline + edges.delta;
            (
                baseline - font.ascent_px(run.size),
                baseline + font.descent_px(run.size),
            )
        } else {
            self.measured_edges(line, edges, run)
        }
    }

    /// `measured_edges` is the room a run was measured with, its style's height
    /// multiplier included: skparagraph's `ClipContext` before a height style
    /// overrides it.
    fn measured_edges(&self, line: &Line, edges: &LineEdges, run: &PlacedRun) -> (f32, f32) {
        if run.placeholder.is_some() {
            // A placeholder was measured as its own box against the line's
            // baseline, which is the room skparagraph's placeholder runs carry.
            return (
                run.bounds.y + edges.delta,
                run.bounds.bottom() + edges.delta,
            );
        }
        let font = self.faces.get(run.font);
        let heights = style_heights(font, run.size, run.height, run.leading_distribution);
        // A line the paragraph's height behavior trims takes its runs'
        // typographic room on that side instead of their styled room
        // (`LineMetricStyle`): the fonts' own, a height multiplier's left out.
        let ascent = if edges.typographic_ascent {
            font.ascent_px(run.size)
        } else {
            heights.ascent
        };
        let descent = if edges.typographic_descent {
            font.descent_px(run.size)
        } else {
            heights.descent
        };
        let top = line.baseline - ascent + edges.delta;
        (top, top + ascent + descent)
    }

    /// `box_direction` is the direction of the run a box covers, or the
    /// paragraph's direction for a line's trailing whitespace, which
    /// skparagraph splits off and reports in the paragraph's direction.
    fn box_direction(&self, line: &Line, run: &PlacedRun) -> TextDirection {
        if line.is_ghost(run) {
            self.layout.direction
        } else if run.rtl {
            TextDirection::Rtl
        } else {
            TextDirection::Ltr
        }
    }

    /// `extend_to_widest` widens a line's boxes to the paragraph's widest line:
    /// skparagraph's `RectWidthStyle::kMax`, which also covers an
    /// alignment-shifted right-to-left line's left edge with a box of its own.
    fn extend_to_widest(&self, line: &Line, start: usize, boxes: &mut Vec<TextBox>) {
        let Some(first) = boxes.get(start).copied() else {
            return;
        };
        let line_end = line.left + line.width;
        if first.rect.x > line.left && first.direction == TextDirection::Rtl {
            boxes.insert(
                start + 1,
                TextBox {
                    rect: Rect::from_ltrb(0.0, first.rect.y, first.rect.x, first.rect.bottom()),
                    direction: TextDirection::Rtl,
                },
            );
        }
        let widest = self.layout.width_with_trailing_spaces;
        let Some(last) = boxes.last().copied() else {
            return;
        };
        if last.direction == TextDirection::Ltr
            && last.rect.right() >= line_end
            && last.rect.right() < widest
        {
            boxes.push(TextBox {
                rect: Rect::from_ltrb(last.rect.right(), last.rect.y, widest, last.rect.bottom()),
                direction: TextDirection::Ltr,
            });
        }
    }
}

/// `LineEdges` is the vertical scale one line's boxes are measured on, as
/// skparagraph's `TextLine` keeps it: the line's top and whole height, the
/// rounding of that height (`delta`), and how far a height multiplier or a
/// strut raised the line above its fonts (`shift`), and whether it is the
/// paragraph's first or last line, whose boxes the line-spacing styles trim.
struct LineEdges {
    top: f32,
    height: f32,
    delta: f32,
    shift: f32,
    first: bool,
    last: bool,
    /// `typographic_ascent` and `typographic_descent` are whether this line
    /// takes its runs' fonts' own room on that side rather than their styled
    /// room, as skparagraph gives the first line its raw ascent and the last
    /// its raw descent where the paragraph's height behavior trims them.
    typographic_ascent: bool,
    typographic_descent: bool,
}

impl LineEdges {
    fn of(
        line: &Line,
        heights: &LineHeights,
        behavior: TextHeightBehavior,
        first: bool,
        last: bool,
    ) -> Self {
        let whole = line.ascent + line.descent;
        Self {
            top: line.baseline - line.ascent,
            height: heights.height(),
            delta: heights.height() - whole,
            shift: line.ascent - heights.leading / 2.0 - heights.raw_ascent,
            first,
            last,
            typographic_ascent: first && !behavior.apply_height_to_first_ascent,
            typographic_descent: last && !behavior.apply_height_to_last_descent,
        }
    }
}

/// `run_extent` is the horizontal stretch of `run`'s glyphs whose clusters
/// start in `range`, or `None` when it has none there.
fn run_extent(run: &PlacedRun, range: &Range<usize>) -> Option<(f32, f32)> {
    let mut cells = run
        .glyphs
        .iter()
        .filter(|glyph| glyph.cluster >= range.start && glyph.cluster < range.end);
    let first = cells.next()?;
    let (left, right) = cells.fold(
        (first.x, first.x + first.advance),
        |(left, right), glyph| (left.min(glyph.x), right.max(glyph.x + glyph.advance)),
    );
    Some((left, right))
}

/// `joins` reports whether `run`'s box continues `last`, the box of `before`,
/// the run left of it: skparagraph merges the boxes of runs in one direction,
/// font and height multiplier whose vertical edges agree and which touch.
fn joins(
    before: &PlacedRun,
    run: &PlacedRun,
    last: &TextBox,
    rect: &Rect,
    direction: TextDirection,
) -> bool {
    let touching =
        (last.rect.right() - rect.x).abs() < 1e-3 || (rect.right() - last.rect.x).abs() < 1e-3;
    before.placeholder.is_none()
        && run.placeholder.is_none()
        && last.direction == direction
        && before.font == run.font
        && before.size == run.size
        && before.height == run.height
        && (last.rect.y - rect.y).abs() < 1e-3
        && (last.rect.bottom() - rect.bottom()).abs() < 1e-3
        && touching
}

/// `joined` is `rect` and `last` as one box: the touching boxes share their
/// vertical edges, so only the horizontal ones move.
fn joined(last: Rect, rect: Rect) -> Rect {
    Rect::from_ltrb(
        last.x.min(rect.x),
        last.y,
        last.right().max(rect.right()),
        last.bottom(),
    )
}
