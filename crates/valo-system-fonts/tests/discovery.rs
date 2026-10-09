//! OS-dependent by nature: each test skips (eprintln + return, the GPU
//! pattern) when this machine lacks the faces it needs, and asserts
//! strictly when they exist.

use valo_system_fonts::SystemFonts;
use valo_text::{
    Font, FontAttrs, FontCollection, FontSource, Paragraph, ParagraphBuilder, TextStyle,
};

/// Sans families a stock install of this system carries with a bold face,
/// most common first.
#[cfg(any(target_os = "macos", target_os = "ios"))]
const FAMILIES: &[&str] = &["Helvetica"];
#[cfg(target_os = "windows")]
const FAMILIES: &[&str] = &["Segoe UI", "Arial"];
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
const FAMILIES: &[&str] = &[
    "DejaVu Sans",
    "Noto Sans",
    "Liberation Sans",
    "Ubuntu Sans",
    "Nimbus Sans",
];

/// Families this system installs inside a font collection (`.ttc`), at a face
/// index past the first.
#[cfg(any(target_os = "macos", target_os = "ios"))]
const COLLECTED_FAMILIES: &[&str] = &["Helvetica"];
#[cfg(target_os = "windows")]
const COLLECTED_FAMILIES: &[&str] = &["Cambria Math", "Microsoft YaHei UI", "Yu Gothic UI"];
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
const COLLECTED_FAMILIES: &[&str] = &["Noto Sans CJK SC", "Noto Sans CJK JP"];

/// Variable families this system installs, under the names they are asked
/// for by.
#[cfg(any(target_os = "macos", target_os = "ios"))]
const VARIABLE_FAMILIES: &[&str] = &[".SF NS", "SF Pro", "SF Pro Text"];
#[cfg(target_os = "windows")]
const VARIABLE_FAMILIES: &[&str] = &["Segoe UI Variable", "Bahnschrift"];
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
const VARIABLE_FAMILIES: &[&str] = &["Ubuntu Sans", "Noto Sans", "Cantarell"];

/// The first of `families` installed on this system, with its faces.
fn first_installed(
    system: &mut SystemFonts,
    families: &[&'static str],
) -> Option<(&'static str, Vec<Font>)> {
    families.iter().find_map(|&family| {
        let faces = system.family(family);
        (!faces.is_empty()).then_some((family, faces))
    })
}

fn fira_sans() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/fonts/fira_sans.ttf"
    );
    std::fs::read(path).expect("fira_sans.ttf")
}

fn paragraph(text: &str, family: &str, fonts: &mut FontCollection) -> Paragraph {
    let mut builder = ParagraphBuilder::new(fonts);
    builder.add_text(
        text,
        &TextStyle::new(family, 16.0, valo_geometry::Color::BLACK),
    );
    builder.build()
}

#[test]
fn installed_family_registers_all_variants() {
    let mut system = SystemFonts::load();
    let Some((family, faces)) = first_installed(&mut system, FAMILIES) else {
        eprintln!("SKIP: none of {FAMILIES:?} installed");
        return;
    };
    assert!(faces.iter().all(|face| face.matches(family)));
    assert!(
        faces.iter().any(|face| face.attrs().weight >= 700),
        "a bold variant came along"
    );
}

#[test]
fn collection_files_thread_their_face_index_through() {
    let mut system = SystemFonts::load();
    let Some((family, faces)) = first_installed(&mut system, COLLECTED_FAMILIES) else {
        eprintln!("SKIP: none of {COLLECTED_FAMILIES:?} installed");
        return;
    };
    assert!(faces.iter().all(|face| face.matches(family)));
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
    let installed = first_installed(&mut system, FAMILIES);
    let covered = system
        .face_for_codepoint('中', FontAttrs::default())
        .is_some();
    let (Some((family, _)), true) = (installed, covered) else {
        eprintln!("SKIP: this machine cannot answer the demanded faces");
        return;
    };
    // Asked for in lower case: the system answers a family case-insensitively.
    let demanded_family = family.to_ascii_lowercase();

    let mut collection = FontCollection::new();
    collection.register("Fira Sans", fira_sans()).unwrap();

    // Fira covers the latin; the system family and the CJK are demands — the
    // OUT-OF-BAND loop (no source installed on the collection).
    let first = paragraph("Hello 中文", &demanded_family, &mut collection);
    let demand = first.demand().clone();
    assert!(demand.families.contains(&demanded_family));
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
        paragraph("Hello 中文", &demanded_family, &mut collection)
            .demand()
            .is_empty(),
        "one round of satisfaction resolves everything"
    );

    // The LIVE path needs no loop at all: install the source and the
    // collection answers its own misses mid-shape.
    let mut live = FontCollection::new();
    live.add_source(SystemFonts::load());
    assert!(
        paragraph("Hello 中文", &demanded_family, &mut live)
            .demand()
            .is_empty(),
        "an installed source resolves during the build itself"
    );
}

#[test]
fn variable_system_fonts_expand_into_weighted_instances() {
    let mut system = SystemFonts::load();
    for &name in VARIABLE_FAMILIES {
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
    eprintln!("SKIP: none of {VARIABLE_FAMILIES:?} installed as a variable font");
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[test]
fn the_system_family_carries_its_weights() {
    let mut system = SystemFonts::load();
    let fonts = system.system_family(17.0);
    if fonts.is_empty() {
        eprintln!("SKIP: no system UI font");
        return;
    }
    assert!(fonts.iter().all(|font| font.covers('a')));
    assert!(fonts.iter().any(|font| font.attrs().weight >= 700));
    assert!(fonts.iter().any(|font| font.attrs().weight <= 400));
}
