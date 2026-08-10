//! Depth-buffer clips.
//! `cargo run -p valo --example clips` → target/examples/clips.png
//!
//! Clips never track state in the renderer: the recorder knows each clip's
//! EXPIRY (the depth slot of the restore that closes its scope), and the
//! clip renders a depth CEILING at that z — outside the shape for
//! Intersect, inside it for Difference. In-scope draws (lower z) fail the
//! depth test exactly where excluded; draws after the restore (higher z)
//! sail over. Restore renders nothing.
//!
//! What to look at:
//! - a grid clipped by a ROTATED rrect ∩ an axis-aligned rect (nesting)
//! - a Difference clip punching a hole
//! - the bar at the bottom drawn AFTER the restores: lands unclipped

use valo::{ClipOp, Color, DisplayListBuilder, FillRule, Paint, PathBuilder, Rect};

fn scene() -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();

    // Rotated rrect clip ∩ nested rect clip over a colorful grid. The
    // transform is undone inline — clips scope to their SAVE, transforms
    // don't shape their lifetime.
    b.save();
    b.translate(200.0, 180.0);
    b.rotate(0.3);
    b.clip_rrect(
        Rect::new(-140.0, -90.0, 280.0, 180.0),
        36.0,
        ClipOp::Intersect,
    );
    b.rotate(-0.3);
    b.translate(-200.0, -180.0);
    b.clip_rect(Rect::new(50.0, 60.0, 300.0, 260.0), ClipOp::Intersect);
    for row in 0..9 {
        for col in 0..11 {
            let color = [
                Color::rgba(0.9, 0.35, 0.35, 0.95),
                Color::rgba(0.95, 0.75, 0.3, 0.95),
                Color::rgba(0.4, 0.65, 1.0, 0.95),
            ][(row + col) % 3];
            b.draw_rect(
                Rect::new(
                    30.0 + col as f32 * 34.0,
                    40.0 + row as f32 * 32.0,
                    28.0,
                    26.0,
                ),
                &Paint::from_color(color),
            );
        }
    }
    b.restore();

    // Difference: solid panel minus a circle.
    b.save();
    let mut hole = PathBuilder::new();
    hole.circle((510.0, 180.0), 60.0);
    b.clip_path(&hole.build(), FillRule::NonZero, ClipOp::Difference);
    b.draw_rect(
        Rect::new(410.0, 90.0, 200.0, 180.0),
        &Paint::from_color(Color::rgb(0.55, 0.4, 0.9)),
    );
    b.restore();

    // Auto-expiry proof: recorded after both restores → higher z than every
    // ceiling → unclipped everywhere.
    b.draw_rect(
        Rect::new(120.0, 400.0, 400.0, 36.0),
        &Paint::from_color(Color::rgba(0.9, 0.9, 0.95, 0.9)),
    );

    b.build()
}

fn main() {
    valo_harness::run_example("clips", [640, 480], Color::rgb(0.07, 0.07, 0.09), |_ctx| {
        scene()
    });
}
