//! Backdrop blur: frosted glass over live content.
//! `cargo run -p valo --example backdrop`
//!
//! `save_layer_backdrop(bounds, paint, Backdrop { sigma: σ, shared_key: key })` opens a layer already full
//! of blurred parent: it breaks the pass, snapshots the region under the
//! panel (+3σ so edge taps read real scene), blurs it AT SCALE (σ>4 renders
//! the blur at reduced resolution — cost stays ~flat in σ), and seeds the
//! layer with it. Children paint onto that glass, and `restore` composites
//! blur + children as one image; a live `clip_rrect` shapes it into a panel.
//!
//! What to look at:
//! - LEFT panel: a lone glass layer (its own pass break + blur chain).
//! - RIGHT pair: two backdrop layers sharing one `key` — ONE blur of their
//!   union region, and the second panel costs zero filter work. Stats:
//!   `backdrops 2 · shared 1`, and only two snapshots for three panels.

use valo::{Backdrop, ClipOp, Color, DisplayListBuilder, Paint, Point, Rect, Shader};

fn busy_background(b: &mut DisplayListBuilder) {
    b.draw_rect(
        Rect::new(0.0, 0.0, 660.0, 480.0),
        &Paint::from_shader(Shader::linear(
            Point::new(0.0, 0.0),
            Point::new(660.0, 480.0),
            Color::rgb(0.12, 0.2, 0.42),
            Color::rgb(0.55, 0.2, 0.4),
        )),
    );
    for i in 0..12 {
        let x = 40.0 + (i % 4) as f32 * 160.0;
        let y = 50.0 + (i / 4) as f32 * 150.0;
        let hue = [
            Color::rgb(0.95, 0.75, 0.3),
            Color::rgb(0.3, 0.85, 0.6),
            Color::rgb(0.4, 0.65, 1.0),
        ][i % 3];
        b.draw_circle((x, y), 34.0, &Paint::from_color(hue));
        b.draw_rect(
            Rect::new(x + 30.0, y + 20.0, 70.0, 14.0),
            &Paint::from_color(Color::rgba(1.0, 1.0, 1.0, 0.85)),
        );
    }
}

fn glass_panel(b: &mut DisplayListBuilder, rect: Rect, sigma: f32, shared: Option<u64>) {
    b.save();
    b.clip_rrect(rect, 18.0, ClipOp::Intersect);
    b.save_layer_backdrop(
        Some(rect),
        &Paint::default(),
        Backdrop {
            sigma,
            shared_key: shared,
        },
    );
    b.restore();
    // The glass tint + a highlight edge, drawn OVER the blurred panel.
    b.draw_rect(rect, &Paint::from_color(Color::rgba(1.0, 1.0, 1.0, 0.14)));
    b.draw_rect(
        Rect::new(rect.x, rect.y, rect.width, 2.0),
        &Paint::from_color(Color::rgba(1.0, 1.0, 1.0, 0.35)),
    );
    b.restore();
}

fn scene() -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    busy_background(&mut b);

    // A lone panel: one break, one blur chain.
    glass_panel(&mut b, Rect::new(50.0, 130.0, 220.0, 220.0), 14.0, None);

    // A shared pair: one blur of the union serves both tiles.
    glass_panel(&mut b, Rect::new(330.0, 60.0, 280.0, 150.0), 20.0, Some(1));
    glass_panel(&mut b, Rect::new(370.0, 280.0, 240.0, 150.0), 20.0, Some(1));

    b.build()
}

fn main() {
    valo_harness::run_example(
        "backdrop",
        [660, 480],
        Color::rgb(0.07, 0.07, 0.09),
        |_ctx| scene(),
    );
}
