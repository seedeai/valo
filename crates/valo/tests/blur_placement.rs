//! Where a blur puts what it blurs. A Gaussian is symmetric, so a blurred
//! square's ink stays centred on the square whichever way the blur is
//! applied: a draw's image filter, a save layer's, a backdrop's.
//!
//! The downsample is where this can go wrong. Its taps land exactly on its
//! input's edge whenever the input's size and the blur's padding line its
//! grid up with that edge, and a save layer's texture holds its ink out to
//! its edges. A decal blur reads half of the edge texel there and half of
//! the transparent border past it, as a sampler with a transparent border
//! does; reading the edge texel whole on one side and not at all on the
//! other moves the blur by half a downsampled texel, a pixel at σ 8:
//! Canvas2D's `filter: blur(8px)`.

use valo::{
    Backdrop, BlendMode, Color, Context, DisplayList, DisplayListBuilder, ImageFilter, Paint, Rect,
};

const FRAME: usize = 128;
const CENTRE: f64 = FRAME as f64 / 2.0;

/// How far a centroid may sit from the square's centre. The downsample's
/// grid stays put while what it reads moves, so a square that does not
/// line up with it comes out a few hundredths of a pixel off; a lost or
/// doubled edge texel moves it by half a downsampled texel, a pixel or
/// more.
const TOLERANCE: f64 = 0.1;

/// The σ of each downsample: a half, a quarter, an eighth.
const SIGMAS: [f32; 3] = [8.0, 12.0, 24.0];

/// Square sides that put the downsample's taps on the layer's edge at one
/// σ or another.
const SIDES: [f32; 2] = [46.0, 48.0];

fn context(test: &str) -> Option<Context> {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP {test}: no GPU adapter");
        return None;
    };
    Some(Context::new(device, queue))
}

/// A square `side` pixels wide in the middle of the frame, on whole pixels.
fn square(side: f32) -> Rect {
    let origin = CENTRE as f32 - side / 2.0;
    Rect::new(origin, origin, side, side)
}

fn ink() -> Paint {
    Paint::from_color(Color::rgb(1.0, 0.31, 0.47))
}

fn blur(sigma: f32) -> ImageFilter {
    ImageFilter::blur(sigma, sigma)
}

/// The square drawn with the blur as its image filter.
fn drawn(side: f32, sigma: f32) -> DisplayList {
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        square(side),
        &Paint {
            image_filter: Some(blur(sigma)),
            ..ink()
        },
    );
    builder.build()
}

/// The square drawn into a save layer that blurs: Canvas2D's CSS `filter`.
fn layered(side: f32, sigma: f32) -> DisplayList {
    let mut builder = DisplayListBuilder::new();
    builder.save_layer(
        None,
        &Paint {
            image_filter: Some(blur(sigma)),
            ..Paint::default()
        },
    );
    builder.draw_rect(square(side), &ink());
    builder.restore();
    builder.build()
}

/// The square drawn, then a backdrop over the whole frame that blurs it and
/// replaces it with the glass.
fn behind_glass(side: f32, sigma: f32) -> DisplayList {
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(square(side), &ink());
    builder.save_layer_backdrop(
        Some(Rect::new(0.0, 0.0, FRAME as f32, FRAME as f32)),
        &Paint {
            blend_mode: BlendMode::Src,
            ..Paint::default()
        },
        Backdrop::new(blur(sigma)),
    );
    builder.restore();
    builder.build()
}

/// The alpha-weighted centroid of what `list` draws, in pixels from the
/// frame's top-left corner.
fn centroid(context: &mut Context, list: &DisplayList) -> (f64, f64) {
    let size = [FRAME as u32; 2];
    let pixels = context.render_to_rgba(list, size, Some(Color::TRANSPARENT));
    let (mut weight, mut sum_x, mut sum_y) = (0.0f64, 0.0f64, 0.0f64);
    for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
        let alpha = f64::from(pixel[3]);
        weight += alpha;
        sum_x += alpha * ((index % FRAME) as f64 + 0.5);
        sum_y += alpha * ((index / FRAME) as f64 + 0.5);
    }
    assert!(weight > 0.0, "nothing was drawn");
    (sum_x / weight, sum_y / weight)
}

/// Every σ and side through `route` stays centred on the square.
fn assert_centred(test: &str, route: fn(f32, f32) -> DisplayList) {
    let Some(mut context) = context(test) else {
        return;
    };
    for sigma in SIGMAS {
        for side in SIDES {
            let (x, y) = centroid(&mut context, &route(side, sigma));
            assert!(
                (x - CENTRE).abs() <= TOLERANCE && (y - CENTRE).abs() <= TOLERANCE,
                "σ {sigma}, side {side}: the blur's ink is centred at ({x:.3}, {y:.3}), \
                 not on the square's centre ({CENTRE}, {CENTRE})"
            );
        }
    }
}

#[test]
fn a_drawn_blur_stays_centred_on_its_square() {
    assert_centred("a_drawn_blur_stays_centred_on_its_square", drawn);
}

#[test]
fn a_layer_blur_stays_centred_on_its_square() {
    assert_centred("a_layer_blur_stays_centred_on_its_square", layered);
}

#[test]
fn a_backdrop_blur_stays_centred_on_its_square() {
    assert_centred("a_backdrop_blur_stays_centred_on_its_square", behind_glass);
}

/// A layer that its square fills to the edges, blurred by σ 8, which
/// downsamples by half with its taps on the layer's edges: each of those
/// taps reads half of the edge texel and half of the transparent border, so
/// the blur's two sides mirror each other and it keeps the square's mass.
/// Reading the edge texel whole on both sides, or not at all, would keep the
/// sides mirrored but not the mass.
#[test]
fn a_decal_blur_reads_half_of_an_edge_texel_its_tap_lands_on() {
    let Some(mut context) = context("a_decal_blur_reads_half_of_an_edge_texel_its_tap_lands_on")
    else {
        return;
    };
    let side = 48.0;
    let pixels = context.render_to_rgba(
        &layered(side, 8.0),
        [FRAME as u32; 2],
        Some(Color::TRANSPARENT),
    );
    let alpha = |x: usize, y: usize| i32::from(pixels[(y * FRAME + x) * 4 + 3]);
    let middle = FRAME / 2;
    for near in 0..middle {
        let far = FRAME - 1 - near;
        let (left, right) = (alpha(near, middle), alpha(far, middle));
        assert!(
            (left - right).abs() <= 1,
            "columns {near} and {far} must mirror: α {left} and {right}"
        );
    }
    let mass: f32 = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| f32::from(pixel[3]) / 255.0)
        .sum();
    let square = side * side;
    assert!(
        (mass - square).abs() <= 0.01 * square,
        "the blurred square's mass is {mass:.0}, not the square's {square}"
    );
}
