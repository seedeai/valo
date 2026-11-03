//! A family's weights and slants: registration reads them from each file, and resolution
//! picks the nearest face. What no face has is faked by the paragraph that asks for it,
//! tested in `valo-paragraph`.
//!
//! The only test fonts are regular faces, so the styled faces here are Fira Sans with its
//! OS/2 table claiming another weight or slant: the outlines stay Regular, but registration
//! and matching see what a real Medium, Bold or Italic file declares.

use valo_text::{FaceSet, FontAttrs};

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

#[test]
fn weight_matching_picks_nearest_variant() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let bytes = std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap();
    let mut c = FaceSet::default();
    let regular = c
        .register_with(
            "Fira",
            valo_text::FontAttrs {
                weight: 400,
                italic: false,
                ..Default::default()
            },
            bytes.clone(),
        )
        .unwrap();
    let bold = c
        .register_with(
            "Fira",
            valo_text::FontAttrs {
                weight: 700,
                italic: false,
                ..Default::default()
            },
            bytes,
        )
        .unwrap();
    assert_eq!(
        c.family_variant(
            "Fira",
            valo_text::FontAttrs {
                weight: 400,
                italic: false,
                ..Default::default()
            }
        ),
        Some(regular)
    );
    assert_eq!(
        c.family_variant(
            "Fira",
            valo_text::FontAttrs {
                weight: 800,
                italic: false,
                ..Default::default()
            }
        ),
        Some(bold)
    );
    assert_eq!(
        c.family_variant(
            "Fira",
            valo_text::FontAttrs {
                weight: 550,
                italic: true,
                ..Default::default()
            }
        ),
        Some(regular)
    );
}
