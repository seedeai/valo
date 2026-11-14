//! Painting the whole clip, as Impeller's `Canvas::DrawPaint` does: the paint covers every
//! texel of the target and the clips cut it, so it reaches past any shape, a blend that
//! changes transparent pixels changes the whole clip, a shader samples in the draw's own
//! coordinates, and a save layer around it covers its clip.

use valo::{
    BlendMode, ClipOp, Color, Context, DisplayList, DisplayListBuilder, Paint, Point, Rect, Shader,
};

use crate::{context, goldens_dir};

const SIZE: [u32; 2] = [120, 80];
const BACKGROUND: Color = Color::rgb(1.0, 1.0, 1.0);

fn render(context: &mut Context, list: DisplayList) -> Vec<u8> {
    valo_harness::render_to_rgba(context, list, SIZE, Some(BACKGROUND))
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE[0] + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

fn assert_near(actual: [u8; 4], expected: [u8; 4]) {
    let close = actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 2);
    assert!(close, "{actual:?} is not {expected:?}");
}

fn blue() -> Paint {
    Paint::from_color(Color::rgb(0.0, 0.0, 1.0))
}

fn red() -> Paint {
    Paint::from_color(Color::rgb(1.0, 0.0, 0.0))
}

/// A paint fills the clip in force, whatever the transform: inside a rotated clip it shows,
/// outside it the background does.
#[test]
fn a_paint_fills_the_clip_under_any_transform() {
    let Some(mut context) = context("a_paint_fills_the_clip_under_any_transform") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.translate(60.0, 40.0);
    b.rotate(0.6);
    b.clip_rect(Rect::new(-20.0, -20.0, 40.0, 40.0), ClipOp::Intersect, true);
    b.draw_paint(&blue());
    let pixels = render(&mut context, b.build());
    assert_near(pixel(&pixels, 60, 40), [0, 0, 255, 255]);
    assert_near(pixel(&pixels, 2, 2), [255, 255, 255, 255]);
    assert_near(pixel(&pixels, 117, 77), [255, 255, 255, 255]);

    let mut b = DisplayListBuilder::new();
    b.rotate(0.6);
    b.draw_paint(&blue());
    let pixels = render(&mut context, b.build());
    for (x, y) in [(0, 0), (119, 0), (0, 79), (119, 79)] {
        assert_near(pixel(&pixels, x, y), [0, 0, 255, 255]);
    }
}

/// A blend that changes pixels a draw leaves transparent changes the whole clip: Clear
/// empties it and DstIn fades it, beyond any shape drawn there.
#[test]
fn a_destructive_blend_changes_the_whole_clip() {
    let Some(mut context) = context("a_destructive_blend_changes_the_whole_clip") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.draw_rect(Rect::new(0.0, 0.0, 120.0, 80.0), &red());
    b.save();
    b.clip_rect(Rect::new(0.0, 0.0, 40.0, 80.0), ClipOp::Intersect, false);
    b.draw_paint(&Paint {
        blend_mode: BlendMode::Clear,
        ..Paint::from_color(Color::TRANSPARENT)
    });
    b.restore();
    b.clip_rect(Rect::new(40.0, 0.0, 40.0, 80.0), ClipOp::Intersect, false);
    b.draw_paint(&Paint {
        blend_mode: BlendMode::DstIn,
        ..Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.5))
    });
    let pixels = render(&mut context, b.build());
    assert_eq!(pixel(&pixels, 20, 40)[3], 0, "cleared");
    let faded = pixel(&pixels, 60, 40);
    assert!(faded[3].abs_diff(128) <= 2, "faded to half: {faded:?}");
    assert_near(pixel(&pixels, 100, 40), [255, 0, 0, 255]);
}

/// A save layer around a paint covers its own clip, so the paint fills the clip through it at
/// the layer's alpha, and a Clear paint inside it empties the layer of what was drawn before.
#[test]
fn a_layer_around_a_paint_covers_its_clip() {
    let Some(mut context) = context("a_layer_around_a_paint_covers_its_clip") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.clip_rect(Rect::new(10.0, 10.0, 50.0, 60.0), ClipOp::Intersect, false);
    b.save_layer(None, &Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.5)));
    b.draw_paint(&blue());
    b.restore();
    let pixels = render(&mut context, b.build());
    assert_near(pixel(&pixels, 12, 12), [128, 128, 255, 255]);
    assert_near(pixel(&pixels, 58, 68), [128, 128, 255, 255]);
    assert_near(pixel(&pixels, 80, 40), [255, 255, 255, 255]);

    let mut b = DisplayListBuilder::new();
    b.save_layer(None, &Paint::default());
    b.draw_rect(Rect::new(0.0, 0.0, 120.0, 80.0), &red());
    b.draw_paint(&Paint {
        blend_mode: BlendMode::Clear,
        ..Paint::from_color(Color::TRANSPARENT)
    });
    b.draw_rect(Rect::new(80.0, 0.0, 40.0, 80.0), &blue());
    b.restore();
    let pixels = render(&mut context, b.build());
    assert_near(pixel(&pixels, 40, 40), [255, 255, 255, 255]);
    assert_near(pixel(&pixels, 100, 40), [0, 0, 255, 255]);
}

fn gradient() -> Paint {
    Paint::from_shader(Shader::linear(
        Point::new(0.0, 0.0),
        Point::new(30.0, 0.0),
        Color::rgb(1.0, 0.0, 0.0),
        Color::rgb(0.0, 0.0, 1.0),
    ))
}

/// A paint's shader samples in the draw's own coordinates, as it would on a rect covering the
/// whole target.
#[test]
fn a_paints_shader_samples_in_local_coordinates() {
    let Some(mut context) = context("a_paints_shader_samples_in_local_coordinates") else {
        return;
    };
    let record = |fill: &dyn Fn(&mut DisplayListBuilder)| {
        let mut b = DisplayListBuilder::new();
        b.translate(30.0, 10.0);
        b.scale(2.0, 1.5);
        fill(&mut b);
        b.build()
    };
    let painted = render(&mut context, record(&|b| b.draw_paint(&gradient())));
    let covered = render(
        &mut context,
        record(&|b| b.draw_rect(Rect::new(-15.0, -10.0, 60.0, 60.0), &gradient())),
    );
    let difference = painted
        .iter()
        .zip(&covered)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(difference <= 2, "differs by {difference}");
}

/// A gradient background under a rotation, a paint in a rounded clip, and a Multiply paint
/// over both.
#[test]
fn draw_paint_golden() {
    let Some(mut context) = context("draw_paint_golden") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.save();
    b.translate(60.0, 40.0);
    b.rotate(0.4);
    b.draw_paint(&gradient());
    b.restore();
    b.save();
    b.clip_rrect(
        Rect::new(20.0, 15.0, 50.0, 50.0),
        16.0,
        ClipOp::Intersect,
        true,
    );
    b.draw_paint(&Paint::from_color(Color::rgba(1.0, 1.0, 0.2, 0.8)));
    b.restore();
    b.clip_rect(Rect::new(0.0, 40.0, 120.0, 40.0), ClipOp::Intersect, true);
    b.draw_paint(&Paint {
        blend_mode: BlendMode::Multiply,
        ..Paint::from_color(Color::rgb(0.3, 0.8, 0.6))
    });
    let pixels = render(&mut context, b.build());
    valo_harness::assert_golden(goldens_dir(), "draw_paint", SIZE, &pixels);
}
