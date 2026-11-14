//! GPU-free pipeline proof: shaping, fallback, wrapping, bidi, ink, and
//! the retained tiers (build = shape once; layout = wrap; recolor = place).

use valo_geometry::Color;
use valo_paragraph::{
    LeadingDistribution, Paragraph, ParagraphBuilder, ParagraphStyle, TextAlign,
    TextHeightBehavior, TextStyle,
};
use valo_text::{Font, FontCollection};
mod valo {
    pub use valo_geometry::Point;
}

fn fonts() -> FontCollection {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut c = FontCollection::new();
    let latin = c
        .register(
            "Fira Sans",
            std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap(),
        )
        .unwrap();
    let arabic = c
        .register(
            "Noto Sans Arabic",
            std::fs::read(format!("{dir}/noto_sans_arabic.ttf")).unwrap(),
        )
        .unwrap();
    let hebrew = c
        .register(
            "Noto Sans Hebrew",
            std::fs::read(format!("{dir}/noto_sans_hebrew.ttf")).unwrap(),
        )
        .unwrap();
    c.add_fallback(latin);
    c.add_fallback(arabic);
    c.add_fallback(hebrew);
    c
}

fn style(size: f32) -> TextStyle {
    TextStyle::new("Fira Sans", size, Color::BLACK)
}

fn laid_out(c: &mut FontCollection, text: &str, width: f32) -> Paragraph {
    let mut b = ParagraphBuilder::new(c);
    b.add_text(text, &style(20.0));
    let mut p = b.build();
    p.layout(width);
    p
}

#[test]
fn shapes_and_places_latin() {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_text("Hello valo", &style(24.0));
    let mut p = b.build();
    p.layout(f32::INFINITY);
    assert_eq!(p.lines().len(), 1);
    let run = &p.lines()[0].runs[0];
    assert_eq!(run.glyphs.len(), 10);
    assert!(p.width() > 60.0 && p.height() > 20.0);
    let xs: Vec<f32> = run.glyphs.iter().map(|g| g.x).collect();
    assert!(xs.windows(2).all(|w| w[1] > w[0]));
}

#[test]
fn trailing_whitespace_can_contribute_to_the_line_advance() {
    let mut collection = fonts();
    let trimmed = laid_out(&mut collection, "Valo ", f32::INFINITY).width();

    let mut builder = ParagraphBuilder::new(&mut collection);
    builder.style(ParagraphStyle {
        preserve_trailing_whitespace: true,
        ..Default::default()
    });
    builder.add_text("Valo ", &style(20.0));
    let mut preserved = builder.build();
    preserved.layout(f32::INFINITY);

    assert!(preserved.width() > trimmed);
}

#[test]
fn wraps_greedily_and_respects_width() {
    let mut c = fonts();
    let p = laid_out(&mut c, "the quick brown fox jumps over the lazy dog", 180.0);
    assert!(p.lines().len() >= 3);
    for line in p.lines() {
        assert!(line.width <= 180.5, "line overflows: {}", line.width);
    }
    let p2 = laid_out(&mut c, "one\ntwo", f32::INFINITY);
    assert_eq!(p2.lines().len(), 2);
}

#[test]
fn relayout_rewraps_without_reshaping() {
    let mut c = fonts();
    let mut p = laid_out(&mut c, "the quick brown fox jumps over the lazy dog", 400.0);
    let wide = p.lines().len();
    p.layout(150.0);
    assert!(p.lines().len() > wide, "narrower width wraps more");
    p.layout(150.0); // cache hit; just must not change anything
    assert!(p.lines().len() > wide);
}

#[test]
fn update_color_is_a_repaint() {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_text("red ", &style(20.0));
    b.add_text(
        "blue",
        &TextStyle::new("Fira Sans", 20.0, Color::rgb(0.0, 0.0, 1.0)),
    );
    let mut p = b.build();
    p.layout(f32::INFINITY);
    let before: Vec<f32> = p.lines()[0]
        .runs
        .iter()
        .flat_map(|r| r.glyphs.iter())
        .map(|g| g.x)
        .collect();
    p.update_color(1, Color::rgb(0.0, 1.0, 0.0));
    let runs = &p.lines()[0].runs;
    assert_eq!(runs.last().unwrap().color, Color::rgb(0.0, 1.0, 0.0));
    let after: Vec<f32> = runs
        .iter()
        .flat_map(|r| r.glyphs.iter())
        .map(|g| g.x)
        .collect();
    assert_eq!(before, after, "recoloring never moves a glyph");
}

#[test]
fn max_lines_truncates_and_ellipsis_fits() {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.style(ParagraphStyle {
        max_lines: Some(2),
        ellipsis: Some("…".to_owned()),
        ..Default::default()
    });
    b.add_text(
        "the quick brown fox jumps over the lazy dog again and again",
        &style(20.0),
    );
    let mut p = b.build();
    p.layout(180.0);
    assert_eq!(p.lines().len(), 2);
    assert!(p.truncated());
    let last = &p.lines()[1];
    assert!(last.width <= 180.5, "ellipsis line fits: {}", last.width);
    // The ellipsis rides as its own run after the sliced content.
    assert!(last.runs.len() >= 2, "content + ellipsis runs");
    let rightmost = last
        .runs
        .iter()
        .map(|r| r.bounds.right())
        .fold(f32::MIN, f32::max);
    assert!(rightmost <= 180.5);
}

#[test]
fn justify_stretches_word_gaps() {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.style(ParagraphStyle {
        align: TextAlign::Justify,
        ..Default::default()
    });
    b.add_text("the quick brown fox jumps over the lazy dog", &style(20.0));
    let mut p = b.build();
    p.layout(200.0);
    assert!(p.lines().len() >= 2);
    // Every non-final line fills the width; the last stays ragged.
    for line in &p.lines()[..p.lines().len() - 1] {
        let right = line
            .drawn_runs()
            .map(|r| r.bounds.right())
            .fold(f32::MIN, f32::max);
        assert!(
            (right - 200.0).abs() < 1.0,
            "justified line ends at the edge: {right}"
        );
    }
    let last = p.lines().last().unwrap();
    let right = last
        .drawn_runs()
        .map(|r| r.bounds.right())
        .fold(f32::MIN, f32::max);
    assert!(right < 199.0, "last line is ragged: {right}");
}

#[test]
fn fallback_splits_runs_per_script() {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_text("AB سلام CD", &style(20.0));
    let mut p = b.build();
    p.layout(f32::INFINITY);
    let runs = &p.lines()[0].runs;
    assert!(runs.len() >= 3);
    let arabic = c.family("Noto Sans Arabic").unwrap();
    assert!(runs.iter().any(|r| r.font == arabic));
}

#[test]
fn rtl_paragraph_reorders_visually() {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_text(
        "שלום ABC עולם",
        &TextStyle::new("Noto Sans Hebrew", 20.0, Color::BLACK),
    );
    let mut p = b.build();
    p.layout(f32::INFINITY);
    let runs = &p.lines()[0].runs;
    assert!(runs.len() >= 3);
    let hebrew = c.family("Noto Sans Hebrew").unwrap();
    let rightmost = runs.iter().map(|r| r.bounds.x).fold(f32::MIN, f32::max);
    assert_eq!(
        runs.iter().find(|r| r.bounds.x == rightmost).unwrap().font,
        hebrew,
        "logical start of an RTL paragraph lands visually right"
    );
}

#[test]
fn align_shifts_lines() {
    let mut c = fonts();
    let mut left = ParagraphBuilder::new(&mut c);
    left.add_text("hi", &style(20.0));
    let mut l = left.build();
    l.layout(300.0);
    let mut right = ParagraphBuilder::new(&mut c);
    right.style(ParagraphStyle {
        align: TextAlign::Right,
        ..Default::default()
    });
    right.add_text("hi", &style(20.0));
    let mut r = right.build();
    r.layout(300.0);
    let lx = l.lines()[0].runs[0].glyphs[0].x;
    let rx = r.lines()[0].runs[0].glyphs[0].x;
    assert!(rx > lx + 200.0);
}

#[test]
fn color_emoji_contributes_to_paragraph_ink_bounds() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut fonts = FontCollection::new();
    fonts
        .register(
            "Noto Color Emoji",
            std::fs::read(format!("{dir}/noto_color_emoji_subset.ttf")).unwrap(),
        )
        .unwrap();
    let mut builder = ParagraphBuilder::new(&mut fonts);
    builder.add_text(
        "🚀",
        &TextStyle::new("Noto Color Emoji", 32.0, Color::WHITE),
    );
    let mut paragraph = builder.build();
    paragraph.layout(f32::INFINITY);
    let bounds = paragraph.ink_bounds().expect("color glyph has visible ink");
    assert!(bounds.width > 10.0 && bounds.height > 10.0);
}

// ── styles, intrinsics, editor surface ──────────────────────────────────────

#[test]
fn letter_spacing_widens_and_wraps_earlier() {
    let mut c = fonts();
    let plain = laid_out(&mut c, "space me out", f32::INFINITY);
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_text(
        "space me out",
        &TextStyle {
            letter_spacing: 3.0,
            ..style(20.0)
        },
    );
    let mut spaced = b.build();
    spaced.layout(f32::INFINITY);
    // 12 clusters × 3px.
    assert!((spaced.width() - plain.width() - 36.0).abs() < 0.5);
}

/// Spacing tighter than the glyphs are wide walks the pen backwards. `width`
/// is a layout box and has to keep its floor at zero, so the signed answer
/// needs an accessor of its own — Canvas2D's `TextMetrics.width` reports the
/// negative and Blink's `TextMetrics::Update` never clamps it.
#[test]
fn tight_letter_spacing_gives_a_negative_advance() {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_text(
        "ii",
        &TextStyle {
            letter_spacing: -4.0,
            ..style(8.0)
        },
    );
    let mut paragraph = b.build();
    paragraph.layout(f32::INFINITY);
    let advance = paragraph.advance();
    assert!(
        advance < -1.0,
        "two 8px `i`s at -4px spacing must end left of where they started, got {advance}"
    );
    assert_eq!(
        paragraph.width(),
        0.0,
        "the layout box keeps its floor at zero"
    );
}

/// The last glyph's pen is not the advance: it sits one glyph's advance short
/// of it. Text with no ink has nothing else to place a bounding box against.
#[test]
fn last_glyph_origin_trails_the_advance_by_one_glyph() {
    let mut c = fonts();
    let mut b = ParagraphBuilder::new(&mut c);
    // Canvas2D measures what it was handed, trailing spaces included.
    b.style(ParagraphStyle {
        preserve_trailing_whitespace: true,
        ..ParagraphStyle::default()
    })
    .add_text("   ", &style(24.0));
    let mut spaces = b.build();
    spaces.layout(f32::INFINITY);
    let advance = spaces.advance();
    let origin = spaces
        .last_glyph_origin()
        .expect("three spaces place three glyphs");
    assert!(advance > 0.0, "three spaces advance, got {advance}");
    assert!(
        (origin - advance * 2.0 / 3.0).abs() < 0.01,
        "the third of three equal glyphs starts two thirds along, got {origin} of {advance}"
    );

    let empty = laid_out(&mut c, "", f32::INFINITY);
    assert_eq!(empty.last_glyph_origin(), None, "nothing placed, no origin");
}

#[test]
fn height_multiplier_scales_lines() {
    let mut c = fonts();
    let plain = laid_out(&mut c, "one\ntwo", f32::INFINITY);
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_text(
        "one\ntwo",
        &TextStyle {
            height: Some(2.0),
            ..style(20.0)
        },
    );
    let mut tall = b.build();
    tall.layout(f32::INFINITY);
    assert!(
        (tall.height() - 80.0).abs() < 0.5,
        "2.0 × 20px × 2 lines: {}",
        tall.height()
    );
    assert!(tall.height() > plain.height() + 20.0);
}

/// An even leading puts half a line's extra height above the text and half
/// below, so the baseline lies where CSS puts it, half the leading below the
/// line's top plus the font's ascent: skparagraph's half leading
/// (`Run::calculateMetrics`). A proportional one shares it as the ascent and
/// descent do, and neither changes a line without a height multiplier.
#[test]
fn an_even_leading_puts_the_baseline_where_css_does() {
    let mut c = fonts();
    let laid = |c: &mut FontCollection, height: Option<f32>, leading_distribution| {
        let mut b = ParagraphBuilder::new(c);
        b.add_text(
            "one\ntwo",
            &TextStyle {
                height,
                leading_distribution,
                ..style(20.0)
            },
        );
        let mut paragraph = b.build();
        paragraph.layout(f32::INFINITY);
        paragraph
    };
    let even = laid(&mut c, Some(1.5), LeadingDistribution::Even);
    let (font, _) = even.primary_font().expect("the text has a font");
    let (ascent, descent) = (font.ascent_px(20.0), font.descent_px(20.0));
    let half_leading = (30.0 - (ascent + descent)) / 2.0;
    let near = |a: f32, b: f32| (a - b).abs() < 0.01;
    assert!(
        near(even.lines()[0].baseline, half_leading + ascent),
        "{}",
        even.lines()[0].baseline
    );
    assert!(near(even.lines()[1].baseline, 30.0 + half_leading + ascent));
    assert!(near(even.height(), 60.0));

    let proportional = laid(&mut c, Some(1.5), LeadingDistribution::Proportional);
    let share = ascent * 30.0 / (ascent + descent);
    assert!(near(proportional.lines()[0].baseline, share));

    let unset = laid(&mut c, None, LeadingDistribution::Even);
    let plain = laid(&mut c, None, LeadingDistribution::Proportional);
    assert!(near(unset.lines()[0].baseline, plain.lines()[0].baseline));
    assert!(near(unset.height(), plain.height()));
}

/// `line_gap_paragraph` is "one two" in the line-gap test font at 20 px, a line
/// `height` times that when set, sharing it as `leading_distribution` says.
fn line_gap_paragraph(height: Option<f32>, leading_distribution: LeadingDistribution) -> Paragraph {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut c = FontCollection::new();
    let font = std::fs::read(format!("{dir}/line_gap_test.ttf")).unwrap();
    c.register("Line Gap Test", font).unwrap();
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_text(
        "one two",
        &TextStyle {
            height,
            leading_distribution,
            ..TextStyle::new("Line Gap Test", 20.0, Color::BLACK)
        },
    );
    let mut paragraph = b.build();
    paragraph.layout(f32::INFINITY);
    paragraph
}

/// A font's line gap is shared half above and half below its text, with or
/// without a height multiplier, as skparagraph's run metrics share it
/// (`Run::calculateMetrics`): a proportional height scales the room above and
/// below, gap included, and an even one adds half the difference to each.
#[test]
fn a_fonts_line_gap_is_shared_half_above_and_half_below() {
    // The test font's ascent, descent and line gap are 0.935, 0.265 and 0.2 of its
    // size: 18.7, 5.3 and 4 px at 20 px.
    let near = |a: f32, b: f32| (a - b).abs() < 0.01;
    let natural = line_gap_paragraph(None, LeadingDistribution::Proportional);
    assert!(
        near(natural.lines()[0].baseline, 18.7 + 2.0),
        "{}",
        natural.lines()[0].baseline
    );
    assert!(near(natural.height(), 28.0), "{}", natural.height());

    let proportional = line_gap_paragraph(Some(1.5), LeadingDistribution::Proportional);
    assert!(near(proportional.lines()[0].baseline, 20.7 * 30.0 / 28.0));
    assert!(near(proportional.height(), 30.0));

    let even = line_gap_paragraph(Some(1.5), LeadingDistribution::Even);
    assert!(near(even.lines()[0].baseline, 20.7 + 1.0));
    assert!(near(even.height(), 30.0));
}

/// A paragraph's first line can keep its fonts' own ascent, and its last line
/// their own descent, whatever height its text asks for: Flutter's
/// `TextHeightBehavior`, Skia's `kDisableFirstAscent` and `kDisableLastDescent`
/// (`TextWrapper::breakTextIntoLines`). The lines between keep their height.
#[test]
fn a_paragraph_can_keep_its_fonts_own_room_above_and_below() {
    let mut c = fonts();
    let laid = |c: &mut FontCollection, text_height_behavior| {
        let mut b = ParagraphBuilder::new(c);
        b.style(ParagraphStyle {
            text_height_behavior,
            ..ParagraphStyle::default()
        });
        b.add_text(
            "one\ntwo\nthree",
            &TextStyle {
                height: Some(2.0),
                ..style(20.0)
            },
        );
        let mut paragraph = b.build();
        paragraph.layout(f32::INFINITY);
        paragraph
    };
    let near = |a: f32, b: f32| (a - b).abs() < 0.01;
    let tall = laid(&mut c, TextHeightBehavior::default());
    let flush = laid(
        &mut c,
        TextHeightBehavior {
            apply_height_to_first_ascent: false,
            apply_height_to_last_descent: false,
        },
    );
    let (font, _) = tall.primary_font().expect("the text has a font");
    let (ascent, descent) = (font.ascent_px(20.0), font.descent_px(20.0));
    let [first, middle, last] = [0, 1, 2].map(|index| &flush.lines()[index]);
    assert!(near(first.ascent, ascent), "{}", first.ascent);
    assert!(near(first.baseline, ascent));
    assert!(near(last.descent, descent), "{}", last.descent);
    assert!(near(middle.ascent, tall.lines()[1].ascent));
    assert!(near(middle.descent, tall.lines()[1].descent));
    // Each end loses what its height added there.
    let added_above = tall.lines()[0].ascent - ascent;
    let added_below = tall.lines()[2].descent - descent;
    assert!(near(
        flush.height(),
        tall.height() - added_above - added_below
    ));
}

#[test]
fn intrinsic_widths_bound_the_layout() {
    let mut c = fonts();
    let p = laid_out(&mut c, "the quick brown fox jumps", 120.0);
    assert!(p.min_intrinsic_width() > 0.0);
    assert!(p.max_intrinsic_width() > p.min_intrinsic_width());
    // min = widest word; every line must fit at min width when laid out there.
    let mut b = ParagraphBuilder::new(&mut c);
    b.add_text("the quick brown fox jumps", &style(20.0));
    let mut narrow = b.build();
    narrow.layout(p.min_intrinsic_width() + 0.5);
    for line in narrow.lines() {
        assert!(line.width <= p.min_intrinsic_width() + 0.5);
    }
}

#[test]
fn caret_and_position_round_trip() {
    let mut c = fonts();
    let p = laid_out(&mut c, "hello world", f32::INFINITY);
    // Caret advances monotonically through the text.
    let xs: Vec<f32> = (0..=5).map(|i| p.caret_for_offset(i).x).collect();
    assert!(xs.windows(2).all(|w| w[1] > w[0]), "{xs:?}");
    // Position lookup at a caret x returns the same offset.
    let caret = p.caret_for_offset(6);
    let hit = p.glyph_position_at(valo::Point::new(caret.x + 0.5, caret.y + 4.0));
    assert_eq!(hit.offset, 6);
    // Past the end → trailing affinity at the last cluster.
    let end = p.glyph_position_at(valo::Point::new(10_000.0, 4.0));
    assert_eq!(end.offset, 11);
    assert!(!end.downstream);
}

/// Skia's rule: a point in a cluster's left half is the cluster's start,
/// downstream, and one in its right half its end, upstream.
#[test]
fn position_takes_the_edge_of_the_half_under_the_point() {
    let p = simple("ab", 32.0, f32::INFINITY);
    let line = &p.lines()[0];
    let b = &line.runs[0].glyphs[1];
    let left = p.glyph_position_at(valo::Point::new(b.x + b.advance * 0.25, line.baseline));
    assert_eq!((left.offset, left.downstream), (1, true));
    let right = p.glyph_position_at(valo::Point::new(b.x + b.advance * 0.75, line.baseline));
    assert_eq!((right.offset, right.downstream), (2, false));
}

#[test]
fn position_past_a_hard_break_stays_before_it() {
    let p = simple("ab\ncd", 32.0, f32::INFINITY);
    let line = &p.lines()[0];
    let hit = p.glyph_position_at(valo::Point::new(10_000.0, line.baseline));
    assert_eq!(hit.offset, 2);
}

/// In RTL text a cluster's text ends at its left, so its left half is its end.
#[test]
fn rtl_position_left_half_is_the_cluster_end() {
    let mut fonts = fonts();
    let mut b = ParagraphBuilder::new(&mut fonts);
    let text = "سلام";
    b.add_text(
        text,
        &TextStyle::new("Noto Sans Arabic", 24.0, Color::WHITE),
    );
    let mut p = b.build();
    p.layout(f32::INFINITY);
    let line = &p.lines()[0];
    let leftmost = line.runs[0]
        .glyphs
        .iter()
        .min_by(|a, b| a.x.total_cmp(&b.x))
        .unwrap();
    let end = leftmost.cluster + text[leftmost.cluster..].chars().next().unwrap().len_utf8();
    let left = p.glyph_position_at(valo::Point::new(
        leftmost.x + leftmost.advance * 0.25,
        line.baseline,
    ));
    assert_eq!((left.offset, left.downstream), (end, false));
    let right = p.glyph_position_at(valo::Point::new(
        leftmost.x + leftmost.advance * 0.75,
        line.baseline,
    ));
    assert_eq!((right.offset, right.downstream), (leftmost.cluster, true));
}

#[test]
fn rects_for_range_cover_selection() {
    let mut c = fonts();
    let p = laid_out(&mut c, "the quick brown fox", 90.0);
    assert!(p.lines().len() >= 2);
    let rects = p.rects_for_range(4..15); // "quick brown" across lines
    assert!(rects.len() >= 2, "one box per line: {rects:?}");
    let caret = p.caret_for_offset(4);
    assert!(rects.iter().any(|r| (r.x - caret.x).abs() < 0.5));
}

#[test]
fn word_boundary_finds_words() {
    let mut c = fonts();
    let p = laid_out(&mut c, "hello brave world", f32::INFINITY);
    assert_eq!(p.word_boundary(7), 6..11); // inside "brave"
    assert_eq!(p.word_boundary(0), 0..5);
}

// ── editor-correctness regressions ──────────────────────────────────────────

fn simple(text: &str, size: f32, max_width: f32) -> Paragraph {
    let mut fonts = fonts();
    let mut b = ParagraphBuilder::new(&mut fonts);
    b.add_text(text, &TextStyle::new("Fira Sans", size, Color::WHITE));
    let mut p = b.build();
    p.layout(max_width);
    p
}

/// C1: one wrap iteration used to push TWICE (overflow + mandatory break),
/// overshooting max_lines and losing the ellipsis.
#[test]
fn max_lines_never_overshoots() {
    let mut fonts = fonts();
    let mut b = ParagraphBuilder::new(&mut fonts);
    b.style(ParagraphStyle {
        max_lines: Some(1),
        ellipsis: Some("…".into()),
        ..Default::default()
    });
    b.add_text(
        "hello world\nmore",
        &TextStyle::new("Fira Sans", 16.0, Color::WHITE),
    );
    let mut p = b.build();
    p.layout(60.0); // fits roughly one word
    assert_eq!(p.lines().len(), 1, "max_lines is a hard cap");
    assert!(p.truncated());
    let last_run = p.lines()[0].runs.last().expect("ellipsis run");
    assert!(
        last_run.glyphs.iter().any(|g| g.cluster > 0),
        "the ellipsis is present and carries the truncation offset"
    );
}

/// A paragraph cut to one line with an ellipsis still reports the width its whole text asks
/// for: hosts size a loosely constrained title by it.
#[test]
fn one_ellipsized_line_reports_its_whole_text_as_max_intrinsic_width() {
    const TEXT: &str = "the quick brown fox jumps over the lazy dog";
    let one_line = ParagraphStyle {
        max_lines: Some(1),
        ellipsis: Some("…".into()),
        ..Default::default()
    };
    let truncated = monospace(TEXT, one_line, 60.0);
    let unbounded = monospace(TEXT, ParagraphStyle::default(), f32::INFINITY);
    assert!(truncated.truncated());
    assert_close(
        truncated.max_intrinsic_width(),
        unbounded.max_intrinsic_width(),
    );
    assert_close(
        truncated.min_intrinsic_width(),
        truncated.max_intrinsic_width(),
    );
}

/// skparagraph's `SkParagraph_Ellipsis`, in a monospace font for its Ahem: the text
/// `max_lines` or an ellipsis cuts off still counts toward both intrinsic widths, and the min
/// of one line, or of an ellipsis without `max_lines`, is its max.
#[test]
fn intrinsic_widths_count_the_text_max_lines_cuts_off() {
    const TEXT: &str = "This\nis a wrapping test. It should wrap at manual newlines, and if softWrap is true, also at spaces.";
    let advance = monospace("X", ParagraphStyle::default(), f32::INFINITY).max_intrinsic_width();
    // (max_lines, ellipsis, min and max in characters): the longest word is "newlines,",
    // the longest hard line the second.
    let rows = [
        (Some(1), false, 95.0, 95.0),
        (Some(3), false, 9.0, 95.0),
        (None, false, 9.0, 95.0),
        (Some(1), true, 95.0, 95.0),
        (Some(3), true, 9.0, 95.0),
        (None, true, 95.0, 95.0),
    ];
    for (max_lines, ellipsized, min, max) in rows {
        let style = ParagraphStyle {
            max_lines,
            ellipsis: ellipsized.then(|| "\u{2026}".to_owned()),
            ..Default::default()
        };
        let p = monospace(TEXT, style, 50.0);
        assert_close(p.min_intrinsic_width(), min * advance);
        assert_close(p.max_intrinsic_width(), max * advance);
    }
}

/// The row of skparagraph's `SkParagraph_Ellipsis` with an ellipsis and no `max_lines`: the
/// line a hard break ends is kept, the first line that overflows ends with the ellipsis, and
/// nothing is laid out after it (Skia's height there is two lines).
#[test]
fn ellipsis_without_max_lines_cuts_at_the_first_overflowing_line() {
    const TEXT: &str = "This\nis a wrapping test. It should wrap at manual newlines, and if softWrap is true, also at spaces.";
    let style = ParagraphStyle {
        ellipsis: Some("\u{2026}".to_owned()),
        ..Default::default()
    };
    let p = monospace(TEXT, style, 50.0);
    assert!(p.truncated());
    let lines = p.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].range, 0..5, "the hard line is kept whole");
    let ellipsized = &lines[1];
    let ellipsis = ellipsized.runs.last().expect("the ellipsis run");
    assert!(ellipsis.glyphs.iter().all(|g| g.cluster >= 5));
    assert!(ellipsized.width <= 50.0, "{} fits", ellipsized.width);
    let line_height = |line: &valo_paragraph::Line| line.ascent + line.descent;
    assert_close(p.height(), line_height(&lines[0]) + line_height(ellipsized));
}

/// Without a width to overflow, an ellipsis cuts nothing: Flutter's `TextPainter intrinsic
/// dimensions` lays out "X X X" with an ellipsis unconstrained and gets one whole line whose
/// min intrinsic width is its max.
#[test]
fn ellipsis_without_max_lines_cuts_nothing_at_unbounded_width() {
    let style = ParagraphStyle {
        ellipsis: Some("e".to_owned()),
        ..Default::default()
    };
    let p = monospace("X X X", style, f32::INFINITY);
    let advance = monospace("X", ParagraphStyle::default(), f32::INFINITY).max_intrinsic_width();
    assert!(!p.truncated());
    assert_eq!(p.lines().len(), 1);
    assert_close(p.width(), 5.0 * advance);
    assert_close(p.min_intrinsic_width(), 5.0 * advance);
    assert_close(p.max_intrinsic_width(), 5.0 * advance);
}

/// Lines that fit and end at hard breaks are all kept: only an overflowing line is cut.
#[test]
fn ellipsis_without_max_lines_keeps_hard_lines_that_fit() {
    let style = ParagraphStyle {
        ellipsis: Some("\u{2026}".to_owned()),
        ..Default::default()
    };
    let p = monospace("one\ntwo\nthree", style, 50.0);
    assert!(!p.truncated());
    assert_eq!(p.lines().len(), 3);
}

/// An empty ellipsis ellipsizes nothing, as skparagraph's `ParagraphStyle::ellipsized` says:
/// without `max_lines` the text wraps as if there were none.
#[test]
fn empty_ellipsis_without_max_lines_wraps() {
    const TEXT: &str = "the quick brown fox jumps over the lazy dog";
    let with_empty = ParagraphStyle {
        ellipsis: Some(String::new()),
        ..Default::default()
    };
    let p = monospace(TEXT, with_empty, 60.0);
    let plain = monospace(TEXT, ParagraphStyle::default(), 60.0);
    assert!(!p.truncated());
    assert_eq!(p.lines().len(), plain.lines().len());
}

fn monospace(text: &str, paragraph_style: ParagraphStyle, max_width: f32) -> Paragraph {
    let mut fonts = FontCollection::new();
    fonts
        .register("JetBrains Mono", super::asset("jetbrains_mono.ttf"))
        .unwrap();
    let mut b = ParagraphBuilder::new(&mut fonts);
    b.style(paragraph_style);
    b.add_text(text, &TextStyle::new("JetBrains Mono", 10.0, Color::BLACK));
    let mut p = b.build();
    p.layout(max_width);
    p
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.01,
        "{actual} is not {expected}"
    );
}

/// C2: a trailing newline opens an empty final line — the caret after
/// typing Enter lives there, not after the previous word.
#[test]
fn trailing_newline_opens_an_empty_line() {
    let p = simple("hi\n", 16.0, f32::INFINITY);
    assert_eq!(p.lines().len(), 2);
    assert_eq!(p.lines()[1].range, 3..3);
    let caret = p.caret_for_offset(3);
    let below_first = p.lines()[0].baseline;
    assert!(
        caret.y + caret.height * 0.5 > below_first,
        "caret sits on the second line: {caret:?}"
    );
    assert!(caret.height > 0.0);
}

/// C3: an empty paragraph still has one line and a usable caret.
#[test]
fn empty_paragraph_has_one_line() {
    let p = simple("", 24.0, f32::INFINITY);
    assert_eq!(p.lines().len(), 1);
    assert!(
        p.primary_font().is_some(),
        "style face survives without glyphs"
    );
    let caret = p.caret_for_offset(0);
    assert!(
        caret.height > 10.0,
        "caret has the style's height: {caret:?}"
    );
    assert!(p.height() > 10.0, "paragraph reserves one line of height");
}

/// C4: a blank first line measures as the paragraph's style, not as the
/// old hardcoded 16px fallback.
#[test]
fn blank_first_line_uses_the_style_metrics() {
    let p = simple("\nBig", 48.0, f32::INFINITY);
    assert_eq!(p.lines().len(), 2);
    let (blank, real) = (&p.lines()[0], &p.lines()[1]);
    assert!(
        (blank.ascent - real.ascent).abs() < 0.5,
        "blank {} vs real {}",
        blank.ascent,
        real.ascent
    );
}

/// C6: the caret's trailing offset steps one CLUSTER (e + U+0301 is one),
/// never landing inside a grapheme.
#[test]
fn caret_never_splits_a_grapheme() {
    let p = simple("e\u{301}x", 32.0, f32::INFINITY);
    let line = &p.lines()[0];
    let g = &line.runs[0].glyphs[0];
    let hit = p.glyph_position_at(valo::Point::new(g.x + g.advance, line.baseline));
    assert_ne!(hit.offset, 1, "offset 1 is inside e+combining-acute");
    assert_eq!(hit.offset, 3, "trailing edge is the next cluster");
}

/// C7: in RTL text the caret for offset 0 sits at the line's RIGHT side —
/// logical progress moves the caret left.
#[test]
fn rtl_caret_leads_on_the_right() {
    let mut fonts = fonts();
    let mut b = ParagraphBuilder::new(&mut fonts);
    let text = "سلام";
    b.add_text(
        text,
        &TextStyle::new("Noto Sans Arabic", 24.0, Color::WHITE),
    );
    let mut p = b.build();
    p.layout(f32::INFINITY);
    let start = p.caret_for_offset(0).x;
    let end = p.caret_for_offset(text.len()).x;
    assert!(
        start > end,
        "RTL caret moves right→left: start {start} vs end {end}"
    );
}

/// C8: the renderer-facing ink bounds contain the advance box (bearings and
/// overhang can only widen it) while decorations keep the advance box.
#[test]
fn ink_bounds_contain_the_advance_box() {
    let p = simple("fjord", 32.0, f32::INFINITY);
    let run = &p.lines()[0].runs[0];
    assert!(run.ink.x <= run.bounds.x);
    assert!(run.ink.right() >= run.bounds.right());
    assert!(run.ink.y <= run.bounds.y);
    assert!(run.ink.bottom() >= run.bounds.bottom());
    assert!(
        run.ink.height > run.bounds.height,
        "Fira's ink box exceeds its ascent/descent box"
    );
}

/// The renderer hands a glyph raster the font a run resolved to, out of the
/// face set the paragraph cloned when it was built; a raster whose glyphs
/// render in another process reads back the id the font was made with.
#[test]
fn a_run_renders_with_the_remote_id_its_font_was_made_with() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let data = std::sync::Arc::new(std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap());
    let mut c = FontCollection::new();
    c.add(Font::from_data_with_remote_id(data, 0, 7).unwrap());
    let p = laid_out(&mut c, "Hello", f32::INFINITY);
    let run = &p.lines()[0].runs[0];
    assert_eq!(p.faces().get(run.font).remote_id(), Some(7));
}

#[test]
fn a_line_reports_its_fonts_cap_height() {
    use skrifa::MetadataProvider;
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let data = std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap();
    let font = skrifa::FontRef::new(&data).unwrap();
    let metrics = font.metrics(
        skrifa::instance::Size::unscaled(),
        skrifa::instance::LocationRef::default(),
    );
    let expected = metrics.cap_height.unwrap() * 20.0 / f32::from(metrics.units_per_em);
    let mut c = fonts();
    let p = laid_out(&mut c, "Cap", f32::INFINITY);
    let line = &p.line_metrics()[0];
    assert!(
        (line.cap_height - expected).abs() < 1e-4,
        "{} vs {expected}",
        line.cap_height
    );
    assert!(line.cap_height > 0.0 && line.cap_height < line.ascent);
}

/// A line that `max_lines` keeps last but a hard break ends gets no ellipsis: skparagraph
/// ellipsizes only a line it cut (`needEllipsis && !fHardLineBreak` in
/// `TextWrapper::breakTextIntoLines`). The text after it is still cut off.
#[test]
fn a_hard_break_at_the_line_limit_gets_no_ellipsis() {
    let style = ParagraphStyle {
        max_lines: Some(1),
        ellipsis: Some("\u{2026}".to_owned()),
        ..Default::default()
    };
    let p = monospace("hello\nworld", style, f32::INFINITY);
    let advance = monospace("X", ParagraphStyle::default(), f32::INFINITY).max_intrinsic_width();
    assert!(p.truncated());
    let lines = p.lines();
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].runs.iter().all(|run| !run.text.is_empty()),
        "no ellipsis run"
    );
    assert_close(lines[0].width, 5.0 * advance);
}
