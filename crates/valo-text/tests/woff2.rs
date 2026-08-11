#![cfg(feature = "woff2")]
//! WOFF2 registration: icon and web fonts ship brotli-wrapped; the `woff2`
//! feature unwraps them at the registration boundary.

use valo_text::{FaceSet, Font, FontAttrs, FontCollection};

fn font_awesome_woff2() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/fonts/fa_regular_400.woff2"
    );
    std::fs::read(path).expect("fa_regular_400.woff2")
}

#[test]
fn woff2_bytes_register_and_cover_their_icons() {
    let font = Font::from_bytes(font_awesome_woff2()).expect("woff2 unwraps and parses");
    assert!(
        font.family().starts_with("Font Awesome"),
        "{}",
        font.family()
    );
    assert!(
        font.covers('\u{f005}'),
        "the star icon maps (Private Use Area)"
    );

    // The registered-alias route works on compressed bytes too.
    let mut collection = FaceSet::default();
    let id = collection
        .register("icons", font_awesome_woff2())
        .expect("register unwraps");
    assert_eq!(collection.family("icons"), Some(id));
    assert_eq!(
        collection.resolve(&["icons".to_owned()], FontAttrs::default(), '\u{f005}'),
        id
    );
}
