//! A draw's mask blur, as Impeller's `CreateMaskBlur` makes it: a solid
//! colour blurs as it is drawn; a shader paint or an image blurs a white
//! mask of its geometry and fills it with its own contents, which stay
//! sharp inside the soft edge. An inner or outer style clips the blur to the
//! geometry or to outside it, for that one draw only; a solid style draws
//! the sharp shape under its blur.
//!
//! The scenes are Impeller's playground tests from
//! `aiks_dl_blur_unittests.cc` at a content scale of one, checked at a few
//! pixels where the styles differ, since Impeller's own goldens are images.

use valo::{
    BlurStyle, Color, Context, DisplayList, DisplayListBuilder, FillRule, Image, ImageDesc,
    ImageFilter, MaskBlur, Paint, PaintStyle, PathBuilder, Point, Rect, Shader, SpreadMode, Stroke,
    TileMode,
};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 450;
const BACKGROUND: Color = Color::rgb(0.1, 0.1, 0.1);
/// The background's channels in the target, 0.1 of 255.
const BACKGROUND_CHANNEL: u8 = 26;

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * WIDTH + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

fn render(context: &mut Context, list: &DisplayList) -> Vec<u8> {
    context.render_to_rgba(list, [WIDTH, HEIGHT], Some(Color::TRANSPARENT))
}

fn is_background(pixel: [u8; 4]) -> bool {
    pixel[..3]
        .iter()
        .all(|channel| channel.abs_diff(BACKGROUND_CHANNEL) <= 3)
}

fn is_red(pixel: [u8; 4]) -> bool {
    pixel == [255, 0, 0, 255]
}

fn styled(blur: BlurStyle, sigma: f32) -> MaskBlur {
    MaskBlur { sigma, style: blur }
}

/// Impeller's `GaussianBlurStyle{Inner,Outer,Solid}` and their gradient
/// variants: a triangle with a mask blur of σ 30 over a dark background,
/// then a red square beside it "to make sure the clip area is reset". A
/// small blue square inside the triangle, drawn last, is valo's addition:
/// nothing after the blur is clipped inside the triangle either.
fn triangle_scene(paint: Paint) -> DisplayList {
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        Rect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32),
        &Paint::from_color(BACKGROUND),
    );
    let mut triangle = PathBuilder::new();
    triangle.move_to((200.0, 200.0));
    triangle.line_to((300.0, 400.0));
    triangle.line_to((100.0, 400.0));
    triangle.close();
    builder.draw_path(&triangle.build(), FillRule::NonZero, &paint);
    builder.draw_rect(
        Rect::new(0.0, 0.0, 200.0, 200.0),
        &Paint::from_color(Color::rgb(1.0, 0.0, 0.0)),
    );
    builder.draw_rect(
        Rect::new(195.0, 320.0, 10.0, 10.0),
        &Paint::from_color(Color::rgb(0.0, 0.0, 1.0)),
    );
    builder.build()
}

fn green(style: BlurStyle) -> Paint {
    Paint {
        mask_blur: Some(styled(style, 30.0)),
        ..Paint::from_color(Color::rgb(0.0, 1.0, 0.0))
    }
}

/// Impeller's red gradient, mirrored along the diagonal.
fn gradient(style: BlurStyle) -> Paint {
    Paint {
        shader: Some(Shader::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(200.0, 200.0),
            stops: vec![
                valo::GradientStop {
                    offset: 0.0,
                    color: Color::rgb(0.9568, 0.2627, 0.2118),
                },
                valo::GradientStop {
                    offset: 1.0,
                    color: Color::rgb(0.7568, 0.2627, 0.2118),
                },
            ],
            spread: SpreadMode::Reflect,
            local: valo::Matrix::IDENTITY,
        }),
        mask_blur: Some(styled(style, 30.0)),
        ..Paint::from_color(Color::WHITE)
    }
}

/// What a triangle scene shows at three places: 30 pixels inside the
/// triangle's base, 12 pixels below it, and the squares drawn after.
struct TriangleFacts {
    inside: [u8; 4],
    below: [u8; 4],
}

fn triangle_facts(context: &mut Context, paint: Paint) -> TriangleFacts {
    let pixels = render(context, &triangle_scene(paint));
    assert!(
        is_red(pixel(&pixels, 100, 100)) && is_red(pixel(&pixels, 196, 196)),
        "the red square after the blur is whole"
    );
    assert_eq!(
        pixel(&pixels, 200, 325),
        [0, 0, 255, 255],
        "the blue square after the blur is whole"
    );
    TriangleFacts {
        inside: pixel(&pixels, 200, 370),
        below: pixel(&pixels, 200, 412),
    }
}

#[test]
fn gaussian_blur_style_inner() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP gaussian_blur_style_inner");
        return;
    };
    let mut context = Context::new(device, queue);
    let facts = triangle_facts(&mut context, green(BlurStyle::Inner));
    assert!(
        (120..245).contains(&facts.inside[1]),
        "inside, the blur: {:?}",
        facts.inside
    );
    assert!(
        is_background(facts.below),
        "outside, nothing: {:?}",
        facts.below
    );
}

#[test]
fn gaussian_blur_style_outer() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP gaussian_blur_style_outer");
        return;
    };
    let mut context = Context::new(device, queue);
    let facts = triangle_facts(&mut context, green(BlurStyle::Outer));
    assert!(
        is_background(facts.inside),
        "inside, nothing: {:?}",
        facts.inside
    );
    assert!(facts.below[1] > 60, "outside, the blur: {:?}", facts.below);
}

#[test]
fn gaussian_blur_style_solid() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP gaussian_blur_style_solid");
        return;
    };
    let mut context = Context::new(device, queue);
    let facts = triangle_facts(&mut context, green(BlurStyle::Solid));
    assert_eq!(facts.inside, [0, 255, 0, 255], "inside, the sharp shape");
    assert!(facts.below[1] > 60, "outside, the blur: {:?}", facts.below);
}

/// The gradient keeps its own colour wherever it shows: red over the dark
/// background, green and blue at the gradient's 0.2627 and 0.2118.
fn shows_the_gradient(pixel: [u8; 4], alpha: f32) -> bool {
    let over_background = |channel: f32| channel * 255.0 * alpha + 26.0 * (1.0 - alpha);
    pixel[1].abs_diff(over_background(0.2627) as u8) <= 4
        && pixel[2].abs_diff(over_background(0.2118) as u8) <= 4
}

#[test]
fn gaussian_blur_style_inner_gradient() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP gaussian_blur_style_inner_gradient");
        return;
    };
    let mut context = Context::new(device, queue);
    let facts = triangle_facts(&mut context, gradient(BlurStyle::Inner));
    assert!(
        facts.inside[0] > 120 && facts.inside[0] < 230,
        "inside, the blurred gradient: {:?}",
        facts.inside
    );
    assert!(
        is_background(facts.below),
        "outside, nothing: {:?}",
        facts.below
    );
}

#[test]
fn gaussian_blur_style_outer_gradient() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP gaussian_blur_style_outer_gradient");
        return;
    };
    let mut context = Context::new(device, queue);
    let facts = triangle_facts(&mut context, gradient(BlurStyle::Outer));
    assert!(
        is_background(facts.inside),
        "inside, nothing: {:?}",
        facts.inside
    );
    assert!(
        facts.below[0] > 60,
        "outside, the blurred gradient: {:?}",
        facts.below
    );
}

#[test]
fn gaussian_blur_style_solid_gradient() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP gaussian_blur_style_solid_gradient");
        return;
    };
    let mut context = Context::new(device, queue);
    let facts = triangle_facts(&mut context, gradient(BlurStyle::Solid));
    assert!(
        shows_the_gradient(facts.inside, 1.0),
        "inside, the sharp gradient: {:?}",
        facts.inside
    );
    assert!(
        facts.below[0] > 60,
        "outside, the blurred gradient: {:?}",
        facts.below
    );
}

/// Impeller's `GradientOvalStrokeMaskBlur` and its styles, without the
/// white guide lines: a stroked oval, 20 wide, with a red-to-blue gradient
/// and a mask blur of σ 10. The style clips to the stroke, not to the oval
/// it outlines.
fn stroked_oval(style: BlurStyle) -> DisplayList {
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        Rect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32),
        &Paint::from_color(BACKGROUND),
    );
    builder.translate(100.0, 100.0);
    let paint = Paint {
        shader: Some(Shader::linear(
            Point::new(0.0, 0.0),
            Point::new(200.0, 200.0),
            Color::rgb(1.0, 0.0, 0.0),
            Color::rgb(0.0, 0.0, 1.0),
        )),
        style: PaintStyle::Stroke(Stroke::new(20.0)),
        mask_blur: Some(styled(style, 10.0)),
        ..Paint::from_color(Color::WHITE)
    };
    // `DlRoundRect::MakeRectXY(200 × 60, 50, 100)`: radii scaled to fit.
    builder.draw_rrect_radii_elliptical(
        Rect::new(0.0, 0.0, 200.0, 60.0),
        [[15.0, 30.0]; 4],
        &paint,
    );
    builder.build()
}

/// What a stroked oval shows on its stroke's top, 12 pixels above the
/// stroke, and inside the ring, 8 pixels below the stroke.
struct StrokeFacts {
    on_stroke: [u8; 4],
    above: [u8; 4],
    in_the_ring: [u8; 4],
}

fn stroke_facts(context: &mut Context, style: BlurStyle) -> StrokeFacts {
    let pixels = render(context, &stroked_oval(style));
    StrokeFacts {
        on_stroke: pixel(&pixels, 200, 100),
        above: pixel(&pixels, 200, 78),
        in_the_ring: pixel(&pixels, 200, 118),
    }
}

#[test]
fn gradient_oval_stroke_mask_blur_inner() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP gradient_oval_stroke_mask_blur_inner");
        return;
    };
    let mut context = Context::new(device, queue);
    let facts = stroke_facts(&mut context, BlurStyle::Inner);
    assert!(
        !is_background(facts.on_stroke) && facts.on_stroke[1] > 3,
        "on the stroke, the blur, the background showing through: {:?}",
        facts.on_stroke
    );
    assert!(is_background(facts.above), "{:?}", facts.above);
    assert!(is_background(facts.in_the_ring), "{:?}", facts.in_the_ring);
}

#[test]
fn gradient_oval_stroke_mask_blur_outer() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP gradient_oval_stroke_mask_blur_outer");
        return;
    };
    let mut context = Context::new(device, queue);
    let facts = stroke_facts(&mut context, BlurStyle::Outer);
    assert!(is_background(facts.on_stroke), "{:?}", facts.on_stroke);
    assert!(!is_background(facts.above), "{:?}", facts.above);
    assert!(!is_background(facts.in_the_ring), "{:?}", facts.in_the_ring);
}

#[test]
fn gradient_oval_stroke_mask_blur_solid() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP gradient_oval_stroke_mask_blur_solid");
        return;
    };
    let mut context = Context::new(device, queue);
    let facts = stroke_facts(&mut context, BlurStyle::Solid);
    assert!(
        facts.on_stroke[1] <= 3 && facts.on_stroke[3] == 255,
        "on the stroke, the sharp gradient: {:?}",
        facts.on_stroke
    );
    assert!(!is_background(facts.above), "{:?}", facts.above);
    assert!(!is_background(facts.in_the_ring), "{:?}", facts.in_the_ring);
}

/// A checkerboard of red and blue squares, `square` texels each.
fn checkerboard(context: &mut Context, side: u32, square: u32) -> Image {
    let texels: Vec<u8> = (0..side * side)
        .flat_map(|index| {
            let (x, y) = (index % side / square, index / side / square);
            if (x + y) % 2 == 0 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            }
        })
        .collect();
    context.upload_image(
        ImageDesc {
            size: [side, side],
            premultiplied: true,
            mips: false,
        },
        &texels,
    )
}

/// Impeller's `MaskBlurTexture` with a checkerboard for its photograph: the
/// image's edge goes soft and its squares stay their own colours, where
/// blurring the image would mix them.
#[test]
fn mask_blur_texture() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP mask_blur_texture");
        return;
    };
    let mut context = Context::new(device, queue);
    let image = checkerboard(&mut context, 128, 16);
    let mut builder = DisplayListBuilder::new();
    builder.draw_image(
        &image,
        Rect::new(136.0, 136.0, 128.0, 128.0),
        &Paint {
            mask_blur: Some(MaskBlur::new(8.0)),
            ..Paint::from_color(Color::rgb(0.0, 1.0, 0.0))
        },
    );
    let pixels = render(&mut context, &builder.build());
    // Texel (56, 56), the middle of a red square.
    assert_eq!(pixel(&pixels, 192, 192), [255, 0, 0, 255]);
    let edge = pixel(&pixels, 136, 144);
    assert!(
        (90..170).contains(&edge[3]) && edge[2] <= 2,
        "the image's edge is soft and keeps its red: {edge:?}"
    );
    let past_the_edge = pixel(&pixels, 130, 144);
    assert!(
        past_the_edge[3] > 10 && past_the_edge[2] <= 2,
        "past the edge, the edge texels fill the halo: {past_the_edge:?}"
    );
}

/// Not one of Impeller's: a gradient with a hard step keeps the step sharp
/// under a mask blur, where blurring its colours would bleed one side into
/// the other.
#[test]
fn a_mask_blurred_gradient_keeps_its_colours() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_mask_blurred_gradient_keeps_its_colours");
        return;
    };
    let mut context = Context::new(device, queue);
    let step = |offset, color| valo::GradientStop { offset, color };
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        Rect::new(100.0, 100.0, 200.0, 200.0),
        &Paint {
            shader: Some(Shader::Linear {
                start: Point::new(100.0, 0.0),
                end: Point::new(300.0, 0.0),
                stops: vec![
                    step(0.0, Color::rgb(1.0, 0.0, 0.0)),
                    step(0.5, Color::rgb(1.0, 0.0, 0.0)),
                    step(0.5, Color::rgb(0.0, 0.0, 1.0)),
                    step(1.0, Color::rgb(0.0, 0.0, 1.0)),
                ],
                spread: SpreadMode::Pad,
                local: valo::Matrix::IDENTITY,
            }),
            mask_blur: Some(MaskBlur::new(12.0)),
            ..Paint::from_color(Color::WHITE)
        },
    );
    let pixels = render(&mut context, &builder.build());
    assert_eq!(pixel(&pixels, 196, 200), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 204, 200), [0, 0, 255, 255]);
}

/// Not one of Impeller's: a draw takes its mask blur and then its image
/// filter, one after the other, where Impeller's geometry path drops the
/// image filter.
#[test]
fn a_mask_blur_and_an_image_filter_both_blur_a_draw() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_mask_blur_and_an_image_filter_both_blur_a_draw");
        return;
    };
    let mut context = Context::new(device, queue);
    let mut alpha_beside = |image_filter: Option<ImageFilter>| {
        let mut builder = DisplayListBuilder::new();
        builder.draw_rect(
            Rect::new(100.0, 100.0, 100.0, 100.0),
            &Paint {
                mask_blur: Some(MaskBlur::new(4.0)),
                image_filter,
                ..Paint::from_color(Color::WHITE)
            },
        );
        pixel(&render(&mut context, &builder.build()), 206, 150)[3]
    };
    let mask_blur_alone = alpha_beside(None);
    let both = alpha_beside(Some(ImageFilter::blur(4.0, 4.0)));
    assert!(
        both > mask_blur_alone + 5,
        "the image filter widens the mask blur: {both} against {mask_blur_alone}"
    );
}

/// The lead's departure from Impeller's `TextureContentsWithDestinationRectScaled`:
/// an image drawn whole at twice its size blurs as far, in the pixels it is
/// drawn at, as one drawn at its own size.
#[test]
fn an_image_drawn_at_twice_its_size_blurs_as_far() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP an_image_drawn_at_twice_its_size_blurs_as_far");
        return;
    };
    let mut context = Context::new(device, queue);
    let mut halo = |side: u32| {
        let white = context.upload_image(
            ImageDesc {
                size: [side, side],
                premultiplied: true,
                mips: false,
            },
            &[255, 255, 255, 255].repeat((side * side) as usize),
        );
        let mut builder = DisplayListBuilder::new();
        builder.draw_image(
            &white,
            Rect::new(100.0, 100.0, 64.0, 64.0),
            &Paint {
                image_filter: Some(ImageFilter::blur(4.0, 4.0).with_tile_mode(TileMode::Decal)),
                ..Paint::from_color(Color::WHITE)
            },
        );
        let pixels = render(&mut context, &builder.build());
        [160, 166, 170].map(|x| pixel(&pixels, x, 132)[3])
    };
    let own_size = halo(64);
    let twice = halo(32);
    for (own, scaled) in own_size.into_iter().zip(twice) {
        assert!(
            own.abs_diff(scaled) <= 6,
            "the halo across the edge matches: {own_size:?} against {twice:?}"
        );
    }
}
