//! Backdrop base colors belong below the sampled scene and before the blur.
use valo::{
    Backdrop, BlendMode, Color, ColorFilter, Context, DisplayListBuilder, ImageFilter, Paint, Rect,
};

/// The same composition used by WinUI's acrylic recipe.
fn backdrop_over(color: Color, sigma: f32) -> Backdrop {
    Backdrop::new(ImageFilter::compose(
        ImageFilter::blur(sigma, sigma),
        ImageFilter::color(ColorFilter::Blend(color, BlendMode::DstOver)),
    ))
}

fn scene(base: Option<Color>, reference: bool, sigma: f32) -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    if reference {
        b.draw_rect(
            Rect::new(0.0, 0.0, 96.0, 64.0),
            &Paint::from_color(base.unwrap()),
        );
    }
    b.draw_rect(
        Rect::new(0.0, 0.0, 48.0, 64.0),
        &Paint::from_color(Color::rgba(1.0, 0.0, 0.0, 0.5)),
    );
    if reference && sigma == 0.0 {
        return b.build();
    }
    let backdrop = if !reference {
        base.map_or_else(
            || Backdrop::blur(sigma),
            |color| backdrop_over(color, sigma),
        )
    } else {
        Backdrop::blur(sigma)
    };
    b.save_layer_backdrop(
        Some(Rect::new(16.0, 12.0, 64.0, 40.0)),
        &Paint {
            blend_mode: BlendMode::Src,
            ..Default::default()
        },
        backdrop,
    );
    b.restore();
    b.build()
}

#[test]
fn backdrop_base_matches_scene_composited_over_color_before_blur() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        return;
    };
    let mut context = Context::new(device, queue);
    let base = Color::rgb(0.0, 0.0, 1.0);
    for sigma in [0.0, 3.0, 12.0] {
        let actual = context.render_to_rgba(
            &scene(Some(base), false, sigma),
            [96, 64],
            Some(Color::TRANSPARENT),
        );
        let reference = context.render_to_rgba(
            &scene(Some(base), true, sigma),
            [96, 64],
            Some(Color::TRANSPARENT),
        );
        for y in 14..50 {
            for x in 18..78 {
                let at = (y * 96 + x) * 4;
                for c in 0..4 {
                    assert!(
                        (actual[at + c] as i16 - reference[at + c] as i16).abs() <= 3,
                        "sigma={sigma} at({x},{y}) channel{c}:{} != {}",
                        actual[at + c],
                        reference[at + c]
                    );
                }
            }
        }
    }
}

#[test]
fn shared_backdrop_does_not_reuse_different_background_colors() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        return;
    };
    let mut context = Context::new(device, queue);
    let mut b = DisplayListBuilder::new();
    for (x, color) in [
        (0.0, Color::rgb(1.0, 0.0, 0.0)),
        (32.0, Color::rgb(0.0, 0.0, 1.0)),
    ] {
        b.save_layer_backdrop(
            Some(Rect::new(x, 0.0, 32.0, 32.0)),
            &Paint::default(),
            backdrop_over(color, 2.0).shared(7),
        );
        b.restore();
    }
    let pixels = context.render_to_rgba(&b.build(), [64, 32], Some(Color::TRANSPARENT));
    assert_eq!(
        &pixels[(16 * 64 + 16) * 4..(16 * 64 + 16) * 4 + 4],
        &[255, 0, 0, 255]
    );
    assert_eq!(
        &pixels[(16 * 64 + 48) * 4..(16 * 64 + 48) * 4 + 4],
        &[0, 0, 255, 255]
    );
}

/// Composed backdrop filters match the same chain on a rasterized layer,
/// including stage order, nonzero sample origins and unequal transformed axes.
#[test]
fn composed_backdrop_matches_layer_filter_order_and_transform() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        return;
    };
    let mut context = Context::new(device, queue);
    let color = ImageFilter::color(ColorFilter::Matrix([
        2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        1.0, 0.0,
    ]));
    let blur = ImageFilter::blur(6.0, 2.0);
    let mut results = Vec::new();
    for filter in [
        ImageFilter::compose(blur.clone(), color.clone()),
        ImageFilter::compose(color, blur),
    ] {
        let render = |context: &mut Context, backdrop: bool| {
            let mut b = DisplayListBuilder::new();
            b.concat(&valo::Matrix::scale(1.25, 1.5));
            if !backdrop {
                b.save_layer(
                    Some(Rect::new(0.0, 0.0, 128.0, 96.0)),
                    &Paint {
                        image_filter: Some(filter.clone()),
                        ..Paint::default()
                    },
                );
            }
            for (x, red) in [(0.0, 0.25), (64.0, 1.0)] {
                b.draw_rect(
                    Rect::new(x, 0.0, 64.0, 96.0),
                    &Paint::from_color(Color::rgb(red, 0.0, 0.0)),
                );
            }
            if backdrop {
                b.save_layer_backdrop(
                    Some(Rect::new(16.0, 16.0, 96.0, 64.0)),
                    &Paint::default(),
                    Backdrop::new(filter.clone()),
                );
            }
            b.restore();
            context.render_to_rgba(&b.build(), [160, 144], Some(Color::TRANSPARENT))
        };
        let actual = render(&mut context, true);
        let expected = render(&mut context, false);
        for y in 55..85 {
            for x in 65..95 {
                for c in 0..4 {
                    let at = (y * 160 + x) * 4 + c;
                    assert!(
                        actual[at].abs_diff(expected[at]) <= 3,
                        "at ({x},{y}) channel {c}: {} != {}",
                        actual[at],
                        expected[at]
                    );
                }
            }
        }
        results.push(actual);
    }
    let at = (72 * 160 + 80) * 4;
    assert!(
        results[0][at].abs_diff(results[1][at]) > 10,
        "filter order must remain observable"
    );
}

/// A color-only backdrop transforms background pixels while children retain sharp edges.
#[test]
fn color_only_backdrop_leaves_foreground_unfiltered() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        return;
    };
    let mut context = Context::new(device, queue);
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 64.0, 64.0),
        &Paint::from_color(Color::rgb(1.0, 0.0, 0.0)),
    );
    b.save_layer_backdrop(
        Some(Rect::new(8.0, 8.0, 48.0, 48.0)),
        &Paint::default(),
        Backdrop::new(ImageFilter::color(ColorFilter::Blend(
            Color::rgb(0.0, 0.0, 1.0),
            BlendMode::Src,
        ))),
    );
    b.draw_rect(
        Rect::new(24.0, 24.0, 16.0, 16.0),
        &Paint::from_color(Color::rgb(0.0, 1.0, 0.0)),
    );
    b.restore();
    let pixels = context.render_to_rgba(&b.build(), [64, 64], Some(Color::TRANSPARENT));
    for (x, expected) in [
        (4, [255, 0, 0, 255]),
        (23, [0, 0, 255, 255]),
        (24, [0, 255, 0, 255]),
        (40, [0, 0, 255, 255]),
    ] {
        let at = (32 * 64 + x) * 4;
        assert_eq!(&pixels[at..at + 4], &expected);
    }
}
