//! The windowed flush hot path in a loop, for sampling profilers — the
//! benchmark frame (30 cards + stroked rings + cached labels, 800×600)
//! planned, encoded, rendered, and GPU-waited per frame, nothing else.
//!
//! ```sh
//! CARGO_PROFILE_RELEASE_DEBUG=true cargo build --release -p valo-harness --example frame_profile
//! samply record target/release/examples/frame_profile 8
//! ```

use valo::DrawParagraphExt;

fn main() {
    let seconds: f32 = std::env::args()
        .nth(1)
        .and_then(|argument| argument.parse().ok())
        .unwrap_or(8.0);
    let (device, queue) = valo_harness::headless_device().expect("a GPU adapter");
    let mut context = valo::Context::new(device.clone(), queue);

    let mut collection = valo::FontCollection::new();
    let fira = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/fonts/fira_sans.ttf"
    ))
    .expect("fira_sans.ttf");
    let face = collection.register("Fira Sans", fira).expect("fira parses");
    collection.add_fallback(face);
    let mut fonts = collection;

    let labels: Vec<valo::Paragraph> = (0..30)
        .map(|index| {
            let mut builder = valo::ParagraphBuilder::new(&mut fonts);
            let style = valo::TextStyle::new("Fira Sans", 13.0, valo::Color::rgb(0.91, 0.91, 0.94));
            builder.add_text(&format!("icon-label-{index}"), &style);
            let mut paragraph = builder.build();
            paragraph.layout(120.0);
            paragraph
        })
        .collect();

    let offscreen = valo::Offscreen::new(&device, [800, 600]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f32(seconds);
    let mut frames = 0u32;
    let (mut record_seconds, mut render_seconds, mut poll_seconds) = (0.0, 0.0, 0.0);
    let mut stats = valo::RenderStats::default();
    while std::time::Instant::now() < deadline {
        let start = std::time::Instant::now();
        let list = record(&labels);
        let recorded = std::time::Instant::now();
        stats = context.render(
            &list,
            &offscreen.target(Some(valo::Color::rgb(0.06, 0.06, 0.08))),
        );
        let rendered = std::time::Instant::now();
        // Third argument: "no-poll" never polls (the leak probe),
        // "poll" polls non-blocking (frame-loop hygiene), default waits.
        match std::env::args().nth(3).as_deref() {
            Some("no-poll") => {}
            Some("poll") => {
                let _ = device.poll(wgpu::PollType::Poll);
            }
            _ => {
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
            }
        }
        poll_seconds += rendered.elapsed().as_secs_f64();
        record_seconds += (recorded - start).as_secs_f64();
        render_seconds += (rendered - recorded).as_secs_f64();
        frames += 1;
        // Fourth argument "vsync" paces like a real 120Hz app — memory
        // probes at unthrottled rates drown in in-flight staging.
        if std::env::args().nth(4).as_deref() == Some("vsync") {
            std::thread::sleep(std::time::Duration::from_millis(7));
        }
    }
    let per_frame = 1000.0 / frames as f64;
    println!("{frames} frames");
    println!(
        "record {:.3} ms · render(cpu encode+submit) {:.3} ms · poll(gpu+wait) {:.3} ms",
        record_seconds * per_frame,
        render_seconds * per_frame,
        poll_seconds * per_frame
    );
    println!("{stats:?}");
    println!("{:?}", context.memory_report());
}

fn record(labels: &[valo::Paragraph]) -> valo::DisplayList {
    // Second argument: card count (default 30), or "layers" — the cards
    // wrapped in three big translucent save layers, the scroll-page shape
    // that fills the target pool with MSAA layer textures.
    let mode = std::env::args().nth(2);
    let layered = mode.as_deref() == Some("layers");
    let cards: usize = if layered {
        30
    } else {
        mode.and_then(|argument| argument.parse().ok())
            .unwrap_or(30)
    };
    let mut builder = valo::DisplayListBuilder::new();
    let group_paint = valo::Paint {
        color: valo::Color::rgba(1.0, 1.0, 1.0, 0.5),
        ..Default::default()
    };
    // Cycle the labels so a card count above the label count still profiles.
    for (index, label) in labels.iter().cycle().take(cards).enumerate() {
        if layered && index % 10 == 0 {
            if index > 0 {
                builder.restore();
            }
            builder.save_layer(None, &group_paint);
        }
        let x = (index % 6) as f32 * 130.0 + 10.0;
        let y = (index / 6) as f32 * 110.0 + 10.0;
        builder.draw_rrect_radii_elliptical(
            valo::Rect::new(x, y, 120.0, 96.0),
            [[8.0; 2]; 4],
            &valo::Paint::from_color(valo::Color::rgb(0.118, 0.118, 0.157)),
        );
        builder.draw_circle(
            (x + 60.0, y + 36.0),
            22.0,
            &valo::Paint {
                color: valo::Color::rgb(0.227, 0.227, 0.29),
                style: valo::PaintStyle::Stroke(valo::Stroke {
                    width: 1.0,
                    cap: valo::Cap::Butt,
                    join: valo::Join::Miter,
                    miter_limit: 4.0,
                    dash: None,
                }),
                ..Default::default()
            },
        );
        builder.draw_paragraph(label, (x + 12.0, y + 68.0));
    }
    if layered {
        builder.restore();
    }
    builder.build()
}
