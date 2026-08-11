//! OS-dependent by nature: each test skips (eprintln + return, the GPU
//! pattern) when this machine lacks the faces it needs, and asserts
//! strictly when they exist.

use std::sync::Arc;

use valo_system_fonts::SystemFonts;
use valo_text::{FontAttrs, FontCollection, FontSource, Paragraph, ParagraphBuilder, TextStyle};

fn fira_sans() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/fonts/fira_sans.ttf"
    );
    std::fs::read(path).expect("fira_sans.ttf")
}

fn paragraph(text: &str, fonts: &mut FontCollection) -> Paragraph {
    let mut builder = ParagraphBuilder::new(fonts);
    builder.add_text(
        text,
        &TextStyle::new("helvetica", 16.0, valo_geometry::Color::BLACK),
    );
    builder.build()
}

#[test]
fn installed_family_registers_all_variants() {
    let mut system = SystemFonts::load();
    let faces = system.family("Helvetica");
    if faces.is_empty() {
        eprintln!("SKIP: no Helvetica installed");
        return;
    }
    assert!(faces.iter().all(|face| face.matches("Helvetica")));
    assert!(
        faces.iter().any(|face| face.attrs().weight >= 700),
        "a bold variant came along"
    );
    assert!(
        faces.iter().any(|face| face.face_index() > 0),
        "collection files (.ttc) thread their face index through"
    );
}

#[test]
fn coverage_scan_finds_a_cjk_face() {
    let mut system = SystemFonts::load();
    if system.face_count() == 0 {
        eprintln!("SKIP: no installed fonts found");
        return;
    }
    let Some(font) = system.face_for_codepoint('中', FontAttrs::default()) else {
        eprintln!("SKIP: nothing installed covers CJK");
        return;
    };
    assert!(font.covers('中'));
}

#[test]
fn demand_loop_reaches_empty() {
    let mut system = SystemFonts::load();
    let answerable = !system.family("Helvetica").is_empty()
        && system
            .face_for_codepoint('中', FontAttrs::default())
            .is_some();
    if !answerable {
        eprintln!("SKIP: this machine cannot answer the demanded faces");
        return;
    }

    let mut collection = FontCollection::new();
    collection.register("Fira Sans", fira_sans()).unwrap();

    // Fira covers the latin; "Helvetica" and the CJK are demands — the
    // OUT-OF-BAND loop (no source installed on the collection).
    let first = paragraph("Hello 中文", &mut collection);
    let demand = first.demand().clone();
    assert!(demand.families.contains(&"helvetica".to_owned()));
    assert!(demand
        .codepoints
        .iter()
        .any(|&(codepoint, _)| codepoint == '中'));

    let grown = system
        .satisfy(collection.faces(), &demand)
        .expect("the system answers");
    assert!(system.satisfy(&grown, &Default::default()).is_none());
    collection.adopt_faces(grown);
    assert!(
        paragraph("Hello 中文", &mut collection).demand().is_empty(),
        "one round of satisfaction resolves everything"
    );

    // The LIVE path needs no loop at all: install the source and the
    // collection answers its own misses mid-shape.
    let mut live = FontCollection::new();
    live.add_source(SystemFonts::load());
    assert!(
        paragraph("Hello 中文", &mut live).demand().is_empty(),
        "an installed source resolves during the build itself"
    );
}

#[test]
fn variable_system_fonts_expand_into_weighted_instances() {
    let mut system = SystemFonts::load();
    for name in [".SF NS", "SF Pro", "SF Pro Text"] {
        let faces = system.family(name);
        if faces
            .iter()
            .any(|face| !face.variation_coordinates().is_empty())
        {
            let weights: std::collections::HashSet<u16> =
                faces.iter().map(|face| face.attrs().weight).collect();
            assert!(weights.len() > 1, "instances span weights: {weights:?}");
            assert!(
                faces.iter().any(|face| face.attrs().weight >= 700),
                "a bold instance exists"
            );
            return;
        }
    }
    eprintln!("SKIP: no variable system font found under known names");
}
