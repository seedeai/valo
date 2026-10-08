//! A target drawn over without clearing (`clear: None`) starts each frame
//! from its own pixels. Multisample scratch never carries a picture from one
//! pass to the next: it is shared by size, as Skia's and Impeller's are, and
//! a target that keeps its pixels has them drawn back into the scratch first.

use valo::{Color, Context, DisplayListBuilder, Offscreen, Paint, Rect};

const SIZE: [u32; 2] = [64, 32];
const RED: Color = Color::rgb(1.0, 0.0, 0.0);
const BLUE: Color = Color::rgb(0.0, 0.0, 1.0);

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE[0] + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

fn square(x: f32, color: Color) -> valo::DisplayList {
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(Rect::new(x, 8.0, 16.0, 16.0), &Paint::from_color(color));
    builder.build()
}

/// Two targets of one size drawn over in turn: neither shows the other's
/// drawing.
#[test]
fn targets_of_one_size_keep_their_own_pixels() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP targets_of_one_size_keep_their_own_pixels");
        return;
    };
    let mut context = Context::new(device.clone(), queue.clone());
    let first = Offscreen::new(&device, SIZE);
    let second = Offscreen::new(&device, SIZE);
    context.render(
        &DisplayListBuilder::new().build(),
        &second.target(Some(Color::TRANSPARENT)),
    );
    context.render(&square(8.0, RED), &first.target(None));
    context.render(&square(40.0, BLUE), &second.target(None));
    let pixels = valo_harness::read_texture_rgba(&device, &queue, second.texture(), SIZE);
    assert_eq!(pixel(&pixels, 48, 16), [0, 0, 255, 255], "its own drawing");
    assert_eq!(
        pixel(&pixels, 16, 16),
        [0, 0, 0, 0],
        "not the other target's"
    );
}

/// Drawing nothing over a target leaves every pixel as it was, its
/// anti-aliased edges included.
#[test]
fn drawing_nothing_over_a_target_leaves_it_exactly() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP drawing_nothing_over_a_target_leaves_it_exactly");
        return;
    };
    let mut context = Context::new(device.clone(), queue.clone());
    let offscreen = Offscreen::new(&device, SIZE);
    let mut builder = DisplayListBuilder::new();
    builder.draw_circle(
        (20.5, 15.25),
        11.3,
        &Paint::from_color(Color::rgba(0.2, 0.6, 0.9, 0.7)),
    );
    context.render(
        &builder.build(),
        &offscreen.target(Some(Color::rgb(1.0, 1.0, 0.0))),
    );
    let before = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), SIZE);
    context.render(&DisplayListBuilder::new().build(), &offscreen.target(None));
    let after = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), SIZE);
    assert_eq!(after, before);
}
