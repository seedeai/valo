//! GPU-free pipeline proof: shaping, fallback, wrapping, bidi, raster, and
//! the retained tiers (build = shape once; layout = wrap; recolor = place).

use valo_geometry::Color;
use valo_text::{
    FaceSet, FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, Rasterizer, TextAlign,
    TextStyle,
};
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
            .runs
            .iter()
            .map(|r| r.bounds.right())
            .fold(f32::MIN, f32::max);
        assert!(
            (right - 200.0).abs() < 1.0,
            "justified line ends at the edge: {right}"
        );
    }
    let last = p.lines().last().unwrap();
    let right = last
        .runs
        .iter()
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
fn rasterizes_alpha_sdf_and_path() {
    let c = fonts();
    let font = c.family("Fira Sans").unwrap();
    let glyph = c.get(font).glyph_for('g').unwrap();
    let mut r = Rasterizer::new();

    let alpha = r.alpha(c.get(font), glyph, 32.0, 0.0).unwrap();
    assert!(alpha.width > 5 && alpha.height > 5);
    assert!(alpha.data.iter().any(|&v| v > 200));

    let sdf = r.sdf(c.get(font), glyph, 32.0).unwrap();
    // 2×-supersampled + padded, then halved: within a px of alpha + 2·pad.
    assert!(sdf.width.abs_diff(alpha.width + 2 * valo_text::SDF_PAD) <= 1);
    assert!(sdf.data.iter().any(|&v| v > 140), "inside rises above 0.5");
    assert!(sdf.data.iter().any(|&v| v < 50), "far outside < 0.5");

    let path = valo_text::glyph_path(c.get(font), glyph, 64.0).unwrap();
    let bounds = path.bounds();
    assert!(bounds.width > 20.0 && bounds.height > 20.0);
    assert!(
        bounds.y < 0.0,
        "baseline-origin, ink above baseline is negative y"
    );
}

#[test]
fn color_emoji_rasterizes_rgba() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut c = FaceSet::default();
    let emoji = c
        .register(
            "Noto Color Emoji",
            std::fs::read(format!("{dir}/noto_color_emoji_subset.ttf")).unwrap(),
        )
        .unwrap();
    let c = c;
    let glyph = c
        .get(emoji)
        .glyph_for('🚀')
        .expect("subset covers the rocket");
    let mut r = Rasterizer::new();
    let img = r
        .color(c.get(emoji), glyph, 32.0)
        .expect("CBDT strike renders");
    assert!(img.width > 10 && img.height > 10);
    assert_eq!(
        img.data.len(),
        (img.width * img.height * 4) as usize,
        "RGBA"
    );
    // Premultiplied color content: some opaque non-gray pixel exists.
    assert!(img
        .data
        .chunks_exact(4)
        .any(|p| p[3] > 200 && (p[0] != p[1] || p[1] != p[2])));
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

#[test]
fn weight_matching_picks_nearest_variant() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let bytes = std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap();
    let mut c = FaceSet::default();
    let regular = c
        .register_with(
            "Fira",
            valo_text::FontAttrs {
                weight: 400,
                italic: false,
                ..Default::default()
            },
            bytes.clone(),
        )
        .unwrap();
    let bold = c
        .register_with(
            "Fira",
            valo_text::FontAttrs {
                weight: 700,
                italic: false,
                ..Default::default()
            },
            bytes,
        )
        .unwrap();
    assert_eq!(
        c.family_variant(
            "Fira",
            valo_text::FontAttrs {
                weight: 400,
                italic: false,
                ..Default::default()
            }
        ),
        Some(regular)
    );
    assert_eq!(
        c.family_variant(
            "Fira",
            valo_text::FontAttrs {
                weight: 800,
                italic: false,
                ..Default::default()
            }
        ),
        Some(bold)
    );
    assert_eq!(
        c.family_variant(
            "Fira",
            valo_text::FontAttrs {
                weight: 550,
                italic: true,
                ..Default::default()
            }
        ),
        Some(regular)
    );
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
