//! FaceSet registration semantics: parse-once fonts, alias names
//! (Skia's `registerTypeface(typeface, familyName)`), and per-glyph
//! coverage across same-family faces (cn-font-split subset chunks).

use valo_text::{FaceSet, Font, FontAttrs};

fn asset(name: &str) -> Vec<u8> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    std::fs::read(format!("{dir}/{name}")).unwrap()
}

#[test]
fn from_bytes_reads_embedded_family() {
    let font = Font::from_bytes(asset("fira_sans.ttf")).unwrap();
    assert_eq!(font.family(), "Fira Sans");
    assert_eq!(font.attrs(), FontAttrs::default());
}

#[test]
fn alias_matches_alongside_embedded_name() {
    let mut font = Font::from_bytes(asset("fira_sans.ttf")).unwrap();
    font.add_alias("Picker Family Name");
    font.add_alias("Fira Sans"); // duplicate of the embedded name — dropped
    let mut c = FaceSet::default();
    let id = c.add(font);

    assert_eq!(c.family("Fira Sans"), Some(id));
    assert_eq!(c.family("Picker Family Name"), Some(id));
    assert_eq!(
        c.family_variant("Picker Family Name", FontAttrs::default()),
        Some(id)
    );
    assert_eq!(c.family("Unknown"), None);
}

#[test]
fn resolve_searches_every_face_of_a_family() {
    // Two faces, ONE family, disjoint coverage — the subset-chunk shape.
    // The first-registered face wins ties on attrs but lacks Arabic; the
    // coverage search must look past it instead of returning tofu.
    let mut c = FaceSet::default();
    let latin = c.register("Chunked", asset("fira_sans.ttf")).unwrap();
    let arabic = c
        .register("Chunked", asset("noto_sans_arabic.ttf"))
        .unwrap();

    let families = vec!["Chunked".to_owned()];
    assert_eq!(c.resolve(&families, FontAttrs::default(), 'A'), latin);
    assert_eq!(c.resolve(&families, FontAttrs::default(), 'ع'), arabic);
}

#[test]
fn fallback_chain_expands_to_every_face_of_a_family() {
    // The 🐟 bug: a fallback FAMILY served as unicode-range chunks is many
    // faces under one name. A chain holding only `family()`'s first match
    // reaches chunk 1 (🍶 renders) but never chunk 2 (🐟 stays missing
    // forever, though its face is registered). `faces` is the expansion
    // hosts build chains with.
    let mut c = FaceSet::default();
    let latin = c.register("Chunked", asset("fira_sans.ttf")).unwrap();
    let arabic = c
        .register("Chunked", asset("noto_sans_arabic.ttf"))
        .unwrap();
    assert_eq!(c.faces("Chunked").collect::<Vec<_>>(), vec![latin, arabic]);
    let c = c.with_fallbacks(c.faces("Chunked").collect());

    // The requested family is absent — everything rides the fallback chain,
    // and BOTH chunks must be reachable.
    let families = vec!["Missing".to_owned()];
    assert_eq!(
        c.resolve_covered(&families, FontAttrs::default(), 'A'),
        (latin, true)
    );
    assert_eq!(
        c.resolve_covered(&families, FontAttrs::default(), 'ع'),
        (arabic, true)
    );
}

#[test]
fn resolve_prefers_families_over_fallbacks_and_tofus_in_the_first() {
    let mut c = FaceSet::default();
    let latin = c.register("Fira Sans", asset("fira_sans.ttf")).unwrap();
    let hebrew = c
        .register("Noto Sans Hebrew", asset("noto_sans_hebrew.ttf"))
        .unwrap();
    c.add_fallback(hebrew);

    let families = vec!["Fira Sans".to_owned()];
    // Covered by the family: stays there.
    assert_eq!(c.resolve(&families, FontAttrs::default(), 'A'), latin);
    // Only the fallback covers Hebrew.
    assert_eq!(c.resolve(&families, FontAttrs::default(), 'א'), hebrew);
    // Nobody covers Arabic: tofu renders in the requested family.
    assert_eq!(c.resolve(&families, FontAttrs::default(), 'ع'), latin);
}

#[test]
fn appended_faces_are_visible_by_length() {
    let font = Font::from_bytes(asset("fira_sans.ttf")).unwrap();
    let a = FaceSet::default();
    let (b, id) = a.with_font(font);
    assert_eq!((a.len(), b.len()), (0, 1));
    assert_eq!(b.get(id).family(), "Fira Sans");
}

/// A built paragraph carries the demand signal — families the
/// collection lacks entirely, and chars nothing present covers. valo only
/// DETECTS; where fonts come from is the host's policy.
#[test]
fn paragraphs_report_font_demand() {
    use valo_text::{ParagraphBuilder, TextStyle};
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
fn shared_bytes_parse_like_owned_bytes() {
    // FontData admits any byte owner (memory-mapped files from system-font
    // sources); the parse must not care which one it got.
    struct Mapped(Vec<u8>);
    impl AsRef<[u8]> for Mapped {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }
    let bytes = asset("fira_sans.ttf");
    let owned = Font::from_bytes(bytes.clone()).unwrap();
    let shared = Font::from_data(std::sync::Arc::new(Mapped(bytes)), 0).unwrap();

    assert_eq!(shared.family(), owned.family());
    assert_eq!(shared.attrs(), owned.attrs());
    assert_eq!(shared.face_index(), 0);
    assert_eq!(shared.data(), owned.data());
}

#[test]
fn family_names_match_case_insensitively() {
    // CSS and the platform managers both match ASCII-case-insensitively;
    // exact-case matching once made a demand loop non-terminating (the
    // answer never satisfied the differently-cased request).
    let mut c = FaceSet::default();
    let id = c.add(Font::from_bytes(asset("fira_sans.ttf")).unwrap());
    assert_eq!(c.family("fira sans"), Some(id));
    assert_eq!(c.family("FIRA SANS"), Some(id));
}

#[test]
fn fallback_chain_picks_nearest_attrs_among_covering() {
    // Two chain faces covering the same script at different weights: a
    // bold span's uncovered char pulls the bold one (Skia's style-aware
    // fallback), not merely the first chain entry that covers.
    let bold_attrs = FontAttrs {
        weight: 700,
        italic: false,
    };
    let mut c = FaceSet::default();
    let regular = c
        .register_with("Chain", FontAttrs::default(), asset("fira_sans.ttf"))
        .unwrap();
    let bold = c
        .register_with("Chain", bold_attrs, asset("fira_sans.ttf"))
        .unwrap();
    c.add_fallback(regular);
    c.add_fallback(bold);

    let families = vec!["Absent".to_owned()];
    assert_eq!(c.resolve(&families, bold_attrs, 'A'), bold);
    assert_eq!(c.resolve(&families, FontAttrs::default(), 'A'), regular);
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

#[test]
fn grown_by_teaches_the_requested_name_and_answers_once() {
    let mut source = ScriptedSource {
        family_bytes: Some(asset("fira_sans.ttf")),
        fallback_bytes: None,
        asked_codepoints: Vec::new(),
    };
    let base = FaceSet::default();
    let demand = valo_text::FontDemand {
        families: vec!["my display font".to_owned()],
        codepoints: Vec::new(),
    };

    let grown = base.grown_by(&mut source, &demand).unwrap();
    // Registered under its own table name AND the requested spelling —
    // the next layout's lookup must hit, whatever the divergence was.
    assert!(grown.family("Fira Sans").is_some());
    assert!(grown.family("my display font").is_some());

    // A stale copy of the same demand grows nothing (no duplicates).
    assert!(grown.grown_by(&mut source, &demand).is_none());
}

#[test]
fn grown_by_answers_codepoints_with_the_demanding_attrs() {
    let bold_attrs = FontAttrs {
        weight: 700,
        italic: false,
    };
    let mut source = ScriptedSource {
        family_bytes: None,
        fallback_bytes: Some(asset("noto_sans_arabic.ttf")),
        asked_codepoints: Vec::new(),
    };
    let mut base = FaceSet::default();
    base.register("Fira Sans", asset("fira_sans.ttf")).unwrap();
    let demand = valo_text::FontDemand {
        families: Vec::new(),
        codepoints: vec![('ع', bold_attrs)],
    };

    let grown = base.grown_by(&mut source, &demand).unwrap();
    // The span's attrs traveled to the source (bold text wants a bold
    // fallback), and the answer landed on the fallback chain.
    assert_eq!(source.asked_codepoints, vec![('ع', bold_attrs)]);
    assert!(grown.resolve_covered(&[], FontAttrs::default(), 'ع').1);

    // Covered now: the same demand is stale and grows nothing.
    assert!(grown.grown_by(&mut source, &demand).is_none());
}

#[test]
fn grown_by_reports_nothing_when_the_source_cannot_answer() {
    let mut source = ScriptedSource {
        family_bytes: None,
        fallback_bytes: None,
        asked_codepoints: Vec::new(),
    };
    let mut base = FaceSet::default();
    base.register("Fira Sans", asset("fira_sans.ttf")).unwrap();
    let demand = valo_text::FontDemand {
        families: vec!["Ghost".to_owned()],
        codepoints: vec![('中', FontAttrs::default())],
    };
    assert!(base.grown_by(&mut source, &demand).is_none());
}

#[test]
fn grown_by_never_answers_private_use_codepoints() {
    // An unregistered icon font's codepoints must render tofu, not some
    // installed vendor font that happens to map the Private Use Area.
    let mut source = ScriptedSource {
        family_bytes: None,
        fallback_bytes: Some(asset("fira_sans.ttf")),
        asked_codepoints: Vec::new(),
    };
    let mut base = FaceSet::default();
    base.register("Fira Sans", asset("fira_sans.ttf")).unwrap();
    let demand = valo_text::FontDemand {
        families: Vec::new(),
        codepoints: vec![('\u{E58A}', FontAttrs::default())],
    };
    assert!(base.grown_by(&mut source, &demand).is_none());
    assert!(
        source.asked_codepoints.is_empty(),
        "the source is never even consulted for PUA"
    );
}

#[test]
fn static_fonts_offer_exactly_one_instance() {
    let instances = Font::instances_from_data(std::sync::Arc::new(asset("fira_sans.ttf")), 0);
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].attrs(), FontAttrs::default());
    assert!(instances[0].variation_coordinates().is_empty());
}
