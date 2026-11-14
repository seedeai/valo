//! A shadow resolved when it is drawn, as Impeller's `DlDispatcherBase::drawShadow`
//! resolves it: the path moved down by the occluder's height and filled with a toned spot
//! colour under a mask blur whose radius is that height over the vertical scale of the
//! whole transform it is drawn under. A picture recorded at no scale and drawn under one
//! blurs as a shadow recorded under that scale does, which a shadow resolved when it is
//! recorded cannot.

use std::sync::Arc;

use valo::{
    Color, Context, DisplayList, DisplayListBuilder, FillRule, MaskBlur, Paint, Path, PathBuilder,
    Rect,
};

use crate::{context, goldens_dir};

const SIZE: [u32; 2] = [240, 160];
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
/// `scale`: the path moved down by the occluder's height in its own coordinates, filled
/// with a quarter-alpha black under a mask blur of the height over the scale, as a sigma.
fn spot_by_hand(b: &mut DisplayListBuilder, path: &Arc<Path>, occluder_z: f32, scale: f32) {
    let radius = occluder_z / scale;
    let paint = Paint {
        mask_blur: Some(MaskBlur::new(radius / KERNEL_RADIUS_PER_SIGMA + 0.5)),
        ..Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.25))
    };
    b.save();
    b.translate(0.0, occluder_z);
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
