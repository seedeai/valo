//! Selection boxes in every height and width style: skparagraph's
//! `ParagraphImpl::getRectsForRange`, ported from `SkParagraphTest`'s
//! `GetRectsForRangeParagraph`, `GetRectsForRangeTight`,
//! `GetRectsForRangeIncludeLineSpacingMiddle`/`Top`/`Bottom`,
//! `GetRectsForRangeStrut` and `BasicHalfLeading`, with the Arabic and Hebrew
//! direction cases.
//!
//! Skia's tests draw in Roboto and Noto Sans CJK, which are not Valo's test
//! fonts. These draw in Fira Sans, whose ascent and descent are 0.935 and 0.265
//! of its size with no line gap, and state each number against the metrics the
//! paragraph reports (or against the constant it resolves to), rather than
//! against Roboto's.

use valo_geometry::Color;
use valo_paragraph::{
    BoxHeightStyle, BoxWidthStyle, Paragraph, ParagraphBuilder, ParagraphStyle, StrutStyle,
    TextBox, TextDirection, TextStyle,
};
use valo_text::{Font, FontCollection};

fn fonts() -> FontCollection {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut collection = FontCollection::new();
    collection
        .register(
            "Fira Sans",
            std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap(),
        )
        .unwrap();
    collection
}

fn style(size: f32) -> TextStyle {
    TextStyle::new("Fira Sans", size, Color::BLACK)
}

fn laid_out(text: &str, text_style: &TextStyle, paragraph_style: ParagraphStyle) -> Paragraph {
    laid_out_in(&mut fonts(), text, text_style, paragraph_style)
}

fn laid_out_in(
    collection: &mut FontCollection,
    text: &str,
    text_style: &TextStyle,
    paragraph_style: ParagraphStyle,
) -> Paragraph {
    let mut builder = ParagraphBuilder::new(collection);
    builder.style(paragraph_style);
    builder.add_text(text, text_style);
    let mut paragraph = builder.build();
    paragraph.layout(f32::INFINITY);
    paragraph
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

/// Skparagraph's `SkParagraph_GetRectsForRangeParagraph`: a `Max` height box is
/// the line's whole height from the rounding offset of it, whatever the run's
/// own font asks for, and a line's trailing spaces are a box of their own that
/// merges with the text's.
#[test]
fn a_max_height_box_is_the_lines_whole_height() {
    let paragraph = laid_out("hello", &style(20.0), ParagraphStyle::default());
    let line = &paragraph.lines()[0];
    let (whole, delta) = line_rounding(&paragraph, 0);
    let result = boxes(&paragraph, BoxHeightStyle::Max);
    assert_eq!(result.len(), 1);
    assert_near(result[0].rect.y, delta);
    assert_near(result[0].rect.bottom(), whole + delta);
    assert_near(result[0].rect.x, 0.0);
    assert_near(result[0].rect.right(), line.width);
    assert_eq!(result[0].direction, TextDirection::Ltr);

    // A line whose height rounds asks for its box to start above the line's
    // top by the difference: 17 px of Fira Sans is 20.4 tall, rounded to 20.
    let paragraph = laid_out("hello", &style(17.0), ParagraphStyle::default());
    let (whole, delta) = line_rounding(&paragraph, 0);
    assert_near(whole, 20.4);
    let result = boxes(&paragraph, BoxHeightStyle::Max);
    assert_near(result[0].rect.y, -0.4);
    assert_near(result[0].rect.bottom(), 20.0);
    assert_near(delta, -0.4);
}

/// Skparagraph's `SkParagraph_GetRectsForRangeTight`: a `Tight` box is the
/// run's own room, the font's ascent and descent at its size, placed against
/// the line's baseline — not the line's taller room.
#[test]
fn a_tight_box_is_the_runs_own_room() {
    let paragraph = laid_out("hello", &style(20.0), ParagraphStyle::default());
    let line = &paragraph.lines()[0];
    let font = font(&paragraph);
    let result = boxes(&paragraph, BoxHeightStyle::Tight);
    assert_eq!(result.len(), 1);
    assert_near(result[0].rect.y, line.baseline - font.ascent_px(20.0));
    assert_near(
        result[0].rect.bottom(),
        line.baseline + font.descent_px(20.0),
    );
    assert_near(
        result[0].rect.height,
        font.ascent_px(20.0) + font.descent_px(20.0),
    );
}

/// Skparagraph's `kTight` case gives a run with a height multiplier its font's
/// OWN ascent and descent around the line's baseline, where the line takes the
/// multiplier's room: a 2 × 20 px line is 40 tall and its text's tight box 24.
#[test]
fn a_height_multiplied_run_keeps_its_fonts_own_tight_box() {
    let paragraph = laid_out(
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

/// Skparagraph's `SkParagraph_GetRectsForRangeIncludeLineSpacingMiddle`: each
/// line's box takes the whole line, the first line's top and the last line's
/// bottom extended by the room a height multiplier added, so neighbouring
/// lines' boxes touch.
#[test]
fn line_spacing_middle_boxes_touch() {
    let paragraph = multi_line();
    let (height, shift) = spacing(&paragraph);
    assert_near(height, 32.0);
    assert_near(shift, 6.233_333);
    let result = boxes(&paragraph, BoxHeightStyle::IncludeLineSpacingMiddle);
    let edges: Vec<(f32, f32)> = result.iter().map(|b| (b.rect.y, b.rect.bottom())).collect();
    assert_near(edges[0].0, shift);
    assert_near(edges[0].1, height + shift / 2.0);
    assert_near(edges[1].0, edges[0].1);
    assert_near(edges[1].1, 2.0 * height + shift / 2.0);
    assert_near(edges[2].0, edges[1].1);
    assert_near(edges[2].1, 3.0 * height);
}

/// Skparagraph's `SkParagraph_GetRectsForRangeIncludeLineSpacingTop`: the room
/// a height multiplier added goes above the first line's text alone.
#[test]
fn line_spacing_top_boxes_start_at_the_first_lines_text() {
    let paragraph = multi_line();
    let (height, shift) = spacing(&paragraph);
    let result = boxes(&paragraph, BoxHeightStyle::IncludeLineSpacingTop);
    let edges: Vec<(f32, f32)> = result.iter().map(|b| (b.rect.y, b.rect.bottom())).collect();
    assert_near(edges[0].0, shift);
    assert_near(edges[0].1, height);
    assert_near(edges[1].0, height);
    assert_near(edges[1].1, 2.0 * height);
    assert_near(edges[2].0, 2.0 * height);
    assert_near(edges[2].1, 3.0 * height);
}

/// Skparagraph's `SkParagraph_GetRectsForRangeIncludeLineSpacingBottom`: the
/// room a height multiplier added goes below the last line's text alone, so
/// every box before it starts where the one above ends.
#[test]
fn line_spacing_bottom_boxes_end_at_the_last_lines_text() {
    let paragraph = multi_line();
    let (height, shift) = spacing(&paragraph);
    let result = boxes(&paragraph, BoxHeightStyle::IncludeLineSpacingBottom);
    let edges: Vec<(f32, f32)> = result.iter().map(|b| (b.rect.y, b.rect.bottom())).collect();
    assert_near(edges[0].0, shift);
    assert_near(edges[0].1, height + shift);
    assert_near(edges[1].0, height + shift);
    assert_near(edges[1].1, 2.0 * height + shift);
    assert_near(edges[2].0, 2.0 * height + shift);
    assert_near(edges[2].1, 3.0 * height);
}

/// A three-line paragraph at a 1.6 × 20 px height: 32 px lines, each with
/// 6.23 px of room added above its text, which the line-spacing tests measure
/// from.
fn multi_line() -> Paragraph {
    let paragraph = laid_out(
        "abc\ndef\nghi",
        &TextStyle {
            height: Some(1.6),
            ..style(20.0)
        },
        ParagraphStyle::default(),
    );
    assert_eq!(paragraph.lines().len(), 3);
    paragraph
}

/// `spacing` is the paragraph's line height and the room its height multiplier
/// added above each line's text.
fn spacing(paragraph: &Paragraph) -> (f32, f32) {
    let font = font(paragraph);
    let line = &paragraph.lines()[0];
    let (_, delta) = line_rounding(paragraph, 0);
    let height = line.ascent + line.descent;
    assert_near(delta, 0.0);
    (height, line.ascent - font.ascent_px(20.0))
}

/// Skparagraph's `SkParagraph_GetRectsForRangeStrutWithHeight`: a 14 px strut
/// with a height of 2 and a leading of 3 gives every box 28 px of strut from
/// 3 × 14 / 2 = 21 px below the line's top.
#[test]
fn a_strut_box_has_the_struts_own_room() {
    let paragraph = laid_out(
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

/// Skparagraph's `SkParagraph_GetRectsForRangeStrut`: the strut's box is the
/// strut's, smaller than a line whose text is taller, and a paragraph without a
/// strut leaves the run's measured box where the strut's would be.
#[test]
fn a_strut_box_is_the_struts_even_under_taller_text() {
    let paragraph = laid_out(
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

    let plain = laid_out("A", &style(20.0), ParagraphStyle::default());
    let strutless = boxes(&plain, BoxHeightStyle::Strut);
    let tight = boxes(&plain, BoxHeightStyle::Tight);
    assert_eq!(strutless, tight);
}

/// Skparagraph's `SkParagraph_BasicHalfLeading`: lines at a 3.6345 × 20 px
/// half leading are 73 px apart, their tight boxes are the text's own 24 px,
/// and the line boxes share the remaining 24.345 px evenly over and under.
#[test]
fn half_leading_lines_are_uniform_and_centred_on_their_text() {
    let paragraph = laid_out(
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
    let paragraph = laid_out("aa bb  ", &style(20.0), ParagraphStyle::default());
    let line = &paragraph.lines()[0];
    let all = boxes(&paragraph, BoxHeightStyle::Max);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_near(all[0].rect.right(), line.width + trailing_advance(line));
    let space = paragraph.boxes_for_range(5..7, BoxHeightStyle::Max, BoxWidthStyle::Tight);
    assert_eq!(space.len(), 1);
    assert_near(space[0].rect.x, line.width);
    assert_near(space[0].rect.right(), line.width + trailing_advance(line));
}

/// The advance of a line's trailing whitespace, its ghosts.
fn trailing_advance(line: &valo_paragraph::Line) -> f32 {
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
    let paragraph = laid_out("", &style(20.0), ParagraphStyle::default());
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
    let paragraph = laid_out("hello world", &style(20.0), ParagraphStyle::default());
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
