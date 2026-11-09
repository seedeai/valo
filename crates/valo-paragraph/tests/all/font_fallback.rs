//! Font fallback through paragraph building: a built paragraph reports
//! what its fonts could not resolve, and building asks a collection's
//! sources before it gives up on a family or a character.

use valo_paragraph::{ParagraphBuilder, TextStyle};
use valo_text::{Font, FontAttrs};

use crate::asset;

/// A built paragraph carries the demand signal — families the
/// collection lacks entirely, and chars nothing present covers. valo only
/// DETECTS; where fonts come from is the host's policy.
#[test]
fn paragraphs_report_font_demand() {
    let mut fonts = valo_text::FontCollection::new();
    let latin = fonts.register("Fira Sans", asset("fira_sans.ttf")).unwrap();
    fonts.add_fallback(latin);

    // Latin under a present family: nothing demanded.
    let mut b = ParagraphBuilder::new(&mut fonts);
    b.add_text(
        "hello",
        &TextStyle::new("Fira Sans", 16.0, valo_geometry::Color::BLACK),
    );
    assert!(b.build().demand().is_empty());

    // Missing family + emoji and CJK nothing covers: both halves report.
    let mut b = ParagraphBuilder::new(&mut fonts);
    b.add_text(
        "hi 🍶 你好",
        &TextStyle::new("Noto Sans", 16.0, valo_geometry::Color::BLACK),
    );
    let p = b.build();
    let d = p.demand();
    let regular = FontAttrs::default();
    assert_eq!(d.families, vec!["Noto Sans".to_owned()]);
    assert_eq!(
        d.codepoints,
        vec![('🍶', regular), ('你', regular), ('好', regular)]
    );

    // Whitespace and covered latin never demand codepoints.
    assert!(!d.codepoints.iter().any(|&(ch, _)| ch == ' '));
    assert!(!d.codepoints.iter().any(|&(ch, _)| ch == 'h'));
}

#[test]
fn a_face_resolution_cannot_reach_does_not_answer_for_coverage() {
    // An icon font is registered under its own name and joins no fallback
    // chain, yet its cmap lists characters the UI text also uses. Counting
    // that as coverage suppressed the fallback the text needed, and the
    // character shaped as .notdef instead: spaced correctly, drawn as nothing.
    let mut fonts = valo_text::FontCollection::new();
    let latin = fonts.register("Fira Sans", asset("fira_sans.ttf")).unwrap();
    fonts.add_fallback(latin);
    fonts
        .register("Private Icons", asset("noto_sans_arabic.ttf"))
        .unwrap();

    let mut b = ParagraphBuilder::new(&mut fonts);
    b.add_text(
        "ع",
        &TextStyle::new("Fira Sans", 16.0, valo_geometry::Color::BLACK),
    );
    let paragraph = b.build();

    assert_eq!(
        paragraph.demand().codepoints,
        vec![('ع', FontAttrs::default())],
        "the unreachable face must not stand in for a fallback"
    );
    assert!(
        !paragraph
            .faces()
            .resolve_covered(&["Fira Sans".to_owned()], FontAttrs::default(), 'ع')
            .1
    );
}

/// A scripted [`FontSource`] standing in for the OS or the network — the
/// growth policy must be provable without either.
struct ScriptedSource {
    family_bytes: Option<Vec<u8>>,
    fallback_bytes: Option<Vec<u8>>,
    asked_codepoints: Vec<(char, FontAttrs)>,
}

impl valo_text::FontSource for ScriptedSource {
    fn family(&mut self, _name: &str) -> Vec<Font> {
        self.family_bytes
            .iter()
            .filter_map(|bytes| Font::from_bytes(bytes.clone()))
            .collect()
    }

    fn face_for_codepoint(&mut self, codepoint: char, attrs: FontAttrs) -> Option<Font> {
        self.asked_codepoints.push((codepoint, attrs));
        let font = Font::from_bytes(self.fallback_bytes.clone()?)?;
        font.covers(codepoint).then_some(font)
    }
}

/// A collection that starts empty and answers from its sources still
/// shapes: the first character asks the source, and the style's faces are
/// prepared again once the answer lands.
#[test]
fn an_empty_collection_shapes_from_its_sources() {
    let mut fonts = valo_text::FontCollection::new();
    fonts.add_source(ScriptedSource {
        family_bytes: Some(asset("fira_sans.ttf")),
        fallback_bytes: None,
        asked_codepoints: Vec::new(),
    });

    let mut b = ParagraphBuilder::new(&mut fonts);
    b.add_text(
        "hello",
        &TextStyle::new("Fira Sans", 16.0, valo_geometry::Color::BLACK),
    );
    let mut paragraph = b.build();
    paragraph.layout(1000.0);

    assert!(
        paragraph.longest_line() > 0.0,
        "the source's face shaped the text"
    );
    assert!(paragraph.demand().is_empty());
}
