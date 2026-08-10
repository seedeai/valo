//! Advanced (dst-reading) blend modes.
//! `cargo run -p valo --example blends` → target/examples/blends.png
//!
//! Multiply/Overlay/…/Luminosity can't be pipeline blend states: they read
//! the destination. Each such draw BREAKS the pass — the target's resolved
//! contents are copied to a snapshot, and one shader computes blend +
//! composite (sampling the snapshot at framebuffer coords), replacing dst.
//!
//! What to look at:
//! - a 7×2 grid of solid squares over a photo-ish gradient background, one
//!   advanced mode each (labels in source order): Multiply, Overlay,
//!   Darken, Lighten, ColorDodge, ColorBurn, HardLight / SoftLight,
//!   Difference, Exclusion, Hue, Saturation, Color, Luminosity
//! - a GRADIENT square with Multiply: gradient sources desugar into an
//!   implicit one-draw layer + texture blend (watch `layers` in stats)
//! - a save_layer composited with Overlay: group blends work the same way
//! - `snapshots N` in the stats = pass breaks this frame

use valo::{BlendMode, Color, DisplayListBuilder, Paint, Point, Rect, Shader};

const MODES: [BlendMode; 13] = [
    BlendMode::Multiply,
    BlendMode::Overlay,
    BlendMode::Darken,
    BlendMode::Lighten,
    BlendMode::ColorDodge,
    BlendMode::ColorBurn,
    BlendMode::HardLight,
    BlendMode::SoftLight,
    BlendMode::Difference,
    BlendMode::Exclusion,
    BlendMode::Hue,
    BlendMode::Saturation,
    BlendMode::Color,
];

fn background(b: &mut DisplayListBuilder) {
    b.draw_rect(
        Rect::new(0.0, 0.0, 660.0, 480.0),
        &Paint::from_shader(Shader::linear(
            Point::new(0.0, 0.0),
            Point::new(660.0, 480.0),
            Color::rgb(0.85, 0.55, 0.35),
            Color::rgb(0.15, 0.35, 0.65),
        )),
    );
}

fn scene() -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    background(&mut b);

    // The advanced grid: one solid square per mode.
    let src = Color::rgb(0.55, 0.75, 0.45);
    for (i, mode) in MODES.into_iter().enumerate() {
        let (col, row) = (i % 7, i / 7);
        b.draw_rect(
            Rect::new(
                30.0 + col as f32 * 90.0,
                40.0 + row as f32 * 90.0,
                74.0,
                74.0,
            ),
            &Paint {
                color: src,
                blend_mode: mode,
                ..Default::default()
            },
        );
    }
    // Luminosity closes the set.
    b.draw_rect(
        Rect::new(30.0 + 6.0 * 90.0, 130.0, 74.0, 74.0),
        &Paint {
            color: src,
            blend_mode: BlendMode::Luminosity,
            ..Default::default()
        },
    );

    // Gradient src + Multiply: desugars to an implicit layer + BlendTexture.
    b.draw_rect(
        Rect::new(30.0, 250.0, 180.0, 120.0),
        &Paint {
            shader: Some(Shader::linear(
                Point::new(30.0, 250.0),
                Point::new(210.0, 370.0),
                Color::WHITE,
                Color::rgb(0.2, 0.2, 0.2),
            )),
            blend_mode: BlendMode::Multiply,
            ..Default::default()
        },
    );

    // A whole GROUP composited with Overlay.
    b.save_layer(
        None,
        &Paint {
            color: Color::rgba(0.0, 0.0, 0.0, 0.9),
            blend_mode: BlendMode::Overlay,
            ..Default::default()
        },
    );
    b.draw_circle(
        (350.0, 320.0),
        60.0,
        &Paint::from_color(Color::rgb(0.9, 0.9, 0.9)),
    );
    b.draw_rect(
        Rect::new(390.0, 280.0, 120.0, 80.0),
        &Paint::from_color(Color::rgb(0.3, 0.3, 0.3)),
    );
    b.restore();

    // Difference over the busy corner: the classic invert-ish look.
    b.draw_circle(
        (580.0, 330.0),
        70.0,
        &Paint {
            color: Color::WHITE,
            blend_mode: BlendMode::Difference,
            ..Default::default()
        },
    );

    b.build()
}

fn main() {
    valo_harness::run_example("blends", [660, 480], Color::rgb(0.07, 0.07, 0.09), |_ctx| {
        scene()
    });
}
