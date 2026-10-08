//! What a glyph run's paint does to its glyphs, following Skia: a colour
//! filter acts on each glyph's own colour before its coverage, so it never
//! reaches past the glyphs, and a colour glyph's pixels pass through it too.

use valo::{
    Color, ColorFilter, Context, DisplayListBuilder, DrawGlyphRunExt, FontCollection, Paint,
    Paragraph, ParagraphBuilder, TextStyle, TextTiers,
};

const SIZE: [u32; 2] = [128, 96];
const ORIGIN: (f32, f32) = (16.0, 8.0);

/// Each text tier at the tests' 64 px: bitmap masks (the default), signed
/// distance fields, and outlines.
const TIERS: [(&str, TextTiers); 3] = [
    (
        "mask",
        TextTiers {
            sdf_min: 162.0,
            path_min: 324.0,
        },
    ),
    (
        "sdf",
        TextTiers {
            sdf_min: 32.0,
            path_min: 1000.0,
        },
    ),
    (
        "outline",
        TextTiers {
            sdf_min: 32.0,
            path_min: 48.0,
        },
    ),
];

fn fonts() -> FontCollection {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut fonts = FontCollection::new();
    let latin = fonts
        .register(
            "Fira Sans",
            std::fs::read(format!("{dir}/fira_sans.ttf")).expect("fira_sans.ttf"),
        )
        .expect("register Fira Sans");
    let emoji = fonts
        .register(
            "Noto Color Emoji",
            std::fs::read(format!("{dir}/noto_color_emoji_subset.ttf")).expect("emoji font"),
        )
        .expect("register Noto Color Emoji");
    fonts.add_fallback(latin);
    fonts.add_fallback(emoji);
    fonts
}

fn paragraph(fonts: &mut FontCollection, text: &str) -> Paragraph {
    let mut builder = ParagraphBuilder::new(fonts);
    builder.add_text(text, &TextStyle::new("Fira Sans", 64.0, Color::WHITE));
    let mut paragraph = builder.build();
    paragraph.layout(f32::INFINITY);
    paragraph
}

/// `Pixels` is a straight-alpha RGBA8 rendering of one paragraph.
struct Pixels(Vec<u8>);

impl Pixels {
    fn at(&self, (x, y): (u32, u32)) -> [u8; 4] {
        let at = ((y * SIZE[0] + x) * 4) as usize;
        [self.0[at], self.0[at + 1], self.0[at + 2], self.0[at + 3]]
    }

    fn all(&self) -> impl Iterator<Item = ((u32, u32), [u8; 4])> + '_ {
        (0..SIZE[1]).flat_map(move |y| (0..SIZE[0]).map(move |x| ((x, y), self.at((x, y)))))
    }

    /// `ink_box` is the smallest box around every pixel that shows.
    fn ink_box(&self) -> [u32; 4] {
        let shown = self
            .all()
            .filter(|(_, pixel)| pixel[3] > 0)
            .map(|(at, _)| at);
        shown.fold([u32::MAX, u32::MAX, 0, 0], |[l, t, r, b], (x, y)| {
            [l.min(x), t.min(y), r.max(x), b.max(y)]
        })
    }
}

fn render(context: &mut Context, paragraph: &Paragraph, paint: &Paint) -> Pixels {
    let mut builder = DisplayListBuilder::new();
    builder.draw_paragraph_with(paragraph, ORIGIN, paint);
    Pixels(context.render_to_rgba(&builder.build(), SIZE, Some(Color::TRANSPARENT)))
}

fn context(test: &str) -> Option<Context> {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP {test}: no GPU adapter");
        return None;
    };
    Some(Context::new(device, queue))
}

/// A colour filter that colours transparent pixels colours the glyphs and
/// nothing between them, in every tier: the filter acts on the run's
/// colour, not on the box the run is drawn in.
#[test]
fn a_colour_filter_on_text_stays_inside_its_glyphs() {
    let Some(mut context) = context("a_colour_filter_on_text_stays_inside_its_glyphs") else {
        return;
    };
    let mut fonts = fonts();
    let word = paragraph(&mut fonts, "H");
    for (tier, tiers) in TIERS {
        context.set_text_tiers(tiers);
        stays_inside_its_glyphs(&mut context, &word, tier);
    }
}

fn stays_inside_its_glyphs(context: &mut Context, word: &Paragraph, tier: &str) {
    let blue = Paint::from_color(Color::rgb(0.0, 0.0, 1.0));
    let plain = render(context, word, &blue);
    let [left, top, right, bottom] = plain.ink_box();
    let between_stems = plain
        .all()
        .find(|((x, y), pixel)| {
            (left..=right).contains(x) && (top..=bottom).contains(y) && pixel[3] == 0
        })
        .map(|(at, _)| at)
        .expect("a pixel inside the H's box but outside its glyph");
    let stem = plain
        .all()
        .find(|(_, pixel)| pixel[3] == 255)
        .map(|(at, _)| at)
        .expect("a pixel the H covers whole");

    let fill_red = ColorFilter::Blend(Color::rgb(1.0, 0.0, 0.0), valo::BlendMode::Src);
    let filtered = render(
        context,
        word,
        &Paint {
            color_filter: Some(fill_red),
            ..blue
        },
    );
    assert_eq!(
        filtered.at(between_stems),
        [0, 0, 0, 0],
        "{tier}: between the stems"
    );
    assert_eq!(filtered.at(stem), [255, 0, 0, 255], "{tier}: in a stem");
}

/// A colour glyph's own pixels pass through the colour filter, inside the
/// glyph only, in every tier: a grey matrix turns an emoji grey and leaves
/// its shape.
#[test]
fn a_colour_filter_on_text_reaches_its_colour_glyphs() {
    let Some(mut context) = context("a_colour_filter_on_text_reaches_its_colour_glyphs") else {
        return;
    };
    let mut fonts = fonts();
    let rocket = paragraph(&mut fonts, "🚀");
    for (tier, tiers) in TIERS {
        context.set_text_tiers(tiers);
        reaches_its_colour_glyphs(&mut context, &rocket, tier);
    }
}

fn reaches_its_colour_glyphs(context: &mut Context, rocket: &Paragraph, tier: &str) {
    let white = Paint::from_color(Color::WHITE);
    let plain = render(context, rocket, &white);
    let colourful = |[r, g, b, a]: [u8; 4]| a == 255 && r.max(g).max(b) - r.min(g).min(b) > 40;
    assert!(
        plain.all().any(|(_, pixel)| colourful(pixel)),
        "{tier}: the emoji draws in colour"
    );

    #[rustfmt::skip]
    let grey = ColorFilter::Matrix([
        0.2126, 0.7152, 0.0722, 0.0, 0.0,
        0.2126, 0.7152, 0.0722, 0.0, 0.0,
        0.2126, 0.7152, 0.0722, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]);
    let filtered = render(
        context,
        rocket,
        &Paint {
            color_filter: Some(grey),
            ..white
        },
    );
    for ((at, before), after) in plain.all().zip(filtered.all().map(|(_, pixel)| pixel)) {
        assert_eq!(
            after[3], before[3],
            "{tier}: the emoji keeps its coverage at {at:?}"
        );
        if after[3] >= 16 {
            let [r, g, b, _] = after;
            assert!(
                r.max(g).max(b) - r.min(g).min(b) <= 3,
                "{tier}: grey at {at:?}: {after:?}"
            );
        }
    }
}

/// A shader-painted run with an effect keeps its shader: the effect runs on
/// the run as it draws without the effect, its glyphs filled with the
/// shader, as Skia draws a draw with an image filter whole into a layer. So
/// does one whose blend reads the destination, which also draws it alone
/// into a layer.
#[test]
fn shader_painted_text_keeps_its_shader_under_an_effect() {
    let Some(mut context) = context("shader_painted_text_keeps_its_shader_under_an_effect") else {
        return;
    };
    let mut fonts = fonts();
    let word = paragraph(&mut fonts, "H");
    let blue = Paint::from_color(Color::rgb(0.0, 0.0, 1.0));
    let plain = render(&mut context, &word, &blue);
    let covered = |(x, y): (u32, u32)| {
        (x.saturating_sub(2)..=x + 2)
            .all(|x| (y.saturating_sub(2)..=y + 2).all(|y| plain.at((x, y))[3] == 255))
    };
    let deep_in_a_stem = plain
        .all()
        .find(|&(at, _)| covered(at))
        .map(|(at, _)| at)
        .expect("a pixel deep inside a stem");

    let green = Color::rgb(0.0, 1.0, 0.0);
    let gradient = Paint {
        shader: Some(valo::Shader::linear(
            valo::Point::new(0.0, 0.0),
            valo::Point::new(128.0, 0.0),
            green,
            green,
        )),
        ..Paint::from_color(Color::WHITE)
    };
    let effects = [
        (
            "image filter",
            Paint {
                image_filter: Some(valo::ImageFilter::blur(1.0, 1.0)),
                ..gradient.clone()
            },
        ),
        (
            "mask blur",
            Paint {
                mask_blur: Some(valo::MaskBlur::new(1.0)),
                ..gradient.clone()
            },
        ),
        (
            // Over a transparent target Multiply leaves the source as it is.
            "destination-reading blend",
            Paint {
                blend_mode: valo::BlendMode::Multiply,
                ..gradient.clone()
            },
        ),
    ];
    for (effect, paint) in effects {
        let [r, g, b, a] = render(&mut context, &word, &paint).at(deep_in_a_stem);
        assert!(
            g > 200 && r < 40 && b < 40 && a > 200,
            "{effect}: the stem is the shader's green, not the paint's white: {:?}",
            [r, g, b, a]
        );
    }
}
