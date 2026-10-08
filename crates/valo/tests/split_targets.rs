//! A target that a destination read splits into several passes. Every pass
//! starts fresh: the next one draws what the target held back in and
//! replays the clips still active, so a split layer inside a split target
//! leaves the target's pixels alone, a clip holds past a split, and a draw's
//! stencil lands in the same pass as its cover, where nothing the next pass
//! draws first can read it.

use valo::{BlendMode, Color, Context, DisplayListBuilder, Offscreen, Paint, Rect};

const SIZE: [u32; 2] = [64, 32];
const RED: Color = Color::rgb(1.0, 0.0, 0.0);
const GREEN: Color = Color::rgb(0.0, 1.0, 0.0);
const BLUE: Color = Color::rgb(0.0, 0.0, 1.0);

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE[0] + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

/// A rect that reads the destination: an advanced blend splits its target.
fn multiply(rect: Rect) -> (Rect, Paint) {
    let paint = Paint {
        blend_mode: BlendMode::Multiply,
        ..Paint::from_color(Color::rgb(0.5, 0.5, 0.5))
    };
    (rect, paint)
}

/// A layer exactly the target's size (a dot in its top-left corner and blue
/// over its right half) that a destination read inside it splits. Nothing it
/// draws reaches (16, 16).
fn split_layer_the_size_of_the_target(builder: &mut DisplayListBuilder) {
    builder.save_layer(None, &Paint::default());
    builder.draw_rect(Rect::new(0.0, 0.0, 1.0, 1.0), &Paint::from_color(BLUE));
    builder.draw_rect(Rect::new(32.0, 0.0, 32.0, 32.0), &Paint::from_color(BLUE));
    let (rect, paint) = multiply(Rect::new(48.0, 8.0, 8.0, 8.0));
    builder.draw_rect(rect, &paint);
    builder.restore();
}

/// The main target split before the layer opens: both are split targets,
/// open at once.
#[test]
fn a_split_layer_keeps_off_a_split_main_target() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_split_layer_keeps_off_a_split_main_target");
        return;
    };
    let mut context = Context::new(device, queue);
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(Rect::new(0.0, 0.0, 32.0, 32.0), &Paint::from_color(RED));
    let (rect, paint) = multiply(Rect::new(40.0, 20.0, 8.0, 8.0));
    builder.draw_rect(rect, &paint);
    split_layer_the_size_of_the_target(&mut builder);
    let pixels = context.render_to_rgba(&builder.build(), SIZE, Some(Color::WHITE));
    assert_eq!(pixel(&pixels, 16, 16), [255, 0, 0, 255]);
}

/// A target drawn over without clearing has its pixels drawn back first;
/// a split layer inside it must not disturb them.
#[test]
fn a_split_layer_keeps_off_a_target_drawn_over() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_split_layer_keeps_off_a_target_drawn_over");
        return;
    };
    let mut context = Context::new(device.clone(), queue.clone());
    let offscreen = Offscreen::new(&device, SIZE);
    let mut background = DisplayListBuilder::new();
    background.draw_rect(
        Rect::new(0.0, 0.0, SIZE[0] as f32, SIZE[1] as f32),
        &Paint::from_color(GREEN),
    );
    context.render(&background.build(), &offscreen.target(None));
    let mut layer = DisplayListBuilder::new();
    split_layer_the_size_of_the_target(&mut layer);
    context.render(&layer.build(), &offscreen.target(None));
    let pixels = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), SIZE);
    assert_eq!(pixel(&pixels, 16, 16), [0, 255, 0, 255]);
}

/// The upper-left half of the square from (0, 0) to (32, 32).
fn upper_left_triangle() -> std::sync::Arc<valo::Path> {
    let mut path = valo::PathBuilder::new();
    path.move_to((0.0, 0.0))
        .line_to((32.0, 0.0))
        .line_to((0.0, 32.0))
        .close();
    path.build()
}

/// The lower-right half of the same square: its bounds cover the upper-left
/// half, its shape does not.
fn lower_right_triangle() -> std::sync::Arc<valo::Path> {
    let mut path = valo::PathBuilder::new();
    path.move_to((32.0, 32.0))
        .line_to((0.0, 32.0))
        .line_to((32.0, 0.0))
        .close();
    path.build()
}

/// A path that reads the destination splits its target, and its stencil
/// has to land in the same pass as its cover: an opaque path drawn after it,
/// which the next pass draws first, must not cover through the stencil the
/// first path left there.
#[test]
fn a_path_reading_the_destination_keeps_its_stencil_with_its_cover() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_path_reading_the_destination_keeps_its_stencil_with_its_cover");
        return;
    };
    let mut context = Context::new(device, queue);
    let mut builder = DisplayListBuilder::new();
    let grey = Paint {
        blend_mode: BlendMode::Multiply,
        ..Paint::from_color(Color::rgb(0.5, 0.5, 0.5))
    };
    builder.draw_path(&upper_left_triangle(), valo::FillRule::NonZero, &grey);
    builder.draw_path(
        &lower_right_triangle(),
        valo::FillRule::NonZero,
        &Paint::from_color(RED),
    );
    let pixels = context.render_to_rgba(&builder.build(), SIZE, Some(Color::WHITE));
    assert_eq!(pixel(&pixels, 4, 4), [128, 128, 128, 255], "the grey path");
    assert_eq!(pixel(&pixels, 28, 28), [255, 0, 0, 255], "the red path");
}

/// Every pass starts fresh, so a pass that follows a split replays the
/// clips still active: a draw after a destination read, inside the same
/// clip, stays inside it.
#[test]
fn a_clip_holds_past_a_split() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_clip_holds_past_a_split");
        return;
    };
    let mut context = Context::new(device, queue);
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(Rect::new(0.0, 0.0, 64.0, 32.0), &Paint::from_color(RED));
    builder.save();
    builder.clip_rect(Rect::new(0.0, 0.0, 32.0, 32.0), valo::ClipOp::Intersect);
    let (rect, paint) = multiply(Rect::new(4.0, 4.0, 8.0, 8.0));
    builder.draw_rect(rect, &paint);
    builder.draw_rect(Rect::new(0.0, 16.0, 64.0, 16.0), &Paint::from_color(BLUE));
    builder.restore();
    let pixels = context.render_to_rgba(&builder.build(), SIZE, Some(Color::WHITE));
    assert_eq!(pixel(&pixels, 8, 24), [0, 0, 255, 255], "inside the clip");
    assert_eq!(pixel(&pixels, 48, 24), [255, 0, 0, 255], "outside it");
    assert_eq!(
        pixel(&pixels, 8, 8),
        [128, 0, 0, 255],
        "the multiplied rect"
    );
}
