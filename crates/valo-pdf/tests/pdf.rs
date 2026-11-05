//! Pages written to a PDF and read back: real text that can be searched, shapes that stay
//! shapes, and a shadow PDF can't hold drawn as a picture.

use std::sync::Arc;

use valo::{Color, Context, DisplayList, DisplayListBuilder, MaskBlur, Paint, Point, Rect, Shader};
use valo_paragraph::{DrawParagraphExt, ParagraphBuilder, TextStyle};
use valo_pdf::{draw_page, PageSettings, PdfDocument};
use valo_text::FontCollection;

const SETTINGS: PageSettings = PageSettings {
    points_per_unit: 0.75,
    pixels_per_unit: 2.0,
    quality: 90,
};

fn context(test: &str) -> Option<Context> {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP {test}: no GPU adapter");
        return None;
    };
    Some(Context::new(device, queue))
}

/// `pdf_of` writes each list as a page of `size`, waiting on the device for its pictures.
fn pdf_of(context: &mut Context, lists: Vec<DisplayList>, size: [f32; 2]) -> lopdf::Document {
    let mut document = PdfDocument::new();
    for list in lists {
        let page = draw_page(context, &Arc::new(list), size, SETTINGS);
        context
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        document.add(pollster::block_on(page).unwrap()).unwrap();
    }
    lopdf::Document::load_mem(&document.finish().unwrap()).unwrap()
}

/// `text` is a line of Fira Sans at 24 px.
fn text(builder: &mut DisplayListBuilder, words: &str) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut fonts = FontCollection::new();
    let fira = fonts
        .register(
            "Fira Sans",
            std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap(),
        )
        .unwrap();
    fonts.add_fallback(fira);
    let mut paragraph = ParagraphBuilder::new(&mut fonts);
    paragraph.add_text(words, &TextStyle::new("Fira Sans", 24.0, Color::BLACK));
    let mut paragraph = paragraph.build();
    paragraph.layout(f32::INFINITY);
    builder.draw_paragraph(&paragraph, Point::new(20.0, 20.0));
}

/// `images` counts the pictures with transparency in `pdf`: each carries its alpha as a soft mask,
/// an image of its own, which isn't counted.
fn images(pdf: &lopdf::Document) -> usize {
    pdf.objects
        .values()
        .filter_map(|object| object.as_stream().ok())
        .filter(|stream| {
            stream
                .dict
                .get(b"Subtype")
                .and_then(|kind| kind.as_name())
                .ok()
                == Some(b"Image")
                && stream.dict.get(b"SMask").is_ok()
        })
        .count()
}

#[test]
fn text_stays_text_that_reads_back_ligatures_and_all() {
    let Some(mut context) = context("text_stays_text_that_reads_back_ligatures_and_all") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 300.0, 100.0),
        &Paint::from_shader(Shader::linear(
            Point::new(0.0, 0.0),
            Point::new(300.0, 0.0),
            Color::rgb(1.0, 0.9, 0.8),
            Color::rgb(0.8, 0.9, 1.0),
        )),
    );
    text(&mut b, "office files");
    let pdf = pdf_of(&mut context, vec![b.build()], [300.0, 100.0]);
    assert_eq!(pdf.get_pages().len(), 1);
    let words = pdf.extract_text(&[1]).unwrap();
    assert!(words.contains("office files"), "read back {words:?}");
    assert_eq!(images(&pdf), 0, "shapes and text need no picture");
}

#[test]
fn a_shadow_goes_in_as_a_picture_under_its_shape() {
    let Some(mut context) = context("a_shadow_goes_in_as_a_picture_under_its_shape") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.draw_rrect(
        Rect::new(40.0, 40.0, 120.0, 80.0),
        12.0,
        &Paint {
            mask_blur: Some(MaskBlur::new(8.0)),
            ..Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.4))
        },
    );
    b.draw_rrect(
        Rect::new(30.0, 30.0, 120.0, 80.0),
        12.0,
        &Paint::from_color(Color::WHITE),
    );
    let pdf = pdf_of(&mut context, vec![b.build(), b2()], [200.0, 160.0]);
    assert_eq!(pdf.get_pages().len(), 2);
    assert_eq!(images(&pdf), 1, "the blur is the one picture");
}

/// `b2` is a page with a plain square.
fn b2() -> DisplayList {
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(10.0, 10.0, 40.0, 40.0),
        &Paint::from_color(Color::rgb(0.2, 0.4, 0.8)),
    );
    b.build()
}
