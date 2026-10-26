//! Runtime filters: an effect's `shade` over a save layer's content and over a backdrop, reading
//! `source` through `uv_of`, past the drawn area by its margin and at a lower resolution when it
//! asks. Skips (with a note) when the machine has no adapter; `VALO_BLESS=1 cargo test`
//! regenerates the golden.

use std::path::Path;
use std::sync::Arc;

use valo::{
    Backdrop, Color, Context, DisplayListBuilder, FilterInputs, ImageFilter, Matrix, Offscreen,
    Paint, Rect, RuntimeEffect, RuntimeShader,
};

fn goldens_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens"))
}

fn floats(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect()
}

fn filter(effect: &Arc<RuntimeEffect>, uniforms: &[f32]) -> ImageFilter {
    let shader = RuntimeShader::new(effect.clone(), floats(uniforms), Vec::new())
        .expect("the uniforms fit the effect");
    ImageFilter::runtime(shader).expect("a filter's effect")
}

/// Moves each row sideways along a sine wave.
const RIPPLE: &str = "
struct Ripple {
    amplitude: f32,
    wavelength: f32,
}
@group(2) @binding(0) var<uniform> ripple: Ripple;

fn shade(position: vec2<f32>) -> vec4<f32> {
    let shift = ripple.amplitude * sin(position.y / ripple.wavelength * 6.2831853);
    return textureSampleLevel(source, source_sampler, uv_of(position + vec2<f32>(shift, 0.0)), 0.0);
}
";

/// Magnifies what lies behind a disc, bending outward toward its rim, where it reads past the
/// disc.
const LENS: &str = "
struct Lens {
    centre: vec2<f32>,
    radius: f32,
    zoom: f32,
}
@group(2) @binding(0) var<uniform> lens: Lens;

fn shade(position: vec2<f32>) -> vec4<f32> {
    let offset = position - lens.centre;
    let d = length(offset) / lens.radius;
    let k = mix(1.0 / lens.zoom, 1.3, d * d * d);
    let seen = textureSampleLevel(source, source_sampler, uv_of(lens.centre + offset * k), 0.0);
    let rim = smoothstep(0.9, 1.0, d) * 0.4;
    let inside = 1.0 - smoothstep(0.985, 1.0, d);
    return mix(seen, vec4<f32>(1.0), rim) * inside;
}
";

/// Reads the source where it is.
const IDENTITY: &str = "
fn shade(position: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(source, source_sampler, uv_of(position), 0.0);
}
";

fn stripes(b: &mut DisplayListBuilder, area: Rect, width: f32) {
    let mut x = area.x;
    let mut index = 0;
    while x < area.right() {
        let colour = if index % 2 == 0 {
            Color::rgb(0.3, 0.55, 1.0)
        } else {
            Color::rgb(0.95, 0.95, 0.97)
        };
        let w = width.min(area.right() - x);
        b.draw_rect(
            Rect::new(x, area.y, w, area.height),
            &Paint::from_color(colour),
        );
        x += width;
        index += 1;
    }
}

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

#[test]
fn runtime_filters_shade_layers_and_backdrops() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP runtime_filters_shade_layers_and_backdrops: no GPU adapter");
        return;
    };
    let ripple = RuntimeEffect::filter(
        RIPPLE,
        FilterInputs {
            margin: 12.0,
            ..FilterInputs::default()
        },
    )
    .expect("valid filter");
    let lens = RuntimeEffect::filter(
        LENS,
        FilterInputs {
            margin: 24.0,
            source_scale: 0.5,
            ..FilterInputs::default()
        },
    )
    .expect("valid filter");

    let mut b = DisplayListBuilder::new();
    // A save layer whose content the ripple moves; its margin brings the stripes' edges in.
    b.save_layer(
        None,
        &Paint {
            image_filter: Some(filter(&ripple, &[10.0, 60.0])),
            ..Paint::default()
        },
    );
    stripes(&mut b, Rect::new(30.0, 20.0, 260.0, 200.0), 20.0);
    b.restore();
    // A lens over a checker, reading past its disc at half resolution.
    checker(&mut b, Rect::new(320.0, 0.0, 320.0, 240.0), 20.0);
    let centre = (480.0, 120.0);
    let radius = 80.0;
    b.save_layer_backdrop(
        Some(Rect::new(
            centre.0 - radius,
            centre.1 - radius,
            2.0 * radius,
            2.0 * radius,
        )),
        &Paint::default(),
        Backdrop::new(filter(&lens, &[centre.0, centre.1, radius, 1.6])),
    );
    b.restore();
    let list = b.build();

    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [640u32, 240u32];
    let offscreen = Offscreen::new(&device, size);
    ctx.render(&list, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "runtime_filters", size, &rgba);
}

/// A filter that reads its source where it is leaves the picture as it was, on a layer and on
/// a backdrop, under a turned and scaled transform: `position` and `uv_of` agree with where
/// things are drawn.
#[test]
fn an_identity_filter_leaves_the_picture_as_it_was() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP an_identity_filter_leaves_the_picture_as_it_was: no GPU adapter");
        return;
    };
    let identity = RuntimeEffect::filter(IDENTITY, FilterInputs::default()).expect("valid filter");
    let turned = Matrix::translation(120.0, 20.0)
        .then(&Matrix::rotation(0.5))
        .then(&Matrix::scale(1.3, 0.9));
    let scene = |filtered: bool| {
        let mut b = DisplayListBuilder::new();
        checker(&mut b, Rect::new(0.0, 0.0, 240.0, 160.0), 16.0);
        b.save();
        b.concat(&turned);
        if filtered {
            b.save_layer(
                None,
                &Paint {
                    image_filter: Some(filter(&identity, &[])),
                    ..Paint::default()
                },
            );
        }
        stripes(&mut b, Rect::new(0.0, 0.0, 100.0, 60.0), 10.0);
        if filtered {
            b.restore();
            b.save_layer_backdrop(
                Some(Rect::new(-20.0, 70.0, 90.0, 50.0)),
                &Paint::default(),
                Backdrop::new(filter(&identity, &[])),
            );
            b.restore();
        }
        b.restore();
        b.build()
    };
    let mut ctx = Context::new(device, queue);
    let size = [240, 160];
    let clear = Some(Color::BLACK);
    let plain = ctx.render_to_rgba(&scene(false), size, clear);
    let filtered = ctx.render_to_rgba(&scene(true), size, clear);
    let mut worst = 0;
    for (a, b) in plain.iter().zip(&filtered) {
        worst = worst.max((*a as i32 - *b as i32).abs());
    }
    // Resampling at a turned edge moves no more than an antialiased edge's shade.
    let differing = plain
        .chunks(4)
        .zip(filtered.chunks(4))
        .filter(|(a, b)| {
            a.iter()
                .zip(b.iter())
                .any(|(a, b)| (*a as i32 - *b as i32).abs() > 8)
        })
        .count();
    assert!(
        differing < 60,
        "{differing} pixels differ by more than 8 (worst {worst})"
    );
}

/// Five bands, each reading one level deeper into the source's pyramid.
const LEVELS: &str = "
fn shade(position: vec2<f32>) -> vec4<f32> {
    let band = floor(position.x / 64.0);
    return textureSampleLevel(source, source_sampler, uv_of(position), band);
}
";

/// The source's level count, as red.
const LEVEL_COUNT: &str = "
fn shade(position: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(source_levels() / 16.0, 0.0, 0.0, 1.0);
}
";

/// A filter that reads levels gets the source's blur pyramid: each level Core Animation's
/// downsample of the one before, down to a texel.
#[test]
fn a_pyramid_blurs_each_level_more() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_pyramid_blurs_each_level_more: no GPU adapter");
        return;
    };
    let pyramid = FilterInputs {
        source_mips: true,
        ..FilterInputs::default()
    };
    let levels = RuntimeEffect::filter(LEVELS, pyramid).expect("valid filter");
    let count = RuntimeEffect::filter(
        LEVEL_COUNT,
        FilterInputs {
            source_scale: 0.5,
            ..pyramid
        },
    )
    .expect("valid filter");

    let mut b = DisplayListBuilder::new();
    checker(&mut b, Rect::new(0.0, 0.0, 320.0, 96.0), 8.0);
    b.save_layer_backdrop(
        Some(Rect::new(0.0, 0.0, 320.0, 64.0)),
        &Paint::default(),
        Backdrop::new(filter(&levels, &[])),
    );
    b.restore();
    // 64 × 32 at half resolution is 32 × 16: six levels.
    b.save_layer_backdrop(
        Some(Rect::new(0.0, 64.0, 64.0, 32.0)),
        &Paint::default(),
        Backdrop::new(filter(&count, &[])),
    );
    b.restore();
    let list = b.build();

    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [320u32, 96u32];
    let offscreen = Offscreen::new(&device, size);
    ctx.render(&list, &offscreen.target(Some(Color::BLACK)));
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    let red_at = |x: u32, y: u32| rgba[((y * size[0] + x) * 4) as usize];
    assert!(
        (red_at(32, 80) as i32 - 96).abs() <= 1,
        "{}",
        red_at(32, 80)
    );
    valo_harness::assert_golden(goldens_dir(), "runtime_filter_pyramid", size, &rgba);
}

/// The area the filter shades, as opaque colour: its origin in red and green, its width plus
/// its height in blue, each over 255.
const BOUNDS: &str = "
fn shade(position: vec2<f32>) -> vec4<f32> {
    let bounds = filter_bounds();
    return vec4<f32>(bounds.x, bounds.y, bounds.z + bounds.w, 255.0) / 255.0;
}
";

/// `filter_bounds()` is the area a filter shades, in its local coordinates: a backdrop's own,
/// under a translation that the local coordinates do not include.
#[test]
fn a_filter_knows_the_area_it_shades() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_filter_knows_the_area_it_shades: no GPU adapter");
        return;
    };
    let bounds = RuntimeEffect::filter(BOUNDS, FilterInputs::default()).expect("valid filter");
    let mut b = DisplayListBuilder::new();
    b.translate(10.0, 20.0);
    b.save_layer_backdrop(
        Some(Rect::new(30.0, 40.0, 50.0, 60.0)),
        &Paint {
            blend_mode: valo::BlendMode::Src,
            ..Paint::default()
        },
        Backdrop::new(filter(&bounds, &[])),
    );
    b.restore();
    let mut ctx = Context::new(device, queue);
    let size = [120, 120];
    let rgba = ctx.render_to_rgba(&b.build(), size, Some(Color::TRANSPARENT));
    let at = ((80 * size[0] + 60) * 4) as usize;
    assert_eq!(&rgba[at..at + 4], [30, 40, 110, 255]);
}
