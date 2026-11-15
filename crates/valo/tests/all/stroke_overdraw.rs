//! A stroke blends each pixel once, however often its geometry overlaps
//! itself, as Impeller's stroke does (`kPreventOverdraw`) and as Skia's
//! does by filling the stroke's outline: a translucent stroke that crosses
//! itself is no darker where it crosses. Each stroke resets what it counted,
//! so the next stroke over it still draws.

use std::sync::Arc;

use valo::{
    Color, Context, DisplayListBuilder, Offscreen, Paint, PaintStyle, Path, PathBuilder, Stroke,
};

const SIZE: [u32; 2] = [64, 32];

/// The crossing of the bow tie's two diagonals.
const CROSSING: (u32, u32) = (32, 16);
/// A pixel on the bow tie's right edge, which the stroke covers once.
const ONE_LAYER: (u32, u32) = (60, 16);

/// Renders the list `record` records over white, on a device of its own;
/// `None`, with a note that `test` is skipped, when the machine has none.
fn render(test: &str, record: impl FnOnce(&mut DisplayListBuilder)) -> Option<Vec<u8>> {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP {test}: no GPU adapter");
        return None;
    };
    let mut context = Context::new(device.clone(), queue.clone());
    let offscreen = Offscreen::new(&device, SIZE);
    let mut builder = DisplayListBuilder::new();
    record(&mut builder);
    context.render(&builder.build(), &offscreen.target(Some(Color::WHITE)));
    Some(valo_harness::read_texture_rgba(
        &device,
        &queue,
        offscreen.texture(),
        SIZE,
    ))
}

fn pixel(pixels: &[u8], (x, y): (u32, u32)) -> [u8; 4] {
    let at = ((y * SIZE[0] + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

fn assert_close(actual: [u8; 4], expected: [u8; 4], what: &str) {
    let close = actual
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual.abs_diff(expected) <= 3);
    assert!(close, "{what}: {actual:?}, expected {expected:?}");
}

/// A bow tie: one open contour whose diagonals cross at [`CROSSING`] and
/// whose right edge runs through [`ONE_LAYER`].
fn bow_tie() -> Arc<Path> {
    let mut path = PathBuilder::new();
    path.move_to((4.0, 4.0))
        .line_to((60.0, 28.0))
        .line_to((60.0, 4.0))
        .line_to((4.0, 28.0));
    path.build()
}

/// Half-transparent blue, 6 wide, which blends white to (128, 128, 255)
/// once and to (64, 64, 255) twice.
fn translucent_stroke() -> Paint {
    Paint {
        color: Color::rgba(0.0, 0.0, 1.0, 0.5),
        style: PaintStyle::Stroke(Stroke::new(6.0)),
        ..Default::default()
    }
}

#[test]
fn a_translucent_stroke_blends_where_it_crosses_itself_once() {
    let Some(pixels) = render("stroke_overdraw::crossing", |builder| {
        builder.draw_path(&bow_tie(), valo::FillRule::NonZero, &translucent_stroke());
    }) else {
        return;
    };
    assert_close(pixel(&pixels, ONE_LAYER), [128, 128, 255, 255], "one layer");
    assert_close(
        pixel(&pixels, CROSSING),
        pixel(&pixels, ONE_LAYER),
        "the crossing, as one layer",
    );
}

#[test]
fn a_stroke_over_a_stroke_still_blends_after_it() {
    let Some(pixels) = render("stroke_overdraw::stroke_over_stroke", |builder| {
        builder.draw_path(&bow_tie(), valo::FillRule::NonZero, &translucent_stroke());
        builder.draw_path(&bow_tie(), valo::FillRule::NonZero, &translucent_stroke());
    }) else {
        return;
    };
    assert_close(pixel(&pixels, ONE_LAYER), [64, 64, 255, 255], "two strokes");
    assert_close(
        pixel(&pixels, CROSSING),
        pixel(&pixels, ONE_LAYER),
        "the crossing, as two strokes",
    );
}
