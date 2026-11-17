//! Skparagraph's strut, ported from its own tests in `SkParagraphTest`:
//! `SkParagraph_GetRectsForRangeStrut`, `...StrutWithHeight`,
//! `...StrutWithHeightAndHalfLeading`, `...StrutFallback`,
//! `SkParagraph_StrutHalfLeadingSimple` and `...Multiline`. Skia's tests use
//! Roboto; these use Fira Sans, whose ascent and descent are 0.935 and 0.265 of
//! its size with no line gap, as Roboto has none, so the heights and leading
//! the tests name come out the same.

use valo_geometry::Color;
use valo_paragraph::{Paragraph, ParagraphBuilder, ParagraphStyle, StrutStyle, TextStyle};
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

/// `strut` is a Fira Sans strut at `size`, enabled, with nothing else set.
fn strut(size: f32) -> StrutStyle {
    StrutStyle {
        enabled: true,
        families: vec!["Fira Sans".to_owned()],
        size,
        ..StrutStyle::default()
    }
}

/// `laid_out` is `text` in `text_style` under `paragraph_style` at the 550 px
/// width Skia's tests lay out at.
fn laid_out(
    collection: &mut FontCollection,
    text: &str,
    text_style: &TextStyle,
    paragraph_style: ParagraphStyle,
) -> Paragraph {
    let mut builder = ParagraphBuilder::new(collection);
    builder.style(paragraph_style);
    builder.add_text(text, text_style);
    let mut paragraph = builder.build();
    paragraph.layout(550.0);
    paragraph
}

fn strut_paragraph(strut_style: StrutStyle) -> ParagraphStyle {
    ParagraphStyle {
        strut_style,
        ..ParagraphStyle::default()
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

/// The font a paragraph resolved, for the metrics a strut is measured from.
fn font(paragraph: &Paragraph) -> &Font {
    paragraph.primary_font().expect("the text has a font").0
}

/// Skparagraph's `SkParagraph_GetRectsForRangeStrutWithHeight`: a 14 px strut
/// with a height of 2 and a leading of 3 hands a 10 px text's line 28 px of
/// strut, from 3 × 14 / 2 = 21 px below the line's top, and the line itself
/// 28 + 42 = 70 px tall.
#[test]
fn a_strut_height_and_leading_set_the_lines_room() {
    let mut collection = fonts();
    let paragraph = laid_out(
        &mut collection,
        "A",
        &style(10.0),
        strut_paragraph(StrutStyle {
            height: 2.0,
            height_override: true,
            leading: Some(3.0),
            ..strut(14.0)
        }),
    );
    let line = &paragraph.lines()[0];
    let font = font(&paragraph);
    let (ascent, descent) = (font.ascent_px(14.0), font.descent_px(14.0));
    // Fira Sans has no line gap, so the strut's height scales its ascent and
    // descent by exactly 28 / 16.8.
    let multiplier = 28.0 / (ascent + descent);
    assert!(near(ascent * multiplier + descent * multiplier, 28.0));
    assert!(
        near(line.ascent - ascent * multiplier, 21.0),
        "{}",
        line.ascent
    );
    assert!(near(line.ascent + line.descent, 70.0));
}

/// Skparagraph's `SkParagraph_GetRectsForRangeStrutWithHeightAndHalfLeading`:
/// half leading gives the same 21 px and 28 px for the strut box, only sharing
/// the height's extra room evenly instead of by the font's ratio.
#[test]
fn a_half_leading_strut_keeps_the_same_height_and_top() {
    let mut collection = fonts();
    let paragraph = laid_out(
        &mut collection,
        "A",
        &style(10.0),
        strut_paragraph(StrutStyle {
            height: 2.0,
            height_override: true,
            leading: Some(3.0),
            half_leading: true,
            ..strut(14.0)
        }),
    );
    let line = &paragraph.lines()[0];
    let font = font(&paragraph);
    let (ascent, descent) = (font.ascent_px(14.0), font.descent_px(14.0));
    let flexible = (28.0 - (ascent + descent)) / 2.0;
    assert!(
        near(line.ascent - (ascent + flexible), 21.0),
        "{}",
        line.ascent
    );
    assert!(near(line.ascent + line.descent, 70.0));
}

/// Skparagraph's `SkParagraph_StrutHalfLeadingSimple`: a 100 px text under a
/// 2 × 100 px forced strut with half leading is a 200 px line, the room shared
/// evenly over and under the text, and the text stays where it was across.
#[test]
fn a_forced_half_leading_strut_centres_a_single_line_on_its_text() {
    let mut collection = fonts();
    let paragraph = laid_out(
        &mut collection,
        "A",
        &TextStyle {
            height: Some(2.0),
            ..style(100.0)
        },
        strut_paragraph(StrutStyle {
            height: 2.0,
            height_override: true,
            half_leading: true,
            force_height: true,
            ..strut(100.0)
        }),
    );
    assert_eq!(paragraph.lines().len(), 1);
    let line = &paragraph.lines()[0];
    assert!(near(line.ascent + line.descent, 200.0));
    let text = &line.runs[0].bounds;
    let line_top = line.baseline - line.ascent;
    let line_bottom = line.baseline + line.descent;
    assert!(near(line_bottom - text.bottom(), text.y - line_top));
    assert!(near(line_top, 0.0));
    assert!(near(text.x, 0.0), "half leading does not move text across");
}

/// Skparagraph's `SkParagraph_StrutHalfLeadingMultiline`: every line of a
/// three-line paragraph under a forced 3 × 20 px strut is 60 px tall, the
/// spacing between the lines' text is uniform, and each line's room is shared
/// evenly over and under its text.
#[test]
fn a_forced_half_leading_strut_spaces_every_line_alike() {
    let mut collection = fonts();
    let paragraph = laid_out(
        &mut collection,
        "abcd\nefgh\nijkl",
        &TextStyle {
            height: Some(3.0),
            ..style(20.0)
        },
        strut_paragraph(StrutStyle {
            height: 3.0,
            height_override: true,
            half_leading: true,
            force_height: true,
            ..strut(20.0)
        }),
    );
    let lines = paragraph.lines();
    assert_eq!(lines.len(), 3);
    for line in lines {
        assert!(near(line.ascent + line.descent, 60.0));
    }
    let boxes: Vec<_> = lines.iter().map(|line| &line.runs[0].bounds).collect();
    assert!(near(
        boxes[1].y - boxes[0].bottom(),
        boxes[2].y - boxes[1].bottom()
    ));
    // Baselines are one line apart, so the text boxes are a line apart less
    // the room they take themselves.
    let line_advance = 60.0;
    let text_height = boxes[0].height;
    assert!(near(
        boxes[1].y - boxes[0].bottom(),
        line_advance - text_height
    ));
    assert!(near(lines[1].baseline - lines[0].baseline, line_advance));
    for (line, text) in lines.iter().zip(&boxes) {
        let line_top = line.baseline - line.ascent;
        let line_bottom = line.baseline + line.descent;
        assert!(near(line_bottom - text.bottom(), text.y - line_top));
    }
    assert!(
        near(boxes[1].x, 0.0),
        "half leading does not move text across"
    );
}

/// Skparagraph's `SkParagraph_GetRectsForRangeStrut`: a 14 px strut under text
/// taller than it changes nothing, since a strut is a floor and not a
/// replacement unless forced.
#[test]
fn a_strut_never_shrinks_a_taller_line() {
    let mut collection = fonts();
    let plain = laid_out(
        &mut collection,
        "A",
        &style(20.0),
        ParagraphStyle::default(),
    );
    let strutted = laid_out(
        &mut collection,
        "A",
        &style(20.0),
        strut_paragraph(strut(14.0)),
    );
    let (plain, strutted) = (&plain.lines()[0], &strutted.lines()[0]);
    assert!(near(strutted.ascent, plain.ascent));
    assert!(near(strutted.descent, plain.descent));
    assert!(near(strutted.baseline, plain.baseline));
    assert!(near(
        strutted.ascent + strutted.descent,
        plain.ascent + plain.descent
    ));

    // Under shorter text the strut's own room shows, the font's line height
    // at 14 px, without a height override or a leading.
    let shorter = laid_out(
        &mut collection,
        "A",
        &style(10.0),
        strut_paragraph(strut(14.0)),
    );
    let line = &shorter.lines()[0];
    assert!(near(
        line.ascent + line.descent,
        font(&shorter).line_height_px(14.0)
    ));
}

/// A forced strut replaces a taller line's room: skparagraph's
/// `forceStrutHeight`, where text may overlap its neighbours.
#[test]
fn a_forced_strut_replaces_a_taller_line() {
    let mut collection = fonts();
    let forced = laid_out(
        &mut collection,
        "A",
        &style(30.0),
        strut_paragraph(StrutStyle {
            force_height: true,
            ..strut(14.0)
        }),
    );
    let line = &forced.lines()[0];
    // Fira Sans's line height at 14 px.
    assert!(
        near(line.ascent + line.descent, 16.8),
        "{}",
        line.ascent + line.descent
    );
    assert!(near(
        line.ascent + line.descent,
        font(&forced).line_height_px(14.0)
    ));
}

/// Skparagraph's `SkParagraph_GetRectsForRangeStrutFallback`: a strut that is
/// not enabled leaves the paragraph exactly as it was.
#[test]
fn a_disabled_strut_changes_nothing() {
    let mut collection = fonts();
    let plain = laid_out(
        &mut collection,
        "one\ntwo",
        &style(20.0),
        ParagraphStyle::default(),
    );
    let disabled = laid_out(
        &mut collection,
        "one\ntwo",
        &style(20.0),
        strut_paragraph(StrutStyle {
            enabled: false,
            height: 3.0,
            height_override: true,
            force_height: true,
            ..strut(40.0)
        }),
    );
    assert_eq!(plain.height(), disabled.height());
    for (plain, disabled) in plain.lines().iter().zip(disabled.lines()) {
        assert_eq!(plain.baseline, disabled.baseline);
        assert_eq!(plain.ascent, disabled.ascent);
        assert_eq!(plain.descent, disabled.descent);
    }
}

/// Skparagraph's empty-paragraph rule (`ParagraphImpl::layout`): no text and a
/// forced strut is exactly the strut's height, so a text field with nothing
/// typed is already the height of its first line.
#[test]
fn a_forced_strut_sets_an_empty_paragraphs_height() {
    let mut collection = fonts();
    let empty = laid_out(
        &mut collection,
        "",
        &style(10.0),
        strut_paragraph(StrutStyle {
            height: 2.0,
            height_override: true,
            half_leading: true,
            force_height: true,
            ..strut(20.0)
        }),
    );
    assert_eq!(empty.lines().len(), 1);
    assert!(near(empty.height(), 40.0), "{}", empty.height());
}

/// An empty line inside text takes the strut's room too, as every line does.
#[test]
fn a_forced_strut_sizes_an_empty_line_between_text() {
    let mut collection = fonts();
    let paragraph = laid_out(
        &mut collection,
        "one\n\ntwo",
        &style(10.0),
        strut_paragraph(StrutStyle {
            height: 2.0,
            height_override: true,
            half_leading: true,
            force_height: true,
            ..strut(20.0)
        }),
    );
    let lines = paragraph.lines();
    assert_eq!(lines.len(), 3);
    for line in lines {
        assert!(near(line.ascent + line.descent, 40.0));
    }
    assert!(near(paragraph.height(), 120.0));
}
