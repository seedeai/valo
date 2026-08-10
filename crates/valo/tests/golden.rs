//! Golden pixel tests — the browser-free proof that rendering works. Runs on a
//! headless native device; skips (with a note) when the machine has no adapter.
//! `VALO_BLESS=1 cargo test` regenerates the checked-in PNGs.

use std::path::Path;
use std::sync::Arc;

use valo::{BlendMode, Color, Context, DisplayListBuilder, MaskBlur, Offscreen, Paint, Rect};

fn goldens_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens"))
}

/// Deterministic M1 scene: alpha stacking, transforms, pipeline blends, nesting,
/// and one culled rect (asserted via stats, invisible in pixels).
fn m1_scene() -> valo::DisplayList {
    let card = {
        let mut b = DisplayListBuilder::new();
        b.draw_rect(
            Rect::new(0.0, 0.0, 120.0, 90.0),
            &Paint::from_color(Color::rgb(0.16, 0.17, 0.22)),
        );
        b.draw_rect(
            Rect::new(10.0, 10.0, 100.0, 20.0),
            &Paint::from_color(Color::rgb(0.35, 0.55, 1.0)),
        );
        Arc::new(b.build())
    };

    let mut b = DisplayListBuilder::new();
    for i in 0..5 {
        b.draw_rect(
            Rect::new(40.0 + i as f32 * 45.0, 40.0, 80.0, 80.0),
            &Paint::from_color(Color::rgba(0.9, 0.25, 0.3, 0.55)),
        );
    }
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
    b.save();
    b.translate(60.0, 300.0);
    b.draw_display_list(&card);
    b.translate(160.0, 20.0);
    b.scale(1.4, 1.4);
    b.draw_display_list(&card);
    b.restore();
    b.draw_rect(
        Rect::new(5000.0, 5000.0, 100.0, 100.0),
        &Paint::from_color(Color::BLACK),
    );
    b.build()
}

#[test]
fn m1_rects_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m1_rects_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [640u32, 480u32];
    let offscreen = Offscreen::new(&device, size);

    let dl = m1_scene();
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));

    // The oracle culled the off-viewport rect on the CPU.
    assert_eq!(
        stats.culled, 1,
        "expected exactly the off-viewport rect culled"
    );
    assert_eq!(stats.draws, dl.draw_count() - 1);
    assert_eq!(stats.snapshots, 0, "no advanced modes in this scene");

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m1_rects", size, &rgba);

    // The HostBuffer ring warms one arena per frame for FRAMES(3) frames;
    // after that, steady rendering must create nothing.
    for _ in 0..2 {
        ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));
    }
    let warm = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));
    assert_eq!(warm.blocks_created, 0, "warm frames create no GPU buffers");
}

// ── M2: stencil-then-cover paths + depth clips + MSAA ──────────────────────

/// A five-point star centered at `c` — SELF-INTERSECTING on purpose: the
/// litmus shape where NonZero and EvenOdd genuinely differ (StC handles both
/// with zero CPU tessellation).
fn star(c: valo::Point, r: f32) -> std::sync::Arc<valo::Path> {
    let mut p = valo::PathBuilder::new();
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

fn m2_scene() -> valo::DisplayList {
    use valo::FillRule;
    let mut b = DisplayListBuilder::new();
    let teal = Paint::from_color(Color::rgba(0.2, 0.8, 0.55, 0.9));

    // Self-intersecting star, both fill rules: NonZero fills the core,
    // EvenOdd leaves a pentagonal hole.
    b.draw_path(
        &star(valo::Point::new(110.0, 110.0), 85.0),
        FillRule::NonZero,
        &teal,
    );
    b.draw_path(
        &star(valo::Point::new(300.0, 110.0), 85.0),
        FillRule::EvenOdd,
        &teal,
    );

    // Curves: circle + rrect through the same StC path (no special cases yet).
    b.draw_circle(
        (440.0, 110.0),
        70.0,
        &Paint::from_color(Color::rgba(0.9, 0.4, 0.3, 0.8)),
    );
    b.draw_rrect(
        Rect::new(520.0, 50.0, 110.0, 120.0),
        24.0,
        &Paint::from_color(Color::rgb(0.35, 0.55, 1.0)),
    );

    // A ROTATED rrect clip over a grid of rects — the depth ceiling excludes
    // exactly the exterior, at MSAA edge quality. The transform is undone
    // inline (clips scope to their save, transforms don't shape their life).
    b.save();
    b.translate(160.0, 320.0);
    b.rotate(0.3);
    b.clip_rrect(
        Rect::new(-110.0, -70.0, 220.0, 140.0),
        30.0,
        valo::ClipOp::Intersect,
    );
    b.rotate(-0.3);
    b.translate(-160.0, -320.0);
    b.clip_rect(
        Rect::new(40.0, 240.0, 250.0, 220.0),
        valo::ClipOp::Intersect,
    ); // nested
    for row in 0..6 {
        for col in 0..8 {
            let hue = (row + col) % 3;
            let color = [
                Color::rgba(0.9, 0.35, 0.35, 0.95),
                Color::rgba(0.95, 0.75, 0.3, 0.95),
                Color::rgba(0.4, 0.65, 1.0, 0.95),
            ][hue];
            b.draw_rect(
                Rect::new(
                    30.0 + col as f32 * 36.0,
                    230.0 + row as f32 * 34.0,
                    30.0,
                    28.0,
                ),
                &Paint::from_color(color),
            );
        }
    }
    b.restore();

    // Difference clip: punch a circle out of a solid panel.
    b.save();
    let mut hole = valo::PathBuilder::new();
    hole.circle((470.0, 330.0), 55.0);
    b.clip_path(&hole.build(), FillRule::NonZero, valo::ClipOp::Difference);
    b.draw_rect(
        Rect::new(380.0, 250.0, 180.0, 160.0),
        &Paint::from_color(Color::rgb(0.55, 0.4, 0.9)),
    );
    b.restore();

    // Expiry proof: this draw comes AFTER both restores — it must land
    // unclipped over everything (slot > every ceiling).
    b.draw_rect(
        Rect::new(240.0, 420.0, 220.0, 30.0),
        &Paint::from_color(Color::rgba(0.9, 0.9, 0.95, 0.9)),
    );

    b.build()
}

#[test]
fn m2_paths_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m2_paths_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [660u32, 480u32];
    let offscreen = Offscreen::new(&device, size);

    let dl = m2_scene();
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));
    assert_eq!(
        stats.clips, 3,
        "rotated rrect + nested rect + difference circle"
    );
    assert_eq!(stats.draws, dl.draw_count());
    assert_eq!(stats.culled, 0);

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m2_paths", size, &rgba);
}

// ── M3: images (upload, mips, sampling, tiling) + gradients ─────────────────

fn checker_pixels(size: u32, cell: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let on = ((x / cell) + (y / cell)).is_multiple_of(2);
            px.extend_from_slice(if on {
                &[235, 235, 240, 255]
            } else {
                &[40, 45, 60, 255]
            });
        }
    }
    px
}

fn m3_images_scene(ctx: &mut Context) -> valo::DisplayList {
    use valo::{Filter, ImageDesc, Sampling, TileMode};
    let small = ctx.upload_image(
        ImageDesc {
            size: [64, 64],
            premultiplied: true,
            mips: true,
        },
        &checker_pixels(64, 8),
    );
    let busy_mips = ctx.upload_image(
        ImageDesc {
            size: [256, 256],
            premultiplied: true,
            mips: true,
        },
        &checker_pixels(256, 2),
    );
    let busy_flat = ctx.upload_image(
        ImageDesc {
            size: [256, 256],
            premultiplied: true,
            mips: false,
        },
        &checker_pixels(256, 2),
    );

    let mut b = DisplayListBuilder::new();
    let paint = Paint::default();
    let nearest = Sampling {
        filter: Filter::Nearest,
        ..Default::default()
    };
    b.draw_image_rect(
        &small,
        Rect::new(0.0, 0.0, 64.0, 64.0),
        Rect::new(40.0, 40.0, 160.0, 160.0),
        nearest,
        &paint,
    );
    b.draw_image(&small, Rect::new(230.0, 40.0, 160.0, 160.0), &paint);
    // Fractional downscale (256→72): flat bilinear aliases, mips average.
    b.draw_image(&busy_mips, Rect::new(440.0, 40.0, 72.0, 72.0), &paint);
    b.draw_image(&busy_flat, Rect::new(530.0, 40.0, 72.0, 72.0), &paint);
    let repeat = Sampling {
        tile_x: TileMode::Repeat,
        tile_y: TileMode::Repeat,
        ..Default::default()
    };
    let mirror = Sampling {
        tile_x: TileMode::Mirror,
        tile_y: TileMode::Mirror,
        ..Default::default()
    };
    b.draw_image_rect(
        &small,
        Rect::new(0.0, 0.0, 192.0, 128.0),
        Rect::new(40.0, 250.0, 240.0, 160.0),
        repeat,
        &paint,
    );
    b.draw_image_rect(
        &small,
        Rect::new(0.0, 0.0, 192.0, 128.0),
        Rect::new(320.0, 250.0, 240.0, 160.0),
        mirror,
        &paint,
    );
    b.draw_image(
        &small,
        Rect::new(440.0, 130.0, 64.0, 64.0),
        &Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.5)),
    );
    b.build()
}

#[test]
fn m3_images_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m3_images_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [640u32, 480u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m3_images_scene(&mut ctx);
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));
    assert_eq!(stats.draws, 7);
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m3_images", size, &rgba);
}

fn m3_gradients_scene() -> valo::DisplayList {
    use valo::{GradientStop, Point, Shader};
    let stops = |list: &[(f32, Color)]| {
        list.iter()
            .map(|&(offset, color)| GradientStop { offset, color })
            .collect::<Vec<_>>()
    };
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(40.0, 40.0, 560.0, 70.0),
        &Paint::from_shader(Shader::Linear {
            start: Point::new(40.0, 0.0),
            end: Point::new(600.0, 0.0),
            stops: stops(&[
                (0.0, Color::rgb(0.9, 0.25, 0.3)),
                (0.3, Color::rgb(0.95, 0.75, 0.3)),
                (0.5, Color::rgb(0.2, 0.8, 0.55)),
                (0.7, Color::rgb(0.35, 0.55, 1.0)),
                (1.0, Color::rgb(0.7, 0.4, 0.9)),
            ]),
            spread: Default::default(),
            local: Default::default(),
        }),
    );
    b.draw_rect(
        Rect::new(40.0, 150.0, 180.0, 180.0),
        &Paint::from_shader(Shader::Radial {
            center: Point::new(130.0, 240.0),
            radius: 90.0,
            stops: stops(&[
                (0.0, Color::WHITE),
                (0.7, Color::rgb(0.35, 0.55, 1.0)),
                (1.0, Color::rgb(0.1, 0.12, 0.25)),
            ]),
            spread: Default::default(),
            focus: None,
            local: Default::default(),
        }),
    );
    b.draw_rect(
        Rect::new(250.0, 150.0, 180.0, 180.0),
        &Paint::from_shader(Shader::Sweep {
            center: Point::new(340.0, 240.0),
            start_angle: 0.0,
            stops: stops(&[
                (0.0, Color::rgb(0.9, 0.25, 0.3)),
                (0.5, Color::rgb(0.35, 0.55, 1.0)),
                (1.0, Color::rgb(0.9, 0.25, 0.3)),
            ]),
            local: Default::default(),
        }),
    );
    // Gradient through StC: the fragment family is orthogonal to geometry.
    b.draw_path(
        &star(valo::Point::new(540.0, 240.0), 90.0),
        valo::FillRule::EvenOdd,
        &Paint::from_shader(Shader::linear(
            Point::new(450.0, 150.0),
            Point::new(630.0, 330.0),
            Color::rgb(0.95, 0.75, 0.3),
            Color::rgb(0.9, 0.25, 0.3),
        )),
    );
    let mut faded = Paint::from_shader(Shader::linear(
        Point::new(40.0, 0.0),
        Point::new(600.0, 0.0),
        Color::rgb(0.2, 0.8, 0.55),
        Color::rgb(0.35, 0.55, 1.0),
    ));
    faded.color = Color::rgba(1.0, 1.0, 1.0, 0.35);
    b.draw_rect(Rect::new(40.0, 380.0, 560.0, 60.0), &faded);
    b.build()
}

#[test]
fn m3_gradients_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m3_gradients_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [660u32, 480u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m3_gradients_scene();
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));
    assert_eq!(stats.draws, 5);
    assert_eq!(stats.snapshots, 0, "no advanced modes in this scene");
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m3_gradients", size, &rgba);
}

/// F1: repeat/reflect spreads on linear + radial. One short
/// gradient span per rect so several tiles are visible; pad rides along
/// as the control row.
fn f1_spreads_scene() -> valo::DisplayList {
    use valo::{GradientStop, Point, Shader, SpreadMode};
    let stops = vec![
        GradientStop {
            offset: 0.0,
            color: Color::rgb(0.9, 0.25, 0.3),
        },
        GradientStop {
            offset: 1.0,
            color: Color::rgb(0.2, 0.4, 1.0),
        },
    ];
    let mut b = DisplayListBuilder::new();
    for (i, spread) in [SpreadMode::Pad, SpreadMode::Repeat, SpreadMode::Reflect]
        .into_iter()
        .enumerate()
    {
        let y = 40.0 + i as f32 * 90.0;
        b.draw_rect(
            Rect::new(40.0, y, 280.0, 60.0),
            &Paint::from_shader(Shader::Linear {
                start: Point::new(60.0, 0.0),
                end: Point::new(120.0, 0.0),
                stops: stops.clone(),
                spread,
                local: Default::default(),
            }),
        );
        b.draw_rect(
            Rect::new(360.0, y, 240.0, 60.0),
            &Paint::from_shader(Shader::Radial {
                center: Point::new(480.0, y + 30.0),
                radius: 30.0,
                stops: stops.clone(),
                spread,
                focus: None,
                local: Default::default(),
            }),
        );
    }
    b.build()
}

#[test]
fn f1_gradient_spreads_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP f1_gradient_spreads_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [640u32, 340u32];
    let offscreen = Offscreen::new(&device, size);
    let stats = ctx.render(
        &f1_spreads_scene(),
        &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))),
    );
    assert_eq!(stats.draws, 6);
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "f1_gradient_spreads", size, &rgba);
}

/// F2: focal radials — the highlight sits at the focus, stop
/// rings emanate from it, and spread modes compose with the focal solve.
fn f2_focal_scene() -> valo::DisplayList {
    use valo::{GradientStop, Point, Shader, SpreadMode};
    let stops = vec![
        GradientStop {
            offset: 0.0,
            color: Color::WHITE,
        },
        GradientStop {
            offset: 1.0,
            color: Color::rgb(0.2, 0.25, 0.9),
        },
    ];
    let mut b = DisplayListBuilder::new();
    for (i, (spread, focal)) in [
        (SpreadMode::Pad, false),
        (SpreadMode::Pad, true),
        (SpreadMode::Reflect, true),
    ]
    .into_iter()
    .enumerate()
    {
        let x = 40.0 + i as f32 * 190.0;
        let center = Point::new(x + 85.0, 125.0);
        // Focus offset toward the upper-left, well inside r=70.
        let focus =
            focal.then(|| valo::FocalCircle::point(Point::new(center.x - 35.0, center.y - 35.0)));
        b.draw_rect(
            Rect::new(x, 40.0, 170.0, 170.0),
            &Paint::from_shader(Shader::Radial {
                center,
                radius: 70.0,
                stops: stops.clone(),
                spread,
                focus,
                local: Default::default(),
            }),
        );
    }
    b.build()
}

#[test]
fn f2_focal_radials_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP f2_focal_radials_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [620u32, 250u32];
    let offscreen = Offscreen::new(&device, size);
    let stats = ctx.render(
        &f2_focal_scene(),
        &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))),
    );
    assert_eq!(stats.draws, 3);
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "f2_focal_radials", size, &rgba);
}

/// Two-point conical gradients — Canvas2D's `createRadialGradient` in full,
/// with a start circle that has its own radius. The four panels are the
/// cases Skia's decomposition splits on: a sphere highlight (small start
/// circle inside the end circle), equal radii (the degenerate "strip"),
/// a shrinking gradient (start circle larger than end), and a start circle
/// sitting exactly on the end circle's rim.
fn conical_scene() -> valo::DisplayList {
    use valo::{FocalCircle, GradientStop, Point, Shader, SpreadMode};
    let stops = vec![
        GradientStop {
            offset: 0.0,
            color: Color::WHITE,
        },
        GradientStop {
            offset: 1.0,
            color: Color::rgb(0.15, 0.2, 0.85),
        },
    ];
    let mut b = DisplayListBuilder::new();
    for (index, (start_offset, start_radius, end_radius)) in [
        ((-28.0f32, -28.0f32), 8.0f32, 70.0f32), // sphere highlight
        ((40.0, 0.0), 45.0, 45.0),               // equal radii: the strip case
        ((25.0, 0.0), 70.0, 22.0),               // shrinking outward
        ((70.0, 0.0), 6.0, 70.0),                // start circle on the rim
    ]
    .into_iter()
    .enumerate()
    {
        let x = 40.0 + index as f32 * 190.0;
        let center = Point::new(x + 85.0, 125.0);
        b.draw_rect(
            Rect::new(x, 40.0, 170.0, 170.0),
            &Paint::from_shader(Shader::Radial {
                center,
                radius: end_radius,
                stops: stops.clone(),
                spread: SpreadMode::Pad,
                focus: Some(FocalCircle {
                    center: Point::new(center.x + start_offset.0, center.y + start_offset.1),
                    radius: start_radius,
                }),
                local: Default::default(),
            }),
        );
    }
    b.build()
}

#[test]
fn conical_gradients_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP conical_gradients_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [800u32, 250u32];
    let offscreen = Offscreen::new(&device, size);
    let background = Color::rgb(0.07, 0.07, 0.09);
    let stats = ctx.render(&conical_scene(), &offscreen.target(Some(background)));
    assert_eq!(stats.draws, 4);
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);

    let at = |x: u32, y: u32| {
        let i = ((y * size[0] + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    };
    // The start circle's own interior is the ramp's 0 end: white, not the
    // single colour a radius-0 focal point would have produced.
    let highlight = at(40 + 85 - 28, 125 - 28);
    assert!(
        highlight.iter().all(|c| *c > 230),
        "sphere highlight should be at the white end of the ramp, got {highlight:?}"
    );
    // Equal radii degenerate to a STRIP: only the band between the two
    // circles' common tangents is covered, and everything above or below it
    // stays transparent. A radius-0 focal solve cannot express that — it
    // covers the whole plane — so this is the case that proves the general
    // algorithm is live. The strip is 45 tall about y = 125; probe well
    // outside it.
    let outside = at(230 + 85, 45);
    let background_bytes = [
        (background.r * 255.0).round() as u8,
        (background.g * 255.0).round() as u8,
        (background.b * 255.0).round() as u8,
    ];
    assert!(
        outside
            .iter()
            .zip(background_bytes)
            .all(|(got, want)| got.abs_diff(want) <= 2),
        "outside the cone should stay background, got {outside:?}"
    );

    valo_harness::assert_golden(goldens_dir(), "conical_gradients", size, &rgba);
}

/// Arcs and stroked text — the path primitives Canvas2D leans on. The pie
/// slice and ring come from `arc`, the tab's shoulders from `arc_to`, the
/// tilted oval from `ellipse`; the word below is drawn twice, once filled
/// and once stroked, and stroking forces the outline tier because atlas
/// rasters only ever carry fill coverage.
#[test]
fn arcs_and_stroked_text_golden() {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    use valo::{
        Cap, DrawGlyphRunExt, Join, PaintStyle, ParagraphBuilder, PathBuilder, Point, Stroke,
        TextStyle,
    };

    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP arcs_and_stroked_text_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let fonts = text_fonts();
    ctx.set_fonts(fonts.clone());
    let size = [660u32, 340u32];
    let offscreen = Offscreen::new(&device, size);
    let background = Color::rgb(0.07, 0.07, 0.09);

    let stroke_of = |width: f32| {
        PaintStyle::Stroke(Stroke {
            width,
            cap: Cap::Round,
            join: Join::Round,
            miter_limit: 4.0,
            dash: None,
        })
    };

    let mut b = DisplayListBuilder::new();

    // A pie slice: centre, out along the start angle, round, and back.
    let pie_center = Point::new(100.0, 100.0);
    let mut pie = PathBuilder::new();
    pie.move_to(pie_center)
        .arc(pie_center, 70.0, -FRAC_PI_2, TAU * 0.7)
        .close();
    b.draw_path(
        &pie.build(),
        valo::FillRule::NonZero,
        &Paint::from_color(Color::rgb(0.96, 0.62, 0.20)),
    );

    // A stroked ring segment — an arc with no fill behind it.
    let mut ring = PathBuilder::new();
    ring.arc(Point::new(250.0, 100.0), 60.0, PI * 0.15, PI * 1.2);
    b.draw_path(
        &ring.build(),
        valo::FillRule::NonZero,
        &Paint {
            color: Color::rgb(0.35, 0.75, 0.95),
            style: stroke_of(12.0),
            ..Default::default()
        },
    );

    // Two `arc_to` shoulders make a tab.
    let mut tab = PathBuilder::new();
    tab.move_to((360.0, 160.0))
        .line_to((360.0, 90.0))
        .arc_to((360.0, 45.0), (405.0, 45.0), 28.0)
        .line_to((470.0, 45.0))
        .arc_to((515.0, 45.0), (515.0, 90.0), 28.0)
        .line_to((515.0, 160.0));
    b.draw_path(
        &tab.build(),
        valo::FillRule::NonZero,
        &Paint {
            color: Color::rgb(0.85, 0.35, 0.55),
            style: stroke_of(9.0),
            ..Default::default()
        },
    );

    // A tilted ellipse, filled.
    let mut oval = PathBuilder::new();
    oval.ellipse((580.0, 100.0), [58.0, 26.0], PI / 5.0, 0.0, TAU);
    b.draw_path(
        &oval.build(),
        valo::FillRule::NonZero,
        &Paint::from_color(Color::rgb(0.55, 0.85, 0.45)),
    );

    // The same word filled and stroked, so the two can be compared.
    let word = |text: &str| {
        let mut builder = ParagraphBuilder::new(&fonts);
        builder.add_text(text, &TextStyle::new("Fira Sans", 72.0, Color::WHITE));
        let mut paragraph = builder.build();
        paragraph.layout(600.0);
        paragraph
    };
    b.draw_paragraph(&word("Stroke"), (40.0, 190.0));
    b.draw_paragraph_with(
        &word("Stroke"),
        (330.0, 190.0),
        &Paint {
            color: Color::WHITE,
            style: stroke_of(2.5),
            ..Default::default()
        },
    );

    let stats = ctx.render(&b.build(), &offscreen.target(Some(background)));
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);

    // Same font, same 72px, same transform — the ONLY difference is the
    // paint style, and it alone moves the stroked word off the atlas: one
    // mask-tier run, no SDF, one outline run.
    assert_eq!(
        stats.text_tiers,
        [1, 0, 1],
        "the stroked word should be the only one on the outline tier"
    );

    // Outlined glyphs are HOLLOW, and that is what a scanline sees: crossing
    // a filled stem inks one run, crossing an outlined one inks two, with the
    // gap between them showing through. Counting runs across the same word
    // drawn both ways is a direct read of the difference, and unlike total
    // ink it does not depend on how the stroke width compares to the stem.
    let ink_runs = |x0: u32, x1: u32, y: u32| {
        let mut runs = 0;
        let mut was_ink = false;
        for x in x0..x1 {
            let i = ((y * size[0] + x) * 4) as usize;
            let is_ink = rgba[i] > 128 && rgba[i + 1] > 128 && rgba[i + 2] > 128;
            if is_ink && !was_ink {
                runs += 1;
            }
            was_ink = is_ink;
        }
        runs
    };
    // A scanline through the x-height of both words.
    let scanline = 190 + 44;
    let filled_runs = ink_runs(40, 320, scanline);
    let outlined_runs = ink_runs(330, 610, scanline);
    assert!(
        filled_runs >= 4,
        "filled word looks blank: {filled_runs} runs"
    );
    assert!(
        outlined_runs > filled_runs,
        "outlined text should break into more runs than filled \
         ({outlined_runs} vs {filled_runs}) — the counters should show through"
    );

    valo_harness::assert_golden(goldens_dir(), "arcs_and_stroked_text", size, &rgba);
}

/// Colour filters — Flutter's `ColorFilter`, both constructors. Each card
/// probes one part of the implementation, so a wrong answer names itself:
/// the matrix's row order and its translation column, the unpremultiply the
/// matrix needs, a Porter-Duff blend, and a dst-reading one (which reaches
/// `composite_advanced` through the +15 id offset).
#[test]
fn color_filters_golden() {
    use valo::{GradientStop, Point, Shader, SpreadMode};

    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP color_filters_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [660u32, 200u32];
    let offscreen = Offscreen::new(&device, size);
    let background = Color::rgb(0.07, 0.07, 0.09);

    let card = |index: u32| Rect::new(40.0 + index as f32 * 160.0, 40.0, 120.0, 120.0);
    let mut b = DisplayListBuilder::new();

    // 1. Swap red and blue, and lift green by a quarter — the translation
    //    column, which Flutter hands over in 0..255 space and we take in 0..1.
    #[rustfmt::skip]
    let swap_and_lift = [
        0.0, 0.0, 1.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0, 0.25,
        1.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    b.draw_rect(
        card(0),
        &Paint {
            color: Color::rgb(0.9, 0.2, 0.3),
            color_filter: Some(valo::ColorFilter::Matrix(swap_and_lift)),
            ..Default::default()
        },
    );

    // 2. Luminance grayscale over a gradient, so the filter has to run per
    //    pixel rather than fold into one colour.
    #[rustfmt::skip]
    let grayscale = [
        0.2126, 0.7152, 0.0722, 0.0, 0.0,
        0.2126, 0.7152, 0.0722, 0.0, 0.0,
        0.2126, 0.7152, 0.0722, 0.0, 0.0,
        0.0,    0.0,    0.0,    1.0, 0.0,
    ];
    b.draw_rect(
        card(1),
        &Paint {
            color: Color::WHITE,
            shader: Some(Shader::Linear {
                start: Point::new(card(1).x, 0.0),
                end: Point::new(card(1).right(), 0.0),
                stops: vec![
                    GradientStop {
                        offset: 0.0,
                        color: Color::rgb(0.95, 0.15, 0.1),
                    },
                    GradientStop {
                        offset: 1.0,
                        color: Color::rgb(0.1, 0.3, 0.95),
                    },
                ],
                spread: SpreadMode::Pad,
                local: Default::default(),
            }),
            color_filter: Some(valo::ColorFilter::Matrix(grayscale)),
            ..Default::default()
        },
    );

    // 3. SrcIn with a constant colour: the tint every coloured icon uses.
    let tint = Color::rgb(0.98, 0.55, 0.1);
    b.draw_rect(
        card(2),
        &Paint {
            color: Color::rgb(0.2, 0.7, 0.35),
            color_filter: Some(valo::ColorFilter::Blend(tint, BlendMode::SrcIn)),
            ..Default::default()
        },
    );

    // 4. Multiply — a dst-reading mode, so this one travels the advanced path.
    b.draw_rect(
        card(3),
        &Paint {
            color: Color::rgb(0.8, 0.8, 0.2),
            color_filter: Some(valo::ColorFilter::Blend(
                Color::rgb(0.5, 0.5, 1.0),
                BlendMode::Multiply,
            )),
            ..Default::default()
        },
    );

    let stats = ctx.render(&b.build(), &offscreen.target(Some(background)));
    assert_eq!(
        stats.layers_rendered, 4,
        "each filtered draw takes exactly one layer"
    );
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);

    let centre = |index: u32| {
        let rect = card(index);
        let (x, y) = ((rect.x + 60.0) as u32, (rect.y + 60.0) as u32);
        let i = ((y * size[0] + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    };
    let close = |got: u8, want: f32| got.abs_diff((want * 255.0).round() as u8) <= 3;

    let swapped = centre(0);
    assert!(
        close(swapped[0], 0.3) && close(swapped[1], 0.45) && close(swapped[2], 0.9),
        "channels should swap and green lift by 0.25, got {swapped:?}"
    );

    let gray = centre(1);
    assert!(
        gray[0].abs_diff(gray[1]) <= 2 && gray[1].abs_diff(gray[2]) <= 2,
        "grayscale should leave the channels equal, got {gray:?}"
    );

    let tinted = centre(2);
    assert!(
        close(tinted[0], 0.98) && close(tinted[1], 0.55) && close(tinted[2], 0.1),
        "SrcIn should replace the fill with the tint, got {tinted:?}"
    );

    let multiplied = centre(3);
    assert!(
        close(multiplied[0], 0.4) && close(multiplied[1], 0.4) && close(multiplied[2], 0.2),
        "multiply should be the product of the two colours, got {multiplied:?}"
    );

    valo_harness::assert_golden(goldens_dir(), "color_filters", size, &rgba);
}

/// F3: mask layers. Content = an opaque two-tone card; masks =
/// a luminance gradient bar (soft left-to-right reveal), an ALPHA circle,
/// and — bottom row — proof that content OUTSIDE the mask ink disappears
/// instead of surviving the composite quad.
fn f3_masks_scene() -> valo::DisplayList {
    use valo::{MaskKind, Point, Shader};
    let card = |b: &mut DisplayListBuilder, x: f32| {
        b.draw_rect(
            Rect::new(x, 40.0, 160.0, 160.0),
            &Paint::from_color(Color::rgb(0.9, 0.3, 0.25)),
        );
        b.draw_rect(
            Rect::new(x + 40.0, 80.0, 80.0, 80.0),
            &Paint::from_color(Color::rgb(0.25, 0.5, 0.95)),
        );
    };
    let mut b = DisplayListBuilder::new();

    // Luminance gradient mask: black→white reveals left→right.
    b.save_layer(None, &Paint::default());
    card(&mut b, 40.0);
    b.save_layer_mask(None, MaskKind::Luminance);
    b.draw_rect(
        Rect::new(40.0, 40.0, 160.0, 160.0),
        &Paint::from_shader(Shader::linear(
            Point::new(40.0, 0.0),
            Point::new(200.0, 0.0),
            Color::BLACK,
            Color::WHITE,
        )),
    );
    b.restore();
    b.restore();

    // Alpha mask: a translucent circle — coverage from alpha alone
    // (the fill is BLACK: luminance would hide everything).
    b.save_layer(None, &Paint::default());
    card(&mut b, 250.0);
    b.save_layer_mask(None, MaskKind::Alpha);
    let mut circle = valo::PathBuilder::new();
    circle.circle((330.0, 120.0), 70.0);
    b.draw_path(
        &circle.build(),
        valo::FillRule::NonZero,
        &Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 1.0)),
    );
    b.restore();
    b.restore();

    // Small mask ink, big content: everything outside the ink must go.
    b.save_layer(None, &Paint::default());
    card(&mut b, 460.0);
    b.save_layer_mask(None, MaskKind::Luminance);
    b.draw_rect(
        Rect::new(500.0, 80.0, 80.0, 80.0),
        &Paint::from_color(Color::WHITE),
    );
    b.restore();
    b.restore();

    b.build()
}

#[test]
fn f3_mask_layers_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP f3_mask_layers_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [660u32, 240u32];
    let offscreen = Offscreen::new(&device, size);
    let stats = ctx.render(
        &f3_masks_scene(),
        &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))),
    );
    // 3 content layers + 3 mask layers actually render (masks never elide).
    assert_eq!(stats.layers_rendered, 6);
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "f3_mask_layers", size, &rgba);
}

/// F4 end to end THROUGH the translator: a polka-dot pattern
/// fill, once at 1× and once at 6× — the zoomed embeds re-tessellate, so
/// the dots stay sharp (the whole point of vector-native SVG).
#[test]
fn f4_pattern_fills_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP f4_pattern_fills_golden: no GPU adapter");
        return;
    };
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
      <defs><pattern id="p" width="6" height="6" patternUnits="userSpaceOnUse">
        <circle cx="3" cy="3" r="2" fill="#2e6bff"/>
      </pattern></defs>
      <path d="M2 2 L22 2 L22 22 L2 22 Z" fill="url(#p)" stroke="#e0e4ff" stroke-width="1"/>
    </svg>"##;
    let svg = valo_svg::translate(svg).expect("pattern svg must translate");
    assert_eq!(svg.size, [24.0, 24.0]);
    assert!(svg.missing.is_empty(), "fully native: {:?}", svg.missing);
    let list = svg.list;

    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [200u32, 170u32];
    let offscreen = Offscreen::new(&device, size);
    let mut b = DisplayListBuilder::new();
    b.save();
    b.translate(10.0, 70.0);
    b.draw_display_list(&list); // 1×
    b.restore();
    b.save();
    b.translate(50.0, 10.0);
    b.scale(6.0, 6.0); // zoomed: dots must stay crisp
    b.draw_display_list(&list);
    b.restore();
    ctx.render(
        &b.build(),
        &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))),
    );
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "f4_pattern_fills", size, &rgba);
}

/// G1+G2 through the translator: a skewed linear, an
/// elliptical bbox radial, a focal ELLIPSE (all riding the shader local
/// matrix — fields valo could not express before), and a two-circle UNION
/// clip desugared onto an alpha mask.
#[test]
fn f5_local_matrix_and_union_clip_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP f5_local_matrix_and_union_clip_golden: no GPU adapter");
        return;
    };
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 60">
      <defs>
        <linearGradient id="skew" gradientUnits="userSpaceOnUse" x1="4" y1="0" x2="24" y2="0" gradientTransform="skewX(30)">
          <stop offset="0" stop-color="#ff4020"/><stop offset="1" stop-color="#2040ff"/>
        </linearGradient>
        <radialGradient id="ellipse">
          <stop offset="0" stop-color="#ffffff"/><stop offset="1" stop-color="#204090"/>
        </radialGradient>
        <radialGradient id="focal" fx="0.25" fy="0.3">
          <stop offset="0" stop-color="#ffffff"/><stop offset="1" stop-color="#903030"/>
        </radialGradient>
        <clipPath id="union">
          <circle cx="102" cy="22" r="12"/><circle cx="112" cy="38" r="12"/>
        </clipPath>
      </defs>
      <rect x="2" y="2" width="26" height="56" fill="url(#skew)"/>
      <rect x="32" y="14" width="26" height="14" fill="url(#ellipse)"/>
      <rect x="62" y="10" width="26" height="40" fill="url(#focal)"/>
      <rect x="90" y="8" width="28" height="44" fill="#30a060" clip-path="url(#union)"/>
    </svg>"##;
    let svg = valo_svg::translate(svg).expect("parses");
    assert!(svg.missing.is_empty(), "fully native: {:?}", svg.missing);

    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [480u32, 240u32];
    let offscreen = Offscreen::new(&device, size);
    let mut b = DisplayListBuilder::new();
    b.scale(4.0, 4.0);
    b.draw_display_list(&svg.list);
    ctx.render(
        &b.build(),
        &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))),
    );
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "f5_local_matrix_union_clip", size, &rgba);
}

/// R2: >8-stop gradients through the baked ramp texture
/// (Impeller's path) — a 12-stop rainbow bar, a 10-stop radial, and a
/// repeating 9-stop ramp proving spread composes with the texture tier.
fn f6_ramp_scene() -> valo::DisplayList {
    use valo::{GradientStop, Point, Shader, SpreadMode};
    let rainbow: Vec<GradientStop> = (0..12)
        .map(|i| {
            let t = i as f32 / 11.0;
            // A hue sweep, saturated: crude HSV→RGB on the hexagon.
            let h = t * 6.0;
            let (r, g, b) = match h as u32 {
                0 => (1.0, h.fract(), 0.0),
                1 => (1.0 - h.fract(), 1.0, 0.0),
                2 => (0.0, 1.0, h.fract()),
                3 => (0.0, 1.0 - h.fract(), 1.0),
                4 => (h.fract(), 0.0, 1.0),
                _ => (1.0, 0.0, 1.0 - h.fract()),
            };
            GradientStop {
                offset: t,
                color: Color::rgb(r, g, b),
            }
        })
        .collect();
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(40.0, 30.0, 560.0, 60.0),
        &Paint::from_shader(Shader::Linear {
            start: Point::new(40.0, 0.0),
            end: Point::new(600.0, 0.0),
            stops: rainbow.clone(),
            spread: SpreadMode::Pad,
            local: Default::default(),
        }),
    );
    let radial: Vec<GradientStop> = (0..10)
        .map(|i| GradientStop {
            offset: i as f32 / 9.0,
            color: if i % 2 == 0 {
                Color::rgb(1.0, 1.0, 1.0)
            } else {
                Color::rgb(0.15, 0.3, 0.9)
            },
        })
        .collect();
    b.draw_rect(
        Rect::new(40.0, 120.0, 260.0, 160.0),
        &Paint::from_shader(Shader::Radial {
            center: Point::new(170.0, 200.0),
            radius: 75.0,
            stops: radial,
            spread: SpreadMode::Pad,
            focus: None,
            local: Default::default(),
        }),
    );
    let mut short = rainbow[..9].to_vec();
    for (i, stop) in short.iter_mut().enumerate() {
        stop.offset = i as f32 / 8.0;
    }
    b.draw_rect(
        Rect::new(340.0, 120.0, 260.0, 160.0),
        &Paint::from_shader(Shader::Linear {
            start: Point::new(340.0, 0.0),
            end: Point::new(430.0, 0.0),
            stops: short,
            spread: SpreadMode::Repeat,
            local: Default::default(),
        }),
    );
    b.build()
}

#[test]
fn f6_ramp_gradients_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP f6_ramp_gradients_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [640u32, 310u32];
    let offscreen = Offscreen::new(&device, size);
    let stats = ctx.render(
        &f6_ramp_scene(),
        &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))),
    );
    assert_eq!(stats.draws, 3);
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "f6_ramp_gradients", size, &rgba);
}

/// R3 tier 1 through the translator: a blur-filtered star and a
/// drop-shadowed card — the everyday real-world filters, reduced onto
/// valo's blurred layers instead of a fallback.
#[test]
fn f7_filters_tier1_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP f7_filters_tier1_golden: no GPU adapter");
        return;
    };
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 60">
      <defs>
        <filter id="blur"><feGaussianBlur stdDeviation="1.5"/></filter>
        <filter id="shadow"><feDropShadow dx="1.5" dy="2" stdDeviation="1" flood-color="#000000" flood-opacity="0.6"/></filter>
      </defs>
      <path d="M30 8 L36 22 L50 22 L39 30 L44 46 L30 37 L16 46 L21 30 L10 22 L24 22 Z" fill="#ffb020" filter="url(#blur)"/>
      <rect x="66" y="10" width="40" height="36" rx="4" fill="#3a7bd5" filter="url(#shadow)"/>
    </svg>"##;
    let svg = valo_svg::translate(svg).expect("parses");
    assert!(
        svg.missing.is_empty(),
        "tier-1 filters are native: {:?}",
        svg.missing
    );

    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [480u32, 240u32];
    let offscreen = Offscreen::new(&device, size);
    let mut b = DisplayListBuilder::new();
    b.scale(4.0, 4.0);
    b.draw_display_list(&svg.list);
    ctx.render(
        &b.build(),
        &offscreen.target(Some(Color::rgb(0.9, 0.9, 0.93))),
    );
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "f7_filters_tier1", size, &rgba);
}

// ── M4: save layers, opacity elision, advanced blends ──────────────────────

fn m4_layers_scene() -> valo::DisplayList {
    use valo::ClipOp;
    let mut b = DisplayListBuilder::new();
    let teal = Paint::from_color(Color::rgb(0.2, 0.8, 0.55));
    let teal_half = Paint::from_color(Color::rgba(0.2, 0.8, 0.55, 0.5));
    let group_alpha = Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.5));
    let circles = |b: &mut DisplayListBuilder, cx: f32, paint: &Paint| {
        b.draw_circle((cx, 92.0), 45.0, paint);
        b.draw_circle((cx - 34.0, 148.0), 45.0, paint);
        b.draw_circle((cx + 34.0, 148.0), 45.0, paint);
    };

    circles(&mut b, 110.0, &teal_half); // per-draw alpha: seams
    b.save_layer(None, &group_alpha); // group alpha: no seams
    circles(&mut b, 330.0, &teal);
    b.restore();
    b.save_layer(None, &group_alpha); // disjoint: elided
    for i in 0..3 {
        b.draw_rrect(
            Rect::new(490.0, 40.0 + i as f32 * 60.0, 120.0, 48.0),
            12.0,
            &Paint::from_color(Color::rgb(0.35, 0.55, 1.0)),
        );
    }
    b.restore();
    // Nested layer with a rotated clip inside.
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

#[test]
fn m4_layers_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m4_layers_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [640u32, 480u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m4_layers_scene();
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));

    assert_eq!(stats.layers_elided, 1, "the disjoint card group");
    assert_eq!(
        stats.layers_rendered, 3,
        "group-alpha trefoil + the nested outer/inner pair"
    );
    assert_eq!(stats.snapshots, 0);
    assert_eq!(stats.clips, 1);

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m4_layers", size, &rgba);
}

fn m4_blends_scene() -> valo::DisplayList {
    use valo::{Point, Shader};
    let modes = [
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
        BlendMode::Luminosity,
    ];
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 660.0, 480.0),
        &Paint::from_shader(Shader::linear(
            Point::new(0.0, 0.0),
            Point::new(660.0, 480.0),
            Color::rgb(0.85, 0.55, 0.35),
            Color::rgb(0.15, 0.35, 0.65),
        )),
    );
    let src = Color::rgb(0.55, 0.75, 0.45);
    for (i, mode) in modes.into_iter().enumerate() {
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
    // Gradient src desugars via an implicit layer.
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
    // Group composite with Overlay.
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
    b.build()
}

#[test]
fn m4_blends_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m4_blends_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [660u32, 480u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m4_blends_scene();
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));

    // 14 solid advanced draws + 1 desugared gradient + 1 group composite.
    assert_eq!(
        stats.snapshots, 16,
        "every dst-reading draw breaks the pass once"
    );
    assert_eq!(
        stats.layers_rendered, 2,
        "implicit gradient layer + overlay group"
    );

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m4_blends", size, &rgba);
}

// ── M5: mask blur (analytic + general) and backdrop blur ───────────────────

fn m5_shadows_scene() -> valo::DisplayList {
    use valo::{PathBuilder, Point, Shader};
    let mut b = DisplayListBuilder::new();
    let shadow = |sigma: f32| Paint {
        color: Color::rgba(0.0, 0.0, 0.0, 0.6),
        mask_blur: Some(MaskBlur::new(sigma)),
        ..Default::default()
    };

    // Analytic: rect spreads + the BoxShadow recipe + a glow.
    for (i, sigma) in [2.0f32, 6.0, 12.0, 24.0].into_iter().enumerate() {
        b.draw_rect(
            Rect::new(45.0 + i as f32 * 150.0, 50.0, 100.0, 70.0),
            &shadow(sigma),
        );
    }
    b.draw_rrect(Rect::new(53.0, 218.0, 160.0, 100.0), 16.0, &shadow(8.0));
    b.draw_rrect(
        Rect::new(45.0, 205.0, 160.0, 100.0),
        16.0,
        &Paint::from_color(Color::rgb(0.93, 0.94, 0.97)),
    );
    b.draw_rrect(
        Rect::new(280.0, 205.0, 160.0, 100.0),
        16.0,
        &Paint {
            color: Color::rgba(0.35, 0.6, 1.0, 0.9),
            mask_blur: Some(MaskBlur::new(12.0)),
            ..Default::default()
        },
    );

    // General path: blurred star (path) + blurred gradient (shader).
    let star = {
        let mut p = PathBuilder::new();
        for i in 0..5 {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * 4.0 * std::f32::consts::PI / 5.0;
            let pt = (120.0 + 55.0 * a.cos(), 400.0 + 55.0 * a.sin());
            if i == 0 {
                p.move_to(pt);
            } else {
                p.line_to(pt);
            }
        }
        p.close();
        p.build()
    };
    b.draw_path(
        &star,
        valo::FillRule::NonZero,
        &Paint {
            color: Color::rgba(0.95, 0.75, 0.3, 0.9),
            mask_blur: Some(MaskBlur::new(6.0)),
            ..Default::default()
        },
    );
    b.draw_rect(
        Rect::new(280.0, 360.0, 120.0, 100.0),
        &Paint {
            shader: Some(Shader::linear(
                Point::new(280.0, 360.0),
                Point::new(400.0, 460.0),
                Color::rgb(0.9, 0.3, 0.4),
                Color::rgb(0.3, 0.4, 0.9),
            )),
            color: Color::WHITE,
            mask_blur: Some(MaskBlur::new(10.0)),
            ..Default::default()
        },
    );
    b.build()
}

#[test]
fn m5_shadows_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m5_shadows_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [660u32, 480u32];
    let offscreen = Offscreen::new(&device, size);
    let stats = ctx.render(
        &m5_shadows_scene(),
        &offscreen.target(Some(Color::rgb(0.89, 0.9, 0.93))),
    );

    // 6 analytic quads cost NO layers or filters; the 2 general draws cost
    // one implicit layer + a separable chain each (σ>4 adds a downsample).
    assert_eq!(stats.layers_rendered, 2);
    assert_eq!(stats.filter_passes, 6);
    assert_eq!(stats.snapshots, 0, "mask blur never reads the target");
    assert_eq!(stats.backdrops, 0);

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m5_shadows", size, &rgba);
}

fn m5_backdrop_scene() -> valo::DisplayList {
    use valo::{ClipOp, Point, Shader};
    let mut b = DisplayListBuilder::new();
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
    }
    let panel = |b: &mut DisplayListBuilder, rect: Rect, sigma: f32, shared: Option<u64>| {
        b.save();
        b.clip_rrect(rect, 18.0, ClipOp::Intersect);
        match shared {
            Some(key) => b.backdrop_blur_shared(rect, sigma, key),
            None => b.backdrop_blur(rect, sigma),
        }
        b.draw_rect(rect, &Paint::from_color(Color::rgba(1.0, 1.0, 1.0, 0.14)));
        b.restore();
    };
    panel(&mut b, Rect::new(50.0, 130.0, 220.0, 220.0), 14.0, None);
    panel(&mut b, Rect::new(330.0, 60.0, 280.0, 150.0), 20.0, Some(1));
    panel(&mut b, Rect::new(370.0, 280.0, 240.0, 150.0), 20.0, Some(1));
    b.build()
}

#[test]
fn m5_backdrop_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m5_backdrop_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [660u32, 480u32];
    let offscreen = Offscreen::new(&device, size);
    let stats = ctx.render(
        &m5_backdrop_scene(),
        &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))),
    );

    // Three tiles, two blur chains: the shared pair blurs its union once.
    assert_eq!(stats.backdrops, 2);
    assert_eq!(stats.shared_backdrops, 1);
    assert_eq!(stats.snapshots, 2, "one region copy per pass break");
    assert_eq!(
        stats.filter_passes, 8,
        "both σ>4 chains halve ≤2× per pass: 2 downsamples + H + V each"
    );
    assert_eq!(stats.clips, 3);

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m5_backdrop", size, &rgba);
}

// ── M5.1: blur styles + per-corner radii ────────────────────────────────────

fn m5_styles_scene() -> valo::DisplayList {
    use valo::{PathBuilder, Point, Shader};
    let mut b = DisplayListBuilder::new();
    let ink = Color::rgb(0.2, 0.25, 0.4);
    let styled = |color: Color, blur: MaskBlur| Paint {
        color,
        mask_blur: Some(blur),
        ..Default::default()
    };

    // The four styles, analytic (one quad each).
    for (i, blur) in [
        MaskBlur::new(8.0),
        MaskBlur::solid(8.0),
        MaskBlur::inner(8.0),
        MaskBlur::outer(8.0),
    ]
    .into_iter()
    .enumerate()
    {
        b.draw_rrect(
            Rect::new(45.0 + i as f32 * 150.0, 45.0, 110.0, 90.0),
            18.0,
            &styled(ink, blur),
        );
    }

    // Per-corner radii: sharp card + matching analytic shadow.
    let radii = [48.0, 0.0, 24.0, 8.0];
    b.draw_rrect_radii(
        Rect::new(53.0, 218.0, 240.0, 130.0),
        radii,
        &styled(Color::rgba(0.0, 0.0, 0.0, 0.55), MaskBlur::new(10.0)),
    );
    b.draw_rrect_radii(
        Rect::new(45.0, 205.0, 240.0, 130.0),
        radii,
        &Paint::from_color(Color::rgb(0.95, 0.96, 0.99)),
    );

    // Styled general paths: blur chain + one combine pass each.
    let star = {
        let mut p = PathBuilder::new();
        for i in 0..5 {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * 4.0 * std::f32::consts::PI / 5.0;
            let pt = (130.0 + 52.0 * a.cos(), 420.0 + 52.0 * a.sin());
            if i == 0 {
                p.move_to(pt);
            } else {
                p.line_to(pt);
            }
        }
        p.close();
        p.build()
    };
    b.draw_path(
        &star,
        valo::FillRule::NonZero,
        &styled(Color::rgb(0.95, 0.75, 0.3), MaskBlur::outer(8.0)),
    );
    b.draw_rect(
        Rect::new(300.0, 370.0, 130.0, 100.0),
        &Paint {
            shader: Some(Shader::linear(
                Point::new(300.0, 370.0),
                Point::new(430.0, 470.0),
                Color::rgb(0.9, 0.3, 0.4),
                Color::rgb(0.3, 0.4, 0.9),
            )),
            color: Color::WHITE,
            mask_blur: Some(MaskBlur::solid(10.0)),
            ..Default::default()
        },
    );
    b.build()
}

#[test]
fn m5_styles_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m5_styles_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [660u32, 500u32];
    let offscreen = Offscreen::new(&device, size);
    let stats = ctx.render(
        &m5_styles_scene(),
        &offscreen.target(Some(Color::rgb(0.85, 0.86, 0.9))),
    );

    // 6 analytic quads free; the 2 styled general draws each cost a blur
    // chain (downsample + H + V at σ>4) plus ONE style-combine pass.
    assert_eq!(stats.layers_rendered, 2);
    assert_eq!(stats.filter_passes, 8);
    assert_eq!(stats.snapshots, 0);

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m5_styles", size, &rgba);
}

// ── M6: text end-to-end ─────────────────────────────────────────────────────

fn text_fonts() -> std::sync::Arc<valo::FontCollection> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut c = valo::FontCollection::new();
    let latin = c
        .register(
            "Fira Sans",
            std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap(),
        )
        .unwrap();
    let arabic = c
        .register(
            "Noto Sans Arabic",
            std::fs::read(format!("{dir}/noto_sans_arabic.ttf")).unwrap(),
        )
        .unwrap();
    let hebrew = c
        .register(
            "Noto Sans Hebrew",
            std::fs::read(format!("{dir}/noto_sans_hebrew.ttf")).unwrap(),
        )
        .unwrap();
    let emoji = c
        .register(
            "Noto Color Emoji",
            std::fs::read(format!("{dir}/noto_color_emoji_subset.ttf")).unwrap(),
        )
        .unwrap();
    c.add_fallback(latin);
    c.add_fallback(arabic);
    c.add_fallback(hebrew);
    c.add_fallback(emoji);
    std::sync::Arc::new(c)
}

fn m6_text_scene(fonts: &std::sync::Arc<valo::FontCollection>) -> valo::DisplayList {
    use valo::{DrawParagraphExt, ParagraphBuilder, TextAlign, TextStyle};
    let ink = Color::rgb(0.92, 0.93, 0.96);
    let accent = Color::rgb(0.95, 0.75, 0.3);
    let body = TextStyle::new("Fira Sans", 22.0, ink);
    let mut b = DisplayListBuilder::new();

    let mut p = ParagraphBuilder::new(fonts);
    p.add_text("valo renders ", &body)
        .add_text("retained", &TextStyle::new("Fira Sans", 30.0, accent))
        .add_text(" paragraphs — the word ", &body)
        .add_text("سلام", &TextStyle::new("Fira Sans", 22.0, accent))
        .add_text(" flows right-to-left inside this line.", &body);
    let mut par = p.build();
    par.layout(560.0);
    b.draw_paragraph(&par, (40.0, 32.0));

    let mut rtl = ParagraphBuilder::new(fonts);
    rtl.add_text(
        "שלום — valo — עולם",
        &TextStyle::new("Noto Sans Hebrew", 24.0, ink),
    );
    let mut rtl = rtl.build();
    rtl.layout(f32::INFINITY);
    b.draw_paragraph(&rtl, (40.0, 170.0));

    let mut center = ParagraphBuilder::new(fonts);
    center.style(TextAlign::Center.into());
    center.add_text(
        "centered",
        &TextStyle::new("Fira Sans", 18.0, Color::rgb(0.6, 0.75, 1.0)),
    );
    let mut center = center.build();
    center.layout(560.0);
    b.draw_paragraph(&center, (40.0, 220.0));

    // SDF tier (rotated + scaled) and outline tier (huge).
    let mut sdf = ParagraphBuilder::new(fonts);
    sdf.add_text("SDF tier", &TextStyle::new("Fira Sans", 26.0, accent));
    let mut sdf_par = sdf.build();
    sdf_par.layout(f32::INFINITY);
    b.save();
    b.translate(60.0, 400.0);
    b.rotate(-0.18);
    b.scale(1.6, 1.6);
    b.draw_paragraph(&sdf_par, (0.0, 0.0));
    b.restore();
    let mut big = ParagraphBuilder::new(fonts);
    big.add_text(
        "Aa",
        &TextStyle::new("Fira Sans", 200.0, Color::rgb(0.35, 0.6, 1.0)),
    );
    let mut big = big.build();
    big.layout(f32::INFINITY);
    b.draw_paragraph(&big, (380.0, 300.0));

    b.build()
}

#[test]
fn m6_text_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m6_text_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let fonts = text_fonts();
    ctx.set_fonts(fonts.clone());
    let size = [660u32, 580u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m6_text_scene(&fonts);
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.09, 0.1, 0.13))));

    assert_eq!(stats.draws, dl.draw_count(), "no text culled");
    assert_eq!(stats.snapshots, 0);
    assert_eq!(stats.filter_passes, 0, "text never blurs");

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m6_text", size, &rgba);

    // Warm atlas: a second frame re-uses every glyph (no new uploads is
    // implied by blocks_created == 0 after ring warmup — cheap sanity).
    let again = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.09, 0.1, 0.13))));
    assert_eq!(again.draws, stats.draws);
}

fn m6_features_scene(fonts: &std::sync::Arc<valo::FontCollection>) -> valo::DisplayList {
    use valo::{DrawParagraphExt, ParagraphBuilder, ParagraphStyle, TextAlign, TextStyle};
    let body = TextStyle::new("Fira Sans", 16.0, Color::rgb(0.92, 0.93, 0.96));
    let mut b = DisplayListBuilder::new();

    // Color emoji on the RGBA atlas family, untinted.
    let mut emoji = ParagraphBuilder::new(fonts);
    emoji.add_text(
        "ship it 🚀 ✨ 🎨",
        &TextStyle::new("Fira Sans", 22.0, Color::rgb(0.92, 0.93, 0.96)),
    );
    let mut emoji = emoji.build();
    emoji.layout(f32::INFINITY);
    b.draw_paragraph(&emoji, (30.0, 24.0));

    // Justify + the repaint tier (update_color must not move a glyph).
    let mut just = ParagraphBuilder::new(fonts);
    just.style(TextAlign::Justify.into());
    just.add_text(
        "justified text stretches every word gap to meet the right edge and ",
        &body,
    );
    just.add_text("this span got recolored", &body);
    let mut just = just.build();
    just.layout(260.0);
    just.update_color(1, Color::rgb(0.95, 0.75, 0.3));
    b.draw_paragraph(&just, (30.0, 90.0));

    // maxLines + ellipsis.
    let mut ell = ParagraphBuilder::new(fonts);
    ell.style(ParagraphStyle {
        max_lines: Some(2),
        ellipsis: Some("…".to_owned()),
        ..Default::default()
    });
    ell.add_text(
        "two lines is all this card gets no matter how much copy the author keeps typing",
        &body,
    );
    let mut ell = ell.build();
    ell.layout(260.0);
    assert!(ell.truncated());
    b.draw_paragraph(&ell, (350.0, 90.0));

    b.build()
}

#[test]
fn m6_features_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m6_features_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let fonts = text_fonts();
    ctx.set_fonts(fonts.clone());
    let size = [660u32, 260u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m6_features_scene(&fonts);
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.09, 0.1, 0.13))));
    assert_eq!(stats.draws, dl.draw_count());

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m6_features", size, &rgba);
}

// ── M7: opaque reorder, GPU timing, export ──────────────────────────────────

/// Heavy overdraw on purpose: opaque cards and shapes stacked over
/// translucent ones — the reorder pass hoists the occluders.
fn m7_scene() -> valo::DisplayList {
    use valo::{PathBuilder, Point, Shader};
    let mut b = DisplayListBuilder::new();
    // Translucent wash under everything.
    for i in 0..6 {
        b.draw_rect(
            Rect::new(20.0 + i as f32 * 90.0, 30.0, 140.0, 380.0),
            &Paint::from_color(Color::rgba(0.9, 0.3, 0.3, 0.4)),
        );
    }
    // Opaque occluders drawn LAST in painter order (hoisted to the front).
    b.draw_rect(
        Rect::new(60.0, 80.0, 300.0, 200.0),
        &Paint::from_color(Color::rgb(0.16, 0.18, 0.24)),
    );
    b.draw_rrect(
        Rect::new(240.0, 160.0, 280.0, 200.0),
        24.0,
        &Paint::from_color(Color::rgb(0.93, 0.94, 0.97)),
    );
    // An opaque gradient counts too (all stops α=1).
    b.draw_rect(
        Rect::new(420.0, 60.0, 190.0, 130.0),
        &Paint::from_shader(Shader::linear(
            Point::new(420.0, 60.0),
            Point::new(610.0, 190.0),
            Color::rgb(0.95, 0.75, 0.3),
            Color::rgb(0.85, 0.35, 0.2),
        )),
    );
    // Opaque path fill (StC cover writes depth as readily as a quad).
    let mut star = PathBuilder::new();
    for i in 0..5 {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * 4.0 * std::f32::consts::PI / 5.0;
        let pt = (140.0 + 70.0 * a.cos(), 360.0 + 70.0 * a.sin());
        if i == 0 {
            star.move_to(pt);
        } else {
            star.line_to(pt);
        }
    }
    star.close();
    b.draw_path(
        &star.build(),
        valo::FillRule::NonZero,
        &Paint::from_color(Color::rgb(0.2, 0.8, 0.55)),
    );
    // A clip scope: its opaques hoist within the scope, never across it.
    b.save();
    b.clip_rect(
        Rect::new(400.0, 240.0, 200.0, 180.0),
        valo::ClipOp::Intersect,
    );
    b.draw_rect(
        Rect::new(380.0, 220.0, 240.0, 220.0),
        &Paint::from_color(Color::rgba(0.4, 0.65, 1.0, 0.5)),
    );
    b.draw_rect(
        Rect::new(430.0, 280.0, 140.0, 100.0),
        &Paint::from_color(Color::rgb(0.55, 0.4, 0.9)),
    );
    b.restore();
    // A Difference clip fully OFF-VIEWPORT: cull-hardening drops it whole.
    b.save();
    let mut far = PathBuilder::new();
    far.circle((5000.0, 5000.0), 50.0);
    b.clip_path(
        &far.build(),
        valo::FillRule::NonZero,
        valo::ClipOp::Difference,
    );
    b.draw_rect(
        Rect::new(40.0, 430.0, 200.0, 30.0),
        &Paint::from_color(Color::rgba(0.9, 0.9, 0.95, 0.9)),
    );
    b.restore();
    b.build()
}

#[test]
fn m7_reorder_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m7_reorder_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [640u32, 480u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m7_scene();
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));

    // 4 root-level occluders sit above 6 translucent washes; the clipped
    // opaque hoists within its scope (past the wash recorded before it).
    assert_eq!(stats.opaque_reordered, 5, "hoisted occluders");
    assert_eq!(stats.culled, 1, "the off-viewport Difference clip");
    assert_eq!(stats.clips, 1, "only the Intersect clip renders");

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m7_reorder", size, &rgba);

    // GPU timing: after a couple frames a resolved timestamp surfaces
    // (feature-gated — the harness requests it when the adapter has it).
    if device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        // Let submitted frames FINISH, then harvest on the next render
        // (gpu_ms reports the freshest completed frame, never blocking).
        ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let gpu_ms = ctx
            .render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))))
            .gpu_ms;
        // Magnitude is backend-dependent (Apple stage-boundary timestamps
        // can quantize a sub-ms frame to 0) — this asserts the machinery
        // round-trips without wedging the device, which the suite finishing
        // at all also proves.
        assert!(gpu_ms.is_finite() && gpu_ms >= 0.0, "gpu_ms sane: {gpu_ms}");
        eprintln!("m7 gpu_ms = {gpu_ms}");
    }
}

#[test]
fn m7_export_unpremultiplies() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m7_export_unpremultiplies: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device, queue);
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 8.0, 8.0),
        &Paint::from_color(Color::rgba(1.0, 0.0, 0.0, 0.5)),
    );
    let dl = b.build();
    // Over a TRANSPARENT clear: premultiplied readback would be ~(128,0,0,128);
    // the export path must hand back straight alpha ~(255,0,0,128).
    let pixels = ctx.render_to_rgba(&dl, [8, 8], Some(Color::TRANSPARENT));
    let px = &pixels[..4];
    assert!(px[0] >= 250, "straight red, not premultiplied: {px:?}");
    assert!(px[3].abs_diff(128) <= 2, "alpha preserved: {px:?}");
}

// ── M8: text tiers across zoom ─────────────────────────────────────────────

/// One scene, three runs sized to cross tiers as zoom changes: 16px body,
/// 80px title, 60px emoji.
fn m8_tier_scene(fonts: &std::sync::Arc<valo::FontCollection>, zoom: f32) -> valo::DisplayList {
    use valo::{DrawParagraphExt, ParagraphBuilder, TextStyle};
    // Origins divide by zoom so DEVICE placement stays fixed — every run
    // stays on the canvas at every zoom, only glyph size changes.
    let mut b = DisplayListBuilder::new();
    b.scale(zoom, zoom);
    let mut body = ParagraphBuilder::new(fonts);
    body.add_text(
        "crisp body text at any zoom",
        &TextStyle::new("Fira Sans", 16.0, Color::rgb(0.9, 0.91, 0.95)),
    );
    let mut body = body.build();
    body.layout(f32::INFINITY);
    b.draw_paragraph(&body, (12.0 / zoom, 8.0 / zoom));

    let mut title = ParagraphBuilder::new(fonts);
    title.add_text(
        "Aa",
        &TextStyle::new("Fira Sans", 80.0, Color::rgb(0.4, 0.65, 1.0)),
    );
    let mut title = title.build();
    title.layout(f32::INFINITY);
    b.draw_paragraph(&title, (12.0 / zoom, 36.0 / zoom));

    let mut emoji = ParagraphBuilder::new(fonts);
    emoji.add_text(
        "🚀",
        &TextStyle::new("Noto Color Emoji", 60.0, Color::WHITE),
    );
    let mut emoji = emoji.build();
    emoji.layout(f32::INFINITY);
    b.draw_paragraph(&emoji, (430.0 / zoom, 30.0 / zoom));
    b.build()
}

#[test]
fn m8_text_tiers_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m8_text_tiers_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let fonts = text_fonts();
    ctx.set_fonts(fonts.clone());

    // Per zoom: expected [mask, sdf, path] run counts. 16px stays mask
    // through 6× (96 dev px); 80px crosses into SDF at 2.5× (200) and
    // paths at 6× (480); the 60px emoji hits the path tier at 6× (360)
    // and must STILL render (color glyphs clamp to a mask raster).
    let cases: [(f32, [u32; 3]); 5] = [
        (0.75, [3, 0, 0]),
        (1.0, [3, 0, 0]),
        (1.4, [3, 0, 0]),
        (2.5, [2, 1, 0]),
        (6.0, [1, 0, 2]),
    ];
    for (zoom, expected) in cases {
        let size = [660u32, 520u32];
        let offscreen = Offscreen::new(&device, size);
        let dl = m8_tier_scene(&fonts, zoom);
        let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.09, 0.1, 0.13))));
        assert_eq!(stats.text_tiers, expected, "tiers at zoom {zoom}");

        if zoom == 2.5 {
            let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
            valo_harness::assert_golden(goldens_dir(), "m8_tiers_z25", size, &rgba);
        }
        if zoom == 6.0 {
            let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
            valo_harness::assert_golden(goldens_dir(), "m8_tiers_z6", size, &rgba);
        }
    }
}

/// Fractional x positions: quarter-px phases keep glyphs texel-aligned —
/// the golden records that x.0/x.25/x.5/x.75 all render sharp, not smeared.
#[test]
fn m8_subpixel_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m8_subpixel_golden: no GPU adapter");
        return;
    };
    use valo::{DrawParagraphExt, ParagraphBuilder, TextStyle};
    let mut ctx = Context::new(device.clone(), queue.clone());
    let fonts = text_fonts();
    ctx.set_fonts(fonts.clone());
    let size = [320u32, 120u32];
    let offscreen = Offscreen::new(&device, size);

    let mut b = DisplayListBuilder::new();
    for (i, dx) in [0.0f32, 0.25, 0.5, 0.75].into_iter().enumerate() {
        let mut p = ParagraphBuilder::new(&fonts);
        p.add_text(
            "Illinois 1111",
            &TextStyle::new("Fira Sans", 15.0, Color::rgb(0.92, 0.93, 0.96)),
        );
        let mut p = p.build();
        p.layout(f32::INFINITY);
        b.draw_paragraph(&p, (14.0 + dx, 8.0 + i as f32 * 26.0));
    }
    let dl = b.build();
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.09, 0.1, 0.13))));
    assert_eq!(stats.text_tiers, [4, 0, 0]);

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m8_subpixel", size, &rgba);
}

// ── M9: editor text — shadows, decorations, spacing, height ────────────────

fn m9_scene(fonts: &std::sync::Arc<valo::FontCollection>) -> valo::DisplayList {
    use valo::{
        Decoration, DecorationKind, DrawParagraphExt, ParagraphBuilder, Point, Shadow, TextStyle,
    };
    let mut b = DisplayListBuilder::new();

    // Shadowed headline: soft drop + hard offset, under the sharp glyphs.
    let mut head = ParagraphBuilder::new(fonts);
    head.add_text(
        "Shadowed",
        &TextStyle {
            shadows: vec![
                Shadow {
                    color: Color::rgba(0.1, 0.3, 0.9, 0.8),
                    offset: Point::new(4.0, 5.0),
                    blur: 6.0,
                },
                Shadow {
                    color: Color::rgba(0.0, 0.0, 0.0, 0.5),
                    offset: Point::new(1.0, 2.0),
                    blur: 0.0,
                },
            ],
            ..TextStyle::new("Fira Sans", 54.0, Color::rgb(0.95, 0.96, 0.99))
        },
    );
    let mut head = head.build();
    head.layout(f32::INFINITY);
    b.draw_paragraph(&head, (30.0, 20.0));

    // Decorations: underline (auto color), colored strike, thick underline.
    let mut deco = ParagraphBuilder::new(fonts);
    let base = TextStyle::new("Fira Sans", 20.0, Color::rgb(0.9, 0.91, 0.95));
    deco.add_text(
        "underlined ",
        &TextStyle {
            decoration: Some(Decoration::new(DecorationKind::Underline)),
            ..base.clone()
        },
    );
    deco.add_text(
        "struck ",
        &TextStyle {
            decoration: Some(Decoration {
                color: Some(Color::rgb(0.95, 0.4, 0.3)),
                ..Decoration::new(DecorationKind::LineThrough)
            }),
            ..base.clone()
        },
    );
    deco.add_text(
        "thick",
        &TextStyle {
            decoration: Some(Decoration {
                thickness: 2.5,
                ..Decoration::new(DecorationKind::Underline)
            }),
            color: Color::rgb(0.95, 0.75, 0.3),
            ..base.clone()
        },
    );
    let mut deco = deco.build();
    deco.layout(f32::INFINITY);
    b.draw_paragraph(&deco, (30.0, 120.0));

    // Letter-spaced caps + tall line height, side by side.
    let mut spaced = ParagraphBuilder::new(fonts);
    spaced.add_text(
        "L E T T E R S",
        &TextStyle {
            letter_spacing: 6.0,
            ..base.clone()
        },
    );
    let mut spaced = spaced.build();
    spaced.layout(f32::INFINITY);
    b.draw_paragraph(&spaced, (30.0, 170.0));

    let mut tall = ParagraphBuilder::new(fonts);
    tall.add_text(
        "double height lines read airy and relaxed",
        &TextStyle {
            height: Some(2.0),
            ..base
        },
    );
    let mut tall = tall.build();
    tall.layout(220.0);
    b.draw_paragraph(&tall, (380.0, 130.0));

    b.build()
}

#[test]
fn m9_editor_text_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m9_editor_text_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let fonts = text_fonts();
    ctx.set_fonts(fonts.clone());
    let size = [660u32, 260u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m9_scene(&fonts);
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.09, 0.1, 0.13))));

    // The soft shadow runs the blur-layer route once; the hard shadow and
    // everything else stay direct.
    assert_eq!(stats.layers_rendered, 1, "one blurred shadow layer");
    assert!(stats.filter_passes >= 2);

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m9_editor_text", size, &rgba);
}

// ── M10: strokes + contour cache ────────────────────────────────────────────

fn m10_scene() -> valo::DisplayList {
    use valo::{Cap, Dash, Join, PaintStyle, PathBuilder, Point, Shader, Stroke};
    let mut b = DisplayListBuilder::new();
    let stroked = |color: Color, stroke: Stroke| Paint {
        color,
        style: PaintStyle::Stroke(stroke),
        ..Default::default()
    };
    let ink = Color::rgb(0.9, 0.91, 0.95);

    // Caps × joins on zigzags.
    for (row, join) in [Join::Miter, Join::Round, Join::Bevel]
        .into_iter()
        .enumerate()
    {
        for (col, cap) in [Cap::Butt, Cap::Round, Cap::Square].into_iter().enumerate() {
            let at = Point::new(50.0 + col as f32 * 140.0, 45.0 + row as f32 * 90.0);
            let mut p = PathBuilder::new();
            p.move_to((at.x, at.y + 40.0))
                .line_to((at.x + 45.0, at.y))
                .line_to((at.x + 90.0, at.y + 40.0));
            b.draw_path(
                &p.build(),
                valo::FillRule::NonZero,
                &stroked(
                    ink,
                    Stroke {
                        cap,
                        join,
                        ..Stroke::new(14.0)
                    },
                ),
            );
        }
    }
    // Miter limit spike vs bevel.
    for (i, limit) in [10.0f32, 1.5].into_iter().enumerate() {
        let x = 500.0 + i as f32 * 80.0;
        let mut p = PathBuilder::new();
        p.move_to((x, 130.0))
            .line_to((x + 30.0, 45.0))
            .line_to((x + 60.0, 130.0));
        b.draw_path(
            &p.build(),
            valo::FillRule::NonZero,
            &stroked(
                Color::rgb(0.95, 0.75, 0.3),
                Stroke {
                    miter_limit: limit,
                    ..Stroke::new(12.0)
                },
            ),
        );
    }
    // Dashed rrect + gradient ring + translucent stroke (join overlap look).
    let mut frame = PathBuilder::new();
    frame.rrect(Rect::new(50.0, 330.0, 250.0, 130.0), 20.0);
    b.draw_path(
        &frame.build(),
        valo::FillRule::NonZero,
        &stroked(
            Color::rgb(0.35, 0.55, 1.0),
            Stroke {
                dash: Some(Dash {
                    intervals: vec![18.0, 12.0],
                    phase: 0.0,
                }),
                cap: Cap::Round,
                ..Stroke::new(6.0)
            },
        ),
    );
    let mut ring = PathBuilder::new();
    ring.circle((430.0, 395.0), 55.0);
    b.draw_path(
        &ring.build(),
        valo::FillRule::NonZero,
        &Paint {
            shader: Some(Shader::linear(
                Point::new(375.0, 340.0),
                Point::new(485.0, 450.0),
                Color::rgb(0.95, 0.4, 0.3),
                Color::rgb(0.35, 0.55, 1.0),
            )),
            color: Color::WHITE,
            style: PaintStyle::Stroke(Stroke::new(12.0)),
            ..Default::default()
        },
    );
    b.build()
}

#[test]
fn m10_strokes_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m10_strokes_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [660u32, 500u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m10_scene();
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.09, 0.1, 0.13))));
    assert_eq!(stats.draws, dl.draw_count());
    assert_eq!(
        stats.opaque_reordered, 0,
        "strips never promote to occluders"
    );

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m10_strokes", size, &rgba);
}

/// Gradient text: the SrcIn-layer desugar. Upright 64px (mask tier) and
/// rotated 170px (SDF tier) — both sample the gradient in run-local space.
#[test]
fn m11_gradient_text_golden() {
    use valo::{DrawGlyphRunExt, ParagraphBuilder, Point, Shader, TextStyle};
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m11_gradient_text_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let fonts = text_fonts();
    ctx.set_fonts(fonts.clone());
    let size = [660u32, 380u32];
    let offscreen = Offscreen::new(&device, size);

    let shader = Shader::linear(
        Point::new(30.0, 30.0),
        Point::new(360.0, 110.0),
        Color::rgb(0.95, 0.4, 0.3),
        Color::rgb(0.35, 0.55, 1.0),
    );
    let paint = Paint {
        shader: Some(shader),
        color: Color::WHITE,
        ..Default::default()
    };
    let paragraph = |text: &str, px: f32| {
        let mut b = ParagraphBuilder::new(&fonts);
        b.add_text(text, &TextStyle::new("Fira Sans", px, Color::WHITE));
        let mut p = b.build();
        p.layout(f32::INFINITY);
        p
    };

    let mut b = DisplayListBuilder::new();
    b.draw_paragraph_with(&paragraph("Gradient", 64.0), (30.0, 20.0), &paint);
    b.save();
    b.translate(240.0, 180.0);
    b.rotate(-0.15);
    b.draw_paragraph_with(&paragraph("big", 170.0), (0.0, 0.0), &paint);
    b.restore();
    let dl = b.build();
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.09, 0.1, 0.13))));
    assert_eq!(
        stats.layers_rendered, 2,
        "one implicit layer per gradient run"
    );

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m11_gradient_text", size, &rgba);
}

/// The Impeller-style global depth line: a REAL layer nested inside an
/// ELIDED opacity group. Before M12 the nested content vanished (its slots
/// resolved in the wrong depth space); now the group alpha lands once on
/// each child — including the nested composite — at honest depths.
#[test]
fn m12_nested_opacity_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m12_nested_opacity_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [420u32, 300u32];
    let offscreen = Offscreen::new(&device, size);

    let mut b = DisplayListBuilder::new();
    // Reference column: the same content flattened by hand at 50% alpha.
    b.draw_rect(
        Rect::new(30.0, 40.0, 60.0, 60.0),
        &Paint::from_color(Color::rgba(0.9, 0.25, 0.2, 0.5)),
    );
    b.draw_rect(
        Rect::new(30.0, 140.0, 90.0, 90.0),
        &Paint::from_color(Color::rgba(0.2, 0.5, 0.9, 0.5)),
    );
    // Test column: elided 50% group { rect; REAL layer { overlapping rects } }.
    b.save_layer(None, &Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.5)));
    b.draw_rect(
        Rect::new(240.0, 40.0, 60.0, 60.0),
        &Paint::from_color(Color::rgb(0.9, 0.25, 0.2)),
    );
    b.save_layer(None, &Paint::from_color(Color::WHITE));
    b.draw_rect(
        Rect::new(240.0, 140.0, 60.0, 60.0),
        &Paint::from_color(Color::rgb(0.2, 0.5, 0.9)),
    );
    b.draw_rect(
        Rect::new(270.0, 170.0, 60.0, 60.0), // overlaps → inner can't elide
        &Paint::from_color(Color::rgb(0.2, 0.5, 0.9)),
    );
    b.restore();
    b.restore();
    let dl = b.build();

    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.09, 0.1, 0.13))));
    assert_eq!(stats.layers_elided, 1, "the outer opacity group elides");
    assert_eq!(stats.layers_rendered, 1, "the nested layer is real");

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    // The nested layer's union (240..330 × 140..230) composites at the group
    // alpha as ONE shade: probe the overlap centre against the single-rect
    // area — equal channels prove no double-blend and no vanishing.
    let px = |x: u32, y: u32| {
        let i = ((y * size[0] + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    };
    assert_eq!(
        px(300, 200),
        px(250, 150),
        "overlap region must match the un-overlapped blue (one group shade)"
    );
    valo_harness::assert_golden(goldens_dir(), "m12_nested_opacity", size, &rgba);
}

/// Shapes a correctness audit un-broke: stroked LINES (flatten used to drop
/// 2-point contours — nothing rendered), stroked RECTS (the closing edge
/// was missing and the seam capped), a dashed closed rect, and lone-point
/// caps. None of these had golden coverage before.
#[test]
fn m12_strokes_fixed_golden() {
    use valo::{Cap, Dash, FillRule, Join, PaintStyle, PathBuilder, Stroke};
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m12_strokes_fixed_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [520u32, 300u32];
    let offscreen = Offscreen::new(&device, size);

    let stroke = |width: f32| Paint {
        color: Color::rgb(0.92, 0.62, 0.2),
        style: PaintStyle::Stroke(Stroke::new(width)),
        ..Default::default()
    };
    let mut b = DisplayListBuilder::new();
    // A plain line segment (was invisible).
    let mut line = PathBuilder::new();
    line.move_to((30.0, 40.0)).line_to((230.0, 60.0));
    b.draw_path(&line.build(), FillRule::NonZero, &stroke(8.0));
    // A stroked rect via the desugar (seam corner must miter, not cap).
    b.draw_rect(Rect::new(30.0, 110.0, 160.0, 80.0), &stroke(12.0));
    // A dashed closed rect (dashing starts at the seam, runs the full edge).
    let mut dashed = stroke(6.0);
    if let PaintStyle::Stroke(s) = &mut dashed.style {
        s.dash = Some(Dash {
            intervals: vec![18.0, 12.0],
            phase: 0.0,
        });
    }
    b.draw_rect(Rect::new(300.0, 40.0, 170.0, 100.0), &dashed);
    // Lone points render as their caps (round dot + square dot).
    let mut dot = PathBuilder::new();
    dot.move_to((320.0, 220.0));
    let mut round_dot = stroke(26.0);
    if let PaintStyle::Stroke(s) = &mut round_dot.style {
        s.cap = Cap::Round;
        s.join = Join::Round;
    }
    b.draw_path(&dot.build(), FillRule::NonZero, &round_dot);
    let mut dot2 = PathBuilder::new();
    dot2.move_to((400.0, 220.0));
    let mut square_dot = stroke(26.0);
    if let PaintStyle::Stroke(s) = &mut square_dot.style {
        s.cap = Cap::Square;
    }
    b.draw_path(&dot2.build(), FillRule::NonZero, &square_dot);
    // A stroked zero-height rect draws as a line (Skia semantics).
    b.draw_rect(Rect::new(30.0, 240.0, 200.0, 0.0), &stroke(8.0));

    let dl = b.build();
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.09, 0.1, 0.13))));
    assert_eq!(stats.draws, 6, "every stroke shape survives to a draw");

    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "m12_strokes_fixed", size, &rgba);
}

/// 005-B4: a retained list with a SHARED backdrop key, drawn twice. valo
/// scopes sharing to ONE drawing of one list (the cache is set aside
/// around nested replays): Impeller's frame-global backdrop_id sharing
/// relies on whole-target snapshots, but valo blurs recorded union
/// REGIONS, and those are list-relative — the second drawing sits
/// somewhere else. Within one drawing, same-key tiles still share.
#[test]
fn redrawn_list_gets_its_own_backdrop_blur() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP redrawn_list_gets_its_own_backdrop_blur: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [400u32, 300u32];
    let offscreen = Offscreen::new(&device, size);

    let mut tile = DisplayListBuilder::new();
    tile.backdrop_blur_shared(Rect::new(0.0, 0.0, 80.0, 60.0), 6.0, 7);
    tile.backdrop_blur_shared(Rect::new(90.0, 0.0, 80.0, 60.0), 6.0, 7);
    let tile = std::sync::Arc::new(tile.build());

    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 400.0, 300.0),
        &Paint::from_color(Color::rgb(0.3, 0.5, 0.8)),
    );
    b.draw_display_list(&tile);
    b.save();
    b.translate(0.0, 150.0);
    b.draw_display_list(&tile);
    b.restore();

    let stats = ctx.render(&b.build(), &offscreen.target(Some(Color::BLACK)));
    assert_eq!(stats.backdrops, 2, "each stamp blurs its own region");
    assert_eq!(
        stats.shared_backdrops, 2,
        "tiles within a stamp still share"
    );
}

/// Impeller's `all_filters_equal` fallback: tiles under ONE shared key that
/// disagree on σ never share a blur — each runs its own (a shared result at
/// the wrong σ is visibly wrong glass).
#[test]
fn mixed_sigma_backdrop_keys_do_not_share() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP mixed_sigma_backdrop_keys_do_not_share: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [400u32, 150u32];
    let offscreen = Offscreen::new(&device, size);

    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 400.0, 150.0),
        &Paint::from_color(Color::rgb(0.8, 0.4, 0.3)),
    );
    b.backdrop_blur_shared(Rect::new(10.0, 10.0, 80.0, 60.0), 4.0, 9);
    b.backdrop_blur_shared(Rect::new(100.0, 10.0, 80.0, 60.0), 12.0, 9);

    let stats = ctx.render(&b.build(), &offscreen.target(Some(Color::BLACK)));
    assert_eq!(stats.backdrops, 2, "each σ blurs independently");
    assert_eq!(stats.shared_backdrops, 0);
}

/// 006-1 smoke: the new stats fields count real work and the memory report
/// sees every pool. Values are scene-dependent; the assertions pin the
/// invariants (counted > 0, split ≤ total), not magic numbers.
#[test]
fn stats_and_memory_report_observe_the_frame() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP stats_and_memory_report_observe_the_frame: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    ctx.set_fonts(text_fonts());
    let size = [300u32, 200u32];
    let offscreen = Offscreen::new(&device, size);

    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(10.0, 10.0, 100.0, 80.0),
        &Paint::from_color(Color::rgb(0.2, 0.5, 0.9)),
    );
    b.save_layer(None, &Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.6)));
    b.draw_circle(
        (200.0, 100.0),
        40.0,
        &Paint::from_color(Color::rgb(0.9, 0.4, 0.2)),
    );
    b.draw_rect(
        Rect::new(120.0, 20.0, 40.0, 40.0),
        &Paint::from_color(Color::rgb(0.4, 0.8, 0.3)),
    );
    b.restore();
    let stats = ctx.render(&b.build(), &offscreen.target(Some(Color::BLACK)));

    assert!(stats.draw_calls >= stats.draws, "covers + fans add calls");
    assert!(stats.render_passes >= 1);
    assert!(stats.pipeline_switches >= 1);
    assert!(stats.uniform_bytes > 0);
    assert!(stats.vertex_bytes > 0, "the circle's fan mesh uploads");
    assert!(stats.plan_ms + stats.encode_ms <= stats.cpu_ms + 0.01);

    let report = ctx.memory_report();
    assert!(report.host_buffer.count >= 1);
    assert!(report.targets.count >= 1, "main scratch + pooled layer");
    assert!(
        report.contours.count >= 1,
        "the circle's flattening is cached"
    );
    assert!(report.total_bytes() > 0);
}

/// Elliptical per-corner rrect radii (the full CSS/Flutter 8-scalar
/// rounded rect): fill, stroke, clip,
/// and a blurred shadow all through the path lowering; the circular fast
/// paths are pinned separately by every existing rrect golden (the
/// circular constructor IS the equal-axes elliptical case).
fn elliptical_rrect_scene() -> valo::DisplayList {
    use valo::{ClipOp, Paint, PaintStyle, Rect, Stroke};
    let mut b = valo::DisplayListBuilder::new();

    // Fill: a "pill on its side" — wide x radii, shallow y.
    b.draw_rrect_radii_elliptical(
        Rect::new(20.0, 20.0, 180.0, 100.0),
        [[60.0, 20.0]; 4],
        &Paint::from_color(valo::Color::rgb(0.32, 0.55, 0.95)),
    );

    // Mixed corners + stroke: every corner a different ellipse.
    b.draw_rrect_radii_elliptical(
        Rect::new(230.0, 20.0, 180.0, 100.0),
        [[50.0, 12.0], [12.0, 50.0], [40.0, 40.0], [0.0, 0.0]],
        &Paint {
            color: valo::Color::rgb(0.95, 0.62, 0.25),
            style: PaintStyle::Stroke(Stroke::new(6.0)),
            ..Paint::default()
        },
    );

    // Clip: stripes through an elliptical window.
    b.save();
    b.clip_rrect_radii_elliptical(
        Rect::new(20.0, 150.0, 180.0, 100.0),
        [[70.0, 25.0]; 4],
        ClipOp::Intersect,
    );
    for i in 0..9 {
        b.draw_rect(
            Rect::new(20.0 + i as f32 * 20.0, 150.0, 10.0, 100.0),
            &Paint::from_color(valo::Color::rgb(0.85, 0.3, 0.5)),
        );
    }
    b.restore();

    // Shadow: elliptical corners force the PATH blur route (the analytic
    // rrect blur is circular-only).
    b.draw_rrect_radii_elliptical(
        Rect::new(250.0, 160.0, 140.0, 80.0),
        [[45.0, 15.0]; 4],
        &Paint {
            color: valo::Color::rgba(0.0, 0.0, 0.0, 0.6),
            mask_blur: Some(valo::MaskBlur::new(6.0)),
            ..Paint::default()
        },
    );
    b.draw_rrect_radii_elliptical(
        Rect::new(242.0, 152.0, 140.0, 80.0),
        [[45.0, 15.0]; 4],
        &Paint::from_color(valo::Color::rgb(0.92, 0.9, 0.85)),
    );

    b.build()
}

#[test]
fn elliptical_rrect_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP elliptical_rrect_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [440u32, 280u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = elliptical_rrect_scene();
    ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "elliptical_rrect", size, &rgba);
}

#[allow(unused_imports)]
use valo::DrawParagraphExt as _;

/// Full 4×4 transforms: the Flutter card tilt — rotateX under a
/// perspective entry — over a rect, an rrect, a stroke, text, and a clip.
/// Property assertions FIRST (the tilt must actually foreshorten: the far
/// edge renders narrower than the near edge), then the pixel golden.
#[test]
fn m12_perspective_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP m12_perspective_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    ctx.set_fonts(text_fonts());
    let size = [360u32, 280u32];

    // rotateX(0.6) with perspective entry(3,2) = 0.004, column-major —
    // exactly what Flutter's Transform widget hands a canvas.
    let (sin, cos) = 0.6_f32.sin_cos();
    #[rustfmt::skip]
    let tilt = valo::Matrix::from_flutter_array(&[
        1.0, 0.0,  0.0, 0.0,
        0.0, cos,  sin, 0.004 * sin,
        0.0, -sin, cos, 0.004 * cos,
        0.0, 0.0,  0.0, 1.0,
    ]);

    let mut b = DisplayListBuilder::new();
    b.save();
    b.translate(180.0, 150.0);
    b.concat(&tilt);
    b.draw_rect(
        Rect::new(-120.0, -80.0, 240.0, 160.0),
        &Paint::from_color(Color::rgb(0.16, 0.2, 0.3)),
    );
    b.draw_rrect_radii_elliptical(
        Rect::new(-100.0, -60.0, 200.0, 50.0),
        [[12.0; 2]; 4],
        &Paint::from_color(Color::rgb(0.35, 0.55, 0.9)),
    );
    b.draw_rect(
        Rect::new(-100.0, 10.0, 200.0, 4.0),
        &Paint::from_color(Color::rgb(0.95, 0.6, 0.25)),
    );
    let mut text = valo::ParagraphBuilder::new(&text_fonts());
    text.add_text(
        "TILTED",
        &valo::TextStyle::new("Fira Sans", 30.0, Color::rgb(0.95, 0.95, 1.0)),
    );
    let mut text = text.build();
    text.layout(200.0);
    b.draw_paragraph(&text, (-52.0, 20.0));
    b.restore();
    let dl = b.build();

    let rgba = ctx.render_to_rgba(&dl, size, Some(Color::rgb(0.07, 0.07, 0.09)));

    // Foreshortening property: the card's top (far) edge spans fewer
    // columns than its bottom (near) edge.
    let span = |row: u32| {
        let mut left = None;
        let mut right = None;
        for x in 0..size[0] {
            let index = ((row * size[0] + x) * 4) as usize;
            let background = rgba[index] == 18 && rgba[index + 1] == 18 && rgba[index + 2] == 23;
            if !background {
                if left.is_none() {
                    left = Some(x);
                }
                right = Some(x);
            }
        }
        match (left, right) {
            (Some(l), Some(r)) => r - l,
            _ => 0,
        }
    };
    let mut top_row = 0;
    for y in 0..size[1] {
        if span(y) > 0 {
            top_row = y;
            break;
        }
    }
    let mut bottom_row = 0;
    for y in (0..size[1]).rev() {
        if span(y) > 0 {
            bottom_row = y;
            break;
        }
    }
    // y-down rotateX(+0.6) tips the TOP edge toward the viewer: the top
    // (near) edge must span more columns than the bottom (far) edge.
    let (near, far) = (span(top_row + 2), span(bottom_row));
    assert!(
        near > far + 20,
        "perspective foreshortens: near {near} vs far {far}"
    );

    valo_harness::assert_golden(goldens_dir(), "m12_perspective", size, &rgba);
}
