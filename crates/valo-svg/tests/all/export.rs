//! Lists written as SVG: drawn back through the SVG reader and compared with the list, glyphs
//! defined once, and effects as SVG's own filters and masks or as pictures, as `NativeEffects`
//! says.

use std::sync::Arc;

use valo::{
    BlendMode, ClipOp, Color, ColorFilter, Context, DisplayList, DisplayListBuilder, FillRule,
    GradientStop, ImageFilter, MaskBlur, Paint, PaintStyle, PathBuilder, Point, Rect, Shader,
    SpreadMode, Stroke,
};
use valo_paragraph::{DrawParagraphExt, ParagraphBuilder, TextStyle};
use valo_svg::{NativeEffects, SvgOptions};
use valo_text::FontCollection;

const SIZE: [f32; 2] = [200.0, 120.0];

fn options(native: NativeEffects) -> SvgOptions {
    SvgOptions {
        pixels_per_unit: 2.0,
        quality: 90,
        native,
    }
}

fn context(test: &str) -> Option<Context> {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP {test}: no GPU adapter");
        return None;
    };
    Some(Context::new(device, queue))
}

/// `svg_of` writes `list` as an SVG document, waiting on the device for its pictures.
fn svg_of(context: &mut Context, list: &Arc<DisplayList>, native: NativeEffects) -> String {
    let svg = valo_svg::write(context, list, SIZE, options(native));
    context
        .device()
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    pollster::block_on(svg).unwrap()
}

/// `pixels` is `list` drawn at the document's size over white.
fn pixels(context: &mut Context, list: impl Into<Arc<DisplayList>>) -> Vec<u8> {
    let size = [SIZE[0] as u32, SIZE[1] as u32];
    valo_harness::render_to_rgba(context, list, size, Some(Color::WHITE))
}

/// `assert_looks_alike` fails when more than 2% of the pixels differ visibly. Only edges may: valo
/// draws small text from a glyph atlas and an SVG's outlines as paths, which antialias apart.
fn assert_looks_alike(drawn: &[u8], read_back: &[u8]) {
    let differing = drawn
        .as_chunks::<4>()
        .0
        .iter()
        .zip(read_back.as_chunks::<4>().0)
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 24))
        .count();
    let share = differing as f32 / (drawn.len() / 4) as f32;
    assert!(share < 0.02, "{:.2}% of the pixels differ", share * 100.0);
}

fn fonts(file: &str) -> FontCollection {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut fonts = FontCollection::new();
    let id = fonts
        .register("Test", std::fs::read(format!("{dir}/{file}")).unwrap())
        .unwrap();
    fonts.add_fallback(id);
    fonts
}

fn text(b: &mut DisplayListBuilder, file: &str, words: &str, at: Point) {
    let mut fonts = fonts(file);
    let mut paragraph = ParagraphBuilder::new(&mut fonts);
    paragraph.add_text(words, &TextStyle::new("Test", 24.0, Color::BLACK));
    let mut paragraph = paragraph.build();
    paragraph.layout(f32::INFINITY);
    b.draw_paragraph(&paragraph, at);
}

fn solid(r: f32, g: f32, b: f32) -> Paint {
    Paint::from_color(Color::rgb(r, g, b))
}

#[test]
fn shapes_and_text_read_back_as_they_were_drawn() {
    let Some(mut context) = context("shapes_and_text_read_back_as_they_were_drawn") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 200.0, 120.0),
        &Paint::from_shader(Shader::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(200.0, 0.0),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: Color::rgb(0.9, 0.95, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: Color::rgb(1.0, 0.9, 0.8),
                },
            ],
            spread: SpreadMode::Pad,
            local: valo::Matrix::IDENTITY,
        }),
    );
    let mut card = PathBuilder::new();
    card.rrect(Rect::new(110.0, 20.0, 70.0, 60.0), 12.0);
    let card = card.build();
    b.draw_path(&card, FillRule::NonZero, &solid(0.2, 0.4, 0.9));
    b.draw_path(
        &card,
        FillRule::NonZero,
        &Paint {
            style: PaintStyle::Stroke(Stroke::new(4.0)),
            ..solid(0.1, 0.1, 0.3)
        },
    );
    b.save();
    b.translate(60.0, 90.0);
    b.rotate(0.3);
    b.clip_rect(Rect::new(-20.0, -10.0, 40.0, 20.0), ClipOp::Intersect);
    b.draw_rect(Rect::new(-40.0, -40.0, 80.0, 80.0), &solid(0.9, 0.2, 0.2));
    b.restore();
    b.save_layer(
        None,
        &Paint {
            blend_mode: BlendMode::Multiply,
            ..Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.6))
        },
    );
    b.draw_rect(Rect::new(140.0, 60.0, 50.0, 50.0), &solid(1.0, 0.8, 0.0));
    b.restore();
    text(&mut b, "fira_sans.ttf", "office", Point::new(10.0, 10.0));
    let page = Arc::new(b.build());

    let svg = svg_of(&mut context, &page, NativeEffects::default());
    let read_back = valo_svg::translate(svg.as_bytes()).unwrap();

    assert!(read_back.missing.is_empty(), "{:?}", read_back.missing);
    assert!(!svg.contains("<image"), "every part is SVG's own");
    assert_looks_alike(
        &pixels(&mut context, page),
        &pixels(&mut context, read_back.list),
    );
}

#[test]
fn text_is_outlines_each_glyph_defined_once() {
    let Some(mut context) = context("text_is_outlines_each_glyph_defined_once") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    text(&mut b, "fira_sans.ttf", "noon noon", Point::new(10.0, 10.0));

    let svg = svg_of(&mut context, &Arc::new(b.build()), NativeEffects::default());

    assert!(!svg.contains("<text"));
    assert_eq!(svg.matches(r#"<path id="t"#).count(), 2, "n and o");
    assert_eq!(svg.matches("<use ").count(), 8, "the space has no outline");
}

/// `shadowed` is a card with a shape's shadow under it, and a text with a group's shadow.
fn shadowed() -> Arc<DisplayList> {
    let mut b = DisplayListBuilder::new();
    b.draw_rrect(
        Rect::new(24.0, 24.0, 80.0, 60.0),
        10.0,
        &Paint {
            mask_blur: Some(MaskBlur::new(6.0)),
            ..Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.4))
        },
    );
    b.draw_rrect(
        Rect::new(20.0, 20.0, 80.0, 60.0),
        10.0,
        &solid(1.0, 1.0, 1.0),
    );
    let mut words = DisplayListBuilder::new();
    text(&mut words, "fira_sans.ttf", "Sale", Point::new(120.0, 40.0));
    let words = Arc::new(words.build());
    b.save_layer(
        None,
        &Paint {
            color_filter: Some(ColorFilter::Blend(
                Color::rgba(0.0, 0.0, 0.0, 0.5),
                BlendMode::SrcIn,
            )),
            image_filter: Some(ImageFilter::blur(3.0, 3.0)),
            ..Paint::default()
        },
    );
    b.translate(2.0, 3.0);
    b.draw_display_list(&words);
    b.restore();
    b.draw_display_list(&words);
    Arc::new(b.build())
}

#[test]
fn shadows_are_svg_filters_where_native_effects_let_them() {
    let Some(mut context) = context("shadows_are_svg_filters_where_native_effects_let_them") else {
        return;
    };

    let svg = svg_of(&mut context, &shadowed(), NativeEffects::default());

    assert_eq!(svg.matches("<feGaussianBlur").count(), 2);
    assert_eq!(
        svg.matches("<feFlood").count(),
        1,
        "the text's shadow is in one colour"
    );
    assert!(!svg.contains("<image"));
}

#[test]
fn shadows_are_pictures_when_native_effects_leave_them_out() {
    let Some(mut context) = context("shadows_are_pictures_when_native_effects_leave_them_out")
    else {
        return;
    };
    let native = NativeEffects {
        shadows: false,
        ..NativeEffects::default()
    };

    let svg = svg_of(&mut context, &shadowed(), native);

    assert!(!svg.contains("<filter"));
    assert_eq!(svg.matches("<image").count(), 2);
    assert!(
        svg.contains("data:image/png;base64,"),
        "a shadow is see-through"
    );
}

#[test]
fn a_colour_glyph_is_its_image_defined_once() {
    let Some(mut context) = context("a_colour_glyph_is_its_image_defined_once") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    text(
        &mut b,
        "noto_color_emoji_subset.ttf",
        "\u{1F680} \u{1F680}",
        Point::new(10.0, 10.0),
    );

    let svg = svg_of(&mut context, &Arc::new(b.build()), NativeEffects::default());

    assert_eq!(
        svg.matches("<image").count(),
        1,
        "the rocket's image, and no picture"
    );
    assert!(svg.contains(r#"<image id="e"#));
    assert_eq!(svg.matches("<use ").count(), 2);
}

#[test]
fn an_assets_mask_is_an_svg_mask_that_reads_back_as_drawn() {
    let Some(mut context) = context("an_assets_mask_is_an_svg_mask_that_reads_back_as_drawn")
    else {
        return;
    };
    let asset = valo_svg::translate(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <mask id="hole"><rect width="100" height="100" fill="white"/><circle cx="50" cy="50" r="30" fill="black"/></mask>
            <rect width="100" height="100" fill="#3366cc" mask="url(#hole)"/>
        </svg>"##,
    )
    .unwrap();
    let mut b = DisplayListBuilder::new();
    b.translate(40.0, 10.0);
    b.draw_display_list(&asset.list);
    let page = Arc::new(b.build());

    let svg = svg_of(&mut context, &page, NativeEffects::default());
    let read_back = valo_svg::translate(svg.as_bytes()).unwrap();

    assert_eq!(svg.matches("<mask ").count(), 1);
    assert!(!svg.contains("<image"));
    assert_looks_alike(
        &pixels(&mut context, page),
        &pixels(&mut context, read_back.list),
    );
}
