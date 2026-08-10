//! Gradients: linear / radial / sweep, on rects AND paths.
//! `cargo run -p valo --example gradients` → target/examples/gradients.png
//!
//! Gradients are a fragment FAMILY, orthogonal to geometry role: the same
//! shader that fills a rect fills a stencil-then-cover quad — so gradients
//! on arbitrary paths compose for free (no special cases). Geometry is in
//! the draw's local space: transform the shape and the gradient rides along.
//!
//! What to look at:
//! - a 5-stop linear bar (uniform stops, ≤8)
//! - radial + sweep
//! - the self-intersecting star filled by a LINEAR gradient (StC cover)
//! - paint.color as opacity multiplier over a gradient

use valo::{
    Color, DisplayListBuilder, FillRule, GradientStop, Paint, PathBuilder, Point, Rect, Shader,
};

fn stops(colors: &[(f32, Color)]) -> Vec<GradientStop> {
    colors
        .iter()
        .map(|&(offset, color)| GradientStop { offset, color })
        .collect()
}

fn scene() -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();

    // 5-stop linear.
    b.draw_rect(
        Rect::new(40.0, 40.0, 560.0, 70.0),
        &Paint::from_shader(Shader::Linear {
            start: Point::new(40.0, 0.0),
            end: Point::new(600.0, 0.0),
            stops: stops(&[
                (0.0, Color::rgb(0.9, 0.25, 0.3)),
                (0.3, Color::rgb(0.95, 0.75, 0.3)),
                (0.5, Color::rgb(0.2, 0.8, 0.55)),
                (0.7, Color::rgb(0.35, 0.55, 1.0)),
                (1.0, Color::rgb(0.7, 0.4, 0.9)),
            ]),
            spread: Default::default(),
            local: Default::default(),
        }),
    );

    // Radial.
    b.draw_rect(
        Rect::new(40.0, 150.0, 180.0, 180.0),
        &Paint::from_shader(Shader::Radial {
            center: Point::new(130.0, 240.0),
            radius: 90.0,
            stops: stops(&[
                (0.0, Color::WHITE),
                (0.7, Color::rgb(0.35, 0.55, 1.0)),
                (1.0, Color::rgb(0.1, 0.12, 0.25)),
            ]),
            spread: Default::default(),
            focus: None,
            local: Default::default(),
        }),
    );

    // Sweep.
    b.draw_rect(
        Rect::new(250.0, 150.0, 180.0, 180.0),
        &Paint::from_shader(Shader::Sweep {
            center: Point::new(340.0, 240.0),
            start_angle: 0.0,
            stops: stops(&[
                (0.0, Color::rgb(0.9, 0.25, 0.3)),
                (0.5, Color::rgb(0.35, 0.55, 1.0)),
                (1.0, Color::rgb(0.9, 0.25, 0.3)), // wrap seamlessly
            ]),
            local: Default::default(),
        }),
    );

    // Gradient THROUGH stencil-then-cover: a star's cover quad, linear fill.
    let star = {
        let mut p = PathBuilder::new();
        for i in 0..5 {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * 4.0 * std::f32::consts::PI / 5.0;
            let pt = (540.0 + 90.0 * a.cos(), 240.0 + 90.0 * a.sin());
            if i == 0 {
                p.move_to(pt);
            } else {
                p.line_to(pt);
            }
        }
        p.close();
        p.build()
    };
    b.draw_path(
        &star,
        FillRule::EvenOdd,
        &Paint::from_shader(Shader::linear(
            Point::new(450.0, 150.0),
            Point::new(630.0, 330.0),
            Color::rgb(0.95, 0.75, 0.3),
            Color::rgb(0.9, 0.25, 0.3),
        )),
    );

    // Opacity multiplier: same gradient, color alpha 0.35.
    let mut faded = Paint::from_shader(Shader::linear(
        Point::new(40.0, 0.0),
        Point::new(600.0, 0.0),
        Color::rgb(0.2, 0.8, 0.55),
        Color::rgb(0.35, 0.55, 1.0),
    ));
    faded.color = Color::rgba(1.0, 1.0, 1.0, 0.35);
    b.draw_rect(Rect::new(40.0, 380.0, 560.0, 60.0), &faded);

    b.build()
}

fn main() {
    valo_harness::run_example(
        "gradients",
        [660, 480],
        Color::rgb(0.07, 0.07, 0.09),
        |_ctx| scene(),
    );
}
