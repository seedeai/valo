//! The smallest useful valo program: record three shapes, render to a PNG.
//! `cargo run -p valo --example hello` → target/examples/hello.png
//!
//! Recording needs no GPU and no `Context` — `scene()` below is pure CPU and
//! could run on any thread. The harness supplies the headless device and
//! writes the result out; a real host would render into its own surface.

use valo::{Color, DisplayListBuilder, Paint, Rect};

fn scene() -> valo::DisplayList {
    let mut builder = DisplayListBuilder::new();

    builder.draw_rrect_radii(
        Rect::new(40.0, 40.0, 400.0, 240.0),
        [24.0; 4],
        &Paint::from_color(Color::rgb(0.13, 0.15, 0.20)),
    );
    builder.draw_rect(
        Rect::new(80.0, 80.0, 160.0, 60.0),
        &Paint::from_color(Color::rgb(0.96, 0.35, 0.25)),
    );
    builder.draw_circle(
        (330.0, 180.0),
        70.0,
        &Paint::from_color(Color::rgba(0.30, 0.75, 0.95, 0.85)),
    );

    builder.build()
}

fn main() {
    valo_harness::run_example("hello", [480, 320], Color::WHITE, |_context| scene());
}
