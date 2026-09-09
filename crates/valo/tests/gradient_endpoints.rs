//! Coincident final stops retain the last color in the padded region.
use valo::{
    Color, Context, DisplayListBuilder, GradientStop, Matrix, Paint, Point, Rect, Shader,
    SpreadMode,
};

/// A gradient coordinate and its expected premultiplied RGBA8 pixel.
type ColorSample = (f32, [u8; 4]);

#[test]
fn uniform_and_texture_gradients_pad_with_the_final_stop_color() {
    let (device, queue) = valo_harness::headless_device().expect("GPU required");
    let mut context = Context::new(device, queue);
    for count in [2, 9] {
        let mut stops: Vec<_> = (0..count - 1)
            .map(|index| GradientStop {
                offset: if count == 2 {
                    1.0
                } else {
                    index as f32 / (count - 2) as f32
                },
                color: Color::rgb(0.0, 0.0, 1.0),
            })
            .collect();
        stops.push(GradientStop {
            offset: 1.0,
            color: Color::rgba(0.0, 0.0, 0.0, 0.25),
        });
        let mut scene = DisplayListBuilder::new();
        scene.draw_rect(
            Rect::new(0.0, 0.0, 64.0, 16.0),
            &Paint::from_shader(Shader::Linear {
                start: Point::new(8.0, 0.0),
                end: Point::new(24.0, 0.0),
                stops,
                spread: SpreadMode::Pad,
                local: Matrix::IDENTITY,
            }),
        );
        let pixels = context.render_to_rgba(&scene.build(), [64, 16], Some(Color::TRANSPARENT));
        let padded = (8 * 64 + 40) * 4;
        assert_eq!(&pixels[padded..padded + 4], &[0, 0, 0, 64], "{count} stops");
        let first = (8 * 64 + 8) * 4;
        assert_eq!(
            &pixels[first..first + 4],
            &[0, 0, 255, 255],
            "{count} stops"
        );
    }
}

/// Samples a gradient at exact binary fractions, with its endpoints at pixel centers.
fn assert_tiled_colors(
    context: &mut Context,
    stops: Vec<GradientStop>,
    spread: SpreadMode,
    reversed: bool,
    samples: &[ColorSample],
) {
    let (origin, length) = if reversed {
        (128.0, -32.0)
    } else {
        (96.0, 32.0)
    };
    let mut scene = DisplayListBuilder::new();
    scene.draw_rect(
        Rect::new(0.0, 0.0, 256.0, 16.0),
        &Paint::from_shader(Shader::Linear {
            start: Point::new(origin + 0.5, 0.0),
            end: Point::new(origin + length + 0.5, 0.0),
            stops: stops.clone(),
            spread,
            local: Matrix::IDENTITY,
        }),
    );
    let pixels = context.render_to_rgba(&scene.build(), [256, 16], Some(Color::TRANSPARENT));
    for &(t, expected) in samples {
        let x = (origin + length * t) as usize;
        let pixel = (8 * 256 + x) * 4;
        let actual = &pixels[pixel..pixel + 4];
        assert_eq!(
            actual,
            &expected,
            "{spread:?}, reversed={reversed}, t={t}, {} stops",
            stops.len()
        );
    }
}

#[test]
fn duplicate_endpoints_follow_impeller_texture_tiling_in_both_paths() {
    let (device, queue) = valo_harness::headless_device().expect("GPU required");
    let mut context = Context::new(device, queue);
    let first = Color::rgb(1.0, 0.0, 0.0);
    let inside = Color::rgb(0.0, 0.0, 1.0);
    let last = Color::rgba(0.0, 0.0, 0.0, 0.25);
    let red = [255, 0, 0, 255];
    let blue = [0, 0, 255, 255];
    let gray = [0, 0, 0, 64];
    let cases: &[(SpreadMode, &[ColorSample])] = &[
        (
            SpreadMode::Pad,
            &[
                (-2.0, red),
                (-0.5, red),
                (0.0, red),
                (0.25, blue),
                (0.5, blue),
                (0.75, blue),
                (1.0, gray),
                (1.5, gray),
                (3.0, gray),
            ],
        ),
        (
            SpreadMode::Repeat,
            &[
                (-2.0, red),
                (-1.5, blue),
                (-1.0, red),
                (-0.5, blue),
                (0.0, red),
                (0.5, blue),
                (1.0, red),
                (1.5, blue),
                (2.0, red),
                (2.5, blue),
                (3.0, red),
            ],
        ),
        (
            SpreadMode::Reflect,
            &[
                (-2.0, red),
                (-1.5, blue),
                (-1.0, gray),
                (-0.5, blue),
                (0.0, red),
                (0.5, blue),
                (1.0, gray),
                (1.5, blue),
                (2.0, red),
                (2.5, blue),
                (3.0, gray),
            ],
        ),
    ];
    for subdivisions in [1, 8] {
        let mut stops = vec![GradientStop {
            offset: 0.0,
            color: first,
        }];
        stops.extend((0..=subdivisions).map(|index| GradientStop {
            offset: index as f32 / subdivisions as f32,
            color: inside,
        }));
        stops.push(GradientStop {
            offset: 1.0,
            color: last,
        });
        for &(spread, samples) in cases {
            for reversed in [false, true] {
                assert_tiled_colors(&mut context, stops.clone(), spread, reversed, samples);
            }
        }
    }
}

#[test]
fn all_stops_at_one_endpoint_preserve_both_outer_colors() {
    let (device, queue) = valo_harness::headless_device().expect("GPU required");
    let mut context = Context::new(device, queue);
    let red = [255, 0, 0, 255];
    let blue = [0, 0, 255, 255];
    for offset in [0.0, 1.0] {
        for count in [2, 9] {
            let mut stops = vec![
                GradientStop {
                    offset,
                    color: Color::rgb(1.0, 0.0, 0.0)
                };
                count
            ];
            stops.last_mut().unwrap().color = Color::rgb(0.0, 0.0, 1.0);
            for (spread, samples) in [
                (
                    SpreadMode::Pad,
                    vec![(-1.0, red), (0.0, red), (1.0, blue), (2.0, blue)],
                ),
                (
                    SpreadMode::Repeat,
                    vec![(-1.0, red), (0.0, red), (1.0, red), (2.0, red)],
                ),
                (
                    SpreadMode::Reflect,
                    vec![
                        (-2.0, red),
                        (-1.0, blue),
                        (0.0, red),
                        (1.0, blue),
                        (2.0, red),
                    ],
                ),
            ] {
                assert_tiled_colors(&mut context, stops.clone(), spread, false, &samples);
            }
        }
    }
}
