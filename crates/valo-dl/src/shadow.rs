//! A shadow resolved into the one draw Impeller makes of it
//! (`DlDispatcherBase::drawShadow`): the occluder's path, moved down by the
//! light, filled with a tonal spot colour under a mask blur whose radius is
//! the occluder's height over the vertical scale of the transform it is
//! drawn under.
//!
//! The renderer resolves a recorded shadow under the transform in force at
//! playback, which is what the blur's radius needs; the recorder resolves it
//! under the transform it knows, for the bounds it records. Both go through
//! [`SpotShadow::resolve`], so the two never disagree on what a shadow is.

use valo_geometry::{Color, Matrix};

use crate::{MaskBlur, Paint};

/// `LIGHT_POSITION_Y` is the light's direction's vertical component, Impeller's
/// `light_position` of `(0, -1, 1)`: the light is above the occluder, so the
/// shadow falls down.
const LIGHT_POSITION_Y: f32 = -1.0;

/// `LIGHT_RADIUS` is Impeller's `kLightRadius`, the light's radius over its
/// height. Impeller writes it `800 / 600`, an integer division: one.
const LIGHT_RADIUS: f32 = (800 / 600) as f32;

/// `KERNEL_RADIUS_PER_SIGMA` is Impeller's `kKernelRadiusPerSigma`, √3: the
/// blur radius one standard deviation of the Gaussian spans.
const KERNEL_RADIUS_PER_SIGMA: f32 = 1.732_050_8;

/// `SpotShadow` is the draw a shadow resolves to under one transform: its
/// path, moved by `offset`, filled with `paint`.
///
/// Flutter's `drawShadow` names an occluder by its elevation and the view's
/// device pixel ratio, and Impeller turns that into this draw only when it
/// plays the shadow back, because the blur's radius divides by the vertical
/// scale of the whole transform the shadow is drawn under.
#[derive(Clone, Debug, PartialEq)]
pub struct SpotShadow {
    /// `offset` is appended to the current transform before the path is
    /// filled: a move down by the occluder's height, in the path's own
    /// coordinates, as Impeller pre-concatenates it.
    pub offset: Matrix,
    /// `paint` fills the moved path: the tonal spot colour, with a normal
    /// mask blur in the path's coordinates.
    pub paint: Paint,
}

impl SpotShadow {
    /// `resolve` is the shadow of an occluder painted `color` at
    /// `elevation`, in a view of `device_pixel_ratio`, drawn under
    /// `transform`: Impeller's `DlDispatcherBase::drawShadow`.
    ///
    /// The occluder stands `device_pixel_ratio × elevation` above the canvas.
    /// The blur's radius is that height over the length of `transform`'s
    /// y axis, Impeller's `GetCurrentTransform().GetScale().y`, as a sigma.
    /// `None` when `transform` flattens the y axis, under which the shadow
    /// shows nowhere.
    pub fn resolve(
        color: Color,
        elevation: f32,
        device_pixel_ratio: f32,
        transform: &Matrix,
    ) -> Option<Self> {
        let vertical_scale = vertical_scale(transform);
        if !(vertical_scale > 0.0 && vertical_scale.is_finite()) {
            return None;
        }
        let occluder_z = device_pixel_ratio * elevation;
        let radius = LIGHT_RADIUS * occluder_z / vertical_scale;
        let paint = Paint {
            mask_blur: Some(MaskBlur::new(radius_to_sigma(radius))),
            ..Paint::from_color(tonal_spot_color(color))
        };
        Some(Self {
            offset: Matrix::translation(0.0, -occluder_z * LIGHT_POSITION_Y),
            paint,
        })
    }
}

/// `vertical_scale` is the length of `transform`'s y basis vector, z
/// included: Impeller's `Matrix::GetScale().y`.
fn vertical_scale(transform: &Matrix) -> f32 {
    transform.to_mat4().y_axis.truncate().length()
}

/// `radius_to_sigma` is Impeller's `Radius` to `Sigma` conversion
/// (`impeller/geometry/sigma.cc`), which its `MaskBlurDescriptor` applies to
/// the shadow's radius.
fn radius_to_sigma(radius: f32) -> f32 {
    if radius > 0.0 {
        radius / KERNEL_RADIUS_PER_SIGMA + 0.5
    } else {
        0.0
    }
}

/// `tonal_spot_color` is the spot colour of a shadow of `color`: a quarter of
/// its alpha, then Skia's `SkShadowUtils::ComputeTonalColors`, as Impeller's
/// `drawShadow` ports it.
fn tonal_spot_color(color: Color) -> Color {
    let alpha = color.a * 0.25;
    let max = color.r.max(color.g).max(color.b);
    let min = color.r.min(color.g).min(color.b);
    let luminance = (min + max) * 0.5;

    let alpha_adjust = (2.6 + (-2.66667 + 1.06667 * alpha) * alpha) * alpha;
    let color_alpha = (3.544762 + (-4.891428 + 2.3466 * luminance) * luminance) * luminance;
    let color_alpha = (alpha_adjust * color_alpha).clamp(0.0, 1.0);

    let greyscale_alpha = (alpha * (1.0 - 0.4 * luminance)).clamp(0.0, 1.0);

    let color_scale = color_alpha * (1.0 - greyscale_alpha);
    let tonal_alpha = color_scale + greyscale_alpha;
    let unpremultiplied_scale = if tonal_alpha != 0.0 {
        color_scale / tonal_alpha
    } else {
        0.0
    };
    Color {
        r: unpremultiplied_scale * color.r,
        g: unpremultiplied_scale * color.g,
        b: unpremultiplied_scale * color.b,
        a: tonal_alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sigma(shadow: &SpotShadow) -> f32 {
        shadow.paint.mask_blur.expect("a shadow is blurred").sigma
    }

    #[test]
    fn a_shadow_falls_by_its_height_and_blurs_by_it_over_the_scale() {
        let shadow = SpotShadow::resolve(Color::BLACK, 4.0, 2.0, &Matrix::scale(2.0, 2.0))
            .expect("the transform keeps its y axis");
        assert_eq!(shadow.offset, Matrix::translation(0.0, 8.0));
        // A radius of 8 / 2 = 4, as a sigma: 4 / √3 + 0.5.
        assert!((sigma(&shadow) - 2.809_401).abs() < 1e-5, "{shadow:?}");
    }

    /// What the record-time version got wrong: the radius divides by the
    /// whole transform's scale, so a widget scaled by two blurs half as far
    /// in its own coordinates, the same in pixels.
    #[test]
    fn a_scaled_transform_narrows_the_blur_in_local_coordinates() {
        let at_ratio = SpotShadow::resolve(Color::BLACK, 4.0, 2.0, &Matrix::scale(2.0, 2.0));
        let scaled = SpotShadow::resolve(Color::BLACK, 4.0, 2.0, &Matrix::scale(4.0, 4.0));
        let radius = |shadow: Option<SpotShadow>| (sigma(&shadow.unwrap()) - 0.5) * 1.732_050_8;
        assert!((radius(at_ratio) - 4.0).abs() < 1e-4);
        assert!((radius(scaled) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn the_scale_is_the_y_axis_length_whatever_the_rotation() {
        let turned = Matrix::rotation(0.7).then(&Matrix::scale(1.0, 3.0));
        let shadow = SpotShadow::resolve(Color::BLACK, 3.0, 1.0, &turned).unwrap();
        assert!(((sigma(&shadow) - 0.5) * 1.732_050_8 - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_flattened_transform_shows_no_shadow() {
        assert!(SpotShadow::resolve(Color::BLACK, 4.0, 1.0, &Matrix::scale(1.0, 0.0)).is_none());
    }

    /// Black at full alpha: luminance 0, so only the greyscale alpha, a
    /// quarter, remains.
    #[test]
    fn a_black_occluder_casts_a_quarter_alpha_black_spot() {
        let spot = tonal_spot_color(Color::BLACK);
        assert_eq!((spot.r, spot.g, spot.b), (0.0, 0.0, 0.0));
        assert!((spot.a - 0.25).abs() < 1e-6, "{spot:?}");
    }

    /// A coloured occluder keeps some of its colour: Skia's tonal colour for
    /// opaque red, worked by hand from `ComputeTonalColors`.
    #[test]
    fn a_coloured_occluder_casts_a_tinted_spot() {
        let spot = tonal_spot_color(Color::rgb(1.0, 0.0, 0.0));
        // A quarter alpha and a luminance of a half: a greyscale alpha of 0.2.
        let alpha_adjust = (2.6 + (-2.66667 + 1.06667 * 0.25) * 0.25) * 0.25;
        let color_alpha = alpha_adjust * (3.544762 + (-4.891428 + 2.3466 * 0.5) * 0.5) * 0.5;
        let color_scale = color_alpha * 0.8;
        let tonal_alpha = color_scale + 0.2;
        assert!((spot.a - tonal_alpha).abs() < 1e-6, "{spot:?}");
        assert!(
            (spot.r - color_scale / tonal_alpha).abs() < 1e-6,
            "{spot:?}"
        );
        assert_eq!((spot.g, spot.b), (0.0, 0.0));
    }
}
