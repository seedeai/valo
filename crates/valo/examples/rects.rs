//! Rects, blends, transforms, retained lists.
//! `cargo run -p valo --example rects` → target/examples/rects.png
//!
//! What to look at:
//! - alpha stacking: overlaps darken (SrcOver over premultiplied color)
//! - the transform stack: save/translate/rotate/restore, canvas semantics
//! - pipeline blend modes: each purple square hits the bar differently
//! - a RETAINED DisplayList stamped twice — the editor pattern: record a
//!   layer once, embed it per frame for the cost of two ops
//! - one rect far off-viewport: culled on the CPU by record-time bounds
//!   (watch `culled 1` in the stats line)

use std::sync::Arc;

use valo::{BlendMode, Color, DisplayListBuilder, Paint, Rect};

fn scene() -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();

    // Alpha stacking.
    for i in 0..5 {
        b.draw_rect(
            Rect::new(40.0 + i as f32 * 45.0, 40.0, 80.0, 80.0),
            &Paint::from_color(Color::rgba(0.9, 0.25, 0.3, 0.55)),
        );
    }

    // Matrix stack: rotated bars share a pivot.
    for i in 0..6 {
        b.save();
        b.translate(430.0, 120.0);
        b.rotate(i as f32 * std::f32::consts::FRAC_PI_6);
        b.draw_rect(
            Rect::new(20.0, -8.0, 130.0, 16.0),
            &Paint::from_color(Color::rgba(0.2, 0.8, 0.55, 0.8)),
        );
        b.restore();
    }

    // Pipeline-expressible blend modes over a base bar.
    b.draw_rect(
        Rect::new(40.0, 200.0, 520.0, 60.0),
        &Paint::from_color(Color::rgb(0.25, 0.28, 0.38)),
    );
    for (i, mode) in [
        BlendMode::Plus,
        BlendMode::Screen,
        BlendMode::Modulate,
        BlendMode::Xor,
    ]
    .into_iter()
    .enumerate()
    {
        b.draw_rect(
            Rect::new(60.0 + i as f32 * 130.0, 185.0, 100.0, 90.0),
            &Paint {
                color: Color::rgba(0.55, 0.4, 0.9, 0.9),
                blend_mode: mode,
                ..Default::default()
            },
        );
    }

    // The retained card: recorded ONCE, embedded twice (second scaled).
    let card = {
        let mut c = DisplayListBuilder::new();
        c.draw_rect(
            Rect::new(0.0, 0.0, 120.0, 90.0),
            &Paint::from_color(Color::rgb(0.16, 0.17, 0.22)),
        );
        c.draw_rect(
            Rect::new(10.0, 10.0, 100.0, 20.0),
            &Paint::from_color(Color::rgb(0.35, 0.55, 1.0)),
        );
        Arc::new(c.build())
    };
    b.save();
    b.translate(60.0, 300.0);
    b.draw_display_list(&card);
    b.translate(160.0, 20.0);
    b.scale(1.4, 1.4);
    b.draw_display_list(&card);
    b.restore();

    // Culled: the record-time oracle knows this never intersects the viewport.
    b.draw_rect(
        Rect::new(5000.0, 5000.0, 100.0, 100.0),
        &Paint::from_color(Color::BLACK),
    );

    b.build()
}

fn main() {
    valo_harness::run_example("rects", [640, 480], Color::rgb(0.07, 0.07, 0.09), |_ctx| {
        scene()
    });
}
