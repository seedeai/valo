//! What selection and fallback actually do, case by case, against this
//! machine's installed fonts:
//! `cargo run -p valo-system-fonts --example selection_demo`

use std::sync::Arc;

use valo_system_fonts::SystemFonts;
use valo_text::{
    FaceSet, Font, FontAttrs, FontCollection, FontId, FontSource, ParagraphBuilder, TextStyle,
};

fn attrs(weight: u16) -> FontAttrs {
    FontAttrs {
        weight,
        italic: false,
    }
}

fn describe(collection: &FaceSet, id: FontId) -> String {
    let font = collection.get(id);
    format!(
        "{} — weight {}, italic {}, face_index {}",
        font.family(),
        font.attrs().weight,
        font.attrs().italic,
        font.face_index()
    )
}

fn fira_sans() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/fonts/fira_sans.ttf"
    );
    std::fs::read(path).expect("fira_sans.ttf")
}

fn main() {
    let mut system = SystemFonts::load();
    println!("scan: {} installed faces\n", system.face_count());

    weight_and_style_matching(&mut system);
    family_list_order(&mut system);
    localized_aliases(&mut system);
    the_demand_loop(&mut system);
    true_tofu(&mut system);
}

/// One family, many requests: nearest variant by (style, weight distance).
fn weight_and_style_matching(system: &mut SystemFonts) {
    println!("── weight/style matching inside one family ──");
    let mut collection = FaceSet::default();
    for font in system.family("Helvetica") {
        collection.add(font);
    }
    println!("registered faces of Helvetica:");
    for at in 0..collection.len() {
        println!("    {}", describe(&collection, FontId(at as u32)));
    }
    for request in [400, 700, 600, 550, 300, 100] {
        let id = collection
            .family_variant("Helvetica", attrs(request))
            .unwrap();
        println!("  request weight {request} → {}", describe(&collection, id));
    }
    let italic = collection
        .family_variant(
            "Helvetica",
            FontAttrs {
                weight: 400,
                italic: true,
            },
        )
        .unwrap();
    println!("  request italic     → {}", describe(&collection, italic));

    let mut pingfang = FaceSet::default();
    for font in system.family("PingFang SC") {
        pingfang.add(font);
    }
    if let Some(id) = pingfang.family_variant(
        "PingFang SC",
        FontAttrs {
            weight: 400,
            italic: true,
        },
    ) {
        println!(
            "  italic from a family with NO italics (PingFang SC) → {} (upright: no synthesis)",
            describe(&pingfang, id)
        );
    }
    println!();
}

/// The CSS family list: first name that covers the character wins.
fn family_list_order(system: &mut SystemFonts) {
    println!("── family list order ──");
    let mut collection = FaceSet::default();
    collection.register("Fira Sans", fira_sans()).unwrap();
    for font in system.family("Helvetica") {
        collection.add(font);
    }
    let families = vec![
        "No Such Family".to_owned(),
        "Helvetica".to_owned(),
        "Fira Sans".to_owned(),
    ];
    let (id, covered) = collection.resolve_covered(&families, attrs(400), 'A');
    println!(
        "  [\"No Such Family\", \"Helvetica\", \"Fira Sans\"] for 'A' → {} (covered {covered})",
        describe(&collection, id)
    );
    println!();
}

/// Localized name-table entries all register as aliases.
fn localized_aliases(system: &mut SystemFonts) {
    println!("── localized aliases ──");
    let mut collection = FaceSet::default();
    for font in system.family("PingFang SC") {
        collection.add(font);
    }
    if collection.is_empty() {
        println!("  (PingFang SC not installed)\n");
        return;
    }
    let first = collection.get(FontId(0));
    println!(
        "  PingFang SC face 0 answers to: {:?} + aliases {:?}",
        first.family(),
        first.aliases()
    );
    for name in ["PINGFANG sc", "苹方-简"] {
        println!(
            "  family(\"{name}\") → {:?}",
            collection.family(name).map(|id| describe(&collection, id))
        );
    }
    println!();
}

/// The real flow: shaping reports, the source answers, relayout is clean.
fn the_demand_loop(system: &mut SystemFonts) {
    println!("── the demand loop (paragraph: bold \"Hello 中文\" in \"helvetica\") ──");
    let mut collection = FaceSet::default();
    collection.register("Fira Sans", fira_sans()).unwrap();
    let mut fonts = FontCollection::new();
    fonts.adopt_faces(collection);

    let style = |family: &str| {
        let mut style = TextStyle::new(family, 16.0, valo_geometry::Color::BLACK);
        style.weight = 700;
        style
    };
    let mut builder = ParagraphBuilder::new(&mut fonts);
    builder.add_text("Hello 中文", &style("helvetica"));
    let paragraph = builder.build();
    println!("  demand after first layout: {:?}", paragraph.demand());

    let grown = fonts.faces().grown_by(system, paragraph.demand()).unwrap();
    println!("  grown collection now holds:");
    for at in fonts.len()..grown.len() {
        println!("    + {}", describe(&grown, FontId(at as u32)));
    }
    let (resolved, _) = grown.resolve_covered(&["helvetica".to_owned()], attrs(700), 'H');
    let resolved_name = describe(&grown, resolved);
    let (cjk, _) = grown.resolve_covered(&["helvetica".to_owned()], attrs(700), '中');
    let cjk_name = describe(&grown, cjk);
    let mut grown_collection = FontCollection::new();
    grown_collection.adopt_faces(grown);

    let mut builder = ParagraphBuilder::new(&mut grown_collection);
    builder.add_text("Hello 中文", &style("helvetica"));
    let rebuilt = builder.build();
    println!("  demand after relayout: {:?}", rebuilt.demand());
    println!("  'H' now renders in: {resolved_name}");
    println!("  '中' now renders in: {cjk_name}");
    println!();
}

/// Nothing installed covers it: tofu face + the demand records it.
fn true_tofu(system: &mut SystemFonts) {
    println!("── true tofu (U+13000 EGYPTIAN HIEROGLYPH A001) ──");
    let mut collection = FaceSet::default();
    collection.register("Fira Sans", fira_sans()).unwrap();
    let answer = system.face_for_codepoint('\u{13000}', attrs(400));
    println!("  system answer: {:?}", answer.as_ref().map(Font::family));
    let (id, covered) = collection.resolve_covered(&[], attrs(400), '\u{13000}');
    println!(
        "  resolve → {} (covered {covered}: renders .notdef, demand records the char)",
        describe(&collection, id)
    );
}
