//! A family's weights and slants: registration reads them from each file, resolution picks
//! the nearest face, and what no face has is faked (Skia's `fakeMissingFontStyles`).
//!
//! The only test fonts are regular faces, so the styled faces here are Fira Sans with its
//! OS/2 table claiming another weight or slant: the outlines stay Regular, but registration
//! and matching see what a real Medium, Bold or Italic file declares.

use valo_text::{
    FaceSet, FontAttrs, FontCollection, FontSynthesis, Paragraph, ParagraphBuilder, PlacedRun,
    TextStyle,
};

fn asset(name: &str) -> Vec<u8> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    std::fs::read(format!("{dir}/{name}")).unwrap()
}

/// Fira Sans whose OS/2 table declares `weight` and, when `italic`, the italic style.
fn fira_declaring(weight: u16, italic: bool) -> Vec<u8> {
    let mut bytes = asset("fira_sans.ttf");
    let os2 = table_offset(&bytes, b"OS/2");
    bytes[os2 + 4..os2 + 6].copy_from_slice(&weight.to_be_bytes());
    if italic {
        let selection = u16::from_be_bytes([bytes[os2 + 62], bytes[os2 + 63]]);
        // `fsSelection`: ITALIC on, REGULAR off.
        let selection = (selection | 0x0001) & !0x0040;
        bytes[os2 + 62..os2 + 64].copy_from_slice(&selection.to_be_bytes());
    }
    bytes
}

/// Where the table tagged `tag` starts in an SFNT file.
fn table_offset(bytes: &[u8], tag: &[u8; 4]) -> usize {
    let count = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    (0..count)
        .map(|index| 12 + 16 * index)
        .find(|record| &bytes[*record..*record + 4] == tag)
        .map(|record| u32::from_be_bytes(bytes[record + 8..record + 12].try_into().unwrap()))
        .expect("the table is in the file") as usize
}

fn attrs(weight: u16, italic: bool) -> FontAttrs {
    FontAttrs {
        weight,
        italic,
        ..FontAttrs::default()
    }
}

/// The Seede case: four static faces registered under one family name each claim their own
/// weight, so every weight request finds its face instead of tying on 400 and taking the
/// first registered.
#[test]
fn registered_faces_match_their_own_weights() {
    let mut faces = FaceSet::default();
    let weights = [400, 500, 600, 700];
    let ids: Vec<_> = weights
        .iter()
        .map(|&weight| {
            faces
                .register("Inter", fira_declaring(weight, false))
                .unwrap()
        })
        .collect();
    for (weight, id) in weights.into_iter().zip(ids) {
        assert_eq!(faces.get(id).attrs().weight, weight);
        assert_eq!(
            faces.family_variant("Inter", attrs(weight, false)),
            Some(id),
            "a {weight} request"
        );
    }
}

/// Registration order no longer decides: Bold registered first still answers only bold.
#[test]
fn registration_order_does_not_pick_the_weight() {
    let mut faces = FaceSet::default();
    let bold = faces.register("Inter", fira_declaring(700, false)).unwrap();
    let regular = faces.register("Inter", fira_declaring(400, false)).unwrap();
    assert_eq!(
        faces.family_variant("Inter", attrs(400, false)),
        Some(regular)
    );
    assert_eq!(faces.family_variant("Inter", attrs(700, false)), Some(bold));
}

#[test]
fn an_italic_file_matches_an_italic_request() {
    let mut faces = FaceSet::default();
    let upright = faces.register("Inter", fira_declaring(400, false)).unwrap();
    let italic = faces.register("Inter", fira_declaring(400, true)).unwrap();
    assert!(faces.get(italic).attrs().italic);
    assert_eq!(
        faces.family_variant("Inter", attrs(400, true)),
        Some(italic)
    );
    assert_eq!(
        faces.family_variant("Inter", attrs(400, false)),
        Some(upright)
    );
}

/// The face answers to the name it was registered under, and not to the file's own family.
#[test]
fn a_registered_face_answers_only_its_given_name() {
    let mut faces = FaceSet::default();
    let id = faces.register("Brand", asset("fira_sans.ttf")).unwrap();
    assert_eq!(faces.get(id).family(), "Brand");
    assert!(faces.get(id).aliases().is_empty());
    assert_eq!(faces.family("Fira Sans"), None);
}

/// `text` laid out on one line in Inter at `weight` and `italic`, 32 px.
fn paragraph(fonts: &mut FontCollection, text: &str, weight: u16, italic: bool) -> Paragraph {
    let style = TextStyle {
        weight,
        italic,
        ..TextStyle::new("Inter", 32.0, valo_geometry::Color::BLACK)
    };
    let mut paragraph = ParagraphBuilder::new(fonts).add_text(text, &style).build();
    paragraph.layout(f32::INFINITY);
    paragraph
}

fn runs(paragraph: &Paragraph) -> Vec<&PlacedRun> {
    paragraph
        .lines()
        .iter()
        .flat_map(|line| &line.runs)
        .collect()
}

fn regular_only() -> FontCollection {
    let mut fonts = FontCollection::new();
    fonts.register("Inter", fira_declaring(400, false)).unwrap();
    fonts
}

/// Bold asked of a family with only a Regular face fakes it, with the stroke Skia uses at this
/// size, and moves nothing: glyph for glyph, the positions and advances are Regular's.
#[test]
fn fake_bold_keeps_the_faces_advances() {
    let mut fonts = regular_only();
    let regular = paragraph(&mut fonts, "Hamburgefonstiv", 400, false);
    let bold = paragraph(&mut fonts, "Hamburgefonstiv", 700, false);
    let (regular, bold) = (runs(&regular), runs(&bold));
    assert_eq!(regular[0].synthesis, FontSynthesis::NONE);
    assert_eq!(bold[0].synthesis.skew, 0.0);
    let stroke = bold[0].synthesis.embolden;
    assert!(stroke > 1.0 / 32.0 && stroke < 1.0 / 24.0, "{stroke}");
    for (plain, synthesized) in regular[0].glyphs.iter().zip(&bold[0].glyphs) {
        assert_eq!(
            (plain.x, plain.advance),
            (synthesized.x, synthesized.advance)
        );
    }
}

/// A family that has the weight or the slant draws its own face, unfaked.
#[test]
fn no_synthesis_when_the_face_has_the_style() {
    let mut fonts = FontCollection::new();
    fonts.register("Inter", fira_declaring(400, false)).unwrap();
    let bold = fonts.register("Inter", fira_declaring(700, false)).unwrap();
    let italic = fonts.register("Inter", fira_declaring(400, true)).unwrap();
    for (weight, slanted, face) in [(700, false, bold), (400, true, italic)] {
        let paragraph = paragraph(&mut fonts, "Hamburgefonstiv", weight, slanted);
        for run in runs(&paragraph) {
            assert_eq!(run.font, face);
            assert_eq!(
                run.synthesis,
                FontSynthesis::NONE,
                "{weight} italic={slanted}"
            );
        }
    }
    // Bold italic: matching takes the slant before the weight, so the Italic face answers and
    // only the bold is faked on it.
    let paragraph = paragraph(&mut fonts, "Hamburgefonstiv", 700, true);
    let run = runs(&paragraph)[0];
    assert_eq!(run.font, italic);
    assert!(run.synthesis.embolden > 0.0);
    assert_eq!(run.synthesis.skew, 0.0);
}

/// A run's ink box is what culling and layers trust, so it must still hold every synthesized
/// glyph: the slant reaches right of the last glyph's top, and fake bold grows every edge.
#[test]
fn run_ink_holds_the_synthesized_glyphs() {
    let mut fonts = regular_only();
    for (weight, italic) in [(700, false), (400, true), (900, true)] {
        let paragraph = paragraph(&mut fonts, "Wolf fill", weight, italic);
        let ink = runs(&paragraph)
            .iter()
            .map(|run| run.ink)
            .reduce(|a, b| a.union(&b))
            .unwrap();
        let drawn = paragraph.ink_bounds().unwrap();
        let holds = ink.x <= drawn.x
            && ink.y <= drawn.y
            && ink.right() >= drawn.right()
            && ink.y + ink.height >= drawn.y + drawn.height;
        assert!(
            holds,
            "{weight} italic={italic}: run ink {ink:?} misses drawn {drawn:?}"
        );
        let plain = paragraph_ink(&mut fonts, "Wolf fill");
        assert!(
            drawn.right() > plain.right(),
            "the synthesized ink reaches further right"
        );
    }
}

fn paragraph_ink(fonts: &mut FontCollection, text: &str) -> valo_geometry::Rect {
    paragraph(fonts, text, 400, false).ink_bounds().unwrap()
}
