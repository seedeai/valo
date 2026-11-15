//! The matrix image filter: dart:ui's `ImageFilter.matrix`, Flutter's `DlMatrixImageFilter`, drawn
//! as Impeller's `MatrixFilterContents` draws it. The content is drawn first and its pixels moved,
//! the matrix taken through the transform where the filter applies, and read as its sampling says.
//! Skips (with a note) when the machine has no adapter; `VALO_BLESS=1 cargo test` regenerates the
//! golden.

use std::f32::consts::FRAC_PI_2;

use valo::{
    Backdrop, Color, ColorFilter, Context, DisplayListBuilder, Filter, ImageFilter, Matrix,
    MipmapMode, Offscreen, Paint, Rect, Sampling, TileMode,
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
/// What the scenes are cleared to.
const GROUND: Color = Color {
    r: 0.0,
    g: 1.0,
    b: 0.0,
    a: 1.0,
};
const RED_PIXEL: [u8; 4] = [255, 0, 0, 255];
const BLUE_PIXEL: [u8; 4] = [0, 0, 255, 255];
const GROUND_PIXEL: [u8; 4] = [0, 255, 0, 255];
const SIZE: [u32; 2] = [64, 64];

fn sampling(filter: Filter) -> Sampling {
    Sampling {
        filter,
        mipmap: MipmapMode::None,
        tile_x: TileMode::Clamp,
        tile_y: TileMode::Clamp,
    }
}

fn moved(matrix: Matrix, filter: Filter) -> ImageFilter {
    ImageFilter::matrix(matrix, sampling(filter))
}

/// `about` is `matrix` applied about `(x, y)`.
fn about(x: f32, y: f32, matrix: Matrix) -> Matrix {
    Matrix::translation(x, y)
        .then(&matrix)
        .then(&Matrix::translation(-x, -y))
}

fn filtered_layer(b: &mut DisplayListBuilder, filter: ImageFilter) {
    b.save_layer(
        None,
        &Paint {
            image_filter: Some(filter),
            ..Paint::default()
        },
    );
}

fn render(context: &mut Context, b: DisplayListBuilder) -> Vec<u8> {
    valo_harness::render_to_rgba(context, b.build(), SIZE, Some(GROUND))
}

fn pixel(pixels: &[u8], x: usize, y: usize) -> [u8; 4] {
    let at = (y * SIZE[0] as usize + x) * 4;
    pixels[at..at + 4].try_into().expect("four channels")
}

/// A layer's pixels go where the matrix moves them, and nothing is left where they were.
#[test]
fn a_layer_shows_where_its_matrix_moves_it() {
    let Some(mut context) = context("a_layer_shows_where_its_matrix_moves_it") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    filtered_layer(
        &mut b,
        moved(Matrix::translation(20.0, 0.0), Filter::Linear),
    );
    b.draw_rect(Rect::new(10.0, 10.0, 10.0, 10.0), &Paint::from_color(RED));
    b.restore();
    let pixels = render(&mut context, b);
    assert_eq!(pixel(&pixels, 35, 15), RED_PIXEL, "moved");
    assert_eq!(
        pixel(&pixels, 15, 15),
        GROUND_PIXEL,
        "not where it was drawn"
    );
}

/// The matrix is in the layer's local coordinates: under a scale of two, a translation of ten
/// moves twenty pixels (Impeller's `effect_transform · matrix · effect_transform⁻¹`).
#[test]
fn a_matrix_is_taken_through_the_transform_where_it_applies() {
    let Some(mut context) = context("a_matrix_is_taken_through_the_transform_where_it_applies")
    else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.scale(2.0, 2.0);
    filtered_layer(
        &mut b,
        moved(Matrix::translation(10.0, 0.0), Filter::Linear),
    );
    b.draw_rect(Rect::new(2.0, 2.0, 5.0, 5.0), &Paint::from_color(RED));
    b.restore();
    let pixels = render(&mut context, b);
    assert_eq!(pixel(&pixels, 30, 8), RED_PIXEL, "moved by twenty pixels");
    assert_eq!(pixel(&pixels, 8, 8), GROUND_PIXEL);
}

/// One-pixel stripes, red on even columns and blue on odd ones, scaled four times: nearest
/// sampling keeps every pixel one stripe's colour, linear sampling mixes neighbours.
#[test]
fn a_scaled_layer_is_read_as_its_sampling_says() {
    let Some(mut context) = context("a_scaled_layer_is_read_as_its_sampling_says") else {
        return;
    };
    let stripes = |filter| {
        let mut b = DisplayListBuilder::new();
        filtered_layer(&mut b, moved(Matrix::scale(4.0, 4.0), filter));
        for column in 0..16 {
            let colour = if column % 2 == 0 { RED } else { BLUE };
            b.draw_rect(
                Rect::new(column as f32, 0.0, 1.0, 16.0),
                &Paint::from_color(colour),
            );
        }
        b.restore();
        b
    };
    let nearest = render(&mut context, stripes(Filter::Nearest));
    // Pixel 6's centre, 6.5, reads the stripes at 1.625: the second stripe's texel, a quarter of
    // a texel from its edge with the third. Each stripe is four pixels wide.
    assert_eq!(pixel(&nearest, 6, 20), BLUE_PIXEL);
    assert_eq!(pixel(&nearest, 8, 20), RED_PIXEL);
    assert_eq!(pixel(&nearest, 11, 20), RED_PIXEL);
    assert_eq!(pixel(&nearest, 12, 20), BLUE_PIXEL);
    let linear = render(&mut context, stripes(Filter::Linear));
    let mixed = pixel(&linear, 6, 20);
    assert!(
        mixed[0] > 16 && mixed[2] > 16,
        "linear sampling mixes the stripes: {mixed:?}"
    );
}

/// A quarter turn about the bar's centre stands it upright: a matrix that does not keep the
/// layer's pixels on its axes draws them turned.
#[test]
fn a_turned_layer_draws_its_pixels_turned() {
    let Some(mut context) = context("a_turned_layer_draws_its_pixels_turned") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    let quarter_turn = about(30.0, 30.0, Matrix::rotation(FRAC_PI_2));
    filtered_layer(&mut b, moved(quarter_turn, Filter::Linear));
    b.draw_rect(Rect::new(20.0, 25.0, 20.0, 10.0), &Paint::from_color(RED));
    b.restore();
    let pixels = render(&mut context, b);
    assert_eq!(pixel(&pixels, 30, 22), RED_PIXEL, "the bar stands upright");
    assert_eq!(
        pixel(&pixels, 22, 30),
        GROUND_PIXEL,
        "and no longer lies flat"
    );
}

/// A filter after the matrix reads the moved pixels: an axis-aligned move hands its snapshot on
/// as it is, a turn draws it into a texture of its own first.
#[test]
fn a_composed_filter_reads_the_moved_pixels() {
    let Some(mut context) = context("a_composed_filter_reads_the_moved_pixels") else {
        return;
    };
    let yellow = ColorFilter::Blend(Color::rgb(1.0, 1.0, 0.0), valo::BlendMode::SrcIn);
    let quarter_turn = about(30.0, 30.0, Matrix::rotation(FRAC_PI_2));
    for (inner, upright, flat) in [
        (
            moved(Matrix::translation(20.0, 0.0), Filter::Linear),
            (45, 30),
            (25, 30),
        ),
        (moved(quarter_turn, Filter::Nearest), (30, 22), (22, 30)),
    ] {
        let mut b = DisplayListBuilder::new();
        filtered_layer(
            &mut b,
            ImageFilter::compose(ImageFilter::color(yellow), inner),
        );
        b.draw_rect(Rect::new(20.0, 25.0, 20.0, 10.0), &Paint::from_color(RED));
        b.restore();
        let pixels = render(&mut context, b);
        assert_eq!(pixel(&pixels, upright.0, upright.1), [255, 255, 0, 255]);
        assert_eq!(pixel(&pixels, flat.0, flat.1), GROUND_PIXEL);
    }
}

/// A draw's own matrix filter moves what it draws.
#[test]
fn a_draw_shows_where_its_matrix_moves_it() {
    let Some(mut context) = context("a_draw_shows_where_its_matrix_moves_it") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(10.0, 10.0, 10.0, 10.0),
        &Paint {
            image_filter: Some(moved(Matrix::translation(0.0, 30.0), Filter::Linear)),
            ..Paint::from_color(RED)
        },
    );
    let pixels = render(&mut context, b);
    assert_eq!(pixel(&pixels, 15, 45), RED_PIXEL);
    assert_eq!(pixel(&pixels, 15, 15), GROUND_PIXEL);
}

/// Flutter's magnifier: a backdrop scaled about the lens's centre. Red left of x = 45 and blue
/// right of it; scaled twice about x = 50, the edge moves to x = 40 inside the lens and stays at
/// 45 outside it.
#[test]
fn a_backdrop_matrix_magnifies_what_lies_beneath() {
    let Some(mut context) = context("a_backdrop_matrix_magnifies_what_lies_beneath") else {
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.draw_rect(Rect::new(0.0, 0.0, 45.0, 64.0), &Paint::from_color(RED));
    b.draw_rect(Rect::new(45.0, 0.0, 19.0, 64.0), &Paint::from_color(BLUE));
    let magnify = about(50.0, 32.0, Matrix::scale(2.0, 2.0));
    b.save_layer_backdrop(
        Some(Rect::new(30.0, 12.0, 30.0, 40.0)),
        &Paint::default(),
        Backdrop::new(moved(magnify, Filter::Linear)),
    );
    b.restore();
    let pixels = render(&mut context, b);
    assert_eq!(pixel(&pixels, 38, 32), RED_PIXEL, "left of the moved edge");
    assert_eq!(
        pixel(&pixels, 42, 32),
        BLUE_PIXEL,
        "inside the lens, magnified"
    );
    assert_eq!(
        pixel(&pixels, 42, 5),
        RED_PIXEL,
        "outside the lens, as it was"
    );
}

/// A checkerboard scaled with nearest and with linear sampling, a bar turned, and a magnifier
/// over an edge, which it moves from x = 120 to 130.
fn matrix_filters_scene() -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    for (index, filter) in [Filter::Nearest, Filter::Linear].into_iter().enumerate() {
        let x = 10.0 + index as f32 * 70.0;
        b.save();
        b.translate(x, 10.0);
        filtered_layer(&mut b, moved(Matrix::scale(4.0, 4.0), filter));
        for column in 0..15 {
            for row in 0..15 {
                let colour = if (column + row) % 2 == 0 { RED } else { BLUE };
                b.draw_rect(
                    Rect::new(column as f32, row as f32, 1.0, 1.0),
                    &Paint::from_color(colour),
                );
            }
        }
        b.restore();
        b.restore();
    }
    filtered_layer(
        &mut b,
        moved(about(185.0, 40.0, Matrix::rotation(0.5)), Filter::Linear),
    );
    b.draw_rect(Rect::new(160.0, 30.0, 50.0, 20.0), &Paint::from_color(RED));
    b.restore();
    b.draw_rect(Rect::new(10.0, 90.0, 110.0, 60.0), &Paint::from_color(RED));
    b.draw_rect(
        Rect::new(120.0, 90.0, 110.0, 60.0),
        &Paint::from_color(BLUE),
    );
    b.save_layer_backdrop(
        Some(Rect::new(90.0, 100.0, 60.0, 40.0)),
        &Paint::default(),
        Backdrop::new(moved(
            about(110.0, 120.0, Matrix::scale(2.0, 2.0)),
            Filter::Nearest,
        )),
    );
    b.restore();
    b.build()
}

#[test]
fn matrix_filters_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP matrix_filters_golden: no GPU adapter");
        return;
    };
    let mut context = Context::new(device.clone(), queue.clone());
    let size = [240u32, 160u32];
    let offscreen = Offscreen::new(&device, size);
    context.render(
        &matrix_filters_scene(),
        &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))),
    );
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "matrix_filters", size, &rgba);
}
