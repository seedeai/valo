use valo_geometry::Color;

use crate::paint::{BlendMode, ColorFilter};

/// `apply` evaluates a color filter against one straight-alpha color.
pub(crate) fn apply(filter: ColorFilter, destination: Color) -> Color {
    match filter {
        ColorFilter::Matrix(matrix) => apply_matrix(&matrix, destination),
        // ColorFilter.mode treats the constant as source and content as dst.
        ColorFilter::Blend(source, mode) => blend(destination, source, mode),
    }
}

fn apply_matrix(matrix: &[f32; 20], color: Color) -> Color {
    let input = [color.r, color.g, color.b, color.a, 1.0];
    let channel = |row: usize| {
        let start = row * 5;
        (0..5)
            .map(|column| matrix[start + column] * input[column])
            .sum::<f32>()
            .clamp(0.0, 1.0)
    };
    Color::rgba(channel(0), channel(1), channel(2), channel(3))
}

/// `blend` evaluates one blend mode using premultiplied Porter-Duff arithmetic.
fn blend(destination: Color, source: Color, mode: BlendMode) -> Color {
    let destination_premultiplied = destination.premultiplied();
    let source_premultiplied = source.premultiplied();
    let porter_duff = |source_factor: f32, destination_factor: f32| {
        unpremultiply(add(
            scale(source_premultiplied, source_factor),
            scale(destination_premultiplied, destination_factor),
        ))
    };
    match mode {
        BlendMode::Clear => Color::TRANSPARENT,
        BlendMode::Src => source,
        BlendMode::Dst => destination,
        BlendMode::SrcOver => porter_duff(1.0, 1.0 - source.a),
        BlendMode::DstOver => porter_duff(1.0 - destination.a, 1.0),
        BlendMode::SrcIn => porter_duff(destination.a, 0.0),
        BlendMode::DstIn => porter_duff(0.0, source.a),
        BlendMode::SrcOut => porter_duff(1.0 - destination.a, 0.0),
        BlendMode::DstOut => porter_duff(0.0, 1.0 - source.a),
        BlendMode::SrcAtop => porter_duff(destination.a, 1.0 - source.a),
        BlendMode::DstAtop => porter_duff(1.0 - destination.a, source.a),
        BlendMode::Xor => porter_duff(1.0 - destination.a, 1.0 - source.a),
        BlendMode::Plus => unpremultiply([
            (source_premultiplied[0] + destination_premultiplied[0]).min(1.0),
            (source_premultiplied[1] + destination_premultiplied[1]).min(1.0),
            (source_premultiplied[2] + destination_premultiplied[2]).min(1.0),
            (source_premultiplied[3] + destination_premultiplied[3]).min(1.0),
        ]),
        BlendMode::Modulate => {
            unpremultiply(multiply(source_premultiplied, destination_premultiplied))
        }
        mode => blend_advanced(destination, source, mode),
    }
}

fn blend_advanced(destination: Color, source: Color, mode: BlendMode) -> Color {
    const BLEND_EPSILON: f32 = 1e-3;
    let destination_rgb = [destination.r, destination.g, destination.b];
    let source_rgb = [source.r, source.g, source.b];
    let result = match mode {
        BlendMode::Screen => zip(destination_rgb, source_rgb, |d, s| s + d - s * d),
        BlendMode::Overlay => zip(destination_rgb, source_rgb, |d, s| {
            if d > 0.5 {
                2.0 * d - 1.0 + s - (2.0 * d - 1.0) * s
            } else {
                2.0 * s * d
            }
        }),
        BlendMode::Darken => zip(destination_rgb, source_rgb, f32::min),
        BlendMode::Lighten => zip(destination_rgb, source_rgb, f32::max),
        BlendMode::ColorDodge => zip(destination_rgb, source_rgb, |d, s| {
            if d < BLEND_EPSILON {
                0.0
            } else if (1.0 - s).abs() < BLEND_EPSILON {
                1.0
            } else {
                (d / (1.0 - s)).min(1.0)
            }
        }),
        BlendMode::ColorBurn => zip(destination_rgb, source_rgb, |d, s| {
            if (1.0 - d).abs() < BLEND_EPSILON {
                1.0
            } else if s < BLEND_EPSILON {
                0.0
            } else {
                1.0 - ((1.0 - d) / s).min(1.0)
            }
        }),
        BlendMode::HardLight => zip(destination_rgb, source_rgb, |d, s| {
            if s > 0.5 {
                2.0 * s - 1.0 + d - (2.0 * s - 1.0) * d
            } else {
                2.0 * d * s
            }
        }),
        BlendMode::SoftLight => zip(destination_rgb, source_rgb, |d, s| {
            let curve = if d > 0.25 {
                d.sqrt()
            } else {
                ((16.0 * d - 12.0) * d + 4.0) * d
            };
            if s > 0.5 {
                d + (2.0 * s - 1.0) * (curve - d)
            } else {
                d - (1.0 - 2.0 * s) * d * (1.0 - d)
            }
        }),
        BlendMode::Difference => zip(destination_rgb, source_rgb, |d, s| (d - s).abs()),
        BlendMode::Exclusion => zip(destination_rgb, source_rgb, |d, s| d + s - 2.0 * d * s),
        BlendMode::Multiply => zip(destination_rgb, source_rgb, |d, s| d * s),
        BlendMode::Hue => set_luminosity(
            set_saturation(source_rgb, saturation(destination_rgb)),
            luminosity(destination_rgb),
        ),
        BlendMode::Saturation => set_luminosity(
            set_saturation(destination_rgb, saturation(source_rgb)),
            luminosity(destination_rgb),
        ),
        BlendMode::Color => set_luminosity(source_rgb, luminosity(destination_rgb)),
        BlendMode::Luminosity => set_luminosity(destination_rgb, luminosity(source_rgb)),
        _ => unreachable!("Porter-Duff mode routed to advanced blending"),
    };
    apply_blended_color(destination, source, result)
}

fn apply_blended_color(destination: Color, source: Color, blended: [f32; 3]) -> Color {
    let overlap_alpha = source.a * destination.a;
    let mut blended_source = [
        blended[0] * overlap_alpha + source.r * source.a * (1.0 - destination.a),
        blended[1] * overlap_alpha + source.g * source.a * (1.0 - destination.a),
        blended[2] * overlap_alpha + source.b * source.a * (1.0 - destination.a),
        overlap_alpha + source.a * (1.0 - destination.a),
    ];
    let destination = destination.premultiplied();
    for channel in 0..4 {
        blended_source[channel] += destination[channel] * (1.0 - blended_source[3]);
    }
    unpremultiply(blended_source)
}

fn zip(a: [f32; 3], b: [f32; 3], function: impl Fn(f32, f32) -> f32) -> [f32; 3] {
    [
        function(a[0], b[0]),
        function(a[1], b[1]),
        function(a[2], b[2]),
    ]
}

fn luminosity(color: [f32; 3]) -> f32 {
    color[0] * 0.3 + color[1] * 0.59 + color[2] * 0.11
}

fn saturation(color: [f32; 3]) -> f32 {
    color[0].max(color[1]).max(color[2]) - color[0].min(color[1]).min(color[2])
}

fn set_saturation(color: [f32; 3], value: f32) -> [f32; 3] {
    let minimum = color[0].min(color[1]).min(color[2]);
    let maximum = color[0].max(color[1]).max(color[2]);
    if minimum < maximum {
        color.map(|channel| (channel - minimum) * value / (maximum - minimum))
    } else {
        [0.0; 3]
    }
}

fn set_luminosity(color: [f32; 3], value: f32) -> [f32; 3] {
    clip_color(color.map(|channel| channel + value - luminosity(color)))
}

fn clip_color(mut color: [f32; 3]) -> [f32; 3] {
    let luminosity = luminosity(color);
    let minimum = color[0].min(color[1]).min(color[2]);
    let maximum = color[0].max(color[1]).max(color[2]);
    if minimum < 0.0 {
        color = color.map(|channel| {
            luminosity + (channel - luminosity) * luminosity / (luminosity - minimum + 1e-3)
        });
    }
    if maximum > 1.0 {
        color = color.map(|channel| {
            luminosity + (channel - luminosity) * (1.0 - luminosity) / (maximum - luminosity + 1e-3)
        });
    }
    color
}

fn scale(color: [f32; 4], factor: f32) -> [f32; 4] {
    color.map(|channel| channel * factor)
}

fn add(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

fn multiply(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2], a[3] * b[3]]
}

fn unpremultiply(color: [f32; 4]) -> Color {
    if color[3] <= 0.0 {
        return Color::TRANSPARENT;
    }
    Color::rgba(
        color[0] / color[3],
        color[1] / color[3],
        color[2] / color[3],
        color[3],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_MODES: [BlendMode; 29] = [
        BlendMode::Clear,
        BlendMode::Src,
        BlendMode::Dst,
        BlendMode::SrcOver,
        BlendMode::DstOver,
        BlendMode::SrcIn,
        BlendMode::DstIn,
        BlendMode::SrcOut,
        BlendMode::DstOut,
        BlendMode::SrcAtop,
        BlendMode::DstAtop,
        BlendMode::Xor,
        BlendMode::Plus,
        BlendMode::Modulate,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::HardLight,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Multiply,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];

    #[test]
    fn every_blend_filter_produces_a_finite_straight_color() {
        let destination = Color::rgba(0.17, 0.73, 0.41, 0.37);
        let source = Color::rgba(0.91, 0.12, 0.64, 0.58);
        for mode in ALL_MODES {
            let color = apply(ColorFilter::Blend(source, mode), destination);
            for channel in [color.r, color.g, color.b, color.a] {
                assert!(channel.is_finite(), "{mode:?} produced {color:?}");
                assert!((-1e-5..=1.00001).contains(&channel), "{mode:?}: {color:?}");
            }
        }
    }

    #[test]
    fn transparent_black_flood_matches_the_filter_result() {
        let visible = Color::rgba(0.2, 0.4, 0.8, 0.7);
        assert!(ColorFilter::Blend(visible, BlendMode::SrcOver).modifies_transparent_black());
        assert!(!ColorFilter::Blend(visible, BlendMode::Dst).modifies_transparent_black());
        let mut matrix = [0.0; 20];
        matrix[18] = 1.0;
        matrix[19] = 0.25;
        assert!(ColorFilter::Matrix(matrix).modifies_transparent_black());
    }
}
