//! Text tiers under zoom (Skia's SubRunControl policy).
//! `cargo run -p valo --example text_zoom`
//!
//! One card drawn at four zooms. Watch the tiers do their jobs:
//! - body text stays a DEVICE-SCALE mask at every zoom — crisp, re-rastered
//!   as the scale changes (Impeller's quantized-scale behavior);
//! - the headline crosses into SDF past 162 device px (Skia's buckets),
//!   then into real outlines past 324;
//! - emoji clamp to their biggest bitmap instead of vanishing.
//!
//! `stats.text_tiers` counts runs per tier: [mask, sdf, path].

use valo::{
    Color, DisplayListBuilder, DrawParagraphExt, FontCollection, ParagraphBuilder, TextStyle,
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
    let emoji = c
        .register(
            "Noto Color Emoji",
            std::fs::read(format!("{dir}/noto_color_emoji_subset.ttf")).unwrap(),
        )
        .unwrap();
    c.add_fallback(latin);
    c.add_fallback(emoji);
    c
}

fn card(b: &mut DisplayListBuilder, fonts: &mut FontCollection, zoom: f32, at: (f32, f32)) {
    b.save();
    b.translate(at.0, at.1);
    b.scale(zoom, zoom);
    let mut title = ParagraphBuilder::new(fonts);
    title.add_text(
        "Aa 🚀",
        &TextStyle::new("Fira Sans", 64.0, Color::rgb(0.4, 0.65, 1.0)),
    );
    let mut title = title.build();
    title.layout(f32::INFINITY);
    b.draw_paragraph(&title, (0.0, 0.0));

    let mut body = ParagraphBuilder::new(fonts);
    body.add_text(
        "device-scale masks keep this crisp",
        &TextStyle::new("Fira Sans", 13.0, Color::rgb(0.9, 0.91, 0.95)),
    );
    let mut body = body.build();
    body.layout(f32::INFINITY);
    b.draw_paragraph(&body, (0.0, 78.0));
    b.restore();
}

fn scene(fonts: &mut FontCollection) -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    card(&mut b, fonts, 0.75, (24.0, 24.0));
    card(&mut b, fonts, 1.0, (24.0, 120.0));
    card(&mut b, fonts, 1.6, (24.0, 240.0));
    card(&mut b, fonts, 3.2, (24.0, 420.0));
    b.build()
}

fn main() {
    let mut fonts = fonts();
    valo_harness::run_example(
        "text_zoom",
        [660, 800],
        Color::rgb(0.09, 0.1, 0.13),
        |_ctx| scene(&mut fonts),
    );
}
