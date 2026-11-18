//! A shadow resolved when it is drawn, as Impeller's `DlDispatcherBase::drawShadow`
//! resolves it: the path moved down by the occluder's height in device pixels and filled
//! with a toned spot colour under a mask blur whose radius is that height over the vertical
//! scale of the whole transform it is drawn under. A picture recorded at no scale and drawn
//! under one blurs as a shadow recorded under that scale does, which a shadow resolved when
//! it is recorded cannot.

use std::sync::Arc;

use valo::{
    Color, Context, DisplayList, DisplayListBuilder, FillRule, MaskBlur, Paint, Path, PathBuilder,
    Rect,
};

use crate::{context, goldens_dir};

const SIZE: [u32; 2] = [240, 160];
/// One row of `SIZE` RGBA pixels, so a readback splits into rows at compile time.
const BYTES_PER_ROW: usize = SIZE[0] as usize * 4;
const BACKGROUND: Color = Color::rgb(0.96, 0.96, 0.96);

/// `KERNEL_RADIUS_PER_SIGMA` is Impeller's `kKernelRadiusPerSigma`, √3.
const KERNEL_RADIUS_PER_SIGMA: f32 = 1.732_050_8;

fn render(context: &mut Context, list: DisplayList) -> Vec<u8> {
    valo_harness::render_to_rgba(context, list, SIZE, Some(BACKGROUND))
}

fn card() -> Arc<Path> {
    let mut path = PathBuilder::new();
    path.rect(Rect::new(20.0, 20.0, 40.0, 30.0));
    path.build()
}

fn pill() -> Arc<Path> {
    let mut path = PathBuilder::new();
    path.rrect(Rect::new(70.0, 20.0, 40.0, 30.0), 12.0);
    path.build()
}

/// `shadow_picture` is a card's and a pill's shadows at `elevation` in a view of
/// `device_pixel_ratio`, recorded at no scale, as a widget's picture is.
fn shadow_picture(elevation: f32, device_pixel_ratio: f32) -> Arc<DisplayList> {
    let mut b = DisplayListBuilder::new();
    for path in [card(), pill()] {
        b.draw_shadow(
            &path,
            FillRule::NonZero,
            Color::BLACK,
            elevation,
            false,
            device_pixel_ratio,
        );
    }
    Arc::new(b.build())
}

/// `scaled` is `record` recorded under a scale of `scale`.
fn scaled(scale: f32, record: impl FnOnce(&mut DisplayListBuilder)) -> DisplayList {
    let mut b = DisplayListBuilder::new();
    b.scale(scale, scale);
    record(&mut b);
    b.build()
}

/// `spot_by_hand` is what Impeller draws for one shadow of a black occluder, a card or a
/// pill, under
/// `scale`: the path moved down by the occluder's height in device pixels — the height over
/// `scale` in its own coordinates — filled with a quarter-alpha black under a mask blur of
/// the height over the scale, as a sigma.
fn spot_by_hand(b: &mut DisplayListBuilder, path: &Arc<Path>, occluder_z: f32, scale: f32) {
    let radius = occluder_z / scale;
    let paint = Paint {
        mask_blur: Some(MaskBlur::new(radius / KERNEL_RADIUS_PER_SIGMA + 0.5)),
        ..Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.25))
    };
    b.save();
    b.translate(0.0, occluder_z / scale);
    // Impeller draws both in closed form, as valo's builder draws a
    // mask-blurred rect and rounded rect.
    match path.as_round_rect() {
        Some((rect, radii)) => b.draw_rrect(rect, radii[0][0], &paint),
        None => b.draw_rect(path.bounds(), &paint),
    }
    b.restore();
}

fn max_difference(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0)
}

/// `ink_centre_y` is the row the ink of `pixels` balances on, rows weighted by
/// how far their colour sits from `background`: a symmetric blurred shape
/// weighs on its own centre however far it spreads.
fn ink_centre_y(pixels: &[u8], background: Color) -> f32 {
    let background_luminance = (background.r + background.g + background.b) / 3.0;
    let mut weighted_rows = 0.0f64;
    let mut total_weight = 0.0f64;
    for (row, row_pixels) in pixels.as_chunks::<BYTES_PER_ROW>().0.iter().enumerate() {
        for pixel in row_pixels.as_chunks::<4>().0 {
            let luminance = (pixel[0] as f32 + pixel[1] as f32 + pixel[2] as f32) / (3.0 * 255.0);
            let darkness = (background_luminance - luminance).max(0.0) as f64;
            weighted_rows += darkness * row as f64;
            total_weight += darkness;
        }
    }
    (weighted_rows / total_weight) as f32
}

/// A shadow draws what Impeller's dispatcher draws for it, the spot worked by hand.
#[test]
fn a_shadow_draws_impellers_spot() {
    let Some(mut context) = context("a_shadow_draws_impellers_spot") else {
        return;
    };
    let resolved = render(
        &mut context,
        scaled(2.0, |b| b.draw_display_list(&shadow_picture(4.0, 2.0))),
    );
    let by_hand = render(
        &mut context,
        scaled(2.0, |b| {
            for path in [card(), pill()] {
                spot_by_hand(b, &path, 8.0, 2.0);
            }
        }),
    );
    let difference = max_difference(&resolved, &by_hand);
    assert!(difference <= 3, "differs by {difference}");
}

/// Flutter drops a shadow in device pixels, as Impeller pre-concatenates its
/// translation onto the transform: elevation one in a view of pixel ratio three
/// under a scale of three drops the spot three device pixels below the card.
/// A drop in the path's own coordinates would put it nine there, six too far.
#[test]
fn a_shadow_drops_in_device_pixels_under_a_scale() {
    let Some(mut context) = context("a_shadow_drops_in_device_pixels_under_a_scale") else {
        return;
    };
    let path = card();
    let pixels = render(
        &mut context,
        scaled(3.0, |b| {
            b.draw_shadow(&path, FillRule::NonZero, Color::BLACK, 1.0, false, 3.0);
        }),
    );
    // The card's centre is 35 local pixels down, at a scale of three, plus
    // the three device pixels of the drop.
    let expected = 35.0 * 3.0 + 3.0;
    let centre = ink_centre_y(&pixels, BACKGROUND);
    assert!(
        (centre - expected).abs() < 2.0,
        "the spot's centre is at {centre}, not {expected}"
    );
}

/// The case a shadow resolved at record time gets wrong: a picture recorded at no scale and
/// drawn under a scale of two, as a widget under a scaled layer is, blurs by the scale it is
/// drawn under, the same as a shadow recorded under it, not by the one it was recorded at.
#[test]
fn a_shadow_in_a_picture_blurs_by_the_scale_it_is_drawn_under() {
    let Some(mut context) = context("a_shadow_in_a_picture_blurs_by_the_scale_it_is_drawn_under")
    else {
        return;
    };
    let embedded = render(
        &mut context,
        scaled(2.0, |b| b.draw_display_list(&shadow_picture(6.0, 1.0))),
    );
    let recorded_under_the_scale = render(
        &mut context,
        scaled(2.0, |b| {
            for path in [card(), pill()] {
                b.draw_shadow(&path, FillRule::NonZero, Color::BLACK, 6.0, false, 1.0);
            }
        }),
    );
    let difference = max_difference(&embedded, &recorded_under_the_scale);
    assert!(difference <= 3, "differs by {difference}");

    let resolved_when_recorded = render(
        &mut context,
        scaled(2.0, |b| {
            for path in [card(), pill()] {
                spot_by_hand(b, &path, 6.0, 1.0);
            }
        }),
    );
    let difference = max_difference(&embedded, &resolved_when_recorded);
    assert!(
        difference > 3,
        "the blur did not follow the scale: differs by {difference}"
    );
}

/// Shadows at a few elevations, of a rectangle, a rounded rectangle and a circle, which are
/// drawn in closed form, and of a triangle, a mask-blurred fill, at a pixel ratio of one and of
/// two, and under a scaled transform.
#[test]
fn shadows_golden() {
    let Some(mut context) = context("shadows_golden") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.draw_display_list(&shadow_picture(2.0, 1.0));
    b.save();
    b.translate(120.0, 0.0);
    b.draw_display_list(&shadow_picture(8.0, 1.0));
    b.restore();
    b.save();
    b.translate(0.0, 70.0);
    b.draw_display_list(&shadow_picture(4.0, 2.0));
    let mut circle = PathBuilder::new();
    circle.circle((150.0, 35.0), 18.0);
    let circle = circle.build();
    b.draw_shadow(
        &circle,
        FillRule::NonZero,
        Color::rgb(0.2, 0.3, 0.9),
        6.0,
        true,
        1.0,
    );
    let mut triangle = PathBuilder::new();
    triangle
        .move_to((185.0, 15.0))
        .line_to((225.0, 15.0))
        .line_to((205.0, 45.0))
        .close();
    b.draw_shadow(
        &triangle.build(),
        FillRule::NonZero,
        Color::BLACK,
        4.0,
        false,
        1.0,
    );
    b.translate(180.0, 50.0);
    b.scale(0.5, 0.5);
    b.draw_display_list(&shadow_picture(8.0, 1.0));
    b.restore();
    // The occluders over the shadows of the top row.
    let white = Paint::from_color(Color::WHITE);
    for offset in [0.0, 120.0] {
        b.save();
        b.translate(offset, 0.0);
        for path in [card(), pill()] {
            b.draw_path(&path, FillRule::NonZero, &white);
        }
        b.restore();
    }
    let pixels = render(&mut context, b.build());
    valo_harness::assert_golden(goldens_dir(), "shadows", SIZE, &pixels);
}
