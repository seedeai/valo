//! Save layers + group opacity + elision.
//! `cargo run -p valo --example layers` → target/examples/layers.png
//!
//! What to look at:
//! - LEFT: three overlapping circles at per-draw alpha 0.5 — the overlaps
//!   darken (each draw blends separately).
//! - MIDDLE: the same circles inside `save_layer(alpha 0.5)` — the GROUP
//!   fades as one: children render opaque into an offscreen texture, the
//!   composite applies the alpha once. This is what save layers are FOR.
//! - RIGHT: disjoint cards inside `save_layer(alpha)` — the recorder proved
//!   them pairwise-disjoint + alpha-linear, so NO texture exists: the alpha
//!   rides each draw at the composite's z. Watch `layers elided 1` in the
//!   stats — pixels identical, texture free.
//! - BOTTOM: a nested layer (layer in layer) with a rotated clip inside.

use valo::{ClipOp, Color, DisplayListBuilder, Paint, Rect};

fn three_circles(b: &mut DisplayListBuilder, cx: f32, cy: f32, paint: &Paint) {
    b.draw_circle((cx, cy - 28.0), 45.0, paint);
    b.draw_circle((cx - 34.0, cy + 28.0), 45.0, paint);
    b.draw_circle((cx + 34.0, cy + 28.0), 45.0, paint);
}

fn scene() -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    let teal = Paint::from_color(Color::rgb(0.2, 0.8, 0.55));
    let teal_half = Paint::from_color(Color::rgba(0.2, 0.8, 0.55, 0.5));
    let group_alpha = Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.5));

    // Per-draw alpha: overlaps show through.
    three_circles(&mut b, 110.0, 120.0, &teal_half);

    // Group alpha: one texture, one fade — no internal seams.
    b.save_layer(None, &group_alpha);
    three_circles(&mut b, 330.0, 120.0, &teal);
    b.restore();

    // Elided group alpha: disjoint children, so the texture never exists.
    b.save_layer(None, &group_alpha);
    for i in 0..3 {
        b.draw_rrect(
            Rect::new(490.0, 40.0 + i as f32 * 60.0, 120.0, 48.0),
            12.0,
            &Paint::from_color(Color::rgb(0.35, 0.55, 1.0)),
        );
    }
    b.restore();

    // Nested: an outer half-alpha layer containing an inner layer with a
    // clip (the clip forfeits the INNER layer's elision, not correctness).
    b.save_layer(None, &group_alpha);
    b.draw_rect(
        Rect::new(120.0, 280.0, 400.0, 140.0),
        &Paint::from_color(Color::rgb(0.25, 0.28, 0.38)),
    );
    b.save_layer(None, &Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.8)));
    b.save();
    b.translate(320.0, 350.0);
    b.rotate(0.25);
    b.clip_rect(Rect::new(-140.0, -45.0, 280.0, 90.0), ClipOp::Intersect);
    b.rotate(-0.25);
    b.translate(-320.0, -350.0);
    for i in 0..8 {
        b.draw_circle(
            (170.0 + i as f32 * 45.0, 350.0),
            26.0,
            &Paint::from_color(Color::rgb(0.95, 0.75, 0.3)),
        );
    }
    b.restore();
    b.restore();
    b.restore();

    b.build()
}

fn main() {
    valo_harness::run_example("layers", [640, 480], Color::rgb(0.07, 0.07, 0.09), |_ctx| {
        scene()
    });
}
