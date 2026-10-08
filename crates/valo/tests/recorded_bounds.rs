//! The recorder pads a draw's bounds for its effects by its own rules (3σ
//! for a blur, a drop shadow's offset on both sides, a stroke's joins), and
//! the planner sizes what it draws by Impeller's. The two are kept apart on
//! purpose; this guards that the recorder's are never the smaller: every
//! pixel a draw paints lies inside the bounds recorded for it, which is what
//! culling and a layer's texture rely on.

use valo::{
    BlendMode, Color, Context, DisplayListBuilder, ImageFilter, MaskBlur, Paint, PaintStyle, Point,
    Rect,
};

const SIZE: [u32; 2] = [160, 160];

/// `Scene` records one draw, with whatever transform or layer it needs.
type Scene = Box<dyn Fn(&mut DisplayListBuilder)>;

/// `painted_outside_bounds` renders `draw` onto a transparent target and
/// returns the first pixel it painted outside the bounds the list records.
fn painted_outside_bounds(
    context: &mut Context,
    draw: impl FnOnce(&mut DisplayListBuilder),
) -> Option<(u32, u32)> {
    let mut builder = DisplayListBuilder::new();
    draw(&mut builder);
    let list = builder.build();
    let bounds = list
        .bounds()
        .rect()
        .expect("the draw shows within a rectangle");
    let pixels = context.render_to_rgba(&list, SIZE, Some(Color::TRANSPARENT));
    let inside = |x: u32, y: u32| {
        let (x, y) = (x as f32, y as f32);
        x + 1.0 > bounds.x && x < bounds.right() && y + 1.0 > bounds.y && y < bounds.bottom()
    };
    (0..SIZE[1])
        .flat_map(|y| (0..SIZE[0]).map(move |x| (x, y)))
        .find(|&(x, y)| pixels[((y * SIZE[0] + x) * 4 + 3) as usize] > 0 && !inside(x, y))
}

fn blue() -> Paint {
    Paint::from_color(Color::rgb(0.0, 0.0, 1.0))
}

fn star() -> std::sync::Arc<valo::Path> {
    let mut path = valo::PathBuilder::new();
    path.move_to((80.0, 40.0))
        .line_to((95.0, 110.0))
        .line_to((45.0, 65.0))
        .line_to((115.0, 65.0))
        .line_to((65.0, 110.0))
        .close();
    path.build()
}

#[test]
fn every_pixel_a_draw_paints_lies_inside_its_recorded_bounds() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP every_pixel_a_draw_paints_lies_inside_its_recorded_bounds");
        return;
    };
    let mut context = Context::new(device, queue);
    let rect = Rect::new(60.0, 60.0, 40.0, 30.0);
    let scenes: Vec<(&str, Scene)> = vec![
        (
            "blurred rect",
            Box::new(move |b| {
                let paint = Paint {
                    image_filter: Some(ImageFilter::blur(6.0, 3.0)),
                    ..blue()
                };
                b.draw_rect(rect, &paint);
            }),
        ),
        (
            "drop shadow",
            Box::new(move |b| {
                let shadow =
                    ImageFilter::drop_shadow(Point::new(9.0, -7.0), 4.0, 4.0, Color::BLACK);
                let paint = Paint {
                    image_filter: Some(shadow),
                    ..blue()
                };
                b.draw_rect(rect, &paint);
            }),
        ),
        (
            "mask-blurred star",
            Box::new(|b| {
                let paint = Paint {
                    mask_blur: Some(MaskBlur::new(5.0)),
                    ..blue()
                };
                b.draw_path(&star(), valo::FillRule::NonZero, &paint);
            }),
        ),
        (
            "mitred stroke",
            Box::new(|b| {
                let paint = Paint {
                    style: PaintStyle::Stroke(valo::Stroke {
                        join: valo::Join::Miter,
                        miter_limit: 4.0,
                        ..valo::Stroke::new(6.0)
                    }),
                    ..blue()
                };
                b.draw_path(&star(), valo::FillRule::NonZero, &paint);
            }),
        ),
        (
            "turned, blurred rect",
            Box::new(move |b| {
                b.translate(80.0, 80.0);
                b.rotate(0.6);
                b.translate(-80.0, -80.0);
                let paint = Paint {
                    image_filter: Some(ImageFilter::blur(8.0, 2.0)),
                    ..blue()
                };
                b.draw_rect(rect, &paint);
            }),
        ),
        (
            "blurred layer",
            Box::new(move |b| {
                let paint = Paint {
                    image_filter: Some(ImageFilter::blur(5.0, 5.0)),
                    ..Paint::default()
                };
                b.save_layer(None, &paint);
                b.draw_rect(rect, &blue());
                b.restore();
            }),
        ),
        (
            "layer with a drop shadow, scaled",
            Box::new(move |b| {
                b.scale(1.5, 1.5);
                let shadow =
                    ImageFilter::drop_shadow(Point::new(-6.0, 5.0), 3.0, 3.0, Color::BLACK);
                b.save_layer(
                    None,
                    &Paint {
                        image_filter: Some(shadow),
                        ..Paint::default()
                    },
                );
                b.draw_rect(Rect::new(40.0, 40.0, 30.0, 20.0), &blue());
                b.restore();
            }),
        ),
        (
            "multiplied, blurred rect",
            Box::new(move |b| {
                let paint = Paint {
                    blend_mode: BlendMode::Multiply,
                    image_filter: Some(ImageFilter::blur(4.0, 4.0)),
                    ..blue()
                };
                b.draw_rect(rect, &paint);
            }),
        ),
    ];
    for (name, draw) in scenes {
        assert_eq!(
            painted_outside_bounds(&mut context, |b| draw(b)),
            None,
            "{name} paints outside its recorded bounds"
        );
    }
}
