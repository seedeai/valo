//! Text end-to-end: FontCollection → ParagraphBuilder
//! (fallback, bidi, shaping, wrapping) → GlyphRun ops → atlas tiers.
//! `cargo run -p valo --example text`
//!
//! What to look at:
//! - TOP: a wrapped paragraph mixing sizes/colors in one layout, with an
//!   embedded Arabic word (font FALLBACK splits runs; bidi places the RTL
//!   segment correctly inside the LTR line).
//! - An RTL Hebrew line: logical start lands visually right.
//! - CENTER/RIGHT aligned lines against the same width.
//! - BOTTOM LEFT: rotated + scaled text — the SDF tier (one raster serves
//!   the transform; edges stay clean).
//! - BOTTOM RIGHT: huge text — the OUTLINE tier (real paths, stencil-then-
//!   cover, sharp at any size).

use valo::{
    Color, DisplayListBuilder, DrawParagraphExt, FontCollection, ParagraphBuilder, ParagraphStyle,
    TextAlign, TextStyle,
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
    let emoji = c
        .register(
            "Noto Color Emoji",
            std::fs::read(format!("{dir}/noto_color_emoji_subset.ttf")).unwrap(),
        )
        .unwrap();
    c.add_fallback(latin);
    c.add_fallback(arabic);
    c.add_fallback(hebrew);
    c.add_fallback(emoji);
    c
}

fn scene(fonts: &mut FontCollection) -> valo::DisplayList {
    let ink = Color::rgb(0.92, 0.93, 0.96);
    let accent = Color::rgb(0.95, 0.75, 0.3);
    let body = TextStyle::new("Fira Sans", 22.0, ink);

    let mut b = DisplayListBuilder::new();

    // A wrapped multi-style paragraph with an embedded RTL word.
    let mut p = ParagraphBuilder::new(fonts);
    p.add_text("valo renders ", &body)
        .add_text("retained", &TextStyle::new("Fira Sans", 30.0, accent))
        .add_text(
            " paragraphs — wrapped, shaped, and reordered: the word ",
            &body,
        )
        .add_text("سلام", &TextStyle::new("Fira Sans", 22.0, accent))
        .add_text(" flows right-to-left inside this line.", &body);
    let mut par = p.build();
    par.layout(560.0);
    b.draw_paragraph(&par, (40.0, 32.0));

    // An RTL paragraph (Hebrew base direction).
    let mut rtl = ParagraphBuilder::new(fonts);
    rtl.add_text(
        "שלום — valo — עולם",
        &TextStyle::new("Noto Sans Hebrew", 24.0, ink),
    );
    let mut rtl = rtl.build();
    rtl.layout(f32::INFINITY);
    b.draw_paragraph(&rtl, (40.0, 190.0));

    // Alignment against one width.
    for (i, align) in [TextAlign::Left, TextAlign::Center, TextAlign::Right]
        .into_iter()
        .enumerate()
    {
        let mut a = ParagraphBuilder::new(fonts);
        a.style(ParagraphStyle {
            align,
            ..Default::default()
        });
        a.add_text(
            "aligned",
            &TextStyle::new("Fira Sans", 18.0, Color::rgb(0.6, 0.75, 1.0)),
        );
        let mut a = a.build();
        a.layout(560.0);
        b.draw_paragraph(&a, (40.0, 240.0 + i as f32 * 26.0));
    }

    // SDF tier: the same run rotated + scaled.
    let mut sdf = ParagraphBuilder::new(fonts);
    sdf.add_text("SDF tier", &TextStyle::new("Fira Sans", 26.0, accent));
    let mut sdf_par = sdf.build();
    sdf_par.layout(f32::INFINITY);
    b.save();
    b.translate(60.0, 420.0);
    b.rotate(-0.18);
    b.scale(1.6, 1.6);
    b.draw_paragraph(&sdf_par, (0.0, 0.0));
    b.restore();

    // Outline tier: device size ≫ atlas budget → real paths.
    let mut big = ParagraphBuilder::new(fonts);
    big.add_text(
        "Aa",
        &TextStyle::new("Fira Sans", 200.0, Color::rgb(0.35, 0.6, 1.0)),
    );
    let mut big = big.build();
    big.layout(f32::INFINITY);
    b.draw_paragraph(&big, (380.0, 330.0));

    // Color emoji (CBDT via swash) ride the RGBA atlas page, untinted.
    let mut emoji = ParagraphBuilder::new(fonts);
    emoji.add_text("ship it 🚀 ✨ 🎨", &body);
    let mut emoji = emoji.build();
    emoji.layout(f32::INFINITY);
    b.draw_paragraph(&emoji, (40.0, 540.0));

    // Justify: word gaps stretch, the last line stays ragged.
    let mut just = ParagraphBuilder::new(fonts);
    just.style(TextAlign::Justify.into());
    just.add_text(
        "justified text stretches every word gap to meet the right edge and leaves the final line ragged",
        &TextStyle::new("Fira Sans", 15.0, Color::rgb(0.75, 0.78, 0.85)),
    );
    let mut just = just.build();
    just.layout(250.0);
    b.draw_paragraph(&just, (40.0, 590.0));

    // maxLines + ellipsis: the retained paragraph truncates itself.
    let mut ell = ParagraphBuilder::new(fonts);
    ell.style(ParagraphStyle {
        max_lines: Some(2),
        ellipsis: Some("…".to_owned()),
        ..Default::default()
    });
    ell.add_text(
        "two lines is all this card gets no matter how much copy the author keeps typing into it",
        &TextStyle::new("Fira Sans", 15.0, Color::rgb(0.75, 0.78, 0.85)),
    );
    let mut ell = ell.build();
    ell.layout(250.0);
    b.draw_paragraph(&ell, (350.0, 590.0));

    b.build()
}

fn main() {
    let mut fonts = fonts();
    valo_harness::run_example(
        "text",
        [660, 700],
        Color::rgb(0.09, 0.1, 0.13),
        move |_ctx| scene(&mut fonts),
    );
}
