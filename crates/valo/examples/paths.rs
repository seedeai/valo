//! Stencil-then-cover path fills.
//! `cargo run -p valo --example paths` → target/examples/paths.png
//!
//! Any path — self-intersecting, concave, holes — fills with ZERO CPU
//! tessellation: pass 1 winds the flattened contours into the stencil
//! buffer, pass 2 covers the bounds and draws where the winding says
//! "inside", resetting the stencil behind itself.
//!
//! What to look at:
//! - the same self-intersecting star under both fill rules: NonZero fills
//!   the core, EvenOdd leaves the pentagonal hole
//! - a ring (two circles wound oppositely) — holes are just winding
//! - curves (circle, rrect) flattened adaptively at the draw's device scale

use valo::{Color, DisplayListBuilder, FillRule, Paint, PathBuilder, Point, Rect};

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
    let teal = Paint::from_color(Color::rgba(0.2, 0.8, 0.55, 0.9));
    let mut b = DisplayListBuilder::new();

    // One shape, two rules — the StC litmus test.
    b.draw_path(
        &star(Point::new(110.0, 110.0), 85.0),
        FillRule::NonZero,
        &teal,
    );
    b.draw_path(
        &star(Point::new(300.0, 110.0), 85.0),
        FillRule::EvenOdd,
        &teal,
    );

    // A ring: outer circle + inner circle; NonZero cancels where the winding
    // says "hole" (contours run in the same direction here, so use EvenOdd).
    let ring = {
        let mut p = PathBuilder::new();
        p.circle((470.0, 110.0), 75.0);
        p.circle((470.0, 110.0), 40.0);
        p.build()
    };
    b.draw_path(
        &ring,
        FillRule::EvenOdd,
        &Paint::from_color(Color::rgba(0.9, 0.4, 0.3, 0.9)),
    );

    // Curves flatten at device scale: crank the transform, edges stay smooth.
    b.save();
    b.translate(120.0, 320.0);
    b.scale(2.0, 2.0);
    b.draw_rrect(
        Rect::new(0.0, 0.0, 90.0, 60.0),
        18.0,
        &Paint::from_color(Color::rgb(0.35, 0.55, 1.0)),
    );
    b.restore();

    b.draw_circle(
        (430.0, 370.0),
        70.0,
        &Paint::from_color(Color::rgba(0.95, 0.75, 0.3, 0.95)),
    );

    b.build()
}

fn main() {
    valo_harness::run_example("paths", [640, 480], Color::rgb(0.07, 0.07, 0.09), |_ctx| {
        scene()
    });
}
