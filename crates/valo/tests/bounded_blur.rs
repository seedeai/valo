//! A bounded blur reads only inside its bounds: colour beyond them does not
//! bleed in at the edges, the result stays opaque up to them, and what the
//! layer draws is not clipped to them. Each check runs the same scene with an
//! unbounded blur too, to show the scene would catch the bleed.

use std::path::Path;

use valo::{
    Backdrop, ClipOp, Color, Context, DisplayList, DisplayListBuilder, ImageFilter, Matrix, Paint,
    Point, Rect,
};

const SIDE: u32 = 128;
const RED: Color = Color::rgb(1.0, 0.0, 0.0);
const BLUE: Color = Color::rgb(0.0, 0.0, 1.0);
const GREEN: Color = Color::rgb(0.0, 1.0, 0.0);

/// The panel the blur is bounded to, in the scene's local units.
const PANEL: Rect = Rect::new(40.0, 40.0, 48.0, 48.0);

fn goldens_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens"))
}

/// `TwoColours` is the no-bleed scene: red everywhere and blue over
/// [`PANEL`] grown by `blue_overhang`, all under `transform`, blurred with or
/// without [`PANEL`] as the bounds.
struct TwoColours {
    transform: Matrix,
    blue_overhang: f32,
}

impl TwoColours {
    fn at(transform: Matrix) -> Self {
        Self {
            transform,
            blue_overhang: 0.0,
        }
    }

    /// The colours, then a backdrop layer clipped to the panel that blurs
    /// them with `filter`.
    fn under_backdrop(&self, filter: ImageFilter) -> DisplayList {
        let mut builder = DisplayListBuilder::new();
        builder.concat(&self.transform);
        self.draw_colours(&mut builder);
        builder.clip_rect(PANEL, ClipOp::Intersect);
        builder.save_layer_backdrop(None, &Paint::default(), Backdrop::new(filter));
        builder.restore();
        builder.build()
    }

    /// The colours as the children of a layer that blurs them with `filter`.
    fn in_layer(&self, filter: ImageFilter) -> DisplayList {
        let mut builder = DisplayListBuilder::new();
        builder.concat(&self.transform);
        builder.save_layer(
            None,
            &Paint {
                image_filter: Some(filter),
                ..Paint::default()
            },
        );
        self.draw_colours(&mut builder);
        builder.restore();
        builder.build()
    }

    fn draw_colours(&self, builder: &mut DisplayListBuilder) {
        builder.draw_rect(
            Rect::new(-128.0, -128.0, 384.0, 384.0),
            &Paint::from_color(RED),
        );
        builder.draw_rect(PANEL.expand(self.blue_overhang), &Paint::from_color(BLUE));
    }

    /// The largest difference from pure blue over the pixels whose centres
    /// lie inside the panel.
    fn worst_blue_inside(&self, pixels: &[u8]) -> u8 {
        let device_to_local = self.transform.invert().expect("an invertible transform");
        let mut worst = 0;
        for y in 0..SIDE {
            for x in 0..SIDE {
                let centre = Point::new(x as f32 + 0.5, y as f32 + 0.5);
                if !PANEL.contains(device_to_local.map_point(centre)) {
                    continue;
                }
                let at = ((y * SIDE + x) * 4) as usize;
                let expected = [0, 0, 255, 255];
                for channel in 0..4 {
                    worst = worst.max(pixels[at + channel].abs_diff(expected[channel]));
                }
            }
        }
        worst
    }
}

fn render(context: &mut Context, list: &DisplayList) -> Vec<u8> {
    context.render_to_rgba(list, [SIDE, SIDE], Some(Color::TRANSPARENT))
}

/// The pixels nearest the panel's edge are where an unbounded blur mixes in
/// the red; the bounded one has to stay blue there. The bounded downsample
/// runs at full resolution for σ 3 and at a quarter for σ 12.
///
/// The bounds are tested at each tap's centre, as Impeller's bounded
/// downsample tests them. At full resolution its taps sit on texel centres,
/// so nothing leaks. Once it shrinks the region its taps sit between two
/// texels, and one that lands exactly on an edge reads half a texel from
/// beyond; the division by alpha doubles that next to the edge. At σ 12 it
/// measures 34 in the layer test and the translated backdrop test, whose
/// edges fall on taps, and 0 elsewhere: Impeller's own rule, which the
/// downsampled limit allows for.
fn assert_no_bleed(
    context: &mut Context,
    scene: &TwoColours,
    list_of: fn(&TwoColours, ImageFilter) -> DisplayList,
) {
    for sigma in [3.0, 12.0] {
        let bounded = render(
            context,
            &list_of(scene, ImageFilter::bounded_blur(sigma, sigma, PANEL)),
        );
        let unbounded = render(context, &list_of(scene, ImageFilter::blur(sigma, sigma)));
        let transform = scene.transform;
        let bounded_worst = scene.worst_blue_inside(&bounded);
        let unbounded_worst = scene.worst_blue_inside(&unbounded);
        let allowed = if sigma > 4.0 { 37 } else { 3 };
        assert!(
            bounded_worst <= allowed,
            "σ {sigma} under {transform:?}: red bled {bounded_worst} into the bounded blur"
        );
        assert!(
            unbounded_worst > 60,
            "σ {sigma} under {transform:?}: the unbounded blur should bleed, got {unbounded_worst}"
        );
    }
}

#[test]
fn a_bounded_backdrop_blur_does_not_bleed_in_colour_from_beyond_its_bounds() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_bounded_backdrop_blur_does_not_bleed_in_colour_from_beyond_its_bounds");
        return;
    };
    let mut context = Context::new(device, queue);
    let scene = TwoColours::at(Matrix::IDENTITY);
    assert_no_bleed(&mut context, &scene, TwoColours::under_backdrop);
}

/// The bounds are local: a translation has to move them with the panel
/// (Impeller's upstream fix "Fixes translated bounded blurs").
#[test]
fn a_translated_bounded_backdrop_blur_reads_its_bounds_where_they_land() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_translated_bounded_backdrop_blur_reads_its_bounds_where_they_land");
        return;
    };
    let mut context = Context::new(device, queue);
    let scene = TwoColours::at(Matrix::translation(17.0, -9.0));
    assert_no_bleed(&mut context, &scene, TwoColours::under_backdrop);
}

/// Under a rotation the bounds are a quad on the device, and the blur's σ
/// becomes uneven. A rotated panel's own edge is antialiased, half red and
/// inside the bounds, so the blue reaches a little past them here: what is
/// checked is that the red beyond is not read.
#[test]
fn a_rotated_bounded_backdrop_blur_reads_its_bounds_as_a_quad() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_rotated_bounded_backdrop_blur_reads_its_bounds_as_a_quad");
        return;
    };
    let mut context = Context::new(device, queue);
    let scene = TwoColours {
        transform: Matrix::translation(64.0, 64.0)
            .then(&Matrix::rotation(25f32.to_radians()))
            .then(&Matrix::scale(1.25, 1.25))
            .then(&Matrix::translation(-64.0, -64.0)),
        blue_overhang: 1.5,
    };
    assert_no_bleed(&mut context, &scene, TwoColours::under_backdrop);
}

/// A layer's own bounded blur places its bounds with the save point's
/// transform, translation included. σ 12's downsampling taps land exactly
/// on the panel's edges here.
#[test]
fn a_bounded_blur_on_a_layer_reads_its_bounds_where_they_land() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_bounded_blur_on_a_layer_reads_its_bounds_where_they_land");
        return;
    };
    let mut context = Context::new(device, queue);
    let scene = TwoColours::at(Matrix::translation(11.0, 23.0));
    assert_no_bleed(&mut context, &scene, TwoColours::in_layer);
}

/// The bounds limit what the blur reads, not what the layer shows: a child
/// drawn beyond them still draws.
#[test]
fn a_bounded_backdrop_blur_leaves_children_outside_its_bounds_drawn() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_bounded_backdrop_blur_leaves_children_outside_its_bounds_drawn");
        return;
    };
    let mut builder = DisplayListBuilder::new();
    TwoColours::at(Matrix::IDENTITY).draw_colours(&mut builder);
    builder.save_layer_backdrop(
        None,
        &Paint::default(),
        Backdrop::new(ImageFilter::bounded_blur(4.0, 4.0, PANEL)),
    );
    builder.draw_rect(Rect::new(4.0, 4.0, 16.0, 16.0), &Paint::from_color(GREEN));
    builder.restore();
    let mut context = Context::new(device, queue);
    let pixels = render(&mut context, &builder.build());
    let at = ((12 * SIDE + 12) * 4) as usize;
    assert_eq!(&pixels[at..at + 4], &[0, 255, 0, 255]);
}

/// Every blur in a composition runs its own three passes (downsample,
/// vertical, horizontal), as Impeller's composed filters do, so a bounded
/// blur is never merged with the blur beside it: masking the input and
/// dividing by alpha would not survive a merge.
#[test]
fn a_bounded_blur_is_not_merged_with_a_neighbouring_blur() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_bounded_blur_is_not_merged_with_a_neighbouring_blur");
        return;
    };
    let filter_passes = |context: &mut Context, inner: ImageFilter| {
        let mut builder = DisplayListBuilder::new();
        builder.draw_rect(
            PANEL,
            &Paint {
                color: BLUE,
                image_filter: Some(ImageFilter::compose(ImageFilter::blur(2.0, 2.0), inner)),
                ..Paint::default()
            },
        );
        let target = valo::Offscreen::new(context.device(), [SIDE, SIDE]);
        context
            .render(&builder.build(), &target.target(Some(Color::TRANSPARENT)))
            .filter_passes
    };
    let mut context = Context::new(device, queue);
    let unbounded = filter_passes(&mut context, ImageFilter::blur(2.0, 2.0));
    let bounded = filter_passes(&mut context, ImageFilter::bounded_blur(2.0, 2.0, PANEL));
    assert_eq!(unbounded, 6);
    assert_eq!(bounded, 6);
}

/// Tiles under one backdrop key share the first tile's filtered snapshot
/// when it is what each would make. A bounded blur's bounds move with each
/// tile's translation, so tiles at different places blur on their own and
/// each stays inside its own bounds.
#[test]
fn keyed_bounded_backdrops_at_different_places_read_their_own_bounds() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP keyed_bounded_backdrops_at_different_places_read_their_own_bounds");
        return;
    };
    let tile = Rect::new(0.0, 0.0, 24.0, 24.0);
    let origins = [Point::new(16.0, 16.0), Point::new(80.0, 72.0)];
    let scene = |filter: &ImageFilter| {
        let mut builder = DisplayListBuilder::new();
        builder.draw_rect(Rect::new(0.0, 0.0, 128.0, 128.0), &Paint::from_color(RED));
        for origin in origins {
            builder.save();
            builder.translate(origin.x, origin.y);
            builder.draw_rect(tile, &Paint::from_color(BLUE));
            builder.clip_rect(tile, ClipOp::Intersect);
            builder.save_layer_backdrop(
                None,
                &Paint::default(),
                Backdrop::new(filter.clone()).shared(7),
            );
            builder.restore();
            builder.restore();
        }
        builder.build()
    };
    let mut context = Context::new(device, queue);
    let target = valo::Offscreen::new(context.device(), [SIDE, SIDE]);

    let unbounded = context.render(
        &scene(&ImageFilter::blur(3.0, 3.0)),
        &target.target(Some(Color::TRANSPARENT)),
    );
    assert_eq!(
        unbounded.shared_backdrops, 1,
        "unbounded tiles share one snapshot"
    );

    let bounded_list = scene(&ImageFilter::bounded_blur(3.0, 3.0, tile));
    let bounded = context.render(&bounded_list, &target.target(Some(Color::TRANSPARENT)));
    assert_eq!(
        bounded.shared_backdrops, 0,
        "bounded tiles at different places do not"
    );
    let pixels = render(&mut context, &bounded_list);
    for origin in origins {
        for (x, y) in [
            (origin.x + 0.5, origin.y + 0.5),
            (origin.x + 23.5, origin.y + 23.5),
        ] {
            let at = ((y as u32 * SIDE + x as u32) * 4) as usize;
            assert_eq!(&pixels[at..at + 4], &[0, 0, 255, 255], "at ({x}, {y})");
        }
    }
}

/// A frosted panel over stripes, bounded and unbounded side by side, the
/// bounded one also rotated: the bounded panels keep their own colours to the
/// edge, where the unbounded one fades into its surroundings.
#[test]
fn bounded_backdrop_blur_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP bounded_backdrop_blur_golden: no GPU adapter");
        return;
    };
    let size = [240u32, 120u32];
    let mut builder = DisplayListBuilder::new();
    for stripe in 0..12 {
        let colour = if stripe % 2 == 0 {
            Color::rgb(0.95, 0.55, 0.1)
        } else {
            Color::rgb(0.1, 0.3, 0.8)
        };
        builder.draw_rect(
            Rect::new(stripe as f32 * 20.0, 0.0, 20.0, 120.0),
            &Paint::from_color(colour),
        );
    }
    let panel = Rect::new(0.0, 0.0, 56.0, 64.0);
    let frosting = Paint::from_color(Color::rgba(1.0, 1.0, 1.0, 0.2));
    for (origin, rotation, filter) in [
        (Point::new(14.0, 28.0), 0.0, ImageFilter::blur(8.0, 8.0)),
        (
            Point::new(92.0, 28.0),
            0.0,
            ImageFilter::bounded_blur(8.0, 8.0, panel),
        ),
        (
            Point::new(184.0, 18.0),
            12f32.to_radians(),
            ImageFilter::bounded_blur(8.0, 8.0, panel),
        ),
    ] {
        builder.save();
        builder.translate(origin.x, origin.y);
        builder.rotate(rotation);
        builder.clip_rect(panel, ClipOp::Intersect);
        builder.save_layer_backdrop(None, &Paint::default(), Backdrop::new(filter));
        builder.draw_rect(panel, &frosting);
        builder.restore();
        builder.restore();
    }
    let mut context = Context::new(device, queue);
    let pixels = context.render_to_rgba(&builder.build(), size, Some(Color::TRANSPARENT));
    valo_harness::assert_golden(goldens_dir(), "bounded_backdrop_blur", size, &pixels);
}
