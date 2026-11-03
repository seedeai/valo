//! A span's paints, recorded as skparagraph paints them: each line's
//! backgrounds, then its shadows, then its glyphs, then its decorations
//! (`TextLine::paint`); a foreground paint in place of the color
//! (`TextLine::buildTextBlob`); and a background over Skia's run box. The
//! checks read the recorded operations, as Flutter's `PainterTest`s read the
//! display list a paragraph paints into.

use valo_dl::{DisplayListBuilder, Op, Paint, PaintStyle};
use valo_geometry::{Color, Point, Rect, Stroke};
use valo_paragraph::{
    Decoration, DecorationKind, DrawGlyphRunExt, DrawParagraphExt, Paragraph, ParagraphBuilder,
    ParagraphStyle, PlaceholderAlignment, PlaceholderStyle, Shadow, TextBaseline, TextStyle,
};
use valo_text::FontCollection;

const RED: Color = Color::rgb(1.0, 0.0, 0.0);
const GREEN: Color = Color::rgb(0.0, 1.0, 0.0);
const BLUE: Color = Color::rgb(0.0, 0.0, 1.0);
const GRAY: Color = Color::rgb(0.5, 0.5, 0.5);
const YELLOW: Color = Color::rgb(1.0, 1.0, 0.0);

fn fonts() -> FontCollection {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut fonts = FontCollection::new();
    let latin = fonts
        .register(
            "Fira Sans",
            std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap(),
        )
        .unwrap();
    fonts.add_fallback(latin);
    fonts
}

fn style(color: Color) -> TextStyle {
    TextStyle::new("Fira Sans", 20.0, color)
}

fn with_background(style: TextStyle, background: Color) -> TextStyle {
    TextStyle {
        background: Some(Paint::from_color(background)),
        ..style
    }
}

/// `spans` laid out in one paragraph, unwrapped.
fn laid_out(spans: &[(&str, TextStyle)]) -> Paragraph {
    laid_out_in(spans, ParagraphStyle::default(), f32::INFINITY)
}

fn laid_out_in(spans: &[(&str, TextStyle)], style: ParagraphStyle, width: f32) -> Paragraph {
    let mut fonts = fonts();
    let mut builder = ParagraphBuilder::new(&mut fonts);
    builder.style(style);
    for (text, style) in spans {
        builder.add_text(text, style);
    }
    let mut paragraph = builder.build();
    paragraph.layout(width);
    paragraph
}

/// `Drawn` is one recorded draw a paragraph made, without the save, restore
/// and transform that place a shadow.
#[derive(Debug, PartialEq)]
enum Drawn {
    Rect(Rect, Paint),
    Glyphs(Paint),
}

impl Drawn {
    fn color(&self) -> Color {
        match self {
            Drawn::Rect(_, paint) | Drawn::Glyphs(paint) => paint.color,
        }
    }
}

fn drawn(record: impl FnOnce(&mut DisplayListBuilder)) -> Vec<Drawn> {
    let mut builder = DisplayListBuilder::new();
    record(&mut builder);
    builder
        .build()
        .ops()
        .iter()
        .filter_map(|op| match op {
            Op::DrawRect { rect, paint, .. } => Some(Drawn::Rect(*rect, paint.clone())),
            Op::GlyphRun { paint, .. } => Some(Drawn::Glyphs(paint.clone())),
            _ => None,
        })
        .collect()
}

fn drawn_paragraph(paragraph: &Paragraph) -> Vec<Drawn> {
    drawn(|builder| builder.draw_paragraph(paragraph, Point::new(0.0, 0.0)))
}

fn rects(drawn: &[Drawn]) -> Vec<Rect> {
    drawn
        .iter()
        .filter_map(|draw| match draw {
            Drawn::Rect(rect, _) => Some(*rect),
            Drawn::Glyphs(_) => None,
        })
        .collect()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// Skia paints a line's backgrounds, then its shadows, then its text, then
/// its decorations (`TextLine::paint`), so neither a background nor a shadow
/// of the second span covers the first span's glyphs.
#[test]
fn a_line_paints_backgrounds_then_shadows_then_text_then_decorations() {
    let shadowed = |color: Color, background: Color| TextStyle {
        shadows: vec![Shadow {
            color: GRAY,
            offset: Point::new(1.0, 1.0),
            blur: 0.0,
        }],
        decoration: Some(Decoration::new(DecorationKind::Underline)),
        ..with_background(style(color), background)
    };
    let paragraph = laid_out(&[("ab", shadowed(RED, YELLOW)), ("cd", shadowed(GREEN, BLUE))]);

    let order: Vec<(&str, Color)> = drawn_paragraph(&paragraph)
        .iter()
        .map(|draw| match draw {
            Drawn::Rect(..) => ("rect", draw.color()),
            Drawn::Glyphs(_) => ("glyphs", draw.color()),
        })
        .collect();
    assert_eq!(
        order,
        [
            ("rect", YELLOW),
            ("rect", BLUE),
            ("glyphs", GRAY),
            ("glyphs", GRAY),
            ("glyphs", RED),
            ("glyphs", GREEN),
            ("rect", RED),
            ("rect", GREEN),
        ]
    );
}

/// A foreground paint draws the glyphs in place of the color, a stroke as
/// one glyph run with its stroke (Flutter's `DrawStrokedTextImpeller`), while
/// a decoration without its own color still takes the color
/// (`Decorations::calculatePaint`).
#[test]
fn a_foreground_paint_draws_the_glyphs_in_place_of_the_color() {
    let outline = Paint {
        style: PaintStyle::Stroke(Stroke::new(1.5)),
        ..Paint::from_color(BLUE)
    };
    let paragraph = laid_out(&[(
        "Outlined",
        TextStyle {
            foreground: Some(outline.clone()),
            decoration: Some(Decoration::new(DecorationKind::Underline)),
            ..style(RED)
        },
    )]);

    let drawn = drawn_paragraph(&paragraph);
    let glyph_paints: Vec<&Paint> = drawn
        .iter()
        .filter_map(|draw| match draw {
            Drawn::Glyphs(paint) => Some(paint),
            Drawn::Rect(..) => None,
        })
        .collect();
    assert_eq!(glyph_paints, [&outline]);
    let underline = drawn.last().unwrap();
    assert!(matches!(underline, Drawn::Rect(..)));
    assert_eq!(underline.color(), RED);
}

/// Without a foreground the glyphs keep the span's color, as they did before
/// spans had paints, and no background is drawn.
#[test]
fn a_span_without_paints_draws_only_its_colored_glyphs() {
    let paragraph = laid_out(&[("plain", style(RED))]);
    assert_eq!(
        drawn_paragraph(&paragraph),
        [Drawn::Glyphs(Paint::from_color(RED))]
    );
}

/// Each span's background box starts where the one before it ends, from the
/// line's start to its end (Skia's `SkParagraph_MultiStyle_Logo` checks the
/// same of its clip boxes).
#[test]
fn background_boxes_abut_along_the_line() {
    let paragraph = laid_out(&[
        ("google_", with_background(style(RED), GRAY)),
        ("logo", with_background(style(BLUE), YELLOW)),
        ("go", with_background(style(BLUE), GREEN)),
        ("ogle", with_background(style(RED), BLUE)),
    ]);

    let boxes = rects(&drawn_paragraph(&paragraph));
    assert_eq!(boxes.len(), 4);
    assert_eq!(boxes[0].x, 0.0);
    for pair in boxes.windows(2) {
        assert!(near(pair[1].x, pair[0].right()), "{pair:?}");
    }
    let line = &paragraph.lines()[0];
    assert!(near(boxes[3].right(), line.left + line.width));
}

/// Spans Skia shapes as one run, with equal backgrounds, fill one box across
/// a change of color, as skparagraph merges the style blocks whose
/// backgrounds match (`TextStyle::matchOneAttribute`).
#[test]
fn an_equal_background_across_a_color_change_fills_one_box() {
    let paragraph = laid_out(&[
        ("Two ", with_background(style(RED), GRAY)),
        ("colors", with_background(style(BLUE), GRAY)),
    ]);

    let drawn = drawn_paragraph(&paragraph);
    let boxes = rects(&drawn);
    assert_eq!(boxes.len(), 1);
    let line = &paragraph.lines()[0];
    assert_eq!(boxes[0].x, 0.0);
    assert!(near(boxes[0].right(), line.left + line.width));
    let glyph_colors: Vec<Color> = drawn
        .iter()
        .filter(|draw| matches!(draw, Drawn::Glyphs(_)))
        .map(Drawn::color)
        .collect();
    assert_eq!(glyph_colors, [RED, BLUE]);
}

/// The box spans the font's ascent and descent, and a height multiplier
/// scales it to `height × size`, as Skia's run box does
/// (`Run::calculateMetrics`).
#[test]
fn the_background_box_follows_the_height_multiplier() {
    let plain = laid_out(&[("Box", with_background(style(RED), GRAY))]);
    let plain_box = rects(&drawn_paragraph(&plain))[0];
    let run = &plain.lines()[0].runs[0];
    assert!(near(plain_box.y, run.bounds.y));
    assert!(near(plain_box.height, run.bounds.height));

    let tall = laid_out(&[(
        "Box",
        TextStyle {
            height: Some(2.0),
            ..with_background(style(RED), GRAY)
        },
    )]);
    let tall_box = rects(&drawn_paragraph(&tall))[0];
    assert!(near(tall_box.height, 40.0), "{tall_box:?}");
    assert!(near(tall_box.y, 0.0), "the line's ascent holds the box");
    assert!(near(tall_box.bottom(), tall.height()));
}

/// A placeholder keeps its style's background (Skia's
/// `TextStyle::cloneForPlaceholder`), which fills the placeholder's box; one
/// of no height draws nothing (`SkParagraph_PlaceholderOnly`'s case).
#[test]
fn a_placeholder_fills_its_box_with_its_styles_background() {
    let placeholder = |height: f32| PlaceholderStyle {
        width: 30.0,
        height,
        alignment: PlaceholderAlignment::Baseline,
        baseline: TextBaseline::Alphabetic,
        baseline_offset: height,
    };
    for (height, expected_boxes) in [(12.0, 1), (0.0, 0)] {
        let mut fonts = fonts();
        let mut builder = ParagraphBuilder::new(&mut fonts);
        builder.add_placeholder(placeholder(height), &with_background(style(RED), GRAY));
        let mut paragraph = builder.build();
        paragraph.layout(f32::INFINITY);

        let drawn = drawn_paragraph(&paragraph);
        let boxes = rects(&drawn);
        assert_eq!(boxes.len(), expected_boxes, "height {height}");
        if expected_boxes == 1 {
            assert_eq!(boxes[0], paragraph.placeholder_boxes()[0].rect);
        }
        assert!(!drawn.iter().any(|draw| matches!(draw, Drawn::Glyphs(_))));
    }
}

/// The ellipsis takes the style of the text it ends (Skia's
/// `TextLine::shapeEllipsis`): its glyphs take the foreground, and its
/// background continues the box of that text.
#[test]
fn the_ellipsis_takes_the_paints_of_the_text_it_ends() {
    let outline = Paint {
        style: PaintStyle::Stroke(Stroke::new(1.0)),
        ..Paint::from_color(BLUE)
    };
    let span = TextStyle {
        foreground: Some(outline.clone()),
        ..with_background(style(RED), GRAY)
    };
    let paragraph = laid_out_in(
        &[("A sentence far too long for its box", span)],
        ParagraphStyle {
            max_lines: Some(1),
            ellipsis: Some("…".to_owned()),
            ..ParagraphStyle::default()
        },
        120.0,
    );

    let line = &paragraph.lines()[0];
    let ellipsis = line.runs.last().unwrap();
    let drawn = drawn_paragraph(&paragraph);
    let boxes = rects(&drawn);
    assert_eq!(boxes.len(), 1);
    assert!(near(boxes[0].right(), ellipsis.bounds.right()));
    assert!(drawn
        .iter()
        .filter(|draw| matches!(draw, Drawn::Glyphs(_)))
        .all(|draw| *draw == Drawn::Glyphs(outline.clone())));
}

/// One glyph paint for the whole paragraph replaces the spans' foregrounds
/// and colors, and leaves their backgrounds.
#[test]
fn one_glyph_paint_keeps_the_spans_backgrounds() {
    let paragraph = laid_out(&[(
        "Gradient",
        TextStyle {
            foreground: Some(Paint::from_color(BLUE)),
            ..with_background(style(RED), GRAY)
        },
    )]);
    let replacement = Paint::from_color(GREEN);

    let drawn = drawn(|builder| {
        builder.draw_paragraph_with(&paragraph, Point::new(0.0, 0.0), &replacement)
    });
    assert_eq!(drawn.len(), 2);
    assert_eq!(drawn[0].color(), GRAY);
    assert_eq!(drawn[1], Drawn::Glyphs(replacement));
}
