//! How a blur reads past the edge of what it blurs. A blur that leaves its
//! tile mode unspecified takes it from where it is used, as dart:ui gives
//! it: a backdrop mirrors, an image draw clamps, a layer and other draws are
//! decal. A backdrop whose blur reaches past the edge of the target blurs
//! the whole target and reads past its edge that way (Impeller's gutter
//! branch), where one inside the target is cut out of it.

use valo::{
    Backdrop, Color, Context, DisplayList, DisplayListBuilder, ImageDesc, ImageFilter, Paint, Rect,
    TileMode,
};

const SIDE: u32 = 64;
const RED: Color = Color::rgb(1.0, 0.0, 0.0);
const BLUE: Color = Color::rgb(0.0, 0.0, 1.0);

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIDE + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

fn render(context: &mut Context, list: &DisplayList) -> Vec<u8> {
    context.render_to_rgba(list, [SIDE, SIDE], Some(Color::TRANSPARENT))
}

/// Two red rows along the target's top edge over blue, and a glass panel
/// touching that edge, blurred by `filter`.
fn glass_at_the_top_edge(filter: ImageFilter) -> DisplayList {
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        Rect::new(0.0, 0.0, SIDE as f32, SIDE as f32),
        &Paint::from_color(BLUE),
    );
    builder.draw_rect(
        Rect::new(0.0, 0.0, SIDE as f32, 2.0),
        &Paint::from_color(RED),
    );
    builder.save_layer_backdrop(
        Some(Rect::new(8.0, 0.0, 48.0, 24.0)),
        &Paint::default(),
        Backdrop::new(filter),
    );
    builder.restore();
    builder.build()
}

/// At the frame's edge the blur reads above the target: mirrored, the two
/// red rows come back once and the blue under them after; clamped, the top
/// row goes on red forever; decal, there is nothing there, so the glass is
/// half transparent at the edge and the sharp red row shows through it.
#[test]
fn a_backdrop_at_the_frame_edge_mirrors_unless_told_otherwise() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_backdrop_at_the_frame_edge_mirrors_unless_told_otherwise");
        return;
    };
    let mut context = Context::new(device, queue);
    let blur = || ImageFilter::blur(4.0, 4.0);
    let mut top_left = |filter: ImageFilter| {
        let pixels = render(&mut context, &glass_at_the_top_edge(filter));
        pixel(&pixels, 32, 0)
    };
    let unspecified = top_left(blur());
    let mirror = top_left(blur().with_tile_mode(TileMode::Mirror));
    let clamp = top_left(blur().with_tile_mode(TileMode::Clamp));
    let decal = top_left(blur().with_tile_mode(TileMode::Decal));
    for channel in 0..4 {
        assert!(
            unspecified[channel].abs_diff(mirror[channel]) <= 1,
            "an unspecified backdrop blur mirrors: {unspecified:?} against {mirror:?}"
        );
    }
    assert!(
        clamp[0] > mirror[0] + 20,
        "clamped, the red row goes on past the edge: {clamp:?} against {mirror:?}"
    );
    assert!(
        decal[0].abs_diff(mirror[0]) > 20,
        "decal, nothing comes from past the edge: {decal:?} against {mirror:?}"
    );
}

/// A blur on an image draw clamps unless told otherwise, so the image's own
/// edge goes on into the halo; decal, the halo fades.
#[test]
fn a_blurred_image_clamps_its_edge_unless_told_otherwise() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_blurred_image_clamps_its_edge_unless_told_otherwise");
        return;
    };
    let mut context = Context::new(device, queue);
    let green = context.upload_image(
        ImageDesc {
            size: [16, 16],
            premultiplied: true,
            mips: false,
        },
        &[0, 255, 0, 255].repeat(16 * 16),
    );
    let mut beside_the_edge = |filter: ImageFilter| {
        let mut builder = DisplayListBuilder::new();
        builder.draw_image(
            &green,
            Rect::new(24.0, 24.0, 16.0, 16.0),
            &Paint {
                image_filter: Some(filter),
                ..Paint::from_color(Color::WHITE)
            },
        );
        let pixels = render(&mut context, &builder.build());
        pixel(&pixels, 42, 32)
    };
    let unspecified = beside_the_edge(ImageFilter::blur(4.0, 4.0));
    let decal = beside_the_edge(ImageFilter::blur(4.0, 4.0).with_tile_mode(TileMode::Decal));
    assert!(
        unspecified[3] > 250,
        "clamped, the image's edge fills its halo: {unspecified:?}"
    );
    assert!(decal[3] < 200, "decal, the halo fades: {decal:?}");
}
