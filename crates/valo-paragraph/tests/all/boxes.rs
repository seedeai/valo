//! Selection boxes in every height and width style, and the parts of clusters
//! and graphemes they name: skparagraph's `ParagraphImpl::getRectsForRange` and
//! `TextLine::getRectsForRange`, ported from `SkParagraphTest`'s
//! `GetRectsForRangeParagraph`, `GetRectsForRangeTight`,
//! `GetRectsForRangeIncludeLineSpacingMiddle`/`Top`/`Bottom`,
//! `GetRectsForRangeIncludeCombiningCharacter`, `EmojiMultiLineRectsParagraph`,
//! `GetRectsForRangeStrut` and `BasicHalfLeading`.
//!
//! Skia's tests draw in Roboto and Noto Sans CJK, which are not Valo's test
//! fonts. These draw in Fira Sans — ascent and descent 0.935 and 0.265 of its
//! size, no line gap — and state each number against the shaped advances and
//! font metrics the paragraph reports rather than Roboto's. Skia's `Tight` and
//! line-spacing cases draw a line of kaomoji, "(　´･‿･｀)" repeated, in Roboto
//! with a CJK fallback; Fira Sans covers none of its wide characters, so those
//! ports keep Skia's height styles, width styles and ranges but draw
//! `GetRectsForRangeParagraph`'s text, and their box counts are this text's,
//! not Skia's.

use std::ops::Range;

use valo_geometry::Color;
use valo_paragraph::{
    BoxHeightStyle, BoxWidthStyle, Line, Paragraph, ParagraphBuilder, ParagraphStyle, StrutStyle,
    TextBox, TextDirection, TextStyle,
};
use valo_text::{Font, FontCollection};

use crate::asset;

fn fonts() -> FontCollection {
    let mut collection = FontCollection::new();
    collection
        .register("Fira Sans", asset("fira_sans.ttf"))
        .unwrap();
    collection
}

fn style(size: f32) -> TextStyle {
    TextStyle::new("Fira Sans", size, Color::BLACK)
}

/// `laid_out` is `text` at `width`, in one style and a paragraph style of its
/// own: `f32::INFINITY` disables soft wrapping.
fn laid_out(
    collection: &mut FontCollection,
    text: &str,
    text_style: &TextStyle,
    paragraph_style: ParagraphStyle,
    width: f32,
) -> Paragraph {
    let mut builder = ParagraphBuilder::new(collection);
    builder.style(paragraph_style);
    builder.add_text(text, text_style);
    let mut paragraph = builder.build();
    paragraph.layout(width);
    paragraph
}

/// `laid_out_at` is `text` in Fira Sans, wrapped at `width`.
fn laid_out_at(text: &str, text_style: &TextStyle, width: f32) -> Paragraph {
    laid_out(
        &mut fonts(),
        text,
        text_style,
        ParagraphStyle::default(),
        width,
    )
}

/// `laid_out_in` is `text` in `collection`, unwrapped.
fn laid_out_in(
    collection: &mut FontCollection,
    text: &str,
    text_style: &TextStyle,
    paragraph_style: ParagraphStyle,
) -> Paragraph {
    laid_out(collection, text, text_style, paragraph_style, f32::INFINITY)
}

/// `laid_out_fira` is `text` in Fira Sans, unwrapped.
fn laid_out_fira(text: &str, text_style: &TextStyle, paragraph_style: ParagraphStyle) -> Paragraph {
    laid_out_in(&mut fonts(), text, text_style, paragraph_style)
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

fn assert_near(actual: f32, expected: f32) {
    assert!(near(actual, expected), "{actual} is not {expected}");
}

fn boxes(paragraph: &Paragraph, style: BoxHeightStyle) -> Vec<TextBox> {
    paragraph.boxes_for_range(0..paragraph.text().len(), style, BoxWidthStyle::Tight)
}

fn font(paragraph: &Paragraph) -> &Font {
    paragraph.primary_font().expect("the text has a font").0
}

/// The room a line's own text asks for, and the rounding of it that Skia's
/// line boxes start at (`InternalLineMetrics::delta`).
fn line_rounding(paragraph: &Paragraph, line: usize) -> (f32, f32) {
    let line = &paragraph.lines()[line];
    let whole = line.ascent + line.descent;
    let rounded = (f64::from(line.ascent) + f64::from(line.descent)).round() as f32;
    (whole, rounded - whole)
}

/// `glyph_span` is the horizontal stretch of the glyphs of the cluster that
/// starts at `cluster`: a ligature's cluster is one glyph, a cluster of marks
/// several.
fn glyph_span(paragraph: &Paragraph, cluster: usize) -> (f32, f32) {
    let mut left = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    for glyph in paragraph
        .lines()
        .iter()
        .flat_map(|line| line.runs.iter())
        .flat_map(|run| run.glyphs.iter())
        .filter(|glyph| glyph.cluster == cluster)
    {
        left = left.min(glyph.x);
        right = right.max(glyph.x + glyph.advance);
    }
    assert!(left.is_finite(), "cluster {cluster} is not laid out");
    (left, right)
}

/// The advance of a line's trailing whitespace, its ghosts.
fn trailing_advance(line: &Line) -> f32 {
    line.runs
        .iter()
        .filter(|run| {
            run.glyphs
                .first()
                .is_some_and(|glyph| glyph.cluster >= line.content_end)
        })
        .flat_map(|run| run.glyphs.iter())
        .map(|glyph| glyph.advance)
        .sum()
}

/// `widest_with_spaces` is the paragraph's widest line including its trailing
/// whitespace: the width a `Max` width box reaches (skparagraph's
/// `widthWithTrailingSpaces`).
fn widest_with_spaces(paragraph: &Paragraph) -> f32 {
    paragraph
        .lines()
        .iter()
        .map(|line| line.width + trailing_advance(line))
        .fold(0.0, f32::max)
}

/// `WRAPPED_TEXT` is `SkParagraph_GetRectsForRangeParagraph`'s text, which
/// wraps onto four lines of 50 px Fira Sans at the 550 px Skia lays out at.
const WRAPPED_TEXT: &str =
    "12345,  \"67890\" 12345 67890 12345 67890 12345 67890 12345 67890 12345 67890 12345";

/// `wrapped` is `WRAPPED_TEXT` in `text_style` at 550 px.
fn wrapped(text_style: &TextStyle) -> Paragraph {
    laid_out_at(WRAPPED_TEXT, text_style, 550.0)
}

/// A range that falls inside an "fi" ligature takes the part of the ligature
/// its bytes name, as skparagraph measures a cluster in part by the share of
/// its text the range leaves out (`Cluster::sizeToChar`, `sizeFromChar`).
///
/// Fira Sans shapes "office"'s "fi" as one glyph of 11.5 px at 18.38, covering
/// bytes 2..4: the ligature's left half is bytes 2..3, its right half 3..4.
/// Selecting half of it used to give the whole ligature, or nothing.
#[test]
fn a_range_inside_a_ligature_takes_the_part_of_it_it_names() {
    let paragraph = laid_out_fira("office", &style(20.0), ParagraphStyle::default());
    let (left, right) = glyph_span(&paragraph, 2);
    let half = (right - left) / 2.0;

    let left_half = paragraph.rects_for_range(2..3);
    assert_eq!(left_half.len(), 1, "{left_half:?}");
    assert_near(left_half[0].x, left);
    assert_near(left_half[0].right(), left + half);

    let right_half = paragraph.rects_for_range(3..4);
    assert_eq!(right_half.len(), 1, "the range is inside the ligature");
    assert_near(right_half[0].x, left + half);
    assert_near(right_half[0].right(), right);

    let whole = paragraph.rects_for_range(2..4);
    assert_eq!(whole.len(), 1);
    assert_near(whole[0].x, left);
    assert_near(whole[0].right(), right);

    // The "i" with the letter after it takes the ligature's right half and
    // that letter whole.
    let (next_left, next_right) = glyph_span(&paragraph, 4);
    let across = paragraph.rects_for_range(3..5);
    assert_eq!(across.len(), 1);
    assert_near(across[0].x, left + half);
    assert_near(across[0].right(), next_right);
    assert!(next_left >= right);

    // The letter after the ligature alone starts at its right edge.
    let after = paragraph.rects_for_range(4..5);
    assert_eq!(after.len(), 1);
    assert_near(after[0].x, right);
    assert_near(after[0].right(), next_right);

    // The whole word is unchanged.
    let word = paragraph.rects_for_range(0..6);
    assert_eq!(word.len(), 1);
    assert_near(word[0].x, 0.0);
    assert_near(word[0].right(), glyph_span(&paragraph, 5).1);
}

/// A selection takes whole graphemes, as `ParagraphImpl::getRectsForRange`
/// snaps a range's start forward and its end back to grapheme boundaries. "e"
/// and a combining acute are one grapheme, so a range over either half of it
/// selects nothing and only the grapheme whole has a box.
///
/// The shapes are `SkParagraph_GetRectsForRangeIncludeCombiningCharacter`'s,
/// drawn in Latin since no Valo test font covers its Thai. That test is
/// disabled in Skia, and it expects what the code does not give: a box for a
/// mark alone, the whole grapheme's. These follow the code.
#[test]
fn a_selection_takes_whole_graphemes() {
    let paragraph = laid_out_fira("e\u{0301}x", &style(20.0), ParagraphStyle::default());
    assert!(paragraph.rects_for_range(0..0).is_empty());
    let base = paragraph.rects_for_range(0..1);
    let mark = paragraph.rects_for_range(1..3);
    let whole = paragraph.rects_for_range(0..3);
    assert!(base.is_empty(), "half a grapheme has no box: {base:?}");
    assert!(mark.is_empty(), "the other half has none either: {mark:?}");
    assert_eq!(whole.len(), 1);
    assert_near(whole[0].x, 0.0);
    assert_near(whole[0].right(), glyph_span(&paragraph, 0).1);
    // The grapheme and the letter after it.
    assert_eq!(paragraph.rects_for_range(0..4).len(), 1);

    // A grapheme of a base and two marks, as Skia's third case is a run of
    // three: the last mark alone selects nothing, the whole selects the box.
    let stacked = laid_out_fira(
        "x\u{0301}\u{0302}y",
        &style(20.0),
        ParagraphStyle::default(),
    );
    assert!(stacked.rects_for_range(1..3).is_empty());
    assert!(stacked.rects_for_range(3..5).is_empty());
    assert_eq!(stacked.rects_for_range(0..5).len(), 1);
    assert_eq!(stacked.rects_for_range(5..6).len(), 1);
    assert_eq!(stacked.rects_for_range(0..6).len(), 1);
}

/// A right-to-left run's characters are boxes of their own, placed from the
/// paragraph's right: each of "لا"'s two characters selects its own box, and a
/// range inside one of them selects none of it, as the grapheme rule says.
#[test]
fn right_to_left_characters_select_whole() {
    let mut collection = FontCollection::new();
    collection
        .register("Noto Sans Arabic", asset("noto_sans_arabic.ttf"))
        .unwrap();
    let paragraph = laid_out(
        &mut collection,
        "لا",
        &TextStyle::new("Noto Sans Arabic", 40.0, Color::BLACK),
        ParagraphStyle {
            direction: Some(TextDirection::Rtl),
            ..ParagraphStyle::default()
        },
        f32::INFINITY,
    );
    let (left, right) = glyph_span(&paragraph, 0);
    let (first_left, first_right) = glyph_span(&paragraph, 2);
    assert!(first_right <= left, "the first character is the right one");

    let first = paragraph.rects_for_range(0..2);
    assert_eq!(first.len(), 1);
    assert_near(first[0].x, left);
    assert_near(first[0].right(), right);
    let directed = paragraph.boxes_for_range(0..2, BoxHeightStyle::Max, BoxWidthStyle::Tight);
    assert_eq!(directed[0].direction, TextDirection::Rtl);
    let second = paragraph.rects_for_range(2..4);
    assert_eq!(second.len(), 1);
    assert_near(second[0].x, first_left);
    assert_near(second[0].right(), first_right);

    assert!(paragraph.rects_for_range(0..1).is_empty());
    assert!(paragraph.rects_for_range(1..2).is_empty());
    assert!(paragraph.rects_for_range(3..4).is_empty());
}

/// Skparagraph's `SkParagraph_GetRectsForRangeParagraph`: a `Max` height box is
/// its line's whole height, a range over wrapped text gets one box per line it
/// crosses, a line's trailing space is part of the box of text before it, and a
/// range of no length has none.
///
/// Skia's numbers are Roboto's: a 0.40625 px rounding offset and 58.59 px lines
/// at 50 px. Fira Sans is 60 px at 50 with no rounding offset.
#[test]
fn wrapped_ranges_get_a_box_per_line_in_the_line_height() {
    let paragraph = wrapped(&style(50.0));
    assert_eq!(paragraph.lines().len(), 4);
    let height = 60.0;
    let box_for = |range: Range<usize>| {
        paragraph.boxes_for_range(range, BoxHeightStyle::Max, BoxWidthStyle::Tight)
    };

    assert!(box_for(0..0).is_empty());

    let first = box_for(0..1);
    assert_eq!(first.len(), 1);
    assert_near(first[0].rect.x, 0.0);
    assert_near(first[0].rect.right(), glyph_span(&paragraph, 0).1);
    assert_near(first[0].rect.y, 0.0);
    assert_near(first[0].rect.bottom(), height);
    assert_eq!(first[0].direction, TextDirection::Ltr);

    let three = box_for(2..8);
    assert_eq!(three.len(), 1);
    assert_near(three[0].rect.x, glyph_span(&paragraph, 2).0);
    assert_near(three[0].rect.right(), glyph_span(&paragraph, 7).1);
    assert_near(three[0].rect.bottom(), height);

    let rest = box_for(8..21);
    assert_eq!(rest.len(), 1);
    assert_near(rest[0].rect.x, glyph_span(&paragraph, 8).0);
    assert_near(rest[0].rect.right(), paragraph.lines()[0].width);

    // A range across several lines, one box per line. Skia's range crosses four
    // of its lines; Fira Sans wraps the same text so that it starts on the
    // second of four.
    let across = box_for(30..100);
    assert_eq!(across.len(), 3);
    for (index, text_box) in across.iter().enumerate() {
        let line = index + 1;
        assert_near(text_box.rect.y, line as f32 * height);
        assert_near(text_box.rect.bottom(), (line + 1) as f32 * height);
    }
    assert_near(across[0].rect.x, glyph_span(&paragraph, 30).0);
    assert_near(across[2].rect.right(), paragraph.lines()[3].width);
    assert_near(across[2].rect.bottom(), 4.0 * height);

    // A range over the first line's last letters and its trailing space.
    let space = box_for(19..22);
    assert_eq!(space.len(), 1);
    assert_near(space[0].rect.x, glyph_span(&paragraph, 19).0);
    assert_near(space[0].rect.right(), glyph_span(&paragraph, 21).1);
    assert!(box_for(21..21).is_empty());
}

/// Skparagraph's `SkParagraph_GetRectsForRangeTight`: a `Tight` box is its
/// run's own room, so at 50 px of Fira Sans a box is 60 px tall, the font's
/// ascent and descent. Skia's case draws Noto Sans CJK, whose 74 px line is
/// taller than Roboto's; Fira Sans's is its line's own here. Skia's 8..21 is
/// two boxes, its kaomoji falling back to a second font; this text is one
/// font's.
#[test]
fn tight_boxes_are_their_runs_own_room_under_wrapped_text() {
    let paragraph = wrapped(&style(50.0));
    let room = font(&paragraph).line_height_px(50.0);
    assert_near(room, 60.0);
    let box_for = |range: Range<usize>| {
        paragraph.boxes_for_range(range, BoxHeightStyle::Tight, BoxWidthStyle::Tight)
    };

    assert!(box_for(0..0).is_empty());
    let first = box_for(0..1);
    assert_eq!(first.len(), 1);
    assert_near(first[0].rect.x, 0.0);
    assert_near(first[0].rect.right(), glyph_span(&paragraph, 0).1);
    assert_near(first[0].rect.y, 0.0);
    assert_near(first[0].rect.bottom(), room);

    assert_eq!(box_for(2..8).len(), 1);
    assert_eq!(box_for(8..21).len(), 1);
    // Across the wrap: one run per line, each the run's own room.
    let across = box_for(8..46);
    assert_eq!(across.len(), 2);
    for text_box in &across {
        assert_near(text_box.rect.height, room);
    }
    assert_near(
        across[1].rect.y,
        paragraph.lines()[1].baseline - font(&paragraph).ascent_px(50.0),
    );
}

/// Skparagraph's `kTight` case gives a run with a height multiplier its font's
/// OWN ascent and descent around the line's baseline, where the line takes the
/// multiplier's room: a 2 × 20 px line is 40 tall and its text's tight box 24.
#[test]
fn a_height_multiplied_run_keeps_its_fonts_own_tight_box() {
    let paragraph = laid_out_fira(
        "hello",
        &TextStyle {
            height: Some(2.0),
            ..style(20.0)
        },
        ParagraphStyle::default(),
    );
    let line = &paragraph.lines()[0];
    let font = font(&paragraph);
    assert_near(line.ascent + line.descent, 40.0);
    let tight = boxes(&paragraph, BoxHeightStyle::Tight);
    assert_near(tight[0].rect.y, line.baseline - font.ascent_px(20.0));
    assert_near(
        tight[0].rect.bottom(),
        line.baseline + font.descent_px(20.0),
    );
    let widest = boxes(&paragraph, BoxHeightStyle::Max);
    assert_near(widest[0].rect.height, 40.0);
}

/// `spaced` is `WRAPPED_TEXT` at a 1.6 × 50 px height: four 80 px lines, each
/// with 15.58 px of room added above its text, which the line-spacing tests
/// measure from.
fn spaced() -> Paragraph {
    let paragraph = wrapped(&TextStyle {
        height: Some(1.6),
        ..style(50.0)
    });
    assert_eq!(paragraph.lines().len(), 4);
    paragraph
}

/// `spacing` is a spacing paragraph's line height and the room its height
/// multiplier added above each line's text.
fn spacing(paragraph: &Paragraph) -> (f32, f32) {
    let font = font(paragraph);
    let line = &paragraph.lines()[0];
    let (_, delta) = line_rounding(paragraph, 0);
    assert_near(delta, 0.0);
    (
        line.ascent + line.descent,
        line.ascent - font.ascent_px(50.0),
    )
}

/// Skparagraph's `SkParagraph_GetRectsForRangeIncludeLineSpacingMiddle`: every
/// box takes its line and half the room a height multiplier added above its
/// text, all of it on the first line and none below the last, so neighbouring
/// lines' boxes touch. `Max` width boxes reach the paragraph's widest line.
#[test]
fn line_spacing_middle_boxes_touch() {
    let paragraph = spaced();
    let (height, shift) = spacing(&paragraph);
    assert_near(height, 80.0);
    assert_near(shift, 15.583_334);
    let box_for = |range: Range<usize>| {
        paragraph.boxes_for_range(
            range,
            BoxHeightStyle::IncludeLineSpacingMiddle,
            BoxWidthStyle::Max,
        )
    };

    let first = box_for(0..1);
    assert_eq!(first.len(), 1);
    assert_near(first[0].rect.y, shift);
    assert_near(first[0].rect.bottom(), height + shift / 2.0);
    assert_eq!(box_for(2..8).len(), 1);

    // This text's 8..21 ends at the first line's content, so the `Max` width
    // reaches the paragraph's widest line in a box of its own; Skia's kaomoji
    // line has no line end there and gives one box.
    let widest = widest_with_spaces(&paragraph);
    let rest = box_for(8..21);
    assert_eq!(rest.len(), 2);
    assert_near(rest[0].rect.x, glyph_span(&paragraph, 8).0);
    assert_near(rest[0].rect.right(), paragraph.lines()[0].width);
    assert_near(rest[1].rect.x, paragraph.lines()[0].width);
    assert_near(rest[1].rect.right(), widest);
    assert_near(rest[0].rect.y, rest[1].rect.y);

    // Across lines: each line's box starts where the one above ends.
    let across = box_for(30..150);
    let lines = paragraph.lines();
    assert_eq!(across.len(), 3);
    assert_near(across[0].rect.y, height + shift / 2.0);
    assert_near(across[0].rect.bottom(), 2.0 * height + shift / 2.0);
    assert_near(across[1].rect.y, across[0].rect.bottom());
    assert_near(across[1].rect.bottom(), 3.0 * height + shift / 2.0);
    assert_near(across[2].rect.y, across[1].rect.bottom());
    assert_near(across[2].rect.bottom(), 4.0 * height);
    assert_near(across[0].rect.x, glyph_span(&paragraph, 30).0);
    assert_near(across[2].rect.right(), lines[3].width);

    let space = box_for(19..22);
    assert_eq!(space.len(), 2);
    assert_near(space[0].rect.right(), glyph_span(&paragraph, 21).1);
    assert_near(space[1].rect.right(), widest);
    assert!(box_for(21..21).is_empty());
}

/// Skparagraph's `SkParagraph_GetRectsForRangeIncludeLineSpacingTop`: every box
/// takes its line and the room a height multiplier added above its text,
/// except the first line's, which starts where its fonts' own ascent does.
#[test]
fn line_spacing_top_boxes_start_at_the_first_lines_text() {
    let paragraph = spaced();
    let (height, shift) = spacing(&paragraph);
    let box_for = |range: Range<usize>| {
        paragraph.boxes_for_range(
            range,
            BoxHeightStyle::IncludeLineSpacingTop,
            BoxWidthStyle::Max,
        )
    };

    let first = box_for(0..1);
    assert_eq!(first.len(), 1);
    assert_near(first[0].rect.y, shift);
    assert_near(first[0].rect.bottom(), height);

    let across = box_for(30..150);
    assert_eq!(across.len(), 3);
    assert_near(across[0].rect.y, height);
    assert_near(across[0].rect.bottom(), 2.0 * height);
    assert_near(across[1].rect.y, 2.0 * height);
    assert_near(across[1].rect.bottom(), 3.0 * height);
    assert_near(across[2].rect.y, 3.0 * height);
    assert_near(across[2].rect.bottom(), 4.0 * height);

    assert_eq!(box_for(8..21).len(), 2);
    assert_eq!(box_for(19..22).len(), 2);
    assert!(box_for(21..21).is_empty());
}

/// Skparagraph's `SkParagraph_GetRectsForRangeIncludeLineSpacingBottom`: every
/// box takes its line and the room a height multiplier added above its text, so
/// it starts where the box above ends, except the last line's, which ends at
/// the line's bottom.
#[test]
fn line_spacing_bottom_boxes_end_at_the_last_lines_text() {
    let paragraph = spaced();
    let (height, shift) = spacing(&paragraph);
    let box_for = |range: Range<usize>| {
        paragraph.boxes_for_range(
            range,
            BoxHeightStyle::IncludeLineSpacingBottom,
            BoxWidthStyle::Max,
        )
    };

    let first = box_for(0..1);
    assert_eq!(first.len(), 1);
    assert_near(first[0].rect.y, shift);
    assert_near(first[0].rect.bottom(), height + shift);

    let across = box_for(30..150);
    assert_eq!(across.len(), 3);
    assert_near(across[0].rect.y, height + shift);
    assert_near(across[0].rect.bottom(), 2.0 * height + shift);
    assert_near(across[1].rect.y, 2.0 * height + shift);
    assert_near(across[1].rect.bottom(), 3.0 * height + shift);
    assert_near(across[2].rect.y, 3.0 * height + shift);
    assert_near(across[2].rect.bottom(), 4.0 * height);

    assert_eq!(box_for(8..21).len(), 2);
    assert_eq!(box_for(19..22).len(), 2);
    assert!(box_for(21..21).is_empty());
}

/// Boxes of runs in different fonts, as a fallback face makes on one line, stay
/// apart and keep their own tight room; boxes of runs in one font join; and a
/// `Max` height box gives every one of them the line's room.
#[test]
fn boxes_of_different_fonts_stay_apart_and_of_one_font_join() {
    let mut collection = fonts();
    let fira = collection
        .register("Fira Sans", asset("fira_sans.ttf"))
        .unwrap();
    let hebrew = collection
        .register("Noto Sans Hebrew", asset("noto_sans_hebrew.ttf"))
        .unwrap();
    collection.add_fallback(hebrew);
    let (fira_room, hebrew_room) = {
        let faces = collection.faces();
        (
            faces.get(fira).line_height_px(20.0),
            faces.get(hebrew).line_height_px(20.0),
        )
    };
    assert_near(fira_room, 24.0);
    assert!(hebrew_room > fira_room);
    let paragraph = laid_out(
        &mut collection,
        "abc שלום abc",
        &style(20.0),
        ParagraphStyle::default(),
        f32::INFINITY,
    );
    assert_eq!(paragraph.lines().len(), 1);
    let line = &paragraph.lines()[0];
    assert_near(line.ascent + line.descent, hebrew_room);

    let tight = paragraph.boxes_for_range(
        0..paragraph.text().len(),
        BoxHeightStyle::Tight,
        BoxWidthStyle::Tight,
    );
    assert_eq!(tight.len(), 3, "{tight:?}");
    assert_eq!(tight[0].direction, TextDirection::Ltr);
    assert_eq!(tight[1].direction, TextDirection::Rtl);
    assert_eq!(tight[2].direction, TextDirection::Ltr);
    assert_near(tight[0].rect.height, fira_room);
    assert_near(tight[1].rect.height, hebrew_room);
    assert_near(tight[2].rect.height, fira_room);
    // Adjacent, and separate: the fonts are not one run.
    assert_near(tight[0].rect.right(), tight[1].rect.x);
    assert_near(tight[1].rect.right(), tight[2].rect.x);

    let widest = paragraph.boxes_for_range(
        0..paragraph.text().len(),
        BoxHeightStyle::Max,
        BoxWidthStyle::Tight,
    );
    assert_eq!(widest.len(), 3);
    for text_box in &widest {
        assert_near(text_box.rect.y, tight[1].rect.y);
        assert_near(text_box.rect.bottom(), tight[1].rect.bottom());
        assert_near(text_box.rect.height, hebrew_room);
    }

    // Two spans of one font touch, and their boxes join into one.
    let mut collection = fonts();
    let mut builder = ParagraphBuilder::new(&mut collection);
    builder.add_text("ab", &style(20.0));
    builder.add_text("cd", &style(20.0));
    let mut joined = builder.build();
    joined.layout(f32::INFINITY);
    assert_eq!(joined.lines()[0].runs.len(), 2);
    let boxes = joined.rects_for_range(0..4);
    assert_eq!(boxes.len(), 1, "{boxes:?}");
    assert_near(boxes[0].x, 0.0);
    assert_near(boxes[0].right(), joined.lines()[0].width);
}

/// Skparagraph's `SkParagraph_EmojiMultiLineRectsParagraph`: an emoji is one
/// cluster, a selection of whole emoji gets one box per line, and a range
/// inside an emoji's cluster selects nothing.
///
/// Skia's case draws 119 bytes over two lines; this one draws 24 emoji of the
/// test subset at 50 px, which wraps onto three lines of eight.
#[test]
fn emoji_clusters_get_a_box_of_their_own_across_lines() {
    let mut collection = FontCollection::new();
    collection
        .register("Noto Color Emoji", asset("noto_color_emoji_subset.ttf"))
        .unwrap();
    let text = "😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀";
    let paragraph = laid_out(
        &mut collection,
        text,
        &TextStyle::new("Noto Color Emoji", 50.0, Color::BLACK),
        ParagraphStyle::default(),
        550.0,
    );
    assert_eq!(paragraph.lines().len(), 3);
    let box_for = |range: Range<usize>| {
        paragraph.boxes_for_range(range, BoxHeightStyle::Tight, BoxWidthStyle::Tight)
    };

    assert!(box_for(0..0).is_empty());
    let one = box_for(0..4);
    assert_eq!(one.len(), 1);
    assert_near(one[0].rect.x, 0.0);
    assert_near(one[0].rect.right(), glyph_span(&paragraph, 0).1);

    let all = box_for(0..text.len());
    assert_eq!(all.len(), 3, "{all:?}");
    for (index, text_box) in paragraph.lines().iter().enumerate() {
        assert_near(all[index].rect.right(), text_box.width);
    }

    // A range inside an emoji's cluster takes none of it, as Skia's
    // 122..132 case says.
    assert!(box_for(1..3).is_empty());
}

/// A line's height rounds to a whole pixel, and its boxes start where the
/// rounding leaves the line's top (`InternalLineMetrics::delta`): 17 px of
/// Fira Sans is 20.4 tall, so a `Max` box starts 0.4 above its line's top and
/// is 20 tall.
#[test]
fn a_box_starts_at_the_rounding_of_its_lines_height() {
    let paragraph = laid_out_fira("hello", &style(17.0), ParagraphStyle::default());
    let (whole, delta) = line_rounding(&paragraph, 0);
    assert_near(whole, 20.4);
    assert_near(delta, -0.4);
    let result = boxes(&paragraph, BoxHeightStyle::Max);
    assert_near(result[0].rect.y, -0.4);
    assert_near(result[0].rect.bottom(), 20.0);
}

/// Skparagraph's `SkParagraph_GetRectsForRangeStrutWithHeight`: a 14 px strut
/// with a height of 2 and a leading of 3 gives every box 28 px of strut from
/// 3 × 14 / 2 = 21 px below the line's top.
#[test]
fn a_strut_box_has_the_struts_own_room() {
    let paragraph = laid_out_fira(
        "A",
        &style(10.0),
        ParagraphStyle {
            strut_style: StrutStyle {
                enabled: true,
                families: vec!["Fira Sans".to_owned()],
                size: 14.0,
                height: 2.0,
                height_override: true,
                leading: Some(3.0),
                ..StrutStyle::default()
            },
            ..ParagraphStyle::default()
        },
    );
    let result = boxes(&paragraph, BoxHeightStyle::Strut);
    assert_eq!(result.len(), 1);
    assert_near(result[0].rect.y, 21.0);
    assert_near(result[0].rect.bottom(), 49.0);
    assert_near(result[0].rect.height, 28.0);
}

/// A forced strut sets a line's raw metrics to its font's own, as
/// skparagraph's `updateLineMetrics` copies them, so a strut's leading is no
/// room a line-spacing style moves boxes by: a 20 px strut with a leading of 1
/// makes 44 px lines, and every line-spacing box is its line's.
#[test]
fn a_forced_struts_leading_adds_no_line_spacing() {
    let paragraph = laid_out_fira(
        "abc\ndef",
        &style(20.0),
        ParagraphStyle {
            strut_style: StrutStyle {
                enabled: true,
                families: vec!["Fira Sans".to_owned()],
                size: 20.0,
                leading: Some(1.0),
                force_height: true,
                ..StrutStyle::default()
            },
            ..ParagraphStyle::default()
        },
    );
    let lines = boxes(&paragraph, BoxHeightStyle::Max);
    assert_near(lines[0].rect.height, 44.0);
    for style in [
        BoxHeightStyle::IncludeLineSpacingMiddle,
        BoxHeightStyle::IncludeLineSpacingTop,
        BoxHeightStyle::IncludeLineSpacingBottom,
    ] {
        let spaced = boxes(&paragraph, style);
        assert_eq!(spaced.len(), 2, "{style:?}");
        for (spaced, line) in spaced.iter().zip(&lines) {
            assert_near(spaced.rect.y, line.rect.y);
            assert_near(spaced.rect.bottom(), line.rect.bottom());
        }
    }
}

/// Skparagraph's `SkParagraph_GetRectsForRangeStrut`: the strut's box is the
/// strut's, smaller than a line whose text is taller, and a paragraph without a
/// strut leaves the run's measured box where the strut's would be.
#[test]
fn a_strut_box_is_the_struts_even_under_taller_text() {
    let paragraph = laid_out_fira(
        "A",
        &style(20.0),
        ParagraphStyle {
            strut_style: StrutStyle {
                enabled: true,
                families: vec!["Fira Sans".to_owned()],
                size: 14.0,
                ..StrutStyle::default()
            },
            ..ParagraphStyle::default()
        },
    );
    let line = &paragraph.lines()[0];
    let font = font(&paragraph);
    let result = boxes(&paragraph, BoxHeightStyle::Strut);
    assert_near(result[0].rect.y, line.baseline - font.ascent_px(14.0));
    assert_near(
        result[0].rect.bottom(),
        line.baseline + font.descent_px(14.0),
    );
    assert_near(result[0].rect.height, font.line_height_px(14.0));
    // The line itself is taller than the strut's box.
    assert!(line.ascent + line.descent > result[0].rect.height);

    let plain = laid_out_fira("A", &style(20.0), ParagraphStyle::default());
    let strutless = boxes(&plain, BoxHeightStyle::Strut);
    let tight = boxes(&plain, BoxHeightStyle::Tight);
    assert_eq!(strutless, tight);
}

/// Skparagraph's `SkParagraph_BasicHalfLeading`: lines at a 3.6345 × 20 px
/// half leading are 73 px apart, their tight boxes are the text's own 24 px,
/// and the line boxes share the remaining 24.345 px evenly over and under.
#[test]
fn half_leading_lines_are_uniform_and_centred_on_their_text() {
    let paragraph = laid_out_fira(
        "abc\ndef\nghi",
        &TextStyle {
            height: Some(3.6345),
            leading_distribution: valo_paragraph::LeadingDistribution::Even,
            ..style(20.0)
        },
        ParagraphStyle::default(),
    );
    let lines = paragraph.lines();
    assert_eq!(lines.len(), 3);
    let (_, delta) = line_rounding(&paragraph, 0);
    assert_near(delta, 0.31);
    let tight = boxes(&paragraph, BoxHeightStyle::Tight);
    let wide = boxes(&paragraph, BoxHeightStyle::Max);
    let height = 73.0;
    let spacing = tight[1].rect.y - tight[0].rect.bottom();
    assert_near(spacing, height - tight[0].rect.height);
    assert_near(tight[2].rect.y - tight[1].rect.bottom(), spacing);
    for index in 0..3 {
        assert_near(tight[index].rect.height, 24.0);
        assert_near(
            wide[index].rect.bottom() - tight[index].rect.bottom(),
            tight[index].rect.y - wide[index].rect.y,
        );
    }
}

/// Skparagraph's `RectWidthStyle::kMax`: a line's last box reaches the
/// paragraph's widest line, trailing whitespace included, and the paragraph's
/// last line keeps its tight width. The first line here is 36 px and the last
/// line, with its seven trailing spaces, 54.
#[test]
fn a_max_width_box_reaches_the_widest_line() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut collection = FontCollection::new();
    collection
        .register(
            "JetBrains Mono",
            std::fs::read(format!("{dir}/jetbrains_mono.ttf")).unwrap(),
        )
        .unwrap();
    let mono = TextStyle::new("JetBrains Mono", 10.0, Color::BLACK);
    let paragraph = laid_out_in(
        &mut collection,
        "aaaaaa\nbb       ",
        &mono,
        ParagraphStyle::default(),
    );
    let lines = paragraph.lines();
    assert_eq!(lines.len(), 2);
    let advance = lines[0].width / 6.0;
    assert_near(lines[0].width, 6.0 * advance);
    assert_near(lines[1].width, 2.0 * advance);

    let tight = paragraph.boxes_for_range(
        0..lines[0].range.end,
        BoxHeightStyle::Max,
        BoxWidthStyle::Tight,
    );
    let widest = paragraph.boxes_for_range(
        0..lines[0].range.end,
        BoxHeightStyle::Max,
        BoxWidthStyle::Max,
    );
    assert_eq!(tight.len(), 1);
    assert_eq!(widest.len(), 2);
    assert_near(tight[0].rect.right(), lines[0].width);
    assert_near(widest[1].rect.x, lines[0].width);
    assert_near(widest[1].rect.right(), 9.0 * advance);

    // The last line is left alone by the width style.
    let last_tight = paragraph.boxes_for_range(
        lines[1].range.clone(),
        BoxHeightStyle::Max,
        BoxWidthStyle::Tight,
    );
    let last_widest = paragraph.boxes_for_range(
        lines[1].range.clone(),
        BoxHeightStyle::Max,
        BoxWidthStyle::Max,
    );
    assert_eq!(last_tight, last_widest);
}

/// A line's trailing whitespace is a box of its own, as skparagraph splits it
/// off, and selecting it alone gives that box beyond the line's content width;
/// a selection that reaches it merges it into the content's box.
#[test]
fn trailing_whitespace_is_a_box_of_its_own() {
    let paragraph = laid_out_fira("aa bb  ", &style(20.0), ParagraphStyle::default());
    let line = &paragraph.lines()[0];
    let all = boxes(&paragraph, BoxHeightStyle::Max);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_near(all[0].rect.right(), line.width + trailing_advance(line));
    let space = paragraph.boxes_for_range(5..7, BoxHeightStyle::Max, BoxWidthStyle::Tight);
    assert_eq!(space.len(), 1);
    assert_near(space[0].rect.x, line.width);
    assert_near(space[0].rect.right(), line.width + trailing_advance(line));
}

/// Skparagraph's Arabic boxes: a right-to-left run's box reports the run's
/// direction, and a left-to-right paragraph's trailing space reports the
/// paragraph's.
#[test]
fn boxes_report_their_text_direction() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut collection = FontCollection::new();
    collection
        .register(
            "Noto Sans Hebrew",
            std::fs::read(format!("{dir}/noto_sans_hebrew.ttf")).unwrap(),
        )
        .unwrap();
    let hebrew = TextStyle::new("Noto Sans Hebrew", 20.0, Color::BLACK);
    let rtl = laid_out_in(
        &mut collection,
        "שלום",
        &hebrew,
        ParagraphStyle {
            direction: Some(TextDirection::Rtl),
            ..ParagraphStyle::default()
        },
    );
    let result = rtl.boxes_for_range(
        0..rtl.text().len(),
        BoxHeightStyle::Max,
        BoxWidthStyle::Tight,
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].direction, TextDirection::Rtl);

    // skparagraph's `ArabicRectsLTRLeftAlignParagraph`: a left-to-right
    // paragraph holds the Arabic text in a right-to-left box and its trailing
    // space in a left-to-right one of the paragraph's own direction.
    let ltr = laid_out_in(
        &mut collection,
        "abc שלום ",
        &hebrew,
        ParagraphStyle::default(),
    );
    let result = ltr.boxes_for_range(
        0..ltr.text().len(),
        BoxHeightStyle::Max,
        BoxWidthStyle::Tight,
    );
    assert_eq!(result.len(), 3, "{result:?}");
    assert_eq!(result[0].direction, TextDirection::Ltr);
    assert_eq!(result[1].direction, TextDirection::Rtl);
    assert_eq!(result[2].direction, TextDirection::Ltr);
    let line = &ltr.lines()[0];
    assert_near(result[0].rect.x, 0.0);
    assert_near(result[0].rect.right(), result[1].rect.x);
    assert_near(result[1].rect.right(), result[2].rect.x);
    assert_near(result[2].rect.x, line.width);
    assert_near(result[2].rect.right(), line.width + trailing_advance(line));
}

/// An empty paragraph has one zero-width box of its whole height, the caret's,
/// for a range that starts at zero; an empty range has none, as skparagraph's
/// implied last line break answers.
#[test]
fn an_empty_paragraph_has_one_box() {
    let paragraph = laid_out_fira("", &style(20.0), ParagraphStyle::default());
    assert_eq!(
        paragraph.boxes_for_range(0..0, BoxHeightStyle::Tight, BoxWidthStyle::Tight),
        []
    );
    let result = paragraph.boxes_for_range(0..1, BoxHeightStyle::Tight, BoxWidthStyle::Tight);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].rect.x, 0.0);
    assert_eq!(result[0].rect.width, 0.0);
    assert_near(result[0].rect.height, paragraph.height());
    assert!(paragraph
        .boxes_for_range(1..2, BoxHeightStyle::Tight, BoxWidthStyle::Tight)
        .is_empty());
}

/// A range of no length, a range before the text and a range past it have no
/// boxes; a run's part gets only its own characters.
#[test]
fn a_range_selects_only_its_own_characters() {
    let paragraph = laid_out_fira("hello world", &style(20.0), ParagraphStyle::default());
    assert!(paragraph
        .boxes_for_range(3..3, BoxHeightStyle::Max, BoxWidthStyle::Tight)
        .is_empty());
    let hello = paragraph.boxes_for_range(0..5, BoxHeightStyle::Max, BoxWidthStyle::Tight);
    let world = paragraph.boxes_for_range(6..11, BoxHeightStyle::Max, BoxWidthStyle::Tight);
    assert_eq!((hello.len(), world.len()), (1, 1));
    assert_near(hello[0].rect.x, 0.0);
    assert!(hello[0].rect.right() < world[0].rect.x);
    assert_near(world[0].rect.right(), paragraph.lines()[0].width);
}

/// Two sizes on one line: their tight boxes differ, so they stay two boxes,
/// where the max height style makes them one height and lets them join.
#[test]
fn different_sizes_get_their_own_tight_boxes() {
    let mut collection = fonts();
    let mut builder = ParagraphBuilder::new(&mut collection);
    builder.add_text("big", &style(30.0));
    builder.add_text("small", &style(12.0));
    let mut paragraph = builder.build();
    paragraph.layout(f32::INFINITY);
    let line = &paragraph.lines()[0];
    let font = font(&paragraph);
    let (big, small) = (font.ascent_px(30.0), font.ascent_px(12.0));
    let (big_descent, small_descent) = (font.descent_px(30.0), font.descent_px(12.0));
    let tight = paragraph.boxes_for_range(
        0..paragraph.text().len(),
        BoxHeightStyle::Tight,
        BoxWidthStyle::Tight,
    );
    assert_eq!(tight.len(), 2);
    assert_near(tight[0].rect.y, line.baseline - big);
    assert_near(tight[0].rect.bottom(), line.baseline + big_descent);
    assert_near(tight[1].rect.y, line.baseline - small);
    assert_near(tight[1].rect.bottom(), line.baseline + small_descent);
    assert!(tight[0].rect.height > tight[1].rect.height);

    // The line's room makes them one height, but sizes still keep them apart.
    let wide = paragraph.boxes_for_range(
        0..paragraph.text().len(),
        BoxHeightStyle::Max,
        BoxWidthStyle::Tight,
    );
    assert_eq!(wide.len(), 2);
    assert_near(wide[0].rect.y, wide[1].rect.y);
    assert_near(wide[0].rect.bottom(), wide[1].rect.bottom());
}

/// A placeholder's tight box is the placeholder's own: skparagraph measures a
/// placeholder run as its box, not as a font's room.
#[test]
fn a_placeholder_box_is_the_placeholders_own() {
    use valo_paragraph::{PlaceholderAlignment, PlaceholderStyle};
    let mut collection = fonts();
    let mut builder = ParagraphBuilder::new(&mut collection);
    builder.add_text("a", &style(20.0));
    builder.add_placeholder(
        PlaceholderStyle {
            width: 40.0,
            height: 24.0,
            alignment: PlaceholderAlignment::AboveBaseline,
            ..PlaceholderStyle::default()
        },
        &style(20.0),
    );
    builder.add_text("b", &style(20.0));
    let mut paragraph = builder.build();
    paragraph.layout(f32::INFINITY);
    let placeholder = paragraph.placeholder_boxes()[0].rect;
    let (_, delta) = line_rounding(&paragraph, 0);
    assert_near(delta, -0.3);
    let result = boxes(&paragraph, BoxHeightStyle::Tight);
    assert_eq!(result.len(), 3, "{result:?}");
    // The line's rounding offset moves the box with the line's baseline.
    assert_near(result[1].rect.y, placeholder.y + delta);
    assert_near(result[1].rect.bottom(), placeholder.bottom() + delta);
    assert_near(result[1].rect.x, placeholder.x);
    assert_near(result[1].rect.right(), placeholder.right());
}

/// Skparagraph's line-metric styles (`LineMetricStyle::Typographic`): where a
/// paragraph's height behavior trims the first line's ascent and the last
/// line's descent, those boxes take their fonts' own room on that side, the
/// half of the font's line gap left out. The line-gap test font's ascent,
/// descent and gap are 18.7, 5.3 and 4 px at 20 px.
#[test]
fn a_trimmed_line_takes_its_fonts_own_room() {
    use valo_paragraph::TextHeightBehavior;
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut collection = FontCollection::new();
    collection
        .register(
            "Line Gap Test",
            std::fs::read(format!("{dir}/line_gap_test.ttf")).unwrap(),
        )
        .unwrap();
    let text_style = TextStyle::new("Line Gap Test", 20.0, Color::BLACK);
    let trimmed = ParagraphStyle {
        text_height_behavior: TextHeightBehavior {
            apply_height_to_first_ascent: false,
            apply_height_to_last_descent: false,
        },
        ..ParagraphStyle::default()
    };
    let paragraph = laid_out_in(&mut collection, "ab\ncd", &text_style, trimmed);
    let font = font(&paragraph);
    let (ascent, descent, gap) = (
        font.ascent_px(20.0),
        font.descent_px(20.0),
        font.line_height_px(20.0) - font.ascent_px(20.0) - font.descent_px(20.0),
    );
    assert!((ascent - 18.7).abs() < 0.01 && (descent - 5.3).abs() < 0.01);
    assert!((gap - 4.0).abs() < 0.01);

    let lines = paragraph.lines();
    let tight = boxes(&paragraph, BoxHeightStyle::Tight);
    assert_eq!(tight.len(), 2);
    assert_near(tight[0].rect.y, lines[0].baseline - ascent);
    assert_near(tight[0].rect.height, ascent + descent + gap / 2.0);
    assert_near(tight[1].rect.y, lines[1].baseline - (ascent + gap / 2.0));
    assert_near(tight[1].rect.height, ascent + gap / 2.0 + descent);

    // With the behavior on, both lines take their fonts' full room there.
    let untrimmed = laid_out_in(
        &mut collection,
        "ab\ncd",
        &text_style,
        ParagraphStyle::default(),
    );
    let tight = boxes(&untrimmed, BoxHeightStyle::Tight);
    for text_box in &tight {
        assert_near(text_box.rect.height, ascent + descent + gap);
    }
}
