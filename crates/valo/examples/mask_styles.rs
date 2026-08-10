//! Blur styles (Skia's SkBlurStyle) + per-corner rrect radii.
//! `cargo run -p valo --example mask_styles`
//!
//! What to look at:
//! - TOP: one rrect, four styles. Normal = shadow everywhere; Solid = the
//!   sharp shape sitting on its own glow (one draw — no separate fill!);
//!   Inner = blur only inside (pressed look); Outer = halo only.
//! - MIDDLE: per-corner radii `[tl, tr, br, bl]` — a sharp card and its
//!   one-quad analytic shadow share the same corner vocabulary.
//! - BOTTOM: styled GENERAL paths — an Outer glow on a star and a Solid
//!   gradient square. These run blur chain + ONE combine pass merging the
//!   blur with the sharp layer, so any blend mode composites unchanged.

use valo::{Color, DisplayListBuilder, MaskBlur, Paint, PathBuilder, Point, Rect, Shader};

fn styled(color: Color, blur: MaskBlur) -> Paint {
    Paint {
        color,
        mask_blur: Some(blur),
        ..Default::default()
    }
}

fn star(c: Point, r: f32) -> std::sync::Arc<valo::Path> {
    let mut p = PathBuilder::new();
    for i in 0..5 {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * 4.0 * std::f32::consts::PI / 5.0;
        let pt = (c.x + r * a.cos(), c.y + r * a.sin());
        if i == 0 {
            p.move_to(pt);
        } else {
            p.line_to(pt);
        }
    }
    p.close();
    p.build()
}

fn scene() -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    let ink = Color::rgb(0.2, 0.25, 0.4);

    // One rrect, four styles — all analytic, one quad each.
    for (i, blur) in [
        MaskBlur::new(8.0),
        MaskBlur::solid(8.0),
        MaskBlur::inner(8.0),
        MaskBlur::outer(8.0),
    ]
    .into_iter()
    .enumerate()
    {
        let x = 45.0 + i as f32 * 150.0;
        b.draw_rrect(Rect::new(x, 45.0, 110.0, 90.0), 18.0, &styled(ink, blur));
    }

    // Per-corner radii: the sharp card and its analytic shadow agree.
    let radii = [48.0, 0.0, 24.0, 8.0];
    b.draw_rrect_radii(
        Rect::new(53.0, 218.0, 240.0, 130.0),
        radii,
        &styled(Color::rgba(0.0, 0.0, 0.0, 0.55), MaskBlur::new(10.0)),
    );
    b.draw_rrect_radii(
        Rect::new(45.0, 205.0, 240.0, 130.0),
        radii,
        &Paint::from_color(Color::rgb(0.95, 0.96, 0.99)),
    );
    // Solid style needs no second draw: shape + glow in one quad.
    b.draw_rrect_radii(
        Rect::new(370.0, 205.0, 240.0, 130.0),
        [8.0, 48.0, 8.0, 48.0],
        &styled(Color::rgb(0.35, 0.6, 1.0), MaskBlur::solid(14.0)),
    );

    // Styled general paths: blur chain + one combine pass each.
    b.draw_path(
        &star(Point::new(130.0, 420.0), 52.0),
        valo::FillRule::NonZero,
        &styled(Color::rgb(0.95, 0.75, 0.3), MaskBlur::outer(8.0)),
    );
    b.draw_rect(
        Rect::new(300.0, 370.0, 130.0, 100.0),
        &Paint {
            shader: Some(Shader::linear(
                Point::new(300.0, 370.0),
                Point::new(430.0, 470.0),
                Color::rgb(0.9, 0.3, 0.4),
                Color::rgb(0.3, 0.4, 0.9),
            )),
            color: Color::WHITE,
            mask_blur: Some(MaskBlur::solid(10.0)),
            ..Default::default()
        },
    );

    b.build()
}

fn main() {
    valo_harness::run_example(
        "mask_styles",
        [660, 500],
        Color::rgb(0.85, 0.86, 0.9),
        |_ctx| scene(),
    );
}
