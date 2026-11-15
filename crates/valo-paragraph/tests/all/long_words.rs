//! A word wider than its line breaks between clusters, as skparagraph's `TextWrapper` breaks it
//! (`lookAhead` and `moveForward`), and a line cut for an ellipsis keeps the part of the next
//! word that fits. Line ranges are in characters of a monospace font, so every expected value is
//! skparagraph's walk over the same clusters.

use std::ops::Range;

use valo_geometry::Color;
use valo_paragraph::{Paragraph, ParagraphBuilder, ParagraphStyle, TextStyle};
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

/// `text` in the monospace font, laid out `characters` advances wide.
fn mono_in(text: &str, paragraph_style: ParagraphStyle, characters: f32) -> Paragraph {
    let mut fonts = mono_fonts();
    let mut builder = ParagraphBuilder::new(&mut fonts);
    builder.style(paragraph_style);
    builder.add_text(text, &mono_style(SIZE));
    let mut paragraph = builder.build();
    paragraph.layout(characters * advance());
    paragraph
}

/// The advance of one character of the monospace font at `SIZE`.
fn advance() -> f32 {
    let mut fonts = mono_fonts();
    let mut builder = ParagraphBuilder::new(&mut fonts);
    builder.add_text("X", &mono_style(SIZE));
    let mut paragraph = builder.build();
    paragraph.layout(f32::INFINITY);
    paragraph.max_intrinsic_width()
}

fn ranges(paragraph: &Paragraph) -> Vec<Range<usize>> {
    paragraph
        .lines()
        .iter()
        .map(|line| line.range.clone())
        .collect()
}

/// A word too long for any line breaks after the clusters that fit, line after line. On a line
/// that holds words before it, it breaks there only when more than one glyph of it fits
/// (`fClusters.endPos() - fClusters.startPos() > 1`), else it starts the next line.
#[test]
fn a_word_wider_than_its_line_breaks_between_clusters() {
    let rows: [(&str, f32, &[Range<usize>]); 4] = [
        ("eleven", 4.0, &[0..4, 4..6]),
        ("a eleven", 4.0, &[0..4, 4..8]),
        ("aa eleven", 4.0, &[0..3, 3..7, 7..9]),
        ("eleven eleven", 3.0, &[0..3, 3..7, 7..10, 10..13]),
    ];
    for (text, characters, expected) in rows {
        let paragraph = mono_in(text, ParagraphStyle::default(), characters);
        assert_eq!(ranges(&paragraph), expected, "{text:?} in {characters}");
        for line in paragraph.lines() {
            assert!(line.width <= characters * advance() + 0.01, "{text:?} fits");
        }
    }
}

/// skparagraph measures whether a word is too long only up to the cluster a break follows
/// directly, which its walk stops before (`Cluster::isSoftBreak`): "eleven" at the text's end is
/// measured as "eleve", which fits five characters, so it starts the next line and only then
/// breaks; followed by a space it is measured whole and breaks on the first line.
#[test]
fn a_word_is_too_long_by_its_width_before_its_last_cluster_when_a_break_follows_it() {
    let at_the_end = mono_in("a eleven", ParagraphStyle::default(), 5.0);
    assert_eq!(ranges(&at_the_end), [0..2, 2..7, 7..8]);
    let before_a_space = mono_in("a eleven ", ParagraphStyle::default(), 5.0);
    assert_eq!(ranges(&before_a_space), [0..5, 5..9]);
}

/// A cluster wider than any line goes on the line it overflows, after the words before it
/// (`fTooLongCluster`), the space after it riding that line as a ghost
/// (`TextWrapper::trimStartSpaces`), and a line it starts holds it alone.
#[test]
fn a_cluster_wider_than_any_line_stays_on_the_line_it_overflows() {
    let mut fonts = mono_fonts();
    let mut builder = ParagraphBuilder::new(&mut fonts);
    builder.add_text("a ", &mono_style(SIZE));
    builder.add_text("W", &mono_style(4.0 * SIZE));
    builder.add_text(" b", &mono_style(SIZE));
    let mut paragraph = builder.build();
    paragraph.layout(2.0 * advance());
    assert_eq!(ranges(&paragraph), [0..4, 4..5]);
    assert_eq!(paragraph.lines()[0].content_end, 3);

    for characters in [0.5, 0.0, -1.0] {
        let narrow = mono_in("ab", ParagraphStyle::default(), characters);
        assert_eq!(ranges(&narrow), [0..1, 1..2], "{characters}");
    }
}

/// The line `max_lines` keeps last takes the part of the next word that fits when it ends with
/// an ellipsis (`moveForward(hasEllipsis)`), and the ellipsis then takes clusters off its end
/// until it fits (`TextLine::createEllipsis`); without an ellipsis the line ends before the word.
#[test]
fn an_ellipsized_line_keeps_the_part_of_the_next_word_that_fits() {
    let ellipsized = ParagraphStyle {
        max_lines: Some(1),
        ellipsis: Some("\u{2026}".to_owned()),
        ..Default::default()
    };
    let without_max_lines = ParagraphStyle {
        ellipsis: Some("\u{2026}".to_owned()),
        ..Default::default()
    };
    for style in [ellipsized, without_max_lines] {
        let paragraph = mono_in("aaa bbbbbb", style, 6.0);
        assert!(paragraph.truncated());
        let line = &paragraph.lines()[0];
        assert_eq!(paragraph.lines().len(), 1);
        assert_eq!(line.content_end, 5, "\"aaa b\" and the ellipsis");
        assert!(line.width <= 6.0 * advance() + 0.01);
    }

    let one_line = ParagraphStyle {
        max_lines: Some(1),
        ..Default::default()
    };
    let cut = mono_in("aaa bbbbbb", one_line, 6.0);
    assert_eq!(ranges(&cut), vec![0..4]);
}

/// A word too long for a line breaks on the line `max_lines` keeps last as on any other.
#[test]
fn max_lines_cuts_a_broken_word() {
    let style = ParagraphStyle {
        max_lines: Some(1),
        ..Default::default()
    };
    let paragraph = mono_in("eleven", style, 4.0);
    assert!(paragraph.truncated());
    assert_eq!(ranges(&paragraph), vec![0..4]);
}

/// The ellipsis follows the line's trailing whitespace where it fits after it: skparagraph
/// restores the line's break before ellipsizing, "to preserve a space before the ellipsis".
#[test]
fn an_ellipsis_keeps_the_whitespace_before_it_that_it_fits_after() {
    let style = ParagraphStyle {
        max_lines: Some(1),
        ellipsis: Some("\u{2026}".to_owned()),
        ..Default::default()
    };
    let paragraph = mono_in("aaa  bbb", style, 5.5);
    let line = &paragraph.lines()[0];
    assert_eq!(line.content_end, 4, "\"aaa \" and the ellipsis");
    assert!(line.width <= 5.5 * advance() + 0.01);
}

/// skparagraph's `SkParagraph_LongWordParagraph`, in Fira Sans for its Roboto: "A " and a word
/// longer than three lines take four lines, the first three filled to near the width.
#[test]
fn skparagraph_long_word_paragraph() {
    const TEXT: &str = "A veryverylongwordtoseewherethiswillwraporifitwillatallandifitdoesthenthatwouldbeagoodthingbecausethebreakingisworking.";
    const WIDTH: f32 = 500.0;
    let mut fonts = FontCollection::new();
    fonts
        .register("Fira Sans", super::asset("fira_sans.ttf"))
        .unwrap();
    let mut builder = ParagraphBuilder::new(&mut fonts);
    builder.add_text(TEXT, &TextStyle::new("Fira Sans", 31.0, Color::BLACK));
    let mut paragraph = builder.build();
    paragraph.layout(WIDTH);
    let lines = paragraph.lines();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].range.end > 2, "the word starts on the first line");
    for line in &lines[..3] {
        assert!(
            line.width > WIDTH - 20.0 && line.width <= WIDTH,
            "{}",
            line.width
        );
    }
}
