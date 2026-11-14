//! A paint shader's effect used as an image filter, by Flutter's convention for
//! `ImageFilter.shader` (Impeller's `RuntimeEffectFilterContents`): the filter's input is the
//! first image, the input's size in pixels the first uniform, and `position` is in the input's
//! pixels. Skips (with a note) when the machine has no adapter; `VALO_BLESS=1 cargo test`
//! regenerates the golden.

use std::sync::Arc;

use valo::{
    Backdrop, Color, Context, DisplayListBuilder, EffectImage, FillRule, ImageFilter, Offscreen,
    Paint, PathBuilder, Point, Rect, RuntimeEffect, RuntimeShader, Sampling, Shader,
};

use crate::{floats, goldens_dir};

/// Pulls what lies near `centre` toward it, sampling `input` at `position / size`: written once,
/// as Flutter's shaders are, to fill shapes from an image and to filter a backdrop.
const PINCH: &str = "
struct Pinch {
    size: vec2<f32>,
    centre: vec2<f32>,
    radius: f32,
    strength: f32,
}
@group(2) @binding(0) var<uniform> pinch: Pinch;
@group(1) @binding(0) var input: texture_2d<f32>;
@group(1) @binding(1) var input_sampler: sampler;

fn shade(position: vec2<f32>) -> vec4<f32> {
    let offset = position - pinch.centre;
    let falloff = exp(-dot(offset, offset) / (pinch.radius * pinch.radius));
    let seen = pinch.centre + offset * (1.0 + pinch.strength * falloff);
    return textureSampleLevel(input, input_sampler, seen / pinch.size, 0.0);
}
";

fn checker(b: &mut DisplayListBuilder, area: Rect, cell: f32) {
    let columns = (area.width / cell).ceil() as i32;
    let rows = (area.height / cell).ceil() as i32;
    for row in 0..rows {
        for column in 0..columns {
            let colour = if (row + column) % 2 == 0 {
                Color::rgb(0.9, 0.45, 0.25)
            } else {
                Color::rgb(0.15, 0.17, 0.22)
            };
            let x = area.x + column as f32 * cell;
            let y = area.y + row as f32 * cell;
            b.draw_rect(Rect::new(x, y, cell, cell), &Paint::from_color(colour));
        }
    }
}

fn pinch(
    effect: &Arc<RuntimeEffect>,
    size: [f32; 2],
    centre: Point,
    image: Option<valo::Image>,
) -> RuntimeShader {
    let uniforms = floats(&[size[0], size[1], centre.x, centre.y, 40.0, -0.6]);
    let images = vec![EffectImage {
        image,
        sampling: Sampling::default(),
    }];
    RuntimeShader::new(effect.clone(), uniforms, images).expect("the inputs fit the effect")
}

/// One effect fills a rect from a picture drawn to an image, and filters a backdrop: there the
/// renderer binds the backdrop as its first image and sets its size, so the same `position /
/// size` reads it.
#[test]
fn one_effect_paints_and_filters_a_backdrop() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP one_effect_paints_and_filters_a_backdrop: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let effect = RuntimeEffect::shader(PINCH).expect("valid effect");
    let mut picture = DisplayListBuilder::new();
    checker(&mut picture, Rect::new(0.0, 0.0, 160.0, 160.0), 16.0);
    let image = ctx.display_list_to_image(&picture.build(), [160, 160]);

    let mut b = DisplayListBuilder::new();
    // As a paint shader: the image's own size and a centre in the rect's coordinates.
    let painted = pinch(&effect, [160.0, 160.0], Point::new(80.0, 80.0), Some(image));
    b.draw_rect(
        Rect::new(0.0, 0.0, 160.0, 160.0),
        &Paint::from_shader(Shader::runtime(painted).expect("a paint shader")),
    );
    // As a backdrop filter over the same checker drawn beside it: size is the renderer's.
    checker(&mut b, Rect::new(160.0, 0.0, 160.0, 160.0), 16.0);
    let filtered = pinch(&effect, [0.0, 0.0], Point::new(240.0, 80.0), None);
    b.save_layer_backdrop(
        Some(Rect::new(180.0, 20.0, 120.0, 120.0)),
        &Paint::default(),
        Backdrop::new(ImageFilter::runtime(filtered).expect("a filter by Flutter's convention")),
    );
    b.restore();

    let size = [320u32, 160u32];
    let offscreen = Offscreen::new(&device, size);
    ctx.render(&b.build(), &offscreen.target(Some(Color::BLACK)));
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    let pixel = |x: u32, y: u32| {
        let at = ((y * size[0] + x) * 4) as usize;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    };
    // Far from either centre the pinch reads where it is: the two halves agree.
    for (x, y) in [(8, 8), (24, 150), (150, 8)] {
        assert_eq!(pixel(x, y), pixel(x + 160, y), "at ({x}, {y})");
    }
    valo_harness::assert_golden(
        goldens_dir(),
        "shader_filter_paints_and_filters",
        size,
        &rgba,
    );
}

/// A filter on a layer reads its input's size: `position / size` runs from 0 to 1 across what
/// the layer's content covers, wherever the layer lies.
#[test]
fn a_filter_reads_its_inputs_size() {
    let Some(mut ctx) = crate::context("a_filter_reads_its_inputs_size") else {
        return;
    };
    let effect = RuntimeEffect::shader(
        "struct U { size: vec2<f32> }
         @group(2) @binding(0) var<uniform> u: U;
         @group(1) @binding(0) var input: texture_2d<f32>;
         @group(1) @binding(1) var input_sampler: sampler;
         fn shade(position: vec2<f32>) -> vec4<f32> {
             let coverage = textureSampleLevel(input, input_sampler, position / u.size, 0.0).a;
             return vec4<f32>(position / u.size, 0.0, 1.0) * coverage;
         }",
    )
    .expect("valid effect");
    let images = vec![EffectImage {
        image: None,
        sampling: Sampling::default(),
    }];
    let shader = RuntimeShader::new(effect, floats(&[0.0, 0.0]), images).expect("the inputs fit");
    let mut b = DisplayListBuilder::new();
    b.save_layer(
        None,
        &Paint {
            image_filter: Some(ImageFilter::runtime(shader).expect("a filter")),
            ..Paint::default()
        },
    );
    b.draw_rect(
        Rect::new(20.0, 30.0, 100.0, 50.0),
        &Paint::from_color(Color::WHITE),
    );
    b.restore();
    let rgba = valo_harness::render_to_rgba(&mut ctx, b.build(), [140, 100], Some(Color::BLACK));
    let pixel = |x: usize, y: usize| {
        let at = (y * 140 + x) * 4;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    };
    // Texel (x, y) of the 100×50 input shades ((x + 0.5) / 100, (y + 0.5) / 50).
    assert_eq!(pixel(20, 30), [1, 3, 0, 255]);
    assert_eq!(pixel(70, 55), [129, 130, 0, 255]);
    assert_eq!(pixel(119, 79), [254, 252, 0, 255]);
    assert_eq!(
        pixel(10, 10),
        [0, 0, 0, 255],
        "the filter shades only its input"
    );
}

/// An effect prepared for a format draws as it would have unprepared, filling a rect and
/// covering a path, the two draws it warms.
#[test]
fn a_prepared_effect_draws() {
    let Some(mut ctx) = crate::context("a_prepared_effect_draws") else {
        return;
    };
    let effect = RuntimeEffect::shader(
        "fn shade(position: vec2<f32>) -> vec4<f32> { return vec4<f32>(0.0, 1.0, 0.0, 1.0); }",
    )
    .expect("valid effect");
    ctx.prepare_runtime_effect(&effect, Offscreen::FORMAT);
    ctx.prepare_runtime_effect(&effect, Offscreen::FORMAT);
    let shader = RuntimeShader::new(effect, Vec::new(), Vec::new()).expect("no inputs");
    let paint = Paint::from_shader(Shader::runtime(shader).expect("a paint shader"));
    let mut b = DisplayListBuilder::new();
    b.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &paint);
    let mut triangle = PathBuilder::new();
    triangle.move_to((10.0, 0.0));
    triangle.line_to((20.0, 0.0));
    triangle.line_to((20.0, 10.0));
    triangle.close();
    b.draw_path(&triangle.build(), FillRule::NonZero, &paint);
    let rgba = valo_harness::render_to_rgba(&mut ctx, b.build(), [20, 10], Some(Color::BLACK));
    assert_eq!(&rgba[5 * 20 * 4 + 5 * 4..][..4], [0, 255, 0, 255]);
    assert_eq!(&rgba[2 * 20 * 4 + 18 * 4..][..4], [0, 255, 0, 255]);
}

/// A filter reading its input where it is leaves a blurred layer as the blur left it: a blur's
/// smaller result is drawn again at one texel per pixel first, as Impeller re-rasterizes a
/// scaled snapshot, so `position` stays in the tree's pixels.
#[test]
fn a_filter_over_a_blur_reads_it_at_full_resolution() {
    let Some(mut ctx) = crate::context("a_filter_over_a_blur_reads_it_at_full_resolution") else {
        return;
    };
    let effect = RuntimeEffect::shader(
        "struct U { size: vec2<f32> }
         @group(2) @binding(0) var<uniform> u: U;
         @group(1) @binding(0) var input: texture_2d<f32>;
         @group(1) @binding(1) var input_sampler: sampler;
         fn shade(position: vec2<f32>) -> vec4<f32> {
             return textureSampleLevel(input, input_sampler, position / u.size, 0.0);
         }",
    )
    .expect("valid effect");
    let images = vec![EffectImage {
        image: None,
        sampling: Sampling::default(),
    }];
    let identity = RuntimeShader::new(effect, floats(&[0.0, 0.0]), images).expect("the inputs fit");
    let blur = ImageFilter::blur(12.0, 12.0);
    let scene = |filter: ImageFilter| {
        let mut b = DisplayListBuilder::new();
        b.save_layer(
            None,
            &Paint {
                image_filter: Some(filter),
                ..Paint::default()
            },
        );
        checker(&mut b, Rect::new(40.0, 40.0, 80.0, 80.0), 20.0);
        b.restore();
        b.build()
    };
    let composed = ImageFilter::compose(
        ImageFilter::runtime(identity).expect("a filter"),
        blur.clone(),
    );
    let size = [160, 160];
    let expected = valo_harness::render_to_rgba(&mut ctx, scene(blur), size, Some(Color::BLACK));
    let actual = valo_harness::render_to_rgba(&mut ctx, scene(composed), size, Some(Color::BLACK));
    let worst = expected
        .iter()
        .zip(&actual)
        .map(|(e, a)| (*e as i32 - *a as i32).abs())
        .max()
        .unwrap_or(0);
    assert!(worst <= 3, "differs by {worst}");
}
