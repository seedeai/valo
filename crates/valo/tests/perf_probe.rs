//! Manual perf breakdown for the bench scene — run with:
//!   cargo test -p valo --test perf_probe --release -- --ignored --nocapture

use std::sync::Arc;

use valo::{
    BlendMode, Color, Context, DisplayList, DisplayListBuilder, DrawParagraphExt, FontCollection,
    MaskBlur, Offscreen, Paint, PaintStyle, ParagraphBuilder, PathBuilder, Point, Rect, Shader,
    Stroke, TextStyle,
};

fn fonts() -> Arc<FontCollection> {
    valo_harness::example_fonts()
}

fn design_canvas(fonts: &Arc<FontCollection>) -> DisplayList {
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 800.0, 600.0),
        &Paint::from_shader(Shader::linear(
            Point::new(0.0, 0.0),
            Point::new(0.0, 600.0),
            Color::rgb(0.13, 0.14, 0.2),
            Color::rgb(0.05, 0.05, 0.09),
        )),
    );
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
    b.draw_rrect(
        Rect::new(520.0, 420.0, 200.0, 120.0),
        18.0,
        &Paint {
            color: Color::rgba(0.95, 0.6, 0.2, 0.8),
            mask_blur: Some(MaskBlur::new(9.0)),
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

fn pan_frame(retained: &Arc<DisplayList>, shift: f32) -> DisplayList {
    let mut b = DisplayListBuilder::new();
    b.save();
    b.translate(shift, shift * 0.4);
    b.scale(1.0 + shift / 900.0, 1.0 + shift / 900.0);
    b.draw_display_list(retained);
    b.restore();
    b.build()
}

struct Avg {
    wall_ms: f64,
    gpu_ms: f64,
    plan_ms: f64,
    encode_ms: f64,
    cpu_ms: f64,
    draw_calls: f64,
    passes: f64,
    filter_passes: f64,
    snapshots: f64,
}

fn run(
    ctx: &mut Context,
    device: &wgpu::Device,
    offscreen: &Offscreen,
    frames: usize,
    mut scene: impl FnMut(usize) -> DisplayList,
    sync: bool,
) -> Avg {
    let mut a = Avg {
        wall_ms: 0.0,
        gpu_ms: 0.0,
        plan_ms: 0.0,
        encode_ms: 0.0,
        cpu_ms: 0.0,
        draw_calls: 0.0,
        passes: 0.0,
        filter_passes: 0.0,
        snapshots: 0.0,
    };
    let mut gpu_samples = 0.0f64;
    for i in 0..frames {
        let dl = scene(i);
        let t = std::time::Instant::now();
        let stats = ctx.render(&dl, &offscreen.target(Some(Color::BLACK)));
        if sync {
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll");
        }
        a.wall_ms += t.elapsed().as_secs_f64() * 1000.0;
        if stats.gpu_ms > 0.0 {
            a.gpu_ms += stats.gpu_ms as f64;
            gpu_samples += 1.0;
        }
        a.plan_ms += stats.plan_ms as f64;
        a.encode_ms += stats.encode_ms as f64;
        a.cpu_ms += stats.cpu_ms as f64;
        a.draw_calls += stats.draw_calls as f64;
        a.passes += stats.render_passes as f64;
        a.filter_passes += stats.filter_passes as f64;
        a.snapshots += stats.snapshots as f64;
    }
    let n = frames as f64;
    a.wall_ms /= n;
    a.gpu_ms /= gpu_samples.max(1.0);
    a.plan_ms /= n;
    a.encode_ms /= n;
    a.cpu_ms /= n;
    a.draw_calls /= n;
    a.passes /= n;
    a.filter_passes /= n;
    a.snapshots /= n;
    a
}

fn report(label: &str, a: &Avg) {
    println!(
        "{label:26} wall {:6.3}ms  gpu {:6.3}ms  cpu {:6.3}ms (plan {:5.3} + encode {:5.3})  calls {:5.1}  passes {:4.1}  filters {:4.1}  snapshots {:3.1}",
        a.wall_ms, a.gpu_ms, a.cpu_ms, a.plan_ms, a.encode_ms, a.draw_calls, a.passes, a.filter_passes, a.snapshots
    );
}

#[test]
#[ignore = "manual perf probe"]
fn pan_zoom_breakdown() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let fonts = fonts();
    let mut ctx = Context::new(device.clone(), queue.clone());
    ctx.set_fonts(fonts.clone());
    let offscreen = Offscreen::new(&device, [800, 600]);
    let retained = Arc::new(design_canvas(&fonts));

    // Warm everything.
    for i in 0..30 {
        ctx.render(
            &pan_frame(&retained, (i as f32 * 3.7) % 240.0),
            &offscreen.target(Some(Color::BLACK)),
        );
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");

    // 1. The floor: a one-rect frame, GPU-synced — pure submit+wait latency.
    let tiny = |_: usize| {
        let mut b = DisplayListBuilder::new();
        b.draw_rect(
            Rect::new(0.0, 0.0, 50.0, 50.0),
            &Paint::from_color(Color::WHITE),
        );
        b.build()
    };
    report(
        "tiny synced",
        &run(&mut ctx, &device, &offscreen, 200, tiny, true),
    );

    // 2. Static canvas (same list every frame), synced vs not.
    let stat = |_: usize| pan_frame(&retained, 60.0);
    report(
        "canvas static synced",
        &run(&mut ctx, &device, &offscreen, 200, stat, true),
    );
    let stat = |_: usize| pan_frame(&retained, 60.0);
    report(
        "canvas static unsynced",
        &run(&mut ctx, &device, &offscreen, 200, stat, false),
    );

    // 3. Pan WITHOUT zoom (translation only — glyph rasters stay cached).
    let pan = |i: usize| {
        let mut b = DisplayListBuilder::new();
        b.save();
        b.translate((i as f32 * 3.7) % 240.0, 1.0);
        b.draw_display_list(&retained);
        b.restore();
        b.build()
    };
    report(
        "pan only synced",
        &run(&mut ctx, &device, &offscreen, 200, pan, true),
    );

    // 4. The bench scene: pan + ZOOM (glyphs re-quantize).
    let before = ctx.memory_report();
    let zoom = |i: usize| pan_frame(&retained, (i as f32 * 3.7) % 240.0);
    report(
        "pan+zoom synced",
        &run(&mut ctx, &device, &offscreen, 200, zoom, true),
    );
    let after = ctx.memory_report();
    println!(
        "atlas mask entries {} -> {} (+{} rasterizations over 200 zooming frames)",
        before.atlas[0].entries,
        after.atlas[0].entries,
        after.atlas[0]
            .entries
            .saturating_sub(before.atlas[0].entries),
    );
}

/// The Figma-board limit test: ~100 frames / ~3k shapes / ~700 text runs /
/// ~100 shadows over a 9600×5400 world, from fit-the-board to 12× detail.
#[test]
#[ignore = "manual perf probe"]
fn figma_board_breakdown() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let fonts = fonts();
    let mut ctx = Context::new(device.clone(), queue.clone());
    ctx.set_fonts(fonts.clone());
    let offscreen = Offscreen::new(&device, [1600, 1000]);
    let board = Arc::new(valo_harness::scenes::figma_board(&fonts));
    println!(
        "board: {} draws recorded, {} depth slots",
        board.draw_count(),
        board.depth_slots()
    );

    let camera = |zoom: f32, cx: f32, cy: f32| {
        let board = board.clone();
        move |_: usize| {
            let mut b = DisplayListBuilder::new();
            b.save();
            b.translate(800.0 - cx * zoom, 500.0 - cy * zoom);
            b.scale(zoom, zoom);
            b.draw_display_list(&board);
            b.restore();
            b.build()
        }
    };
    let views: [(&str, f32, f32, f32); 5] = [
        ("fit board (0.16x)", 0.16, 4800.0, 2700.0),
        ("overview (0.5x)", 0.5, 2400.0, 1400.0),
        ("work (1x)", 1.0, 1200.0, 700.0),
        ("detail (4x)", 4.0, 700.0, 500.0),
        ("extreme (12x)", 12.0, 640.0, 420.0),
    ];
    for (label, zoom, cx, cy) in views {
        let scene = camera(zoom, cx, cy);
        for i in 0..20 {
            ctx.render(&scene(i), &offscreen.target(Some(Color::BLACK)));
        }
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let one = ctx.render(&scene(0), &offscreen.target(Some(Color::BLACK)));
        let a = run(&mut ctx, &device, &offscreen, 60, scene, false);
        println!(
            "  draws {:5}  culled {:5}  layers {:2}  backdrops {:2}",
            one.draws, one.culled, one.layers_rendered, one.backdrops
        );
        report(label, &a);
    }

    // Continuous zoom sweep across the whole range: re-raster churn.
    let board2 = board.clone();
    let sweep = move |i: usize| {
        let z = 0.16 + (i % 80) as f32 * 0.148; // 0.16 → 12 in 80 steps
        let mut b = DisplayListBuilder::new();
        b.save();
        b.translate(800.0 - 700.0 * z, 500.0 - 500.0 * z);
        b.scale(z, z);
        b.draw_display_list(&board2);
        b.restore();
        b.build()
    };
    let before = ctx.memory_report();
    report(
        "zoom sweep 0.16→12",
        &run(&mut ctx, &device, &offscreen, 160, sweep, false),
    );
    let after = ctx.memory_report();
    println!(
        "atlas: mask {}p/{}e -> {}p/{}e   color {}p -> {}p   contours {} -> {}   total {:.1}MB -> {:.1}MB",
        before.atlas[0].pages, before.atlas[0].entries,
        after.atlas[0].pages, after.atlas[0].entries,
        before.atlas[1].pages, after.atlas[1].pages,
        before.contours.count, after.contours.count,
        before.total_bytes() as f64 / 1e6,
        after.total_bytes() as f64 / 1e6,
    );
}
