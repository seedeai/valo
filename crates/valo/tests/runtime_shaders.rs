//! Runtime paint shaders: an effect's `shade` filling rects, paths and strokes, its uniforms,
//! images and override variants reaching the GPU. Skips (with a note) when the machine has no
//! adapter; `VALO_BLESS=1 cargo test` regenerates the golden.

use std::path::Path;
use std::sync::Arc;

use valo::{
    Color, Context, DisplayListBuilder, EffectImage, FillRule, Filter, ImageDesc, Offscreen, Paint,
    PaintStyle, Path as ValoPath, PathBuilder, Point, Rect, RuntimeEffect, RuntimeShader, Sampling,
    Shader, Stroke,
};

fn goldens_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens"))
}

/// Concentric bands around a centre, `count` of them per 80 units.
const RINGS: &str = "
struct Rings {
    centre: vec2<f32>,
    tint: vec4<f32>,
}
@group(2) @binding(0) var<uniform> rings: Rings;
override count: f32 = 6.0;

fn shade(position: vec2<f32>) -> vec4<f32> {
    let band = 0.5 + 0.5 * cos(distance(position, rings.centre) / 80.0 * count * 6.2831853);
    return vec4<f32>(rings.tint.rgb * band, rings.tint.a);
}
";

fn floats(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect()
}

fn rings(effect: &Arc<RuntimeEffect>, centre: Point, tint: [f32; 4]) -> RuntimeShader {
    let [r, g, b, a] = tint;
    let uniforms = floats(&[centre.x, centre.y, 0.0, 0.0, r, g, b, a]);
    RuntimeShader::new(effect.clone(), uniforms, Vec::new()).expect("rings fit the effect")
}

fn star(centre: Point, radius: f32) -> Arc<ValoPath> {
    let mut path = PathBuilder::new();
    for i in 0..5 {
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * 4.0 * std::f32::consts::PI / 5.0;
        let point = (
            centre.x + radius * angle.cos(),
            centre.y + radius * angle.sin(),
        );
        if i == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    path.close();
    path.build()
}

#[test]
fn runtime_shaders_fill_rects_paths_and_strokes() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP runtime_shaders_fill_rects_paths_and_strokes: no GPU adapter");
        return;
    };
    let effect = RuntimeEffect::shader(RINGS).expect("valid effect");
    assert_eq!(effect.uniforms().size, 32);
    assert_eq!(
        effect.uniforms().field("tint").map(|field| field.offset),
        Some(16)
    );

    let mut b = DisplayListBuilder::new();
    // A quad.
    let rect_centre = Point::new(110.0, 120.0);
    b.draw_rect(
        Rect::new(20.0, 30.0, 180.0, 180.0),
        &Paint::from_shader(Shader::Runtime(rings(
            &effect,
            rect_centre,
            [0.35, 0.6, 1.0, 1.0],
        ))),
    );
    // Stencil-then-cover, in a second variant of the same effect, at half the paint's alpha.
    let star_centre = Point::new(320.0, 120.0);
    let fewer = rings(&effect, star_centre, [1.0, 0.55, 0.3, 1.0])
        .constant("count", 2.0)
        .expect("count is an override");
    let mut star_paint = Paint::from_shader(Shader::Runtime(fewer));
    star_paint.color = Color::rgba(0.0, 0.0, 0.0, 0.5);
    b.draw_path(&star(star_centre, 95.0), FillRule::NonZero, &star_paint);
    // A stroke's strip.
    let ring_centre = Point::new(520.0, 120.0);
    let mut stroke = Paint::from_shader(Shader::Runtime(rings(
        &effect,
        ring_centre,
        [0.4, 0.9, 0.55, 1.0],
    )));
    stroke.style = PaintStyle::Stroke(Stroke::new(24.0));
    b.draw_circle((ring_centre.x, ring_centre.y), 70.0, &stroke);
    let list = b.build();

    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [640u32, 240u32];
    let offscreen = Offscreen::new(&device, size);
    ctx.render(&list, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "runtime_shaders", size, &rgba);
}

/// `shade` returns premultiplied colour: the renderer applies the paint's alpha and blends it
/// like any paint. Its images sample as their `Sampling` says.
#[test]
fn shade_returns_premultiplied_colour_and_reads_its_images() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP shade_returns_premultiplied_colour_and_reads_its_images: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let half_blue = RuntimeEffect::shader(
        "fn shade(position: vec2<f32>) -> vec4<f32> { return vec4<f32>(0.0, 0.0, 0.5, 0.5); }",
    )
    .expect("valid effect");
    let checker = RuntimeEffect::shader(
        "@group(1) @binding(0) var checker: texture_2d<f32>;
         @group(1) @binding(1) var checker_sampler: sampler;
         fn shade(position: vec2<f32>) -> vec4<f32> {
             return textureSampleLevel(checker, checker_sampler, (position - vec2<f32>(0.0, 40.0)) / 40.0, 0.0);
         }",
    )
    .expect("valid effect");
    let image = ctx.upload_image(
        ImageDesc {
            size: [2, 2],
            premultiplied: true,
            mips: false,
        },
        &[
            255, 0, 0, 255, /**/ 0, 255, 0, 255, //
            0, 0, 255, 255, /**/ 255, 255, 255, 255,
        ],
    );
    let nearest = Sampling {
        filter: Filter::Nearest,
        ..Sampling::default()
    };

    let mut b = DisplayListBuilder::new();
    let shader = |effect: &Arc<RuntimeEffect>, images| {
        Shader::Runtime(RuntimeShader::new(effect.clone(), Vec::new(), images).unwrap())
    };
    b.draw_rect(
        Rect::new(0.0, 0.0, 20.0, 20.0),
        &Paint::from_shader(shader(&half_blue, Vec::new())),
    );
    let mut faded = Paint::from_shader(shader(&half_blue, Vec::new()));
    faded.color = Color::rgba(0.0, 0.0, 0.0, 0.5);
    b.draw_rect(Rect::new(20.0, 0.0, 20.0, 20.0), &faded);
    let images = vec![EffectImage {
        image,
        sampling: nearest,
    }];
    b.draw_rect(
        Rect::new(0.0, 40.0, 40.0, 40.0),
        &Paint::from_shader(shader(&checker, images)),
    );
    let list = b.build();

    let size = [40u32, 80u32];
    let offscreen = Offscreen::new(&device, size);
    ctx.render(&list, &offscreen.target(Some(Color::BLACK)));
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    let pixel = |x: u32, y: u32| {
        let at = ((y * size[0] + x) * 4) as usize;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    };
    let close = |actual: [u8; 4], expected: [u8; 4]| {
        actual
            .iter()
            .zip(expected)
            .all(|(a, e)| (*a as i32 - e as i32).abs() <= 1)
    };
    // Premultiplied (0, 0, 0.5, 0.5) over opaque black, then at half the paint's alpha.
    assert!(
        close(pixel(10, 10), [0, 0, 128, 255]),
        "{:?}",
        pixel(10, 10)
    );
    assert!(close(pixel(30, 10), [0, 0, 64, 255]), "{:?}", pixel(30, 10));
    // The checker's four texels, one per 20×20 quadrant.
    assert_eq!(pixel(10, 50), [255, 0, 0, 255]);
    assert_eq!(pixel(30, 50), [0, 255, 0, 255]);
    assert_eq!(pixel(10, 70), [0, 0, 255, 255]);
    assert_eq!(pixel(30, 70), [255, 255, 255, 255]);
}
