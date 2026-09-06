//! OS-dependent by nature: each test skips (eprintln + return) when this machine lacks
//! what it needs, and asserts strictly when it has it.

use valo_fontmgr::{FontManager, Slant, Style};

fn manager() -> Option<Box<dyn FontManager>> {
    let manager = valo_fontmgr::platform();
    if manager.face_count() == 0 {
        eprintln!("SKIP: no installed fonts");
        return None;
    }
    Some(manager)
}

#[test]
fn a_family_answers_with_every_face_and_their_styles() {
    let Some(mut manager) = manager() else {
        return;
    };
    let faces = manager.family("Helvetica");
    if faces.is_empty() {
        eprintln!("SKIP: no Helvetica installed");
        return;
    }
    assert!(faces.iter().all(|face| face.covers('a')));
    assert!(faces.iter().any(|face| face.style().weight >= 700), "a bold face");
    assert!(
        faces.iter().any(|face| face.style().slant == Slant::Italic),
        "an italic face"
    );
    let bold = Style {
        weight: 700,
        ..Style::default()
    };
    let nearest = manager.match_family_style("Helvetica", bold).unwrap();
    assert!(nearest.style().weight >= 600);
}

#[test]
fn a_character_is_covered_in_its_language() {
    let Some(mut manager) = manager() else {
        return;
    };
    let Some(face) = manager.match_character(None, Style::default(), &["ja"], '中') else {
        eprintln!("SKIP: nothing installed covers CJK");
        return;
    };
    assert!(face.covers('中'));
    assert!(
        manager
            .match_character(None, Style::default(), &[], '\u{10FFFF}')
            .is_none(),
        "an unassigned code point has no face"
    );
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[test]
fn the_system_font_is_answered_at_text_and_display_sizes() {
    let Some(mut manager) = manager() else {
        return;
    };
    let text = manager.system_font(17.0, Style::default()).expect("a UI font");
    assert!(text.covers('a'));
    let bold = Style {
        weight: 700,
        ..Style::default()
    };
    let display = manager.system_font(28.0, bold).expect("a bold UI font");
    assert!(display.covers('a'));
    assert!(!manager.families().is_empty());
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[test]
fn the_served_tables_are_a_fraction_of_the_file_and_rebuild_a_font() {
    use skrifa::MetadataProvider;
    let Some(mut manager) = manager() else {
        return;
    };
    let face = manager.system_font(17.0, Style::default()).expect("a UI font");
    let rebuilt = valo_fontmgr::assemble(&face.tables());
    eprintln!(
        "system font: file {} bytes, served tables {} bytes",
        face.data().len(),
        rebuilt.len()
    );
    assert!(rebuilt.len() * 4 < face.data().len());
    let font = skrifa::FontRef::new(&rebuilt).expect("the rebuilt font parses");
    assert!(font.charmap().map('a').is_some());
    if let Some(cjk) = manager.match_character(None, Style::default(), &["zh-Hans"], '中') {
        let rebuilt = valo_fontmgr::assemble(&cjk.tables());
        eprintln!(
            "cjk font: file {} bytes, served tables {} bytes",
            cjk.data().len(),
            rebuilt.len()
        );
        assert!(rebuilt.len() * 4 < cjk.data().len());
        let font = skrifa::FontRef::new(&rebuilt).expect("the rebuilt font parses");
        assert!(font.charmap().map('中').is_some(), "a collection face keeps its cmap");
    }
}
