//! Runtime paint shaders: an effect's `shade` filling rects, paths and strokes, its uniforms,
//! images and override variants reaching the GPU. Skips (with a note) when the machine has no
//! adapter; `VALO_BLESS=1 cargo test` regenerates the golden.

use std::sync::Arc;

use valo::{
    Color, Context, DisplayListBuilder, EffectImage, FillRule, Filter, ImageDesc, Offscreen, Paint,
    PaintStyle, Path as ValoPath, PathBuilder, Point, Rect, RuntimeEffect, RuntimeShader, Sampling,
    Shader, Stroke, UniformArray, UniformLayout, UniformType,
};

use crate::{floats, goldens_dir};

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
        image: Some(image),
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

/// An array of `vec4f` in the uniforms reaches the shader element by element at the stride the
/// reflection reports: Flutter's tightly packed float array, four floats an element.
#[test]
fn an_array_uniform_reaches_the_shader() {
    let Some(mut ctx) = crate::context("an_array_uniform_reaches_the_shader") else {
        return;
    };
    let effect = RuntimeEffect::shader(
        "struct Bands { width: f32, colours: array<vec4<f32>, 3> }
         @group(2) @binding(0) var<uniform> bands: Bands;
         fn shade(position: vec2<f32>) -> vec4<f32> {
             return bands.colours[min(u32(position.x / bands.width), 2u)];
         }",
    )
    .expect("valid effect");
    let colours = effect.uniforms().field("colours").expect("a colours field");
    assert_eq!(colours.ty, UniformType::Vec4F);
    assert_eq!(
        colours.array,
        Some(UniformArray {
            count: 3,
            stride: 16
        })
    );
    let fields: [&[f32]; 4] = [
        &[10.0],               // width
        &[1.0, 0.0, 0.0, 1.0], // red
        &[0.0, 1.0, 0.0, 1.0], // green
        &[0.0, 0.0, 1.0, 1.0], // blue
    ];
    let flutter_floats = fields.concat();
    let uniforms = repack(effect.uniforms(), &flutter_floats);
    let shader = RuntimeShader::new(effect, uniforms, Vec::new()).expect("bands fit");
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 30.0, 4.0),
        &Paint::from_shader(Shader::Runtime(shader)),
    );
    let rgba = valo_harness::render_to_rgba(&mut ctx, b.build(), [30, 4], Some(Color::BLACK));
    let pixel = |x: usize| &rgba[x * 4..x * 4 + 4];
    assert_eq!(pixel(5), [255, 0, 0, 255]);
    assert_eq!(pixel(15), [0, 255, 0, 255]);
    assert_eq!(pixel(25), [0, 0, 255, 255]);
}

/// Material's ink sparkle, ported to WGSL as Waft's Material port uses it, mid-ripple with
/// Flutter's constant turbulence seed: the wave, its ring and the sparkles in it.
#[test]
fn material_ink_sparkle() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP material_ink_sparkle: no GPU adapter");
        return;
    };
    let source = include_str!("../../../../assets/shaders/ink_sparkle.wgsl");
    let effect = RuntimeEffect::shader(source).expect("the port is a valid effect");
    let resolution_scale = effect.uniforms().field("u_resolution_scale");
    assert_eq!(resolution_scale.map(|field| field.offset), Some(48));
    let size = [320u32, 200u32];
    let floats = ink_sparkle_floats([size[0] as f32, size[1] as f32], Point::new(120.0, 90.0));
    let uniforms = repack(effect.uniforms(), &floats);
    let shader = RuntimeShader::new(effect, uniforms, Vec::new()).expect("the uniforms fit");

    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32),
        &Paint::from_shader(Shader::Runtime(shader)),
    );
    let mut ctx = Context::new(device.clone(), queue.clone());
    let offscreen = Offscreen::new(&device, size);
    ctx.render(
        &b.build(),
        &offscreen.target(Some(Color::rgb(0.2, 0.22, 0.3))),
    );
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "material_ink_sparkle", size, &rgba);
}

/// `ink_sparkle_floats` are the 28 floats `InkSparkle._updateFragmentShader` writes, in its
/// order, for a box of `size` with the ripple at `centre`, its radius scale at 0.3 and fully shown.
fn ink_sparkle_floats(size: [f32; 2], centre: Point) -> Vec<f32> {
    use std::f32::consts::PI;
    let [width, height] = size;
    let radius_scale = 0.3;
    let turbulence_scale = 1.5f32;
    let turbulence_phase = 1337.0 + radius_scale;
    let rotate_right = PI * 0.0078125;
    let rotation1 = turbulence_phase * rotate_right + 1.7 * PI;
    let rotation2 = turbulence_phase * -rotate_right + 2.0 * PI;
    let rotation3 = turbulence_phase * rotate_right + 2.75 * PI;
    let circle = |base: f32, speed: f32, angle: f32| {
        [
            base + turbulence_phase * speed * (turbulence_scale * angle).cos(),
            base + turbulence_phase * speed * (turbulence_scale * angle).sin(),
        ]
    };
    let [c1x, c1y] = circle(turbulence_scale * 0.5, 0.01, 0.55);
    let [c2x, c2y] = circle(turbulence_scale * 0.2, -0.0066, 0.45);
    let [c3x, c3y] = circle(turbulence_scale, -0.0066, 0.35);
    // `_getTargetRadius`, half the box's diagonal, times `_targetRadiusMultiplier`.
    let target_radius = width.hypot(height) / 2.0 * 2.3;
    let fields: [&[f32]; 13] = [
        &[0.25, 0.45, 0.95, 0.6],            // u_color
        &[1.0, 1.0, 1.0, radius_scale],      // u_composite_1
        &[centre.x, centre.y],               // u_center
        &[target_radius],                    // u_max_radius
        &[1.0 / width, 1.0 / height],        // u_resolution_scale
        &[2.1 / width, 2.1 / height],        // u_noise_scale
        &[turbulence_phase / 1000.0],        // u_noise_phase
        &[c1x, c1y],                         // u_circle1
        &[c2x, c2y],                         // u_circle2
        &[c3x, c3y],                         // u_circle3
        &[rotation1.cos(), rotation1.sin()], // u_rotation1
        &[rotation2.cos(), rotation2.sin()], // u_rotation2
        &[rotation3.cos(), rotation3.sin()], // u_rotation3
    ];
    fields.concat()
}

/// `repack` lays floats packed tightly in field order, as Flutter's `setFloat` indexes them,
/// into the effect's WGSL layout, as Impeller's `padding_layout` does and Waft will.
fn repack(layout: &UniformLayout, floats: &[f32]) -> Vec<u8> {
    let mut bytes = vec![0u8; layout.size as usize];
    let mut next = floats.iter();
    for field in &layout.fields {
        let (count, stride) = field
            .array
            .map_or((1, 0), |array| (array.count, array.stride));
        for element in 0..count {
            let start = (field.offset + element * stride) as usize;
            for component in 0..float_count(field.ty) {
                let value = next.next().expect("a float for every component");
                let at = start + component * 4;
                bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
            }
        }
    }
    assert!(next.next().is_none(), "every float has a component");
    bytes
}

fn float_count(ty: UniformType) -> usize {
    match ty {
        UniformType::F32 => 1,
        UniformType::Vec2F => 2,
        UniformType::Vec3F => 3,
        UniformType::Vec4F => 4,
        other => panic!("not a float type the tests write: {other:?}"),
    }
}
