//! Mask blur: analytic (r)rect shadows + the general filter path.
//! `cargo run -p valo --example shadows`
//!
//! Two very different costs behind one Paint field:
//! - `mask_blur` on a SOLID rect/rrect renders in CLOSED FORM — one quad
//!   whose fragment evaluates gaussian-convolved coverage (erf math). Zero
//!   filter passes; why a Flutter BoxShadow is one draw.
//! - `mask_blur` on anything else (paths, gradients, images) renders the
//!   draw sharp into an implicit layer, blurs it at scale (σ>4 downsamples
//!   first), and composites — watch `filter passes` in the stats.
//!
//! What to look at:
//! - TOP: rect shadows at σ 2/6/12/24 — spread grows, cost stays one quad.
//! - MIDDLE: the BoxShadow recipe (blurred dark rrect under a solid card)
//!   and a colored glow (blurred rrect under itself, no offset).
//! - BOTTOM: general path — a blurred star (path) and a blurred gradient
//!   square; both run the layer + separable-blur chain.

use valo::{Color, DisplayListBuilder, MaskBlur, Paint, PathBuilder, Point, Rect, Shader};

fn shadow_paint(sigma: f32) -> Paint {
    Paint {
        color: Color::rgba(0.0, 0.0, 0.0, 0.6),
        mask_blur: Some(MaskBlur::new(sigma)),
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
    let card = Paint::from_color(Color::rgb(0.93, 0.94, 0.97));

    // Analytic rect shadows: σ grows, still one quad each.
    for (i, sigma) in [2.0f32, 6.0, 12.0, 24.0].into_iter().enumerate() {
        let x = 45.0 + i as f32 * 150.0;
        b.draw_rect(Rect::new(x, 50.0, 100.0, 70.0), &shadow_paint(sigma));
    }

    // The BoxShadow recipe: blurred dark rrect offset under a solid card.
    b.draw_rrect(
        Rect::new(53.0, 218.0, 160.0, 100.0),
        16.0,
        &shadow_paint(8.0),
    );
    b.draw_rrect(Rect::new(45.0, 205.0, 160.0, 100.0), 16.0, &card);

    // A glow: same shape blurred in place, colored.
    b.draw_rrect(
        Rect::new(280.0, 205.0, 160.0, 100.0),
        16.0,
        &Paint {
            color: Color::rgba(0.35, 0.6, 1.0, 0.9),
            mask_blur: Some(MaskBlur::new(12.0)),
            ..Default::default()
        },
    );
    b.draw_rrect(
        Rect::new(280.0, 205.0, 160.0, 100.0),
        16.0,
        &Paint::from_color(Color::rgb(0.16, 0.18, 0.24)),
    );

    // General path: a blurred STAR takes the layer + filter chain.
    b.draw_path(
        &star(Point::new(120.0, 410.0), 55.0),
        valo::FillRule::NonZero,
        &Paint {
            color: Color::rgba(0.95, 0.75, 0.3, 0.9),
            mask_blur: Some(MaskBlur::new(6.0)),
            ..Default::default()
        },
    );
    // A blurred GRADIENT square (shader ⇒ general path, σ>4 downsamples).
    b.draw_rect(
        Rect::new(280.0, 360.0, 120.0, 100.0),
        &Paint {
            shader: Some(Shader::linear(
                Point::new(280.0, 360.0),
                Point::new(400.0, 460.0),
                Color::rgb(0.9, 0.3, 0.4),
                Color::rgb(0.3, 0.4, 0.9),
            )),
            color: Color::WHITE,
            mask_blur: Some(MaskBlur::new(10.0)),
            ..Default::default()
        },
    );
    // The same gradient sharp, for contrast.
    b.draw_rect(
        Rect::new(480.0, 360.0, 120.0, 100.0),
        &Paint {
            shader: Some(Shader::linear(
                Point::new(480.0, 360.0),
                Point::new(600.0, 460.0),
                Color::rgb(0.9, 0.3, 0.4),
                Color::rgb(0.3, 0.4, 0.9),
            )),
            color: Color::WHITE,
            ..Default::default()
        },
    );

    b.build()
}

fn main() {
    valo_harness::run_example("shadows", [660, 480], Color::rgb(0.89, 0.9, 0.93), |_ctx| {
        scene()
    });
}
