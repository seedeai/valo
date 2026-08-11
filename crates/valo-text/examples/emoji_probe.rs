//! Emoji diagnosis: run a downloaded emoji font file through the
//! exact production pipeline — container decode, parse, coverage, raster.
//! Usage: cargo run -p valo-text --example emoji_probe -- <file> <char>

use valo_text::{FaceSet, Font, Rasterizer};

fn main() {
    let path = std::env::args().nth(1).expect("font path");
    let ch = std::env::args()
        .nth(2)
        .and_then(|s| s.chars().next())
        .unwrap_or('🍶');
    let raw = std::fs::read(&path).expect("read font");
    println!(
        "file: {} bytes, magic {:?}",
        raw.len(),
        &raw[..4.min(raw.len())]
    );

    // Container decode, inlined (WOFF2 → sfnt).
    let bytes = match raw.get(..4) {
        Some(b"wOF2") => match woff2_patched::convert_woff2_to_ttf(&mut raw.as_slice()) {
            Ok(ttf) => {
                println!("woff2 → sfnt: {} bytes", ttf.len());
                ttf
            }
            Err(e) => {
                println!("woff2 DECODE FAILED: {e:?}");
                return;
            }
        },
        _ => raw,
    };

    if let Ok(fr) = skrifa::FontRef::new(&bytes) {
        let tags: Vec<String> = fr
            .table_directory()
            .table_records()
            .iter()
            .map(|r| r.tag().to_string())
            .collect();
        println!("tables: {tags:?}");
    }
    if let Ok(dump) = std::env::var("DUMP_SFNT") {
        std::fs::write(&dump, &bytes).expect("dump sfnt");
        println!("dumped decoded sfnt to {dump}");
    }
    let Some(font) = Font::from_bytes(bytes) else {
        println!("Font::from_bytes FAILED (skrifa/harfrust parse)");
        return;
    };
    println!(
        "parsed: family {:?}, aliases {:?}",
        font.family(),
        font.aliases()
    );
    println!("covers {ch:?}: {}", font.covers(ch));
    let Some(glyph) = font.glyph_for(ch) else {
        println!("no glyph id");
        return;
    };
    println!("glyph id: {glyph}");

    let mut c = FaceSet::default();
    let id = c.add(font);
    let mut raster = Rasterizer::new();
    match raster.color(c.get(id), glyph, 64.0) {
        Some(img) => {
            let opaque = img.data.chunks_exact(4).filter(|p| p[3] > 0).count();
            println!(
                "COLOR raster: {}x{}, {} visible px ✓",
                img.width, img.height, opaque
            );
        }
        None => {
            println!("color raster: None — falling back to alpha…");
            match raster.alpha(c.get(id), glyph, 64.0, 0.0) {
                Some(img) => {
                    let ink = img.data.iter().filter(|&&a| a > 0).count();
                    println!("alpha raster: {}x{}, {} ink px", img.width, img.height, ink);
                }
                None => println!("alpha raster: None — glyph rasters to NOTHING"),
            }
        }
    }
}
