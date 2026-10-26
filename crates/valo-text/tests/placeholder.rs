//! Placeholders: boxes laid out inline with text, as Skia's `addPlaceholder`.

use valo_geometry::Color;
use valo_text::{
    FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, PlaceholderAlignment,
    PlaceholderStyle, TextBaseline, TextDirection, TextStyle,
};

fn fonts() -> FontCollection {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut c = FontCollection::new();
    let latin = c
        .register(
            "Fira Sans",
            std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap(),
        )
        .unwrap();
    let hebrew = c
        .register(
            "Noto Sans Hebrew",
            std::fs::read(format!("{dir}/noto_sans_hebrew.ttf")).unwrap(),
        )
        .unwrap();
    c.add_fallback(latin);
    c.add_fallback(hebrew);
    c
}

fn style() -> TextStyle {
    TextStyle::new("Fira Sans", 20.0, Color::BLACK)
}

fn placeholder(width: f32, height: f32, alignment: PlaceholderAlignment) -> PlaceholderStyle {
    PlaceholderStyle {
        width,
        height,
        alignment,
        baseline: TextBaseline::Alphabetic,
        baseline_offset: 0.0,
    }
}

/// "ab", the placeholder, then "cd", laid out unbounded.
fn between_text(placeholder: PlaceholderStyle) -> Paragraph {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_text("ab", &style());
    b.add_placeholder(placeholder, &style());
    b.add_text("cd", &style());
    let mut p = b.build();
    p.layout(f32::INFINITY);
    p
}

/// The text's own baseline, ascent and descent, from the same text without a placeholder.
fn text_metrics() -> (f32, f32, f32) {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_text("abcd", &style());
    let mut p = b.build();
    p.layout(f32::INFINITY);
    let line = &p.lines()[0];
    (line.baseline, line.ascent, line.descent)
}

#[test]
fn a_placeholder_takes_its_width_between_the_text_and_draws_nothing() {
    let p = between_text(placeholder(30.0, 10.0, PlaceholderAlignment::AboveBaseline));
    assert_eq!(p.text(), "ab\u{FFFC}cd");
    let boxes = p.placeholder_boxes();
    assert_eq!(boxes.len(), 1);
    let rect = boxes[0].rect;
    assert!((rect.width - 30.0).abs() < 1e-3);
    assert_eq!(boxes[0].direction, TextDirection::Ltr);
    let runs = &p.lines()[0].runs;
    let text_after = runs
        .iter()
        .find(|run| run.placeholder.is_none() && run.glyphs[0].cluster > 2)
        .expect("the text after the placeholder");
    assert!(
        (text_after.glyphs[0].x - (rect.x + rect.width)).abs() < 1e-3,
        "the text resumes after the box"
    );
    let run = runs.iter().find(|run| run.placeholder == Some(0)).unwrap();
    assert_eq!(
        run.glyphs.len(),
        1,
        "one cluster, for carets and hit testing"
    );
}

#[test]
fn each_alignment_places_the_box_as_skia_does() {
    let (baseline, ascent, descent) = text_metrics();
    let at =
        |alignment| between_text(placeholder(10.0, 8.0, alignment)).placeholder_boxes()[0].rect;
    let above = at(PlaceholderAlignment::AboveBaseline);
    assert_eq!(
        above.y + above.height,
        baseline,
        "above: bottom on the baseline"
    );
    let below = at(PlaceholderAlignment::BelowBaseline);
    assert_eq!(below.y, baseline, "below: top on the baseline");
    let top = at(PlaceholderAlignment::Top);
    assert!(
        (top.y - (baseline - ascent)).abs() < 1e-3,
        "top: top at the font's ascent"
    );
    let bottom = at(PlaceholderAlignment::Bottom);
    assert!(
        (bottom.y + bottom.height - (baseline + descent)).abs() < 1e-3,
        "bottom: bottom at the font's descent"
    );
    let middle = at(PlaceholderAlignment::Middle);
    let font_middle = baseline - (ascent - descent) / 2.0;
    assert!((middle.y + middle.height / 2.0 - font_middle).abs() < 1e-3);
    let mut on_baseline = placeholder(10.0, 8.0, PlaceholderAlignment::Baseline);
    on_baseline.baseline_offset = 6.0;
    let rect = between_text(on_baseline).placeholder_boxes()[0].rect;
    assert_eq!(
        rect.y + 6.0,
        baseline,
        "baseline: the offset point on the baseline"
    );
}

#[test]
fn a_tall_placeholder_makes_its_line_tall_enough() {
    let (_, ascent, descent) = text_metrics();
    let p = between_text(placeholder(
        10.0,
        100.0,
        PlaceholderAlignment::AboveBaseline,
    ));
    let line = &p.lines()[0];
    assert_eq!(line.ascent, 100.0, "the box rises 100 above the baseline");
    assert_eq!(line.descent, descent);
    assert!(p.height() >= 100.0 + descent && ascent < 100.0);
    let rect = p.placeholder_boxes()[0].rect;
    assert!(rect.y >= 0.0, "the box is inside the paragraph");
}

#[test]
fn a_line_of_only_a_placeholder_is_its_size() {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_placeholder(
        placeholder(40.0, 24.0, PlaceholderAlignment::AboveBaseline),
        &style(),
    );
    let mut p = b.build();
    p.layout(f32::INFINITY);
    assert_eq!(p.height(), 24.0);
    assert_eq!(p.placeholder_boxes()[0].rect.y, 0.0);
}

#[test]
fn placeholders_wrap_like_words_and_report_in_the_order_added() {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_placeholder(
        placeholder(60.0, 10.0, PlaceholderAlignment::AboveBaseline),
        &style(),
    );
    b.add_placeholder(
        placeholder(60.0, 10.0, PlaceholderAlignment::AboveBaseline),
        &style(),
    );
    let mut p = b.build();
    p.layout(100.0);
    assert_eq!(p.lines().len(), 2, "the second box wraps to the next line");
    let boxes = p.placeholder_boxes();
    assert_eq!(boxes.len(), 2);
    assert!(boxes[1].rect.y > boxes[0].rect.y);
    assert_eq!(boxes[1].rect.x, 0.0);
}

#[test]
fn a_placeholder_in_right_to_left_text_runs_right_to_left() {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.style(ParagraphStyle {
        direction: Some(TextDirection::Rtl),
        ..Default::default()
    });
    let hebrew = TextStyle::new("Noto Sans Hebrew", 20.0, Color::BLACK);
    b.add_text("שלום", &hebrew);
    b.add_placeholder(
        placeholder(20.0, 10.0, PlaceholderAlignment::AboveBaseline),
        &hebrew,
    );
    b.add_text("עולם", &hebrew);
    let mut p = b.build();
    p.layout(f32::INFINITY);
    let boxes = p.placeholder_boxes();
    assert_eq!(boxes[0].direction, TextDirection::Rtl);
    let first_word = p.lines()[0]
        .runs
        .iter()
        .find(|run| run.placeholder.is_none() && run.glyphs.iter().any(|g| g.cluster == 0))
        .unwrap();
    assert!(
        boxes[0].rect.x + boxes[0].rect.width <= first_word.bounds.x + 1e-3,
        "the box is left of the first word"
    );
}

#[test]
fn a_placeholder_on_the_ideographic_baseline_sits_half_the_descent_lower() {
    let (_, _, descent) = text_metrics();
    let alphabetic = between_text(placeholder(10.0, 8.0, PlaceholderAlignment::AboveBaseline));
    let mut ideographic = placeholder(10.0, 8.0, PlaceholderAlignment::AboveBaseline);
    ideographic.baseline = TextBaseline::Ideographic;
    let ideographic = between_text(ideographic);
    let shift =
        ideographic.placeholder_boxes()[0].rect.y - alphabetic.placeholder_boxes()[0].rect.y;
    assert!((shift - descent / 2.0).abs() < 1e-3);
}
