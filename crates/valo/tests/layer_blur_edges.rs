//! Where a save layer's blur finds the edge of what it reads, following
//! Skia: a blur that clamps, mirrors or repeats reads past the edge of the
//! layer's own extent, the bounds hint when there is one and the clip when
//! there is not (`SkBlurImageFilter.cpp`, `SkCanvas.cpp`'s layer bounds), so
//! the layer's texture covers that extent, not just its content. A decal
//! blur reads transparent past any edge, so its tight texture stays (the
//! planner's `LayerPlan` tests pin that).

use valo::{
    ClipOp, Color, ColorFilter, Context, DisplayListBuilder, ImageFilter, Paint, Rect, TileMode,
};

const SIZE: [u32; 2] = [160, 160];
const ROW: u32 = 80;

fn context(test: &str) -> Option<Context> {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP {test}: no GPU adapter");
        return None;
    };
    Some(Context::new(device, queue))
}

fn white() -> Paint {
    Paint::from_color(Color::WHITE)
}

/// `swap_red_and_green` is a colour filter that leaves white white.
fn swap_red_and_green() -> ImageFilter {
    #[rustfmt::skip]
    let matrix = [
        0.0, 1.0, 0.0, 0.0, 0.0,
        1.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    ImageFilter::color(ColorFilter::Matrix(matrix))
}

/// `layered` records an opaque white `square` in a save layer with `filter`
/// and `hint`, under a clip of most of the target.
fn layered(filter: ImageFilter, hint: Option<Rect>, square: Rect) -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    b.clip_rect(Rect::new(8.0, 8.0, 144.0, 144.0), ClipOp::Intersect);
    b.save_layer(
        hint,
        &Paint {
            image_filter: Some(filter),
            ..Paint::default()
        },
    );
    b.draw_rect(square, &white());
    b.restore();
    b.build()
}

/// `alphas` is the alpha of each pixel of the centre row.
fn alphas(context: &mut Context, list: &valo::DisplayList) -> Vec<u8> {
    let pixels = context.render_to_rgba(list, SIZE, Some(Color::TRANSPARENT));
    (0..SIZE[0])
        .map(|x| pixels[((ROW * SIZE[0] + x) * 4 + 3) as usize])
        .collect()
}

fn square() -> Rect {
    Rect::new(60.0, 60.0, 40.0, 40.0)
}

/// A blur composed under a colour filter clamps (dart:ui resolves both
/// sides of a composition to clamp), and still fades softly off the square:
/// the layer reaches the clip, whose transparent edge it clamps to, so it
/// matches the same blur with decal.
#[test]
fn a_composed_blur_on_a_layer_fades_softly() {
    let Some(mut context) = context("a_composed_blur_on_a_layer_fades_softly") else {
        return;
    };
    let clamped = ImageFilter::compose(swap_red_and_green(), ImageFilter::blur(10.0, 10.0));
    let decal = ImageFilter::compose(
        swap_red_and_green(),
        ImageFilter::blur(10.0, 10.0).with_tile_mode(TileMode::Decal),
    );
    let clamped = alphas(&mut context, &layered(clamped, None, square()));
    let decal = alphas(&mut context, &layered(decal, None, square()));
    assert!(
        (55..60).any(|x| (1..=254).contains(&clamped[x])),
        "a soft edge, not a hard one: {:?}",
        &clamped[40..80]
    );
    let worst = clamped
        .iter()
        .zip(&decal)
        .map(|(a, b)| a.abs_diff(*b))
        .max();
    assert!(
        worst <= Some(3),
        "clamp {:?} against decal {:?}",
        &clamped[30..130],
        &decal[30..130]
    );
}

/// A clamping blur on a layer with a bounds hint clamps at the hint, the
/// layer's edge: a square that reaches the hint on its right stays opaque
/// right up to the hint's edge, and fades softly on its left, where the
/// hint leaves transparent room around it.
#[test]
fn a_clamping_blur_on_a_hinted_layer_clamps_at_the_hint() {
    let Some(mut context) = context("a_clamping_blur_on_a_hinted_layer_clamps_at_the_hint") else {
        return;
    };
    let hint = Rect::new(40.0, 40.0, 80.0, 80.0);
    let to_the_hint = Rect::new(60.0, 60.0, 60.0, 40.0);
    let clamp = ImageFilter::blur(4.0, 4.0).with_tile_mode(TileMode::Clamp);
    let row = alphas(&mut context, &layered(clamp, Some(hint), to_the_hint));
    assert!(
        (50..60).any(|x| (1..=254).contains(&row[x])),
        "soft inside the hint: {:?}",
        &row[40..70]
    );
    assert_eq!(
        row[119],
        255,
        "opaque up to the hint's edge: {:?}",
        &row[100..140]
    );
    assert_eq!(
        row[136],
        0,
        "nothing past the blur's reach: {:?}",
        &row[100..140]
    );
}

/// A filtered draw is left as it was: its effect layer holds the draw and a
/// transparent texel round it, so its composed blur, which clamps, fades
/// off the square as the same blur with decal does.
#[test]
fn a_filtered_draws_composed_blur_is_unchanged() {
    let Some(mut context) = context("a_filtered_draws_composed_blur_is_unchanged") else {
        return;
    };
    let drawn = |context: &mut Context, blur: ImageFilter| {
        let mut b = DisplayListBuilder::new();
        b.clip_rect(Rect::new(8.0, 8.0, 144.0, 144.0), ClipOp::Intersect);
        b.draw_rect(
            square(),
            &Paint {
                image_filter: Some(ImageFilter::compose(swap_red_and_green(), blur)),
                ..white()
            },
        );
        alphas(context, &b.build())
    };
    let clamped = drawn(&mut context, ImageFilter::blur(10.0, 10.0));
    let decal = drawn(
        &mut context,
        ImageFilter::blur(10.0, 10.0).with_tile_mode(TileMode::Decal),
    );
    assert_eq!(clamped, decal);
}
