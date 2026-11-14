//! Hard line breaks and trailing whitespace: a line holds them as clusters it does not draw,
//! its ghosts, as skparagraph's `TextLine` holds `fGhostClusterRange`. The expected values
//! are skparagraph's answers for the same text.

use valo_geometry::{Color, Point, Rect};
use valo_paragraph::{
    Paragraph, ParagraphBuilder, ParagraphStyle, TextAlign, TextDirection, TextStyle,
};
use valo_text::FontCollection;

const SIZE: f32 = 10.0;

fn mono_fonts() -> FontCollection {
    let mut fonts = FontCollection::new();
    fonts
        .register("JetBrains Mono", super::asset("jetbrains_mono.ttf"))
        .unwrap();
    fonts
}

fn mono_style(size: f32) -> TextStyle {
    TextStyle::new("JetBrains Mono", size, Color::BLACK)
}

fn mono_in(text: &str, paragraph_style: ParagraphStyle, max_width: f32) -> Paragraph {
    let mut fonts = mono_fonts();
    let mut builder = ParagraphBuilder::new(&mut fonts);
    builder.style(paragraph_style);
    builder.add_text(text, &mono_style(SIZE));
    let mut paragraph = builder.build();
    paragraph.layout(max_width);
    paragraph
}

fn mono(text: &str) -> Paragraph {
    mono_in(text, ParagraphStyle::default(), f32::INFINITY)
}

/// The advance of one character of the monospace font at `SIZE`.
fn advance() -> f32 {
    mono("X").max_intrinsic_width()
}

fn hebrew(text: &str, direction: TextDirection) -> Paragraph {
    let mut fonts = FontCollection::new();
    fonts
        .register("Noto Sans Hebrew", super::asset("noto_sans_hebrew.ttf"))
        .unwrap();
    let mut builder = ParagraphBuilder::new(&mut fonts);
    builder.style(ParagraphStyle {
        direction: Some(direction),
        ..Default::default()
    });
    builder.add_text(
        text,
        &TextStyle::new("Noto Sans Hebrew", 20.0, Color::BLACK),
    );
    let mut paragraph = builder.build();
    paragraph.layout(f32::INFINITY);
    paragraph
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.01,
        "{actual} is not {expected}"
    );
}

/// The line's full height, as every box on it spans.
fn line_box(paragraph: &Paragraph, line: usize, left: f32, right: f32) -> Rect {
    let line = &paragraph.lines()[line];
    Rect::from_ltrb(
        left,
        line.baseline - line.ascent,
        right,
        line.baseline + line.descent,
    )
}

fn assert_same_rect(actual: Rect, expected: Rect) {
    let apart = [
        actual.x - expected.x,
        actual.y - expected.y,
        actual.right() - expected.right(),
        actual.bottom() - expected.bottom(),
    ];
    assert!(
        apart.iter().all(|distance| distance.abs() < 0.01),
        "{actual:?} is not {expected:?}"
    );
}

/// skparagraph measures a range that ends at a hard break up to the break's start
/// (`TextLine::measureTextInsideOneRun`), so the break's own box has no width, where its
/// line's text ends, on that line (`ParagraphImpl::getRectsForRange` by `textWithNewlines`).
#[test]
fn a_hard_break_is_a_box_of_no_width_where_its_lines_text_ends() {
    let paragraph = mono("aaa\naaa");
    let rects = paragraph.rects_for_range(3..4);
    assert_eq!(rects.len(), 1, "{rects:?}");
    let end = 3.0 * advance();
    assert_same_rect(rects[0], line_box(&paragraph, 0, end, end));
}

/// A caret before a hard break stays at the end of the line the break ends.
#[test]
fn a_caret_before_a_hard_break_ends_its_line() {
    let paragraph = mono("aaa\naaa");
    let caret = paragraph.caret_for_offset(3);
    assert_close(caret.x, 3.0 * advance());
    assert_close(
        caret.y,
        paragraph.lines()[0].baseline - paragraph.lines()[0].ascent,
    );
}

/// A selection through a hard break has the text's box alone.
#[test]
fn a_selection_through_a_hard_break_ends_where_the_text_does() {
    let paragraph = mono("aaa\naaa");
    assert_eq!(
        paragraph.rects_for_range(0..4),
        paragraph.rects_for_range(0..3)
    );
    assert_eq!(paragraph.rects_for_range(0..7).len(), 2);
}

/// The break's cluster comes after the line's trailing spaces, which the line keeps as
/// ghosts: selectable, outside its width (`TextWrapper::trimEndSpaces`).
#[test]
fn a_hard_break_comes_after_the_trailing_spaces() {
    let paragraph = mono("aa \nb");
    let a = advance();
    let line = &paragraph.lines()[0];
    assert_close(line.width, 2.0 * a);
    assert_eq!(
        (line.content_end, line.text_end, line.range.clone()),
        (2, 3, 0..4)
    );
    assert_same_rect(
        paragraph.rects_for_range(3..4)[0],
        line_box(&paragraph, 0, 3.0 * a, 3.0 * a),
    );
    let space = paragraph.rects_for_range(2..3);
    assert_same_rect(space[0], line_box(&paragraph, 0, 2.0 * a, 3.0 * a));
    assert_same_rect(
        paragraph.rects_for_range(0..3)[0],
        line_box(&paragraph, 0, 0.0, 3.0 * a),
    );
}

/// No width counts a hard break: the line's, the paragraph's longest line, or its intrinsic
/// widths (`TextWrapper::trimStartSpaces` leaves the break out of `widthWithSpaces`).
#[test]
fn a_hard_break_widens_nothing() {
    let paragraph = mono("aaa\naaa");
    let three = 3.0 * advance();
    for line in paragraph.lines() {
        assert_close(line.width, three);
    }
    assert_close(paragraph.longest_line(), three);
    assert_close(paragraph.min_intrinsic_width(), three);
    assert_close(paragraph.max_intrinsic_width(), three);
}

/// Nothing is drawn for a hard break, nor for trailing spaces: skparagraph paints a line's
/// trimmed text alone (`TextLine::paint`).
#[test]
fn nothing_is_drawn_for_a_lines_ghosts() {
    let paragraph = mono("a \u{2028}b");
    let line = &paragraph.lines()[0];
    let drawn: Vec<usize> = line
        .drawn_runs()
        .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.cluster))
        .collect();
    assert_eq!(drawn, [0]);
    let placed: Vec<usize> = line
        .runs
        .iter()
        .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.cluster))
        .collect();
    assert_eq!(placed, [0, 1, 2]);
    assert_same_rect(
        paragraph.ink_bounds().unwrap(),
        mono("a\nb").ink_bounds().unwrap(),
    );
}

/// A point past a line's end lands before its hard break, upstream, on the line it ends
/// (`TextLine::getGlyphPositionAtCoordinate` measures the run up to the break's start).
#[test]
fn a_point_past_a_line_lands_before_its_hard_break() {
    let paragraph = mono("aa \nb");
    let line = &paragraph.lines()[0];
    let hit = paragraph.glyph_position_at(Point::new(1000.0, line.baseline));
    assert_eq!((hit.offset, hit.downstream), (3, false));
}

/// A point anywhere on an empty line lands at its start, downstream: skparagraph's empty-line
/// case (`TextLine::getGlyphPositionAtCoordinate`, `fTextExcludingSpaces.end`).
#[test]
fn a_point_on_an_empty_line_lands_at_its_start() {
    let paragraph = mono("a\n\nb");
    let line = &paragraph.lines()[1];
    for x in [-10.0, 0.0, 1000.0] {
        let hit = paragraph.glyph_position_at(Point::new(x, line.baseline));
        assert_eq!((hit.offset, hit.downstream), (2, true), "at {x}");
    }
}

/// A text that is only a newline has the blank line the newline ends, whose break sits at its
/// start, and the empty line after it.
#[test]
fn a_text_of_one_newline_has_two_lines() {
    let paragraph = mono("\n");
    let lines = paragraph.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!((lines[0].range.clone(), lines[0].text_end), (0..1, 0));
    assert!(lines[0].ends_with_hard_break());
    assert_eq!(lines[1].range, 1..1);
    assert!(!lines[1].ends_with_hard_break());
    assert_same_rect(
        paragraph.rects_for_range(0..1)[0],
        line_box(&paragraph, 0, 0.0, 0.0),
    );
    let after = paragraph.caret_for_offset(1);
    assert_close(after.y, lines[1].baseline - lines[1].ascent);
}

/// The empty line a trailing newline opens is aligned like any line, as skparagraph formats
/// it (`TextLine::format`): at the right edge of a right-aligned paragraph.
#[test]
fn a_trailing_newlines_empty_line_is_aligned() {
    let style = ParagraphStyle {
        align: TextAlign::Right,
        ..Default::default()
    };
    let paragraph = mono_in("aa\n", style, 100.0);
    let lines = paragraph.lines();
    assert_eq!(lines.len(), 2);
    assert_close(lines[1].left, 100.0);
    assert_close(paragraph.caret_for_offset(3).x, 100.0);
    assert_same_rect(
        paragraph.rects_for_range(2..3)[0],
        line_box(&paragraph, 0, 100.0, 100.0),
    );
}

/// In a right-to-left paragraph a hard break takes the paragraph's direction (UAX #9 L1), so
/// its box is at the left end of its line, where right-to-left text ends.
#[test]
fn a_right_to_left_hard_break_is_at_its_lines_left_end() {
    let text = "שלום\nשלום";
    let newline = text.find('\n').unwrap();
    let paragraph = hebrew(text, TextDirection::Rtl);
    let line = &paragraph.lines()[0];
    let rects = paragraph.rects_for_range(newline..newline + 1);
    assert_eq!(rects.len(), 1, "{rects:?}");
    assert_same_rect(rects[0], line_box(&paragraph, 0, line.left, line.left));
    let caret = paragraph.caret_for_offset(newline);
    assert_close(caret.x, line.left);
}

/// In a left-to-right paragraph the break after right-to-left text is at the line's right end.
#[test]
fn a_hard_break_after_right_to_left_text_in_a_left_to_right_paragraph_is_at_the_right() {
    let text = "שלום\nא";
    let newline = text.find('\n').unwrap();
    let paragraph = hebrew(text, TextDirection::Ltr);
    let line = &paragraph.lines()[0];
    let end = line.left + line.width;
    assert_same_rect(
        paragraph.rects_for_range(newline..newline + 1)[0],
        line_box(&paragraph, 0, end, end),
    );
}

/// No line a hard break ends is justified (`TextLine::format`), whichever break it is.
#[test]
fn no_line_a_hard_break_ends_is_justified() {
    let style = ParagraphStyle {
        align: TextAlign::Justify,
        ..Default::default()
    };
    let paragraph = mono_in("aa bb\u{2028}cc dd", style, 100.0);
    assert_close(paragraph.lines()[0].width, 5.0 * advance());
}

/// An ellipsis replaces the line's ghosts with itself (`TextLine::createEllipsis`).
#[test]
fn an_ellipsized_line_has_no_ghosts() {
    let style = ParagraphStyle {
        max_lines: Some(1),
        ellipsis: Some("\u{2026}".to_owned()),
        ..Default::default()
    };
    let paragraph = mono_in("aaa bbb ccc", style, 9.0 * advance());
    assert!(paragraph.truncated());
    let line = &paragraph.lines()[0];
    assert_eq!(line.drawn_runs().count(), line.runs.len());
}

/// A blank line takes the metrics of its hard break's run; a line with text leaves its break's
/// out (`TextStretch::extend` ignores a hard break's metrics, `TextStretch::startFrom` takes
/// the run a line starts in).
#[test]
fn a_hard_break_sizes_only_a_line_it_is_alone_on() {
    let mut fonts = mono_fonts();
    let mut builder = ParagraphBuilder::new(&mut fonts);
    builder.add_text("Big", &mono_style(40.0));
    builder.add_text("\n\nsmall", &mono_style(10.0));
    let mut paragraph = builder.build();
    paragraph.layout(f32::INFINITY);
    let height = |line: &valo_paragraph::Line| line.ascent + line.descent;
    let lines = paragraph.lines();
    let small = height(&lines[2]);
    assert!(height(&lines[0]) > 3.0 * small, "the text sizes its line");
    assert_close(height(&lines[1]), small);
}

/// Preserving trailing whitespace makes it part of what the line draws and measures, but never
/// its hard break: no missing-glyph box, no width.
#[test]
fn preserved_trailing_whitespace_stops_before_the_hard_break() {
    let preserve = ParagraphStyle {
        preserve_trailing_whitespace: true,
        ..Default::default()
    };
    let a = advance();
    for text in ["ab \ncd", "ab \u{2029}cd", "ab \u{0C}cd"] {
        let paragraph = mono_in(text, preserve.clone(), f32::INFINITY);
        let line = &paragraph.lines()[0];
        assert_close(line.width, 3.0 * a);
        assert_eq!(line.content_end, 3, "{text:?}");
        let drawn: Vec<usize> = line
            .drawn_runs()
            .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.cluster))
            .collect();
        assert_eq!(drawn, [0, 1, 2], "{text:?}");
        assert_close(
            paragraph.ink_bounds().unwrap().x,
            mono("ab").ink_bounds().unwrap().x,
        );
    }
}

/// Canvas2D callers read where the last preserved space's pen is (`last_glyph_origin`); the
/// ghost model leaves it where it was.
#[test]
fn preserved_trailing_spaces_keep_the_last_glyph_origin() {
    let preserve = ParagraphStyle {
        preserve_trailing_whitespace: true,
        ..Default::default()
    };
    let a = advance();
    let spaces = mono_in("   ", preserve.clone(), f32::INFINITY);
    assert_close(spaces.last_glyph_origin().unwrap(), 2.0 * a);
    assert_close(spaces.width(), 3.0 * a);
    let trailing = mono_in("ab ", preserve, f32::INFINITY);
    assert_close(trailing.last_glyph_origin().unwrap(), 2.0 * a);
}

/// The hard breaks are the characters skparagraph's ICU treats as hard, of the UAX #14 classes
/// LF and BK (`SkUnicode_icu::isHardLineBreak`): each ends its line.
#[test]
fn line_feed_and_the_mandatory_breaks_end_a_line() {
    for ch in ['\n', '\u{0B}', '\u{0C}', '\u{2028}', '\u{2029}'] {
        let text = format!("a{ch}b");
        let paragraph = mono(&text);
        let lines = paragraph.lines();
        assert_eq!(lines.len(), 2, "{text:?}");
        assert!(lines[0].ends_with_hard_break(), "{text:?}");
        assert_eq!(lines[0].text_end, 1, "{text:?}");
    }
}

/// A carriage return or a next-line character alone is no hard break in skparagraph: ICU's
/// line breaks are recorded as soft (`SkUnicode_icu::computeCodeUnitFlags`), so the text stays
/// on one line, and it may wrap there like at a space.
#[test]
fn carriage_return_and_next_line_only_allow_a_break() {
    let a = advance();
    for ch in ['\r', '\u{85}'] {
        let text = format!("aaa{ch}bbb");
        let unbounded = mono(&text);
        assert_eq!(unbounded.lines().len(), 1, "{text:?}");
        assert!(!unbounded.lines()[0].ends_with_hard_break());
        let placed: usize = unbounded.lines()[0]
            .runs
            .iter()
            .map(|run| run.glyphs.len())
            .sum();
        assert_eq!(placed, 7, "every character of {text:?} is on the line");
        let narrow = mono_in(&text, ParagraphStyle::default(), 5.0 * a);
        let ranges: Vec<_> = narrow
            .lines()
            .iter()
            .map(|line| line.range.clone())
            .collect();
        let after = 3 + ch.len_utf8();
        assert_eq!(ranges, [0..after, after..after + 3], "{text:?}");
        assert!(!narrow.lines()[0].ends_with_hard_break());
        assert_eq!(narrow.lines()[0].content_end, 3);
    }
}

/// "\r\n" is one hard break at its line feed: the carriage return before it is trailing
/// whitespace of the line's text, as skparagraph's `TextLine::text` ends at the line feed.
#[test]
fn a_carriage_return_line_feed_breaks_once_at_the_line_feed() {
    let paragraph = mono("ab\r\ncd");
    let lines = paragraph.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        (
            lines[0].range.clone(),
            lines[0].text_end,
            lines[0].content_end
        ),
        (0..4, 3, 2)
    );
    let end = 2.0 * advance();
    assert_close(paragraph.caret_for_offset(2).x, end);
}

/// A text ending in a carriage return opens no empty line, which only a hard break does.
#[test]
fn a_trailing_carriage_return_opens_no_line() {
    assert_eq!(mono("ab\r").lines().len(), 1);
    assert_eq!(mono("ab\u{2028}").lines().len(), 2);
}
