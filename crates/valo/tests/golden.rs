//! Golden pixel tests — the browser-free proof that rendering works. Runs on a
//! headless native device; skips (with a note) when the machine has no adapter.
//! `VALO_BLESS=1 cargo test` regenerates the checked-in PNGs.

use std::path::Path;
use std::sync::Arc;

use valo::{
    Backdrop, BlendMode, Color, Context, DisplayListBuilder, MaskBlur, Offscreen, Paint, Rect,
};

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
/// and once stroked, and both take the mask tier — the rasterizer strokes
/// the outline before rasterizing it, so a stroked glyph is just another
/// cached atlas entry.
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
    let mut fonts = text_fonts();
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
    let mut word = |text: &str| {
        let mut builder = ParagraphBuilder::new(&mut fonts);
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

    // Same font, same 72px, same transform, and the paint style no longer
    // changes the tier: both words are cached, batched mask-tier runs.
    assert_eq!(
        stats.text_tiers,
        [2, 0, 0],
        "a stroked run belongs on the atlas, like the filled one"
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

/// Miter spikes on sharp-vertex glyphs. `A`, `V` and `W` meet at angles
/// sharp enough that a miter join reaches many times the half-width past
/// the glyph's own outline, so an atlas cell sized by inflating the fill
/// bounds by a flat half-width clips the spikes — silently, since nothing
/// else in the suite looks at a stroked glyph's extremities.
///
/// The pixel assertions are what survive a re-bless: the stroked ink must
/// reach measurably beyond the filled ink (the scene really does spike),
/// and the tier that caches must agree with the outline tier on where the
/// ink ends (the cell really does hold the spike).
#[test]
fn stroked_glyph_miters_golden() {
    use valo::{DrawGlyphRunExt, DrawParagraphExt, ParagraphBuilder, TextStyle};

    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP stroked_glyph_miters_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let mut fonts = text_fonts();
    let size = [420u32, 200u32];
    let background = Color::rgb(0.06, 0.06, 0.09);

    const STROKE_WIDTH: f32 = 5.0;
    let origin = (30.0, 30.0);

    let mut scene = |stroked: bool| {
        let mut paragraph = ParagraphBuilder::new(&mut fonts);
        paragraph.add_text(
            // `M`'s inner apex spikes 8.6px up and `W`'s 6.7px down at this
            // size — both several half-widths past the fill.
            "AMWV",
            &TextStyle::new("Fira Sans", 72.0, Color::rgb(0.95, 0.96, 1.0)),
        );
        let mut paragraph = paragraph.build();
        paragraph.layout(f32::INFINITY);
        let mut b = DisplayListBuilder::new();
        if stroked {
            b.draw_paragraph_with(
                &paragraph,
                origin,
                &Paint {
                    color: Color::rgb(0.95, 0.96, 1.0),
                    // A limit high enough that every one of these joins
                    // spikes instead of bevelling.
                    style: valo::PaintStyle::Stroke(valo::Stroke {
                        width: STROKE_WIDTH,
                        cap: valo::Cap::Butt,
                        join: valo::Join::Miter,
                        miter_limit: 16.0,
                        dash: None,
                    }),
                    ..Default::default()
                },
            );
        } else {
            b.draw_paragraph(&paragraph, origin);
        }
        b.build()
    };
    let filled = scene(false);
    let stroked = scene(true);

    let mut ink_bounds = |dl: &valo::DisplayList, tiers: valo::TextTiers| {
        ctx.set_text_tiers(tiers);
        let offscreen = Offscreen::new(&device, size);
        let stats = ctx.render(dl, &offscreen.target(Some(background)));
        let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
        (ink_box(&rgba, size), rgba, stats)
    };

    // The tier under test, and the outline tier as the reference: it builds
    // real stroke geometry, so it can never clip at an atlas boundary.
    let (cached_box, cached_rgba, cached_stats) = ink_bounds(&stroked, valo::TextTiers::default());
    let (outline_box, ..) = ink_bounds(
        &stroked,
        valo::TextTiers {
            sdf_min: 0.0,
            path_min: 0.0,
        },
    );
    let (filled_box, ..) = ink_bounds(&filled, valo::TextTiers::default());
    ctx.set_text_tiers(valo::TextTiers::default());

    // The `A` apex spikes up and the `V`/`W` vertices spike down, both well
    // past a half-width — proof the scene exercises miters at all.
    let half = STROKE_WIDTH * 0.5;
    for (edge, reach) in [
        ("top", filled_box[1] as f32 - outline_box[1] as f32),
        ("bottom", outline_box[3] as f32 - filled_box[3] as f32),
    ] {
        assert!(
            reach > half * 1.5,
            "the {edge} miter should reach more than a half-width past the \
             filled glyph, reached {reach}px"
        );
    }

    // The whole point: the cached tier's ink ends where the outline tier's
    // does. A cell short by a spike shows up here as a clipped edge.
    for (edge, cached, reference) in [
        ("left", cached_box[0], outline_box[0]),
        ("top", cached_box[1], outline_box[1]),
        ("right", cached_box[2], outline_box[2]),
        ("bottom", cached_box[3], outline_box[3]),
    ] {
        assert!(
            cached.abs_diff(reference) <= 2,
            "{edge} edge of the stroked ink: cached tier {cached}, \
             outline tier {reference} — a miter spike was clipped"
        );
    }

    assert_eq!(
        cached_stats.text_tiers[1], 0,
        "an SDF encodes distance from a FILL boundary; stroked runs never \
         belong in that tier"
    );

    valo_harness::assert_golden(goldens_dir(), "stroked_glyph_miters", size, &cached_rgba);
}

/// The payoff. A stroked run used to re-tessellate every glyph and emit a
/// draw call per glyph, every frame, because it was pinned to the outline
/// tier. It is now an ordinary mask-tier run: the second frame rasterizes
/// nothing at all, and the whole run collapses into the same single batched
/// draw the identical filled run gets.
#[test]
fn stroked_text_caches_and_batches() {
    use valo::{DrawGlyphRunExt, DrawParagraphExt, ParagraphBuilder, TextStyle};

    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP stroked_text_caches_and_batches: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let mut fonts = text_fonts();
    let size = [700u32, 120u32];
    let offscreen = Offscreen::new(&device, size);

    const TEXT: &str = "Stroked headline caching";
    let mut scene = |stroked: bool| {
        let mut paragraph = ParagraphBuilder::new(&mut fonts);
        paragraph.add_text(TEXT, &TextStyle::new("Fira Sans", 28.0, Color::WHITE));
        let mut paragraph = paragraph.build();
        paragraph.layout(f32::INFINITY);
        let mut b = DisplayListBuilder::new();
        if stroked {
            b.draw_paragraph_with(
                &paragraph,
                (20.0, 30.0),
                &Paint {
                    color: Color::WHITE,
                    style: valo::PaintStyle::Stroke(valo::Stroke::new(2.0)),
                    ..Default::default()
                },
            );
        } else {
            b.draw_paragraph(&paragraph, (20.0, 30.0));
        }
        b.build()
    };
    let stroked = scene(true);
    let filled = scene(false);

    let mut frame = |dl: &valo::DisplayList, tiers: valo::TextTiers| {
        ctx.set_text_tiers(tiers);
        ctx.render(dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))))
    };

    // Cold, then warm. The run is a single mask-tier run either way.
    let cold = frame(&stroked, valo::TextTiers::default());
    let warm = frame(&stroked, valo::TextTiers::default());
    assert_eq!(cold.text_tiers, [1, 0, 0], "a stroked run is a mask run");
    assert!(
        cold.glyph_rasters > 0,
        "the cold frame should have rastered the stroked glyphs"
    );
    assert_eq!(
        warm.glyph_rasters, 0,
        "the warm frame re-used every stroked glyph from the atlas"
    );

    // One batched draw for the whole run, exactly as if it were filled.
    let filled_warm = {
        frame(&filled, valo::TextTiers::default());
        frame(&filled, valo::TextTiers::default())
    };
    assert_eq!(
        warm.draw_calls, filled_warm.draw_calls,
        "a stroked run should batch like a filled one"
    );

    // What it used to cost: the outline tier tessellates and draws every
    // glyph separately, every frame.
    let outlined = frame(
        &stroked,
        valo::TextTiers {
            sdf_min: 0.0,
            path_min: 0.0,
        },
    );
    ctx.set_text_tiers(valo::TextTiers::default());
    assert_eq!(outlined.text_tiers, [0, 0, 1]);
    let inked_glyphs = TEXT.replace(' ', "").len() as u32;
    assert_eq!(warm.draw_calls, 1, "the whole run is one batched draw");
    assert!(
        outlined.draw_calls >= inked_glyphs,
        "the outline tier draws per glyph, and there are {inked_glyphs} of \
         them: {} calls",
        outlined.draw_calls
    );
}

/// The bounding box [x0, y0, x1, y1) of pixels brighter than the dark
/// background, half-open — the shape of the ink, independent of the tier
/// that drew it.
fn ink_box(rgba: &[u8], size: [u32; 2]) -> [u32; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (size[0], size[1], 0, 0);
    for y in 0..size[1] {
        for x in 0..size[0] {
            let i = ((y * size[0] + x) * 4) as usize;
            if rgba[i] < 128 {
                continue;
            }
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    [x0, y0, x1, y1]
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
    let size = [980u32, 200u32];
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

    // 2. Luminance grayscale over a gradient. Impeller applies the filter to
    //    every stop colour before the gradient interpolates them.
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

    // 5. TRANSLUCENT, with a matrix that shifts one channel. At alpha 1 a
    //    missing unpremultiply is invisible, because premultiplied and
    //    straight colour are the same thing there; at alpha 0.5 it moves every
    //    channel by ~50 levels. This is the card that makes the conversion
    //    testable at all.
    #[rustfmt::skip]
    let lift_blue = [
        1.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0, 0.5,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    b.draw_rect(
        card(4),
        &Paint {
            color: Color::rgba(0.8, 0.2, 0.2, 0.5),
            color_filter: Some(valo::ColorFilter::Matrix(lift_blue)),
            ..Default::default()
        },
    );

    // 6. Filtered AND blurred. The blend folds into the solid source, but the
    //    blur still requires the general effect layer.
    b.draw_rrect_radii(
        card(5),
        [20.0; 4],
        &Paint {
            color: Color::rgb(0.95, 0.25, 0.15),
            mask_blur: Some(valo::MaskBlur::new(6.0)),
            color_filter: Some(valo::ColorFilter::Blend(
                Color::rgb(0.55, 0.55, 0.55),
                BlendMode::SrcIn,
            )),
            ..Default::default()
        },
    );

    let stats = ctx.render(&b.build(), &offscreen.target(Some(background)));
    // Every matrix and constant blend folds into the solid/gradient source,
    // exactly as Impeller's `Contents::ApplyColorFilter` does. Only the blur
    // still needs an effect layer.
    assert_eq!(stats.layers_rendered, 1, "only the blur needs a layer");
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

    // Straight colour (0.8, 0.2, 0.2) with blue lifted to 0.7, premultiplied
    // by 0.5 and composited over the background. Skipping the unpremultiply
    // would land near (60, 22, 88) instead — far outside any tolerance.
    let translucent = centre(4);
    assert!(
        close(translucent[0], 0.435) && close(translucent[2], 0.395),
        "a translucent filtered fill must unpremultiply first, got {translucent:?}"
    );

    // The blurred card's core is fully covered, so its colour is the filter's
    // answer: grey, not the red it was painted.
    let blurred = centre(5);
    assert!(
        close(blurred[0], 0.55) && close(blurred[1], 0.55) && close(blurred[2], 0.55),
        "a filtered blur must be the filter's grey, not its painted red, got {blurred:?}"
    );

    valo_harness::assert_golden(goldens_dir(), "color_filters", size, &rgba);
}

/// Patterns — Canvas2D's `createPattern`, as a paint rather than a tiling of
/// separate image draws. The three cards prove the parts that could silently
/// be wrong: that the tile repeats at all, that the paint's local matrix moves
/// the pattern rather than the shape, and that a pattern fills a PATH (not
/// just a rect) the way a gradient does.
#[test]
fn patterns_golden() {
    use valo::{Filter, ImageDesc, PathBuilder, Sampling, Shader, TileMode};

    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP patterns_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [500u32, 200u32];
    let offscreen = Offscreen::new(&device, size);
    let background = Color::rgb(0.07, 0.07, 0.09);

    // A 32px tile: one light quadrant so rotation and offset are legible.
    let tile = ctx.upload_image(
        ImageDesc {
            size: [32, 32],
            premultiplied: true,
            mips: false,
        },
        &checker_pixels(32, 16),
    );
    let repeat = Sampling {
        filter: Filter::Nearest,
        tile_x: TileMode::Repeat,
        tile_y: TileMode::Repeat,
        ..Default::default()
    };
    let pattern = |local: valo::Matrix| {
        Paint::from_shader(Shader::Image {
            image: tile.clone(),
            sampling: repeat,
            local,
        })
    };

    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(20.0, 20.0, 140.0, 160.0),
        &pattern(Default::default()),
    );
    // The same tile, shifted half a tile and turned — the local matrix acts on
    // the PATTERN, so the card stays put while its content moves.
    b.draw_rect(
        Rect::new(180.0, 20.0, 140.0, 160.0),
        &pattern(valo::Matrix::translation(16.0, 0.0).then(&valo::Matrix::rotation(0.4))),
    );
    // Patterns fill paths, which is the whole point of being a paint.
    let mut circle = PathBuilder::new();
    circle.circle((410.0, 100.0), 75.0);
    b.draw_path(
        &circle.build(),
        valo::FillRule::NonZero,
        &pattern(Default::default()),
    );

    let stats = ctx.render(&b.build(), &offscreen.target(Some(background)));
    assert_eq!(stats.draws, 3);
    assert_eq!(
        stats.layers_rendered, 0,
        "a pattern is a paint, so it needs no layer"
    );
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);

    let at = |x: u32, y: u32| {
        let i = ((y * size[0] + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    };
    // The tile is 32px with 16px quadrants, so two points 32px apart inside
    // the first card must land on the same texel — that IS the repeat.
    assert_eq!(
        at(30, 30),
        at(62, 62),
        "one tile apart should be the same texel"
    );
    assert_ne!(at(30, 30), at(46, 30), "half a tile apart should differ");
    // The circle's centre is inside the path, its corner outside: a pattern
    // must respect coverage rather than filling the bounding box.
    let corner = at(410 - 70, 100 - 70);
    assert!(
        corner
            .iter()
            .zip([
                (background.r * 255.0).round() as u8,
                (background.g * 255.0).round() as u8,
                (background.b * 255.0).round() as u8,
            ])
            .all(|(got, want)| got.abs_diff(want) <= 2),
        "outside the circle must stay background, got {corner:?}"
    );

    valo_harness::assert_golden(goldens_dir(), "patterns", size, &rgba);
}

/// Per-axis tiling, which is what Canvas2D's `repeat-x` / `repeat-y` /
/// `no-repeat` need. The point of `Decal` is that it leaves the shape
/// UNPAINTED past one tile — `Clamp` would smear the tile's border texels
/// over the rest instead, which reads as a plausible but wrong image.
#[test]
fn pattern_tile_modes_cover_only_their_own_axes() {
    use valo::{Filter, ImageDesc, Sampling, Shader, TileMode};

    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP pattern_tile_modes_cover_only_their_own_axes");
        return;
    };
    let mut context = Context::new(device, queue);
    let tile = context.upload_image(
        ImageDesc {
            size: [32, 32],
            premultiplied: true,
            mips: false,
        },
        &checker_pixels(32, 16),
    );
    let mut sample = |tile_x: TileMode, tile_y: TileMode| {
        let mut b = DisplayListBuilder::new();
        b.draw_rect(
            Rect::new(0.0, 0.0, 96.0, 96.0),
            &Paint::from_shader(Shader::Image {
                image: tile.clone(),
                sampling: Sampling {
                    filter: Filter::Nearest,
                    tile_x,
                    tile_y,
                    ..Default::default()
                },
                local: valo::Matrix::IDENTITY,
            }),
        );
        let pixels = context.render_to_rgba(&b.build(), [96, 96], Some(Color::TRANSPARENT));
        // Inside the first tile, one tile to the right, one tile down.
        [(8usize, 8usize), (40, 8), (8, 40)].map(|(x, y)| pixels[(y * 96 + x) * 4 + 3])
    };

    let [origin, right, down] = sample(TileMode::Repeat, TileMode::Repeat);
    assert_eq!([origin, right, down], [255, 255, 255], "repeat fills both");

    let [origin, right, down] = sample(TileMode::Repeat, TileMode::Decal);
    assert_eq!([origin, right], [255, 255], "repeat-x still tiles across");
    assert_eq!(down, 0, "repeat-x must paint nothing below the tile");

    let [origin, right, down] = sample(TileMode::Decal, TileMode::Repeat);
    assert_eq!([origin, down], [255, 255], "repeat-y still tiles down");
    assert_eq!(right, 0, "repeat-y must paint nothing beside the tile");

    let [origin, right, down] = sample(TileMode::Decal, TileMode::Decal);
    assert_eq!(origin, 255, "no-repeat still paints the tile itself");
    assert_eq!([right, down], [0, 0], "no-repeat paints nothing else");
}

/// `Sampling` is public and `draw_image_rect` takes it, so `TileMode::Decal`
/// has to mean the same thing there as it does in a pattern. The cutoff used
/// to live only in `fs_pattern`, which quietly degraded a direct draw to
/// clamp — the edge texels smeared instead of stopping.
#[test]
fn a_direct_image_draw_honours_decal_tiling() {
    use valo::{Filter, ImageDesc, Sampling, TileMode};

    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_direct_image_draw_honours_decal_tiling");
        return;
    };
    let mut context = Context::new(device, queue);
    let tile = context.upload_image(
        ImageDesc {
            size: [32, 32],
            premultiplied: true,
            mips: false,
        },
        &checker_pixels(32, 16),
    );
    // The source rect reaches a tile's width past the image on every side,
    // so the quad's outer band samples uv outside [0, 1] — the only place a
    // direct draw's tile mode can show at all. The image itself lands in the
    // middle third of the destination.
    let mut sample = |tile_x: TileMode, tile_y: TileMode| {
        let mut b = DisplayListBuilder::new();
        b.draw_image_rect(
            &tile,
            Rect::new(-32.0, -32.0, 96.0, 96.0),
            Rect::new(0.0, 0.0, 96.0, 96.0),
            Sampling {
                filter: Filter::Nearest,
                tile_x,
                tile_y,
                ..Default::default()
            },
            &Paint::default(),
        );
        let pixels = context.render_to_rgba(&b.build(), [96, 96], Some(Color::TRANSPARENT));
        [(48usize, 48usize), (10, 48), (48, 10)].map(|(x, y)| pixels[(y * 96 + x) * 4 + 3])
    };

    let [inside, left, above] = sample(TileMode::Clamp, TileMode::Clamp);
    assert_eq!(
        [inside, left, above],
        [255, 255, 255],
        "clamp smears the edge texels outwards, which is its whole job"
    );

    let [inside, left, above] = sample(TileMode::Decal, TileMode::Decal);
    assert_eq!(inside, 255, "the image itself still draws");
    assert_eq!(
        [left, above],
        [0, 0],
        "decal paints nothing past the source"
    );
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
        b.save_layer_backdrop(
            Some(rect),
            &Paint::default(),
            Backdrop {
                sigma,
                shared_key: shared,
            },
        );
        b.restore();
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

fn text_fonts() -> valo::FontCollection {
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
    c
}

fn m6_text_scene(fonts: &mut valo::FontCollection) -> valo::DisplayList {
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
    let mut fonts = text_fonts();
    let size = [660u32, 580u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m6_text_scene(&mut fonts);
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

fn m6_features_scene(fonts: &mut valo::FontCollection) -> valo::DisplayList {
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
    let mut fonts = text_fonts();
    let size = [660u32, 260u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m6_features_scene(&mut fonts);
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
fn m8_tier_scene(fonts: &mut valo::FontCollection, zoom: f32) -> valo::DisplayList {
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
    let mut fonts = text_fonts();

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
        let dl = m8_tier_scene(&mut fonts, zoom);
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
    let mut fonts = text_fonts();
    let size = [320u32, 120u32];
    let offscreen = Offscreen::new(&device, size);

    let mut b = DisplayListBuilder::new();
    for (i, dx) in [0.0f32, 0.25, 0.5, 0.75].into_iter().enumerate() {
        let mut p = ParagraphBuilder::new(&mut fonts);
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

fn m9_scene(fonts: &mut valo::FontCollection) -> valo::DisplayList {
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
    let mut fonts = text_fonts();
    let size = [660u32, 260u32];
    let offscreen = Offscreen::new(&device, size);
    let dl = m9_scene(&mut fonts);
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
    let mut fonts = text_fonts();
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
    let mut paragraph = |text: &str, px: f32| {
        let mut b = ParagraphBuilder::new(&mut fonts);
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

/// A bare `move_to` paints nothing under EVERY cap; an explicit zero-length
/// segment paints for round and square. The two reduce to the same single
/// point once flattened, so this is the pixel-level proof that the contour's
/// `has_segments` metadata survives all the way to the rasterizer.
///
/// Chrome agrees: `moveTo(x, y); stroke()` is blank for all three caps, and
/// adding `lineTo(x, y)` gives blank / circle / square.
#[test]
fn a_bare_move_to_paints_nothing_but_a_zero_length_subpath_does() {
    use valo::{Cap, FillRule, PaintStyle, PathBuilder, Stroke};

    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_bare_move_to_paints_nothing_but_a_zero_length_segment_does");
        return;
    };
    let mut context = Context::new(device, queue);
    // 0 = a bare move, 1 = an explicit zero-length line, 2 = a closepath.
    let mut ink = |kind: u8, cap: Cap| {
        let mut path = PathBuilder::new();
        path.move_to((20.0, 20.0));
        match kind {
            1 => {
                path.line_to((20.0, 20.0));
            }
            2 => {
                path.close();
            }
            _ => {}
        }
        let mut paint = Paint {
            color: Color::rgb(1.0, 1.0, 1.0),
            style: PaintStyle::Stroke(Stroke {
                cap,
                ..Stroke::new(16.0)
            }),
            ..Default::default()
        };
        paint.color = Color::rgb(1.0, 1.0, 1.0);
        let mut b = DisplayListBuilder::new();
        b.draw_path(&path.build(), FillRule::NonZero, &paint);
        let pixels = context.render_to_rgba(&b.build(), [40, 40], Some(Color::TRANSPARENT));
        pixels.chunks_exact(4).filter(|p| p[3] > 0).count()
    };

    for cap in [Cap::Butt, Cap::Round, Cap::Square] {
        assert_eq!(ink(0, cap), 0, "a bare move_to must not paint ({cap:?})");
        assert_eq!(
            ink(2, cap),
            ink(1, cap),
            "move+close must paint exactly like an explicit zero-length line ({cap:?})"
        );
    }
    assert_eq!(
        ink(1, Cap::Butt),
        0,
        "a butt cap has no area to give a zero-length segment"
    );
    // A 16px round cap is a disc of radius 8; a square cap is the full box.
    let round = ink(1, Cap::Round);
    let square = ink(1, Cap::Square);
    assert!(
        (180..=220).contains(&round),
        "a round cap should cover ~π·8² ≈ 201 px, got {round}"
    );
    assert!(
        (250..=262).contains(&square),
        "a square cap should cover 16² = 256 px, got {square}"
    );
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
    // EXPLICIT zero-length segments render as their caps (round dot + square
    // dot). A bare `move_to` would paint nothing at all under either cap —
    // that distinction is asserted on pixels in
    // `a_bare_move_to_paints_nothing_but_a_zero_length_segment_does`.
    let mut dot = PathBuilder::new();
    dot.move_to((320.0, 220.0)).line_to((320.0, 220.0));
    let mut round_dot = stroke(26.0);
    if let PaintStyle::Stroke(s) = &mut round_dot.style {
        s.cap = Cap::Round;
        s.join = Join::Round;
    }
    b.draw_path(&dot.build(), FillRule::NonZero, &round_dot);
    let mut dot2 = PathBuilder::new();
    dot2.move_to((400.0, 220.0)).line_to((400.0, 220.0));
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
    tile.save_layer_backdrop(
        Some(Rect::new(0.0, 0.0, 80.0, 60.0)),
        &Paint::default(),
        Backdrop::blur(6.0).shared(7),
    );
    tile.restore();
    tile.save_layer_backdrop(
        Some(Rect::new(90.0, 0.0, 80.0, 60.0)),
        &Paint::default(),
        Backdrop::blur(6.0).shared(7),
    );
    tile.restore();
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
    b.save_layer_backdrop(
        Some(Rect::new(10.0, 10.0, 80.0, 60.0)),
        &Paint::default(),
        Backdrop::blur(4.0).shared(9),
    );
    b.restore();
    b.save_layer_backdrop(
        Some(Rect::new(100.0, 10.0, 80.0, 60.0)),
        &Paint::default(),
        Backdrop::blur(12.0).shared(9),
    );
    b.restore();

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
    let mut scene_fonts = text_fonts();
    let mut text = valo::ParagraphBuilder::new(&mut scene_fonts);
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

#[test]
fn blend_filter_cpu_and_image_gpu_paths_agree() {
    use valo::ImageDesc;

    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP blend_filter_cpu_and_image_gpu_paths_agree: no GPU adapter");
        return;
    };
    let mut context = Context::new(device.clone(), queue.clone());
    let destination = Color::from_rgba8(43, 186, 105, 94);
    let source = Color::from_rgba8(232, 31, 163, 148);
    let image = context.upload_image(
        ImageDesc {
            size: [1, 1],
            premultiplied: false,
            mips: false,
        },
        &[43, 186, 105, 94],
    );
    let modes = [
        BlendMode::Clear,
        BlendMode::Src,
        BlendMode::Dst,
        BlendMode::SrcOver,
        BlendMode::DstOver,
        BlendMode::SrcIn,
        BlendMode::DstIn,
        BlendMode::SrcOut,
        BlendMode::DstOut,
        BlendMode::SrcAtop,
        BlendMode::DstAtop,
        BlendMode::Xor,
        BlendMode::Plus,
        BlendMode::Modulate,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::HardLight,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Multiply,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];
    let mut builder = DisplayListBuilder::new();
    for (index, mode) in modes.into_iter().enumerate() {
        let filter = valo::ColorFilter::Blend(source, mode);
        let x = index as f32 * 2.0;
        builder.draw_image(
            &image,
            Rect::new(x, 0.0, 1.0, 1.0),
            &Paint {
                color_filter: Some(filter),
                ..Default::default()
            },
        );
        builder.draw_rect(
            Rect::new(x, 1.0, 1.0, 1.0),
            &Paint::from_color(filter.folded_into(destination).unwrap()),
        );
        builder.draw_rect(
            Rect::new(x, 2.0, 1.0, 1.0),
            &Paint {
                color: Color::WHITE,
                shader: Some(valo::Shader::Image {
                    image: image.clone(),
                    sampling: Default::default(),
                    local: valo::Matrix::IDENTITY,
                }),
                color_filter: Some(filter),
                ..Default::default()
            },
        );
    }
    let list = builder.build();
    let offscreen = Offscreen::new(&device, [58, 3]);
    let first_stats = context.render(&list, &offscreen.target(Some(Color::TRANSPARENT)));
    let first = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), [58, 3]);
    assert_eq!(first_stats.filter_passes, 29, "one cached image per filter");
    let second_stats = context.render(&list, &offscreen.target(Some(Color::TRANSPARENT)));
    let second = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), [58, 3]);
    assert_eq!(
        second_stats.filter_passes, 0,
        "filtered images are retained"
    );
    assert_eq!(first, second, "warming the cache cannot change pixels");
    let empty = DisplayListBuilder::new().build();
    context.render(&empty, &offscreen.target(Some(Color::TRANSPARENT)));
    let resumed_stats = context.render(&list, &offscreen.target(Some(Color::TRANSPARENT)));
    assert_eq!(
        resumed_stats.filter_passes, 29,
        "an idle frame releases filtered snapshots"
    );
    for index in 0..29usize {
        let gpu = &first[index * 8..index * 8 + 4];
        let cpu_offset = (58 + index * 2) * 4;
        let cpu = &first[cpu_offset..cpu_offset + 4];
        let pattern_offset = (58 * 2 + index * 2) * 4;
        let pattern = &first[pattern_offset..pattern_offset + 4];
        assert!(
            gpu.iter().zip(cpu).all(|(a, b)| a.abs_diff(*b) <= 2),
            "{}: GPU {gpu:?}, CPU {cpu:?}",
            index
        );
        assert_eq!(gpu, pattern, "{index}: direct image and pattern disagree");
    }
}

#[test]
fn matrix_filter_cpu_and_image_gpu_paths_agree() {
    use valo::ImageDesc;

    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP matrix_filter_cpu_and_image_gpu_paths_agree: no GPU adapter");
        return;
    };
    let destination = Color::from_rgba8(43, 186, 105, 94);
    #[rustfmt::skip]
    let matrix = [
        0.2, 0.3, 0.4, 0.1, 0.05,
        0.5, 0.1, 0.2, 0.1, 0.03,
        0.1, 0.4, 0.2, 0.2, 0.07,
        0.1, 0.2, 0.1, 0.6, 0.08,
    ];
    let filter = valo::ColorFilter::Matrix(matrix);
    let mut context = Context::new(device, queue);
    let image = context.upload_image(
        ImageDesc {
            size: [1, 1],
            premultiplied: false,
            mips: false,
        },
        &[43, 186, 105, 94],
    );
    let mut builder = DisplayListBuilder::new();
    builder.draw_image(
        &image,
        Rect::new(0.0, 0.0, 1.0, 1.0),
        &Paint {
            color_filter: Some(filter),
            ..Default::default()
        },
    );
    builder.draw_rect(
        Rect::new(1.0, 0.0, 1.0, 1.0),
        &Paint::from_color(filter.folded_into(destination).unwrap()),
    );
    let pixels = context.render_to_rgba(&builder.build(), [2, 1], Some(Color::TRANSPARENT));
    assert!(
        pixels[..4]
            .iter()
            .zip(&pixels[4..])
            .all(|(gpu, cpu)| gpu.abs_diff(*cpu) <= 2),
        "GPU {:?}, CPU {:?}",
        &pixels[..4],
        &pixels[4..]
    );
}

#[test]
fn direct_image_filters_after_sampling() {
    use valo::ImageDesc;

    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP direct_image_filters_after_sampling: no GPU adapter");
        return;
    };
    let mut context = Context::new(device, queue);
    let image = context.upload_image(
        ImageDesc {
            size: [2, 1],
            premultiplied: false,
            mips: false,
        },
        &[0, 0, 0, 255, 255, 255, 255, 255],
    );
    // Sampling the two texels first produces 0.5, then this nonlinear clamp
    // produces 1.0. Filtering the source first would sample 0 and 1 back to
    // 0.5, which is the ordering bug this test rejects.
    #[rustfmt::skip]
    let double_red = [
        2.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    let mut builder = DisplayListBuilder::new();
    builder.draw_image(
        &image,
        Rect::new(0.0, 0.0, 1.0, 1.0),
        &Paint {
            color_filter: Some(valo::ColorFilter::Matrix(double_red)),
            ..Default::default()
        },
    );
    let pixel = context.render_to_rgba(&builder.build(), [1, 1], Some(Color::TRANSPARENT));
    assert!(
        pixel[0] >= 250,
        "filter must see the sampled color: {pixel:?}"
    );
    assert_eq!(&pixel[1..], &[0, 0, 255]);
}

#[test]
fn color_filter_that_changes_transparent_black_floods_layer_scope() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP color_filter_that_changes_transparent_black_floods_layer_scope");
        return;
    };
    let mut context = Context::new(device, queue);
    let mut matrix = [0.0; 20];
    matrix[4] = 0.25;
    matrix[9] = 0.5;
    matrix[14] = 0.75;
    matrix[19] = 0.5;
    let mut builder = DisplayListBuilder::new();
    builder.save_layer(
        None,
        &Paint {
            color_filter: Some(valo::ColorFilter::Matrix(matrix)),
            ..Default::default()
        },
    );
    builder.restore();
    let pixels = context.render_to_rgba(&builder.build(), [8, 8], Some(Color::TRANSPARENT));
    for pixel in pixels.chunks_exact(4) {
        assert_eq!(pixel, &[64, 128, 191, 128]);
    }
}

#[test]
fn composed_image_filter_runs_inner_before_outer() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP composed_image_filter_runs_inner_before_outer");
        return;
    };
    #[rustfmt::skip]
    let add_red = [
        1.0, 0.0, 0.0, 0.0, 0.2,
        0.0, 1.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    #[rustfmt::skip]
    let double_red = [
        2.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    let filter = valo::ImageFilter::compose(
        valo::ImageFilter::color(valo::ColorFilter::Matrix(double_red)),
        valo::ImageFilter::color(valo::ColorFilter::Matrix(add_red)),
    );
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        Rect::new(0.0, 0.0, 4.0, 4.0),
        &Paint {
            color: Color::rgb(0.1, 0.0, 0.0),
            image_filter: Some(filter),
            ..Default::default()
        },
    );
    builder.save_layer(
        None,
        &Paint {
            color_filter: Some(valo::ColorFilter::Matrix(double_red)),
            image_filter: Some(valo::ImageFilter::color(valo::ColorFilter::Matrix(add_red))),
            ..Default::default()
        },
    );
    builder.draw_rect(
        Rect::new(0.0, 4.0, 4.0, 4.0),
        &Paint::from_color(Color::rgb(0.1, 0.0, 0.0)),
    );
    builder.restore();
    let mut context = Context::new(device, queue);
    let pixels = context.render_to_rgba(&builder.build(), [4, 8], Some(Color::TRANSPARENT));
    for pixel in pixels.chunks_exact(4) {
        assert!(pixel[0].abs_diff(153) <= 2, "unexpected pixel {pixel:?}");
        assert_eq!(&pixel[1..], &[0, 0, 255]);
    }
}

/// A blur wide enough to downsample must still spread in every direction.
/// The downsample passes used to map their (smaller) target quad through the
/// SOURCE's extent rather than their own, so they read only the top-left
/// corner of their input — the halo lost its left and top halves at exactly
/// the σ where `blur_scale` first drops below 1.
#[test]
fn a_downsampled_blur_spreads_symmetrically() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_downsampled_blur_spreads_symmetrically");
        return;
    };
    let mut context = Context::new(device, queue);
    for sigma in [3.0f32, 6.0, 12.0, 24.0] {
        let mut b = DisplayListBuilder::new();
        b.draw_rect(
            Rect::new(90.0, 90.0, 60.0, 60.0),
            &Paint {
                color: Color::rgb(1.0, 1.0, 1.0),
                image_filter: Some(valo::ImageFilter::blur(sigma, sigma)),
                ..Default::default()
            },
        );
        let pixels = context.render_to_rgba(&b.build(), [240, 240], Some(Color::TRANSPARENT));
        let alpha = |x: usize, y: usize| pixels[(y * 240 + x) * 4 + 3] as i32;
        let reach = (sigma * 1.5).round() as usize;
        let (left, right) = (alpha(90 - reach, 120), alpha(150 + reach, 120));
        let (top, bottom) = (alpha(120, 90 - reach), alpha(120, 150 + reach));
        assert!(left > 8, "σ={sigma} lost its left spread (α={left})");
        assert!(top > 8, "σ={sigma} lost its top spread (α={top})");
        assert!(
            (left - right).abs() <= 16 && (top - bottom).abs() <= 16,
            "σ={sigma} is lopsided: l={left} r={right} t={top} b={bottom}"
        );
    }
}

/// A blur downsampled hard enough that one work texel covers sixteen output
/// pixels has to upscale symmetrically. Filter targets used to snap up to the
/// 32px pool bucket, so the linear sampler's half-texel reach past the used
/// corner found cleared texels on the right and bottom while clamp-to-edge
/// held the left and top — the far borders faded away over half a work texel
/// and the near ones did not. Centroid alone is too coarse for that; the
/// border profiles have to mirror.
#[test]
fn a_sixteenth_scale_blur_upscales_symmetrically() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_sixteenth_scale_blur_upscales_symmetrically");
        return;
    };
    // σ 64 puts `blur_scale` at 1/16, so a 128px layer blurs at 8×8.
    let side = 128usize;
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        Rect::new(40.0, 40.0, 48.0, 48.0),
        &Paint {
            color: Color::rgb(1.0, 0.31, 0.47),
            image_filter: Some(valo::ImageFilter::blur(64.0, 64.0)),
            ..Default::default()
        },
    );
    let mut context = Context::new(device, queue);
    let pixels = context.render_to_rgba(
        &builder.build(),
        [side as u32, side as u32],
        Some(Color::TRANSPARENT),
    );
    let alpha = |x: usize, y: usize| i32::from(pixels[(y * side + x) * 4 + 3]);
    let middle = side / 2;
    // One whole work texel in from each border is where the fade lived.
    for step in 0..16 {
        let (near, far) = (step, side - 1 - step);
        let (left, right) = (alpha(near, middle), alpha(far, middle));
        let (top, bottom) = (alpha(middle, near), alpha(middle, far));
        assert!(
            (left - right).abs() <= 2,
            "column {near} (α={left}) and column {far} (α={right}) must mirror"
        );
        assert!(
            (top - bottom).abs() <= 2,
            "row {near} (α={top}) and row {far} (α={bottom}) must mirror"
        );
    }
}

/// Alpha-weighted centroid of an RGBA buffer, in pixels.
fn ink_centroid(pixels: &[u8], size: [usize; 2]) -> (f32, f32) {
    let (mut weight, mut sum_x, mut sum_y) = (0.0f64, 0.0f64, 0.0f64);
    for y in 0..size[1] {
        for x in 0..size[0] {
            let alpha = f64::from(pixels[(y * size[0] + x) * 4 + 3]);
            weight += alpha;
            sum_x += alpha * x as f64;
            sum_y += alpha * y as f64;
        }
    }
    assert!(weight > 0.0, "nothing was drawn");
    ((sum_x / weight) as f32, (sum_y / weight) as f32)
}

/// A Canvas2D shadow is a `save_layer` carrying BOTH a mask blur and a colour
/// matrix, which is the one route into `mask_blur_then_recolour`. The recolour
/// pass used to read the blur's texture as a raw layer, discarding the used
/// corner a downsampled blur leaves behind — so past σ 4√2, where `blur_scale`
/// first drops to ½, the halo was rescaled into the layer's top-left quadrant.
/// A CSS-filter blur sweep cannot see this: that path never opens a subpass
/// with a colour filter over the blur.
#[test]
fn a_recoloured_mask_blur_stays_centred_past_the_downsample_threshold() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_recoloured_mask_blur_stays_centred_past_the_downsample_threshold");
        return;
    };
    // Straight-through RGB with alpha retinted to the shadow colour — the same
    // shape of matrix Canvas2D's shadow uses, and not foldable into the paint.
    let mut shadow_colour = [0.0f32; 20];
    shadow_colour[4] = 0.82;
    shadow_colour[9] = 0.20;
    shadow_colour[14] = 0.27;
    shadow_colour[18] = 1.0;

    let size = [240usize, 240];
    let mut context = Context::new(device, queue);
    for sigma in [4.0f32, 5.6, 5.7, 12.0, 24.0] {
        let mut builder = DisplayListBuilder::new();
        builder.save_layer(
            None,
            &Paint {
                color_filter: Some(valo::ColorFilter::Matrix(shadow_colour)),
                mask_blur: Some(MaskBlur::new(sigma)),
                ..Default::default()
            },
        );
        builder.draw_rect(
            Rect::new(104.0, 104.0, 32.0, 32.0),
            &Paint::from_color(Color::rgb(1.0, 0.31, 0.47)),
        );
        builder.restore();
        let pixels = context.render_to_rgba(
            &builder.build(),
            [size[0] as u32, size[1] as u32],
            Some(Color::TRANSPARENT),
        );
        let (x, y) = ink_centroid(&pixels, size);
        assert!(
            (x - 120.0).abs() <= 1.5 && (y - 120.0).abs() <= 1.5,
            "σ={sigma}: recoloured blur centroid ({x:.2}, {y:.2}) drifted off the source"
        );
    }
}

/// `ImageFilter::DropShadow` — CSS `filter: drop-shadow()`. Varies offset
/// direction, σ, and shadow colour, and includes a translucent source so the
/// "shadow comes from the input's ALPHA, not its colour" rule is visible.
fn drop_shadow_scene() -> valo::DisplayList {
    use valo::{ImageFilter, Point};
    let cases = [
        (Point::new(8.0, 8.0), 0.0, Color::rgba(0.0, 0.0, 0.0, 0.7)),
        (Point::new(8.0, 8.0), 4.0, Color::rgba(0.0, 0.0, 0.0, 0.7)),
        (Point::new(-10.0, 6.0), 6.0, Color::rgba(0.1, 0.2, 0.8, 0.9)),
        (Point::new(0.0, 0.0), 10.0, Color::rgba(0.9, 0.1, 0.1, 1.0)),
    ];
    let mut b = DisplayListBuilder::new();
    for (index, (offset, sigma, color)) in cases.into_iter().enumerate() {
        let x = 40.0 + index as f32 * 130.0;
        for (row, alpha) in [1.0, 0.45].into_iter().enumerate() {
            let y = 40.0 + row as f32 * 130.0;
            b.draw_rrect(
                Rect::new(x, y, 80.0, 80.0),
                14.0,
                &Paint {
                    color: Color::rgba(1.0, 0.72, 0.15, alpha),
                    image_filter: Some(ImageFilter::drop_shadow(offset, sigma, sigma, color)),
                    ..Default::default()
                },
            );
        }
    }
    b.build()
}

#[test]
fn image_filter_drop_shadow_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP image_filter_drop_shadow_golden: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let size = [560u32, 300u32];
    let offscreen = Offscreen::new(&device, size);
    ctx.render(
        &drop_shadow_scene(),
        &offscreen.target(Some(Color::rgb(0.93, 0.94, 0.96))),
    );
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    valo_harness::assert_golden(goldens_dir(), "image_filter_drop_shadow", size, &rgba);
}

/// A rotated explicit `save_layer` must still hold its whole shadow.
///
/// Record-time padding used to be `max(local padding) × max_scale`, which is
/// 10 for a pure rotation — but a 45° turn sends a local `(10, 10)` offset to
/// 14.14 device px, and the combine pass forces everything past the layer
/// transparent. The shadow lost its last 4 px silently.
#[test]
fn a_rotated_layer_keeps_its_whole_drop_shadow() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_rotated_layer_keeps_its_whole_drop_shadow");
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.translate(60.0, 40.0);
    b.rotate(std::f32::consts::FRAC_PI_4);
    b.save_layer(
        None,
        &Paint {
            image_filter: Some(valo::ImageFilter::drop_shadow(
                valo::Point::new(10.0, 10.0),
                0.0,
                0.0,
                Color::rgb(1.0, 0.0, 0.0),
            )),
            ..Default::default()
        },
    );
    b.draw_rect(
        Rect::new(-8.0, -8.0, 16.0, 16.0),
        &Paint::from_color(Color::rgb(1.0, 1.0, 1.0)),
    );
    b.restore();

    let mut context = Context::new(device, queue);
    let pixels = context.render_to_rgba(&b.build(), [120, 120], Some(Color::TRANSPARENT));
    let at = |x: usize, y: usize| {
        let start = (y * 120 + x) * 4;
        [pixels[start], pixels[start + 1], pixels[start + 2]]
    };
    // The rotated square is centred on (60, 40); (10, 10) turned 45° is
    // (0, 14.14), so the shadow's centre lands at (60, 54.14) and its far
    // corner reaches y ≈ 65.5. The scalar bound cropped it at y ≈ 61.3, so
    // y = 63 is the pixel that tells the two apart.
    assert_eq!(at(60, 40), [255, 255, 255], "the source draws as recorded");
    assert_eq!(at(60, 52), [255, 0, 0], "the shadow reaches its offset");
    assert_eq!(at(60, 63), [255, 0, 0], "the shadow is not cropped short");
    assert_eq!(at(60, 70), [0, 0, 0], "and it still ends somewhere");
}

/// The three rules a drop shadow has to obey, asserted on pixels rather than
/// eyeballed: the source survives untouched, an unblurred shadow lands at
/// exactly the offset, and nothing outside source ∪ shadow is painted.
#[test]
fn drop_shadow_places_the_shadow_at_the_offset() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP drop_shadow_places_the_shadow_at_the_offset");
        return;
    };
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(4.0, 4.0, 8.0, 8.0),
        &Paint {
            color: Color::rgb(1.0, 1.0, 1.0),
            image_filter: Some(valo::ImageFilter::drop_shadow(
                valo::Point::new(8.0, 8.0),
                0.0,
                0.0,
                Color::rgb(1.0, 0.0, 0.0),
            )),
            ..Default::default()
        },
    );
    let mut context = Context::new(device, queue);
    let pixels = context.render_to_rgba(&b.build(), [24, 24], Some(Color::TRANSPARENT));
    let at = |x: usize, y: usize| {
        let start = (y * 24 + x) * 4;
        [
            pixels[start],
            pixels[start + 1],
            pixels[start + 2],
            pixels[start + 3],
        ]
    };
    assert_eq!(at(8, 8), [255, 255, 255, 255], "the source is untouched");
    assert_eq!(at(16, 16), [255, 0, 0, 255], "the shadow sits at +8,+8");
    assert_eq!(at(2, 2), [0, 0, 0, 0], "nothing leaks before the source");
    assert_eq!(at(22, 22), [0, 0, 0, 0], "nothing leaks past the shadow");
}

/// Every blend mode, evaluated twice: once folded into a solid paint on the
/// CPU (`valo-dl`'s `color_filter` module) and once through the WGSL filter
/// pass a layer takes. The two implementations are independent transcriptions
/// of the same equations, and nothing else in the suite compares them — so
/// this is the only thing standing between a typo in one of them and silently
/// wrong pixels.
#[test]
fn cpu_and_shader_color_filters_agree() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP cpu_and_shader_color_filters_agree");
        return;
    };
    const MODES: [BlendMode; 29] = [
        BlendMode::Clear,
        BlendMode::Src,
        BlendMode::Dst,
        BlendMode::SrcOver,
        BlendMode::DstOver,
        BlendMode::SrcIn,
        BlendMode::DstIn,
        BlendMode::SrcOut,
        BlendMode::DstOut,
        BlendMode::SrcAtop,
        BlendMode::DstAtop,
        BlendMode::Xor,
        BlendMode::Plus,
        BlendMode::Modulate,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::HardLight,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Multiply,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];
    // Asymmetric channels: a grey pair would agree under Hue and Saturation
    // whatever the code did.
    let destination = Color::rgb(0.25, 0.6, 0.85);
    let source = Color::rgba(0.9, 0.35, 0.15, 0.75);
    let mut context = Context::new(device, queue);

    for mode in MODES {
        let filter = valo::ColorFilter::Blend(source, mode);
        let left = Rect::new(0.0, 0.0, 4.0, 4.0);
        let right = Rect::new(4.0, 0.0, 4.0, 4.0);

        let mut builder = DisplayListBuilder::new();
        // CPU: a solid paint absorbs the filter in `folded_paint`.
        builder.draw_rect(
            left,
            &Paint {
                color: destination,
                color_filter: Some(filter),
                ..Default::default()
            },
        );
        // GPU: the same filter on a bounded layer runs as a WGSL pass.
        builder.save_layer(
            Some(right),
            &Paint {
                color_filter: Some(filter),
                ..Default::default()
            },
        );
        builder.draw_rect(right, &Paint::from_color(destination));
        builder.restore();

        let pixels = context.render_to_rgba(&builder.build(), [8, 4], Some(Color::TRANSPARENT));
        for y in 0..4usize {
            for x in 0..4usize {
                let folded = 4 * (y * 8 + x);
                let shaded = 4 * (y * 8 + x + 4);
                let (a, b) = (&pixels[folded..folded + 4], &pixels[shaded..shaded + 4]);
                let apart = (0..4).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0);
                assert!(
                    apart <= 3,
                    "{mode:?}: CPU fold {a:?} vs shader {b:?} at ({x}, {y})"
                );
            }
        }
    }
}
