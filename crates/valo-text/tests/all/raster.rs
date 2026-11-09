//! Glyph raster: alpha masks, signed distance fields, outline paths and
//! color glyphs, drawn from the fonts the paragraph shapes with.

use valo_text::{FaceSet, FontSynthesis, Rasterizer};

fn fira() -> FaceSet {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut c = FaceSet::default();
    c.register(
        "Fira Sans",
        std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap(),
    )
    .unwrap();
    c
}

#[test]
fn rasterizes_alpha_sdf_and_path() {
    let c = fira();
    let font = c.family("Fira Sans").unwrap();
    let glyph = c.get(font).glyph_for('g').unwrap();
    let mut r = Rasterizer::new();

    let alpha = r
        .alpha(c.get(font), glyph, 32.0, FontSynthesis::NONE, 0.0)
        .unwrap();
    assert!(alpha.width > 5 && alpha.height > 5);
    assert!(alpha.data.iter().any(|&v| v > 200));

    let sdf = r
        .sdf(c.get(font), glyph, 32.0, FontSynthesis::NONE)
        .unwrap();
    // 2×-supersampled + padded, then halved: within a px of alpha + 2·pad.
    assert!(sdf.width.abs_diff(alpha.width + 2 * valo_text::SDF_PAD) <= 1);
    assert!(sdf.data.iter().any(|&v| v > 140), "inside rises above 0.5");
    assert!(sdf.data.iter().any(|&v| v < 50), "far outside < 0.5");

    let path = valo_text::glyph_path(c.get(font), glyph, 64.0, FontSynthesis::NONE).unwrap();
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
        .as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[3] > 200 && (p[0] != p[1] || p[1] != p[2])));
}
