//! A gradient's tile mode past its span, and a sweep's start and end angles: Skia's
//! `SkGradientShader` with `SkTileMode::kDecal` and `MakeSweep(…, startAngle, endAngle, …)`, as
//! Impeller's gradient contents draw them (`linear_gradient_uniform_fill.frag`,
//! `sweep_gradient_fill.frag`). A decal gradient paints nothing past its span, so whatever lies
//! beneath shows there, however opaque the stops. Skips (with a note) when the machine has no
//! adapter; `VALO_BLESS=1 cargo test` regenerates the golden.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use valo::{
    Color, Context, DisplayListBuilder, FocalCircle, GradientStop, Matrix, Offscreen, Paint, Point,
    Rect, Shader, SpreadMode,
};

use crate::{context, goldens_dir};

const RED: Color = Color {
    r: 1.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};
const BLUE: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 1.0,
    a: 1.0,
};
/// What the scenes are cleared to: what a decal gradient leaves showing.
const GROUND: Color = Color {
    r: 0.0,
    g: 1.0,
    b: 0.0,
    a: 1.0,
};
const RED_PIXEL: [u8; 4] = [255, 0, 0, 255];
const BLUE_PIXEL: [u8; 4] = [0, 0, 255, 255];
const GROUND_PIXEL: [u8; 4] = [0, 255, 0, 255];

/// `count` stops, red up to the middle and blue past it: up to eight take the stops in the
/// record, more a baked ramp.
fn stops(count: usize) -> Vec<GradientStop> {
    (0..count)
        .map(|index| {
            let offset = index as f32 / (count - 1) as f32;
            let color = if offset < 0.5 { RED } else { BLUE };
            GradientStop { offset, color }
        })
        .collect()
}

fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    let at = (y * width + x) * 4;
    pixels[at..at + 4].try_into().expect("four channels")
}

fn render(context: &mut Context, shader: Shader, size: [u32; 2]) -> Vec<u8> {
    let mut scene = DisplayListBuilder::new();
    let [width, height] = size.map(|side| side as f32);
    scene.draw_rect(
        Rect::new(0.0, 0.0, width, height),
        &Paint::from_shader(shader),
    );
    valo_harness::render_to_rgba(context, scene.build(), size, Some(GROUND))
}

/// A decal linear gradient paints its span and nothing past either end, through the stops in the
/// record and through a ramp alike.
#[test]
fn a_decal_linear_gradient_paints_only_its_span() {
    let Some(mut context) = context("a_decal_linear_gradient_paints_only_its_span") else {
        return;
    };
    for count in [4, 9] {
        let shader = Shader::Linear {
            start: Point::new(32.0, 0.0),
            end: Point::new(96.0, 0.0),
            stops: stops(count),
            spread: SpreadMode::Decal,
            local: Matrix::IDENTITY,
        };
        let pixels = render(&mut context, shader, [128, 8]);
        let at = |x| pixel(&pixels, 128, x, 4);
        assert_eq!(at(8), GROUND_PIXEL, "before the start, {count} stops");
        assert_eq!(at(33), RED_PIXEL, "just inside the start, {count} stops");
        assert_eq!(at(94), BLUE_PIXEL, "just inside the end, {count} stops");
        assert_eq!(at(120), GROUND_PIXEL, "past the end, {count} stops");
    }
}

/// A decal radial gradient paints its disc and nothing past its radius.
#[test]
fn a_decal_radial_gradient_paints_only_its_disc() {
    let Some(mut context) = context("a_decal_radial_gradient_paints_only_its_disc") else {
        return;
    };
    let shader = Shader::Radial {
        center: Point::new(32.0, 32.0),
        radius: 20.0,
        stops: stops(4),
        spread: SpreadMode::Decal,
        focus: None,
        local: Matrix::IDENTITY,
    };
    let pixels = render(&mut context, shader, [64, 64]);
    assert_eq!(pixel(&pixels, 64, 32, 32), RED_PIXEL, "the centre");
    assert_eq!(
        pixel(&pixels, 64, 32 + 17, 32),
        BLUE_PIXEL,
        "inside the rim"
    );
    assert_eq!(
        pixel(&pixels, 64, 32 + 25, 32),
        GROUND_PIXEL,
        "past the rim"
    );
    let focal = Shader::Radial {
        center: Point::new(32.0, 32.0),
        radius: 20.0,
        stops: stops(4),
        spread: SpreadMode::Decal,
        focus: Some(FocalCircle::point(Point::new(28.0, 32.0))),
        local: Matrix::IDENTITY,
    };
    let pixels = render(&mut context, focal, [64, 64]);
    assert_eq!(
        pixel(&pixels, 64, 2, 2),
        GROUND_PIXEL,
        "past the end circle of a focal gradient"
    );
}

/// The sweep of the quarter from +x to +y, clockwise: down and to the right of its centre.
fn quarter_sweep(spread: SpreadMode) -> Shader {
    Shader::Sweep {
        center: Point::new(32.0, 32.0),
        start_angle: 0.0,
        end_angle: FRAC_PI_2,
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: RED,
            },
            GradientStop {
                offset: 1.0,
                color: BLUE,
            },
        ],
        spread,
        local: Matrix::IDENTITY,
    }
}

/// The pixel at `angle` from +x, clockwise, `distance` from the centre of [`quarter_sweep`].
fn at_angle(pixels: &[u8], angle: f32, distance: f32) -> [u8; 4] {
    let x = 32.0 + distance * angle.cos();
    let y = 32.0 + distance * angle.sin();
    pixel(pixels, 64, x as usize, y as usize)
}

/// Skia's sweep: an angle's offset is how far it lies from the start toward the end, not
/// wrapped round to the start, and angles past the end take their colour from the tile mode.
#[test]
fn a_sweep_runs_from_its_start_angle_to_its_end_angle() {
    let Some(mut context) = context("a_sweep_runs_from_its_start_angle_to_its_end_angle") else {
        return;
    };
    let clamped = render(&mut context, quarter_sweep(SpreadMode::Pad), [64, 64]);
    let middle = at_angle(&clamped, PI / 4.0, 20.0);
    assert!(
        middle[0].abs_diff(128) <= 2 && middle[2].abs_diff(127) <= 2,
        "half way round the quarter is half way between the stops: {middle:?}"
    );
    let near_start = at_angle(&clamped, 0.1, 20.0);
    assert!(near_start[0] > 230, "near the start: {near_start:?}");
    assert_eq!(
        at_angle(&clamped, PI, 20.0),
        BLUE_PIXEL,
        "past the end clamps to the last stop"
    );
    assert_eq!(
        at_angle(&clamped, -PI / 4.0, 20.0),
        BLUE_PIXEL,
        "an angle just short of a whole turn is past the end, not before the start"
    );

    let decal = render(&mut context, quarter_sweep(SpreadMode::Decal), [64, 64]);
    assert_near(
        at_angle(&decal, PI / 4.0, 20.0),
        middle,
        "decal inside the span",
    );
    assert_eq!(
        at_angle(&decal, PI, 20.0),
        GROUND_PIXEL,
        "decal past the end"
    );

    let repeated = render(&mut context, quarter_sweep(SpreadMode::Repeat), [64, 64]);
    assert_near(
        at_angle(&repeated, PI + PI / 4.0, 20.0),
        middle,
        "the third quarter repeats the first",
    );
}

/// `assert_near` asserts two pixels agree within the rounding of a sampled angle.
fn assert_near(actual: [u8; 4], expected: [u8; 4], what: &str) {
    let near = actual
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual.abs_diff(expected) <= 2);
    assert!(near, "{what}: {actual:?} against {expected:?}");
}

/// A whole turn from a start other than zero, repeated, is the whole circle turned to begin
/// there: what Canvas2D's `createConicGradient` and valo-web's `sweepGradient` draw.
#[test]
fn a_repeated_whole_turn_is_the_circle_turned_to_its_start() {
    let Some(mut context) = context("a_repeated_whole_turn_is_the_circle_turned_to_its_start")
    else {
        return;
    };
    let start = FRAC_PI_2;
    let Shader::Sweep { stops, .. } = quarter_sweep(SpreadMode::Repeat) else {
        unreachable!("a quarter sweep is a sweep");
    };
    let shader = Shader::Sweep {
        center: Point::new(32.0, 32.0),
        start_angle: start,
        end_angle: start + TAU,
        stops,
        spread: SpreadMode::Repeat,
        local: Matrix::IDENTITY,
    };
    let pixels = render(&mut context, shader, [64, 64]);
    let just_after = at_angle(&pixels, start + 0.05, 20.0);
    let just_before = at_angle(&pixels, start - 0.05, 20.0);
    assert!(
        just_after[0] > 250 && just_before[2] > 250,
        "the seam lies on the start angle: {just_after:?} after it, {just_before:?} before"
    );
}

/// Decal linear, radial and sweep gradients over a ground they leave showing, and sweeps over
/// part of a turn under each tile mode.
fn gradient_tile_modes_scene() -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    let ramp = stops(2);
    b.draw_rect(
        Rect::new(20.0, 20.0, 200.0, 80.0),
        &Paint::from_shader(Shader::Linear {
            start: Point::new(70.0, 0.0),
            end: Point::new(170.0, 0.0),
            stops: ramp.clone(),
            spread: SpreadMode::Decal,
            local: Matrix::IDENTITY,
        }),
    );
    b.draw_rect(
        Rect::new(240.0, 20.0, 80.0, 80.0),
        &Paint::from_shader(Shader::Radial {
            center: Point::new(280.0, 60.0),
            radius: 30.0,
            stops: ramp.clone(),
            spread: SpreadMode::Decal,
            focus: None,
            local: Matrix::IDENTITY,
        }),
    );
    for (index, spread) in [
        SpreadMode::Pad,
        SpreadMode::Repeat,
        SpreadMode::Reflect,
        SpreadMode::Decal,
    ]
    .into_iter()
    .enumerate()
    {
        let x = 20.0 + index as f32 * 80.0;
        b.draw_rect(
            Rect::new(x, 120.0, 70.0, 70.0),
            &Paint::from_shader(Shader::Sweep {
                center: Point::new(x + 35.0, 155.0),
                start_angle: PI / 4.0,
                end_angle: PI,
                stops: ramp.clone(),
                spread,
                local: Matrix::IDENTITY,
            }),
        );
    }
    b.build()
}

#[test]
fn gradient_tile_modes_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP gradient_tile_modes_golden: no GPU adapter");
        return;
    };
    let mut context = Context::new(device.clone(), queue.clone());
    let size = [340u32, 210u32];
    let offscreen = Offscreen::new(&device, size);
    context.render(
        &gradient_tile_modes_scene(),
        &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))),
    );
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "gradient_tile_modes", size, &rgba);
}
