//! Whole-frame benches on a design-canvas scene. TWO readings per scene:
//!
//! * `throughput_*` — submit without waiting: the pipelined cost a real app
//!   pays (present overlaps GPU work). THE number to watch.
//! * `latency_synced` — full GPU drain per frame: scene cost PLUS the
//!   ~1.4ms submit→wake round-trip on Metal. An upper bound on input-to-
//!   photon latency, NOT a throughput measure. (skpbench avoids this tax by
//!   allowing kMaxFrameLag=3 frames in flight; tests/perf_probe.rs breaks
//!   the split down with gpu_ms/plan_ms/encode_ms.)

use std::cell::Cell;
use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use valo::{
    BlendMode, Color, Context, DisplayList, DisplayListBuilder, DrawParagraphExt, Offscreen, Paint,
    PaintStyle, ParagraphBuilder, PathBuilder, Rect, Shader, Stroke, TextStyle,
};

mod bench_fonts;

fn design_canvas(fonts: &mut valo::FontCollection) -> DisplayList {
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 800.0, 600.0),
        &Paint::from_shader(Shader::linear(
            valo::Point::new(0.0, 0.0),
            valo::Point::new(0.0, 600.0),
            Color::rgb(0.13, 0.14, 0.2),
            Color::rgb(0.05, 0.05, 0.09),
        )),
    );
    // Card grid under a group alpha (elision-eligible children).
    b.save_layer(None, &Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.9)));
    for i in 0..12 {
        let (col, row) = (i % 4, i / 4);
        b.draw_rrect(
            Rect::new(
                30.0 + col as f32 * 190.0,
                60.0 + row as f32 * 130.0,
                170.0,
                110.0,
            ),
            14.0,
            &Paint::from_color(Color::rgba(0.2 + col as f32 * 0.1, 0.3, 0.5, 1.0)),
        );
    }
    b.restore();
    // A blurred accent + a stroked path + a headline + frosted glass.
    b.draw_rrect(
        Rect::new(520.0, 420.0, 200.0, 120.0),
        18.0,
        &Paint {
            color: Color::rgba(0.95, 0.6, 0.2, 0.8),
            mask_blur: Some(valo::MaskBlur::new(9.0)),
            ..Default::default()
        },
    );
    let mut wave = PathBuilder::new();
    wave.move_to((30.0, 480.0));
    for k in 1..60 {
        let x = 30.0 + k as f32 * 12.0;
        wave.line_to((x, 480.0 + (k as f32 * 0.6).sin() * 30.0));
    }
    b.draw_path(
        &wave.build(),
        valo::FillRule::NonZero,
        &Paint {
            color: Color::rgb(0.4, 0.85, 0.7),
            style: PaintStyle::Stroke(Stroke::new(5.0)),
            blend_mode: BlendMode::Screen,
            ..Default::default()
        },
    );
    let mut head = ParagraphBuilder::new(fonts);
    head.add_text(
        "Headline over glass",
        &TextStyle::new("Fira Sans", 42.0, Color::WHITE),
    );
    let mut head = head.build();
    head.layout(700.0);
    b.backdrop_blur(Rect::new(20.0, 20.0, 700.0, 70.0), 8.0);
    b.draw_paragraph(&head, (40.0, 28.0));
    b.build()
}

fn frame_benches(c: &mut Criterion) {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP frame benches: no GPU adapter");
        return;
    };
    let mut fonts = bench_fonts::fonts();
    let mut ctx = Context::new(device.clone(), queue.clone());
    let offscreen = Offscreen::new(&device, [800, 600]);
    let dl = design_canvas(&mut fonts);
    let retained = Arc::new(design_canvas(&mut fonts));

    c.bench_function("frame/throughput_unsynced", |b| {
        b.iter(|| ctx.render(&dl, &offscreen.target(Some(Color::BLACK))))
    });
    c.bench_function("frame/latency_synced", |b| {
        b.iter(|| {
            let stats = ctx.render(&dl, &offscreen.target(Some(Color::BLACK)));
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("device poll");
            stats
        })
    });
    // The editor hot path: a RETAINED list under a moving transform — the
    // planner's caches (contours, glyph rasters, pooled targets) should all hit.
    let t = Cell::new(0.0f32);
    c.bench_function("frame/throughput_pan_zoom", |b| {
        b.iter(|| {
            let shift = t.get();
            t.set((shift + 3.7) % 240.0);
            let mut b2 = DisplayListBuilder::new();
            b2.save();
            b2.translate(shift, shift * 0.4);
            b2.scale(1.0 + shift / 900.0, 1.0 + shift / 900.0);
            b2.draw_display_list(&retained);
            b2.restore();
            let frame = b2.build();
            ctx.render(&frame, &offscreen.target(Some(Color::BLACK)))
        })
    });
}

/// The Figma-board limit tests (tests/perf_probe.rs has the full breakdown):
/// fit-the-board renders EVERYTHING (3.3k draws, 16 backdrops → 49 passes);
/// the work view proves record-time culling (123 of 3304 draws survive).
fn board_benches(c: &mut Criterion) {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP board benches: no GPU adapter");
        return;
    };
    let mut fonts = bench_fonts::fonts();
    let mut ctx = Context::new(device.clone(), queue.clone());
    let offscreen = Offscreen::new(&device, [1600, 1000]);
    let board = Arc::new(valo_harness::scenes::figma_board(&mut fonts));
    let view = |zoom: f32, cx: f32, cy: f32| {
        let mut b = DisplayListBuilder::new();
        b.save();
        b.translate(800.0 - cx * zoom, 500.0 - cy * zoom);
        b.scale(zoom, zoom);
        b.draw_display_list(&board);
        b.restore();
        b.build()
    };
    let fit = view(0.16, 4800.0, 2700.0);
    c.bench_function("board/fit_everything_visible", |b| {
        b.iter(|| ctx.render(&fit, &offscreen.target(Some(Color::BLACK))))
    });
    let work = view(1.0, 1200.0, 700.0);
    c.bench_function("board/work_view_culled", |b| {
        b.iter(|| ctx.render(&work, &offscreen.target(Some(Color::BLACK))))
    });
}

criterion_group!(benches, frame_benches, board_benches);
criterion_main!(benches);
