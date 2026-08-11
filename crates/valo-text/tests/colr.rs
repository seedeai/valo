//! COLRv1 rasterization through the exact production entry
//! point (`Rasterizer::color`) — the same call the glyph atlas makes. The
//! asset is a css2 COLRv1 subset (COLR/CPAL + empty classic outlines), the
//! flavor swash cannot paint; pixels here prove the skrifa painter runs.

use std::collections::HashSet;

use valo_text::{FaceSet, Font, Rasterizer};

fn emoji_collection() -> (FaceSet, valo_text::FontId, u32) {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/fonts/noto_color_emoji_colrv1_subset.ttf"
    );
    let font = Font::from_bytes(std::fs::read(path).unwrap()).unwrap();
    assert_eq!(font.family(), "Noto Color Emoji");
    let glyph = font.glyph_for('🍶').expect("subset covers the bottle");
    let mut c = FaceSet::default();
    let id = c.add(font);
    (c, id, glyph)
}

#[test]
fn colrv1_glyph_rasters_in_color() {
    let (fonts, id, glyph) = emoji_collection();
    let mut raster = Rasterizer::new();
    let image = raster
        .color(fonts.get(id), glyph, 64.0)
        .expect("COLRv1 paints via the skrifa painter");

    assert!(image.width > 16 && image.height > 16, "plausible box");
    // Placement is y-up around the origin, like every other tier.
    assert!(image.top > 0, "sits above the baseline (top={})", image.top);

    let visible: Vec<[u8; 4]> = image
        .data
        .chunks_exact(4)
        .filter(|p| p[3] > 0)
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect();
    assert!(
        visible.len() > 500,
        "substantial ink ({} visible px)",
        visible.len()
    );
    // Premultiplied invariant: no channel exceeds alpha.
    assert!(visible
        .iter()
        .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
    // COLOR, not a silhouette: many distinct opaque colors means palette
    // fills and gradients actually painted.
    let distinct: HashSet<[u8; 4]> = visible.into_iter().collect();
    assert!(
        distinct.len() > 8,
        "expected a multicolor glyph, got {} distinct colors",
        distinct.len()
    );
}

#[test]
fn colrv1_scales_with_px() {
    let (fonts, id, glyph) = emoji_collection();
    let mut raster = Rasterizer::new();
    let small = raster.color(fonts.get(id), glyph, 32.0).unwrap();
    let large = raster.color(fonts.get(id), glyph, 128.0).unwrap();
    assert!(large.width >= small.width * 3, "vector: box scales with px");
    assert!(large.height >= small.height * 3);
}

/// The >`path_min` vanish bug: COLR fonts carry EMPTY classic outlines, and
/// an empty path must read as "no outline" so the renderer's outline tier
/// takes its color fallback instead of stencil-filling nothing.
#[test]
fn colr_glyphs_report_no_outline() {
    let (fonts, id, glyph) = emoji_collection();
    assert!(
        valo_text::glyph_path(fonts.get(id), glyph, 200.0).is_none(),
        "empty COLR outline must be None, not Some(empty)"
    );
}
