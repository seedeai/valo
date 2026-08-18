//! Whole-planner regression scenes. `rect_scene` grew one construction per
//! milestone, so a single render of it walks nearly every path the planner
//! knows; `tests/goldens/planner_scene.png` is its blessed output and the
//! pinned stats keep it honest about which paths it still reaches.
//!
//! That golden was blessed from the planner this one replaces, which makes
//! it a true baseline rather than a self-portrait: re-blessing it would
//! throw away the only record of what the old planner drew. A diff there is
//! a regression to explain, not a golden to accept.

use std::sync::Arc;

use valo::{
    Backdrop, BlendMode, ClipOp, Color, Context, DisplayListBuilder, DrawGlyphRunExt, FillRule,
    GradientStop, ImageDesc, MaskBlur, MaskKind, Offscreen, Paint, ParagraphBuilder, Point, Rect,
    Sampling, Shader, TextStyle,
};

/// `checker_pixels` is a premultiplied RGBA checkerboard for pattern tests.
fn checker_pixels(size: usize, cell: usize) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let on = ((x / cell) + (y / cell)).is_multiple_of(2);
            let i = (y * size + x) * 4;
            px[i..i + 4].copy_from_slice(if on {
                &[220, 130, 40, 255]
            } else {
                &[30, 60, 110, 255]
            });
        }
    }
    px
}

fn stops(colors: &[(f32, Color)]) -> Vec<GradientStop> {
    colors
        .iter()
        .map(|&(offset, color)| GradientStop { offset, color })
        .collect()
}

/// `star` is a five-point self-intersecting path — fill rules differ on it.
fn star(c: Point, r: f32) -> std::sync::Arc<valo::Path> {
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

/// M1+M2 coverage: solid rects (opaque promotion + hoisting, translucency),
/// save/transform/restore, an off-viewport rect for the cull path, opacity
/// layers both elided (disjoint children) and materialized (overlapping
/// children, nested inside the elided scope), a mask layer, and advanced
/// blends on both a solid rect and a layer composite. M3a adds gradients
/// (incl. a baked >8-stop ramp), patterns, paths, strokes, the analytic
/// rrect blur, CPU-folded colour filters, an inline image colour filter,
/// and a textured advanced blend (the implicit-layer route). M3b adds
/// effect layers and the filter recipes. M4 adds depth clips, all three
/// text tiers with their layer routes, and embedded lists both inline and
/// raster-cached.
fn rect_scene(ctx: &mut Context) -> valo::DisplayList {
    let alpha_layer = |a: f32| Paint {
        color: Color::rgba(0.0, 0.0, 0.0, a),
        ..Default::default()
    };
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(40.0, 40.0, 260.0, 180.0),
        &Paint::from_color(Color::rgb(0.85, 0.3, 0.25)),
    );
    b.draw_rect(
        Rect::new(120.0, 100.0, 260.0, 180.0),
        &Paint::from_color(Color::rgba(0.2, 0.5, 0.9, 0.6)),
    );
    b.save();
    b.translate(320.0, 60.0);
    b.scale(1.5, 1.5);
    b.draw_rect(
        Rect::new(0.0, 0.0, 120.0, 90.0),
        &Paint::from_color(Color::rgb(0.3, 0.7, 0.4)),
    );
    b.restore();
    // Behind the translucent one: must be hoisted (opaque) yet composed under.
    b.draw_rect(
        Rect::new(200.0, 240.0, 180.0, 120.0),
        &Paint::from_color(Color::rgb(0.95, 0.8, 0.2)),
    );
    b.draw_rect(
        Rect::new(5000.0, 5000.0, 50.0, 50.0),
        &Paint::from_color(Color::BLACK),
    );

    // Elided opacity group (disjoint children), with a MATERIALIZED nested
    // group inside it (overlapping children) — the nested composite must
    // absorb the outer group's alpha exactly once.
    b.save_layer(None, &alpha_layer(0.7));
    b.draw_rect(
        Rect::new(20.0, 300.0, 80.0, 60.0),
        &Paint::from_color(Color::rgb(0.2, 0.4, 0.9)),
    );
    b.save_layer(None, &alpha_layer(0.6));
    b.draw_rect(
        Rect::new(120.0, 300.0, 80.0, 60.0),
        &Paint::from_color(Color::rgb(0.9, 0.4, 0.2)),
    );
    b.draw_rect(
        Rect::new(150.0, 320.0, 80.0, 60.0),
        &Paint::from_color(Color::rgb(0.4, 0.9, 0.2)),
    );
    b.restore();
    b.restore();

    // A mask layer: luminance of two bars gates what's beneath them.
    b.save_layer_mask(
        Some(Rect::new(380.0, 280.0, 200.0, 140.0)),
        MaskKind::Luminance,
    );
    b.draw_rect(
        Rect::new(380.0, 280.0, 200.0, 60.0),
        &Paint::from_color(Color::WHITE),
    );
    b.draw_rect(
        Rect::new(380.0, 360.0, 200.0, 60.0),
        &Paint::from_color(Color::rgb(0.4, 0.4, 0.4)),
    );
    b.restore();

    // Destination-reading blends: a solid Multiply rect (snapshot + fragment
    // blend), and a Multiply layer composite (implicit break at close).
    b.draw_rect(
        Rect::new(60.0, 380.0, 160.0, 80.0),
        &Paint {
            color: Color::rgba(0.9, 0.6, 0.3, 0.8),
            blend_mode: BlendMode::Multiply,
            ..Default::default()
        },
    );
    b.save_layer(
        None,
        &Paint {
            color: Color::rgba(0.0, 0.0, 0.0, 0.9),
            blend_mode: BlendMode::Overlay,
            ..Default::default()
        },
    );
    b.draw_rect(
        Rect::new(260.0, 380.0, 160.0, 80.0),
        &Paint::from_color(Color::rgb(0.3, 0.5, 0.8)),
    );
    b.restore();

    // A fully off-viewport group: the whole scope must be SKIPPED, planning
    // nothing (not elided, not rendered).
    b.save_layer(None, &alpha_layer(0.5));
    b.draw_rect(
        Rect::new(6000.0, 6000.0, 40.0, 40.0),
        &Paint::from_color(Color::BLACK),
    );
    b.restore();

    // A materialized layer hosting two M2 edge paths: a solid Multiply child
    // (dst-read break while the CURRENT target is a layer — forces the
    // persistent-attachment swap on a layer target), and a skipped mask
    // whose erase must blank the LAYER, not the scene. A mask's DstIn blend
    // is destructive, so a hint-less mask scope floods to the clip and
    // always materializes — the skip case needs an off-viewport HINT (the
    // hint is a crop). The trailing rect proves painting continues after
    // the erase.
    b.save_layer(None, &alpha_layer(0.85));
    b.draw_rect(
        Rect::new(440.0, 60.0, 120.0, 90.0),
        &Paint::from_color(Color::rgb(0.7, 0.3, 0.6)),
    );
    b.draw_rect(
        Rect::new(480.0, 100.0, 120.0, 90.0), // overlaps: the group materializes
        &Paint::from_color(Color::rgb(0.3, 0.6, 0.7)),
    );
    b.draw_rect(
        Rect::new(460.0, 80.0, 100.0, 60.0),
        &Paint {
            color: Color::rgba(0.9, 0.9, 0.2, 0.7),
            blend_mode: BlendMode::Multiply,
            ..Default::default()
        },
    );
    b.save_layer_mask(Some(Rect::new(7000.0, 7000.0, 40.0, 40.0)), MaskKind::Alpha);
    b.draw_rect(
        Rect::new(7000.0, 7000.0, 40.0, 40.0),
        &Paint::from_color(Color::WHITE),
    );
    b.restore();
    b.draw_rect(
        Rect::new(520.0, 160.0, 60.0, 40.0),
        &Paint::from_color(Color::rgb(0.95, 0.5, 0.1)),
    );
    b.restore();

    // ── M3a: shaders, paths, images, analytic blur, folds ──────────────────

    let checker = ctx.upload_image(
        ImageDesc {
            size: [64, 64],
            premultiplied: true,
            mips: false,
        },
        &checker_pixels(64, 8),
    );

    // Gradients: linear, focal radial, sweep, and a >8-stop baked ramp.
    b.draw_rect(
        Rect::new(20.0, 470.0, 130.0, 70.0),
        &Paint::from_shader(Shader::Linear {
            start: Point::new(20.0, 470.0),
            end: Point::new(150.0, 540.0),
            stops: stops(&[
                (0.0, Color::rgb(1.0, 0.2, 0.2)),
                (1.0, Color::rgb(0.2, 0.2, 1.0)),
            ]),
            spread: valo::SpreadMode::Pad,
            local: valo::Matrix::IDENTITY,
        }),
    );
    b.draw_rect(
        Rect::new(170.0, 470.0, 130.0, 70.0),
        &Paint::from_shader(Shader::Radial {
            center: Point::new(235.0, 505.0),
            radius: 70.0,
            focus: Some(valo::FocalCircle {
                center: Point::new(210.0, 490.0),
                radius: 0.0,
            }),
            stops: stops(&[(0.0, Color::WHITE), (1.0, Color::rgb(0.1, 0.4, 0.2))]),
            spread: valo::SpreadMode::Repeat,
            local: valo::Matrix::IDENTITY,
        }),
    );
    b.draw_rect(
        Rect::new(320.0, 470.0, 130.0, 70.0),
        &Paint::from_shader(Shader::Sweep {
            center: Point::new(385.0, 505.0),
            start_angle: 0.7,
            stops: stops(&[
                (0.0, Color::rgb(0.9, 0.9, 0.1)),
                (1.0, Color::rgb(0.5, 0.1, 0.6)),
            ]),
            local: valo::Matrix::IDENTITY,
        }),
    );
    let many: Vec<(f32, Color)> = (0..12)
        .map(|i| {
            let t = i as f32 / 11.0;
            (t, Color::rgb(t, 1.0 - t, (t * 6.3).sin().abs()))
        })
        .collect();
    b.draw_rect(
        Rect::new(470.0, 470.0, 150.0, 34.0),
        &Paint::from_shader(Shader::Linear {
            start: Point::new(470.0, 470.0),
            end: Point::new(620.0, 470.0),
            stops: stops(&many),
            spread: valo::SpreadMode::Pad,
            local: valo::Matrix::IDENTITY,
        }),
    );

    // A pattern (image shader) and a direct image with an inline colour
    // filter on the sampled pixel.
    b.draw_rect(
        Rect::new(470.0, 510.0, 70.0, 60.0),
        &Paint::from_shader(Shader::Image {
            image: checker.clone(),
            sampling: Sampling::default(),
            local: valo::Matrix::IDENTITY,
        }),
    );
    let grayscale = [
        0.299, 0.587, 0.114, 0.0, 0.0, //
        0.299, 0.587, 0.114, 0.0, 0.0, //
        0.299, 0.587, 0.114, 0.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    b.draw_image_rect(
        &checker,
        Rect::new(0.0, 0.0, 64.0, 64.0),
        Rect::new(550.0, 510.0, 70.0, 60.0),
        Sampling::default(),
        &Paint {
            color_filter: Some(valo::ColorFilter::Matrix(grayscale)),
            ..Default::default()
        },
    );

    // Paths: an even-odd star fill, a dashed round stroke, and a solid
    // Multiply star fill (stencil-then-cover whose COVER does the blend).
    b.draw_path(
        &star(Point::new(70.0, 610.0), 45.0),
        valo::FillRule::EvenOdd,
        &Paint::from_color(Color::rgba(0.9, 0.5, 0.1, 0.9)),
    );
    b.draw_path(
        &star(Point::new(180.0, 610.0), 45.0),
        valo::FillRule::NonZero,
        &Paint {
            color: Color::rgb(0.2, 0.7, 0.9),
            style: valo::PaintStyle::Stroke(valo::Stroke {
                width: 5.0,
                cap: valo::Cap::Round,
                join: valo::Join::Round,
                miter_limit: 4.0,
                dash: Some(valo::Dash {
                    intervals: vec![12.0, 6.0],
                    phase: 0.0,
                }),
            }),
            ..Default::default()
        },
    );
    b.draw_path(
        &star(Point::new(290.0, 610.0), 45.0),
        valo::FillRule::NonZero,
        &Paint {
            color: Color::rgba(0.4, 0.9, 0.4, 0.8),
            blend_mode: BlendMode::Multiply,
            ..Default::default()
        },
    );

    // The analytic blurred rect (recorded as RRectBlur — no layer).
    b.draw_rect(
        Rect::new(360.0, 580.0, 100.0, 60.0),
        &Paint {
            color: Color::rgb(0.1, 0.1, 0.3),
            mask_blur: Some(MaskBlur::new(6.0)),
            ..Default::default()
        },
    );

    // CPU-folded colour filters: into a solid, and into a gradient's stops.
    b.draw_rect(
        Rect::new(480.0, 580.0, 60.0, 60.0),
        &Paint {
            color: Color::rgb(0.9, 0.2, 0.2),
            color_filter: Some(valo::ColorFilter::Matrix(grayscale)),
            ..Default::default()
        },
    );
    b.draw_rect(
        Rect::new(550.0, 580.0, 60.0, 60.0),
        &Paint {
            shader: Some(Shader::Linear {
                start: Point::new(550.0, 580.0),
                end: Point::new(610.0, 640.0),
                stops: stops(&[
                    (0.0, Color::rgb(1.0, 0.0, 0.0)),
                    (1.0, Color::rgb(0.0, 0.0, 1.0)),
                ]),
                spread: valo::SpreadMode::Pad,
                local: valo::Matrix::IDENTITY,
            }),
            color_filter: Some(valo::ColorFilter::Matrix(grayscale)),
            ..Default::default()
        },
    );

    // A TEXTURED advanced blend: the pattern materializes in an implicit
    // layer whose composite runs Multiply.
    b.draw_rect(
        Rect::new(20.0, 560.0, 80.0, 30.0),
        &Paint {
            shader: Some(Shader::Image {
                image: checker.clone(),
                sampling: Sampling::default(),
                local: valo::Matrix::IDENTITY,
            }),
            blend_mode: BlendMode::Multiply,
            ..Default::default()
        },
    );

    // ── M3b: effect layers + the filter recipes ─────────────────────────────

    // A mask-blurred path: sharp draw into an effect layer, blur chain,
    // composite.
    b.draw_path(
        &star(Point::new(60.0, 730.0), 40.0),
        valo::FillRule::NonZero,
        &Paint {
            color: Color::rgb(0.9, 0.3, 0.5),
            mask_blur: Some(MaskBlur::new(3.0)),
            ..Default::default()
        },
    );
    // A styled (Outer) blur on a gradient rect: blur + mask-combine pass.
    b.draw_rect(
        Rect::new(130.0, 690.0, 90.0, 70.0),
        &Paint {
            shader: Some(Shader::Linear {
                start: Point::new(130.0, 690.0),
                end: Point::new(220.0, 760.0),
                stops: stops(&[
                    (0.0, Color::rgb(0.2, 0.9, 0.9)),
                    (1.0, Color::rgb(0.9, 0.2, 0.9)),
                ]),
                spread: valo::SpreadMode::Pad,
                local: valo::Matrix::IDENTITY,
            }),
            mask_blur: Some(MaskBlur {
                sigma: 4.0,
                style: valo::BlurStyle::Outer,
            }),
            ..Default::default()
        },
    );
    // A save_layer whose paint carries blur + colour filter: the subpass
    // ordering (blur first, then recolour the halo).
    b.save_layer(
        None,
        &Paint {
            color: Color::rgba(0.0, 0.0, 0.0, 0.9),
            image_filter: Some(valo::ImageFilter::blur(5.0, 5.0)),
            color_filter: Some(valo::ColorFilter::Matrix(grayscale)),
            ..Default::default()
        },
    );
    b.draw_rect(
        Rect::new(250.0, 690.0, 80.0, 60.0),
        &Paint::from_color(Color::rgb(0.9, 0.6, 0.1)),
    );
    b.restore();
    // A drop shadow via image filter on a rect draw.
    b.draw_rect(
        Rect::new(360.0, 690.0, 80.0, 60.0),
        &Paint {
            color: Color::rgb(0.3, 0.8, 0.5),
            image_filter: Some(valo::ImageFilter::DropShadow {
                offset: Point::new(6.0, 6.0),
                sigma_x: 4.0,
                sigma_y: 4.0,
                color: Color::rgba(0.0, 0.0, 0.0, 0.6),
            }),
            ..Default::default()
        },
    );
    // A pattern with a colour filter: prefiltered texture, direct draw.
    b.draw_rect(
        Rect::new(470.0, 690.0, 70.0, 60.0),
        &Paint {
            shader: Some(Shader::Image {
                image: checker,
                sampling: Sampling::default(),
                local: valo::Matrix::IDENTITY,
            }),
            color_filter: Some(valo::ColorFilter::Matrix(grayscale)),
            ..Default::default()
        },
    );
    // A big-σ blur: the downsample chain runs before the separable passes.
    b.draw_rect(
        Rect::new(550.0, 690.0, 70.0, 60.0),
        &Paint {
            color: Color::rgb(0.9, 0.9, 0.9),
            mask_blur: Some(MaskBlur::new(12.0)),
            shader: Some(Shader::Linear {
                start: Point::new(550.0, 690.0),
                end: Point::new(620.0, 750.0),
                stops: stops(&[(0.0, Color::WHITE), (1.0, Color::rgb(0.4, 0.4, 0.9))]),
                spread: valo::SpreadMode::Pad,
                local: valo::Matrix::IDENTITY,
            }),
            ..Default::default()
        },
    );

    // ── M4: clips, text, embedded lists ─────────────────────────────────────

    // An Intersect clip ceilings the star's EXTERIOR, so only the star-shaped
    // part of the rect survives; the following Difference clip ceilings a
    // rect's INTERIOR, punching a hole in the fill under it.
    b.save();
    b.clip_path(
        &star(Point::new(95.0, 855.0), 55.0),
        FillRule::NonZero,
        ClipOp::Intersect,
    );
    b.draw_rect(
        Rect::new(30.0, 795.0, 130.0, 120.0),
        &Paint::from_color(Color::rgb(0.95, 0.7, 0.2)),
    );
    b.restore();
    b.save();
    b.clip_rect(Rect::new(200.0, 825.0, 60.0, 60.0), ClipOp::Difference);
    b.draw_rect(
        Rect::new(175.0, 795.0, 130.0, 120.0),
        &Paint::from_color(Color::rgb(0.3, 0.8, 0.9)),
    );
    b.restore();
    // Past the restores both ceilings have expired: this rect draws whole,
    // straight across the region the two clips scoped.
    b.draw_rect(
        Rect::new(30.0, 920.0, 275.0, 14.0),
        &Paint::from_color(Color::rgba(0.9, 0.3, 0.6, 0.85)),
    );

    // Text, one run per route. Sizes are DEVICE px, so 22 and 26 take the
    // mask tier (snapped and transformed), 200 the SDF tier, and 340 real
    // outlines.
    let mut fonts = valo_harness::example_fonts();
    let mut paragraph = |text: &str, px: f32| {
        let mut builder = ParagraphBuilder::new(&mut fonts);
        builder.add_text(text, &TextStyle::new("Fira Sans", px, Color::WHITE));
        let mut laid_out = builder.build();
        laid_out.layout(f32::INFINITY);
        laid_out
    };
    let text_paint = Paint::from_color(Color::rgb(0.92, 0.92, 0.95));
    b.draw_paragraph_with(
        &paragraph("Snapped mask text", 22.0),
        (320.0, 786.0),
        &text_paint,
    );
    // A shader paint desugars into the mask-plus-SrcIn layer recipe.
    b.draw_paragraph_with(
        &paragraph("Gradient run", 22.0),
        (320.0, 818.0),
        &Paint {
            shader: Some(Shader::Linear {
                start: Point::new(320.0, 818.0),
                end: Point::new(560.0, 844.0),
                stops: stops(&[
                    (0.0, Color::rgb(1.0, 0.5, 0.2)),
                    (1.0, Color::rgb(0.3, 0.6, 1.0)),
                ]),
                spread: valo::SpreadMode::Pad,
                local: valo::Matrix::IDENTITY,
            }),
            color: Color::WHITE,
            ..Default::default()
        },
    );
    // An advanced blend wraps the run in an implicit layer.
    b.draw_paragraph_with(
        &paragraph("Multiply text", 20.0),
        (320.0, 850.0),
        &Paint {
            color: Color::rgb(0.95, 0.85, 0.4),
            blend_mode: BlendMode::Multiply,
            ..Default::default()
        },
    );
    // A mask blur takes the effect layer, sized from the run's RECORDED
    // device bounds rather than from local geometry.
    b.draw_paragraph_with(
        &paragraph("Blurred text", 20.0),
        (320.0, 882.0),
        &Paint {
            color: Color::rgb(0.5, 0.95, 0.7),
            mask_blur: Some(MaskBlur::new(2.0)),
            ..Default::default()
        },
    );
    // A stroked run stays in the mask tier — the rasterizer strokes the
    // outline before rasterizing it — and gives back the alpha its floored
    // hairline width owes.
    b.draw_paragraph_with(
        &paragraph("Stroked text", 22.0),
        (320.0, 908.0),
        &Paint {
            color: Color::rgb(0.4, 0.85, 0.95),
            style: valo::PaintStyle::Stroke(valo::Stroke {
                width: 1.0,
                cap: valo::Cap::Round,
                join: valo::Join::Round,
                miter_limit: 4.0,
                dash: None,
            }),
            ..Default::default()
        },
    );
    // A Difference clip whose shape lands off-viewport excludes nothing
    // visible: the ceiling is skipped entirely and the rect under it draws
    // whole.
    b.save();
    b.clip_rect(Rect::new(7000.0, 7000.0, 50.0, 50.0), ClipOp::Difference);
    b.draw_rect(
        Rect::new(30.0, 938.0, 120.0, 10.0),
        &Paint::from_color(Color::rgb(0.6, 0.9, 0.4)),
    );
    b.restore();

    // A rotated run: the mask tier keeps its upright rasters and transforms
    // the quads instead of snapping them.
    b.save();
    b.translate(400.0, 950.0);
    b.rotate(-0.2);
    b.draw_paragraph_with(&paragraph("Rotated", 26.0), (0.0, 0.0), &text_paint);
    b.restore();

    // Embedded lists: the same child list drawn inline and again through the
    // raster cache. The child carries enough draws to pass admission, so the
    // cached embed fills its texture and samples it in this very frame.
    let mut child_builder = DisplayListBuilder::new();
    for i in 0..20 {
        let (column, row) = (i % 5, i / 5);
        let tone = i as f32 / 19.0;
        child_builder.draw_rect(
            Rect::new(column as f32 * 30.0, row as f32 * 28.0, 24.0, 22.0),
            &Paint::from_color(Color::rgb(0.2 + 0.7 * tone, 0.5, 0.9 - 0.6 * tone)),
        );
    }
    let child = Arc::new(child_builder.build());
    b.save();
    b.translate(20.0, 950.0);
    b.draw_display_list(&child);
    b.restore();
    b.save();
    b.translate(190.0, 950.0);
    b.draw_display_list_cached(&child);
    b.restore();

    // The SDF tier (one raster serves a band of scales) and the outline tier
    // (real paths, stencil-then-cover), side by side.
    b.draw_paragraph_with(&paragraph("Sdf", 200.0), (20.0, 1080.0), &text_paint);
    b.draw_paragraph_with(&paragraph("R", 340.0), (400.0, 1050.0), &text_paint);

    b.build()
}

fn goldens_dir() -> &'static std::path::Path {
    std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens"))
}

/// Records `scene` into a fresh context and renders it once to an offscreen
/// of `size`. The scene is handed the context because recording may need to
/// upload images.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [u32; 2],
    scene: impl FnOnce(&mut Context) -> valo::DisplayList,
) -> (Vec<u8>, valo::RenderStats) {
    let mut ctx = Context::new(device.clone(), queue.clone());
    let offscreen = Offscreen::new(device, size);
    let dl = scene(&mut ctx);
    let stats = ctx.render(&dl, &offscreen.target(Some(Color::rgb(0.07, 0.07, 0.09))));
    let rgba = valo_harness::read_texture_rgba(device, queue, offscreen.texture(), size);
    (rgba, stats)
}

#[test]
fn planner_scene_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP planner_scene_golden: no GPU adapter");
        return;
    };
    let size = [640u32, 1460u32];
    let (rgba, stats) = render(&device, &queue, size, rect_scene);

    valo_harness::assert_golden(goldens_dir(), "planner_scene", size, &rgba);

    // Pin the scene's shape so it keeps exercising every claimed path: the
    // outer opacity group elides; the nested group, the mask, the Overlay
    // group, the overlap group, the pattern-Multiply implicit layer, and the
    // three text layers (gradient, Multiply, blurred) materialize (the
    // off-viewport group and mask are SKIPPED — neither elided nor
    // rendered); the two solid Multiply rects, the Overlay composite, the
    // Multiply star cover, the implicit composite, and the Multiply text
    // composite snapshot. Both clips reach the depth buffer, all three text
    // tiers are exercised, and the cached embed fills its texture and
    // samples it in the same frame.
    assert_eq!(stats.layers_elided, 1);
    assert_eq!(stats.layers_rendered, 13);
    assert_eq!(stats.snapshots, 6);
    assert_eq!(stats.clips, 2);
    // The off-viewport rect, plus the Difference clip whose shape misses the
    // frame — both counted as culled, neither reaching the depth buffer.
    assert_eq!(stats.culled, 2);
    assert_eq!(stats.text_tiers, [6, 1, 1]);
    assert_eq!(stats.raster_quads, 1);
    assert_eq!(stats.raster_fills, 1);
    assert!(
        stats.filter_passes > 0,
        "the M3b scene must run filter passes"
    );
}

/// A busy background for glass to blur: a full-frame gradient with hard-edged
/// shapes over it, so a blurred region is unmistakable against a sharp one.
fn busy_background(b: &mut DisplayListBuilder) {
    b.draw_rect(
        Rect::new(0.0, 0.0, 320.0, 240.0),
        &Paint::from_shader(Shader::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(320.0, 240.0),
            stops: stops(&[
                (0.0, Color::rgb(0.10, 0.25, 0.65)),
                (1.0, Color::rgb(0.75, 0.15, 0.45)),
            ]),
            spread: valo::SpreadMode::Pad,
            local: valo::Matrix::IDENTITY,
        }),
    );
    for i in 0..8 {
        let x = 10.0 + i as f32 * 40.0;
        b.draw_rect(
            Rect::new(x, 0.0, 18.0, 240.0),
            &Paint::from_color(Color::rgba(1.0, 0.95, 0.3, 0.75)),
        );
    }
    for i in 0..5 {
        let hue = [
            Color::rgb(0.2, 0.95, 0.7),
            Color::rgb(0.95, 0.45, 0.15),
            Color::rgb(0.35, 0.75, 1.0),
        ][i % 3];
        b.draw_circle(
            (30.0 + i as f32 * 65.0, 120.0),
            28.0,
            &Paint::from_color(hue),
        );
    }
}

/// The redesign's payoff: an opacity group wrapping frosted glass ELIDES.
/// The old display list made a backdrop a draw that read the target, so any
/// enclosing group had to materialize around it; as a layer property, the
/// group's alpha rides the glass composite instead — one texture for the
/// glass, none for the group, and the glass keeps blurring live content
/// while the group fades.
#[test]
fn backdrop_under_opacity_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP backdrop_under_opacity_golden: no GPU adapter");
        return;
    };
    let size = [320u32, 240u32];
    let glass = Rect::new(45.0, 65.0, 230.0, 105.0);

    let (rgba, stats) = render(&device, &queue, size, |_ctx| {
        let mut b = DisplayListBuilder::new();
        busy_background(&mut b);
        b.save_layer(
            None,
            &Paint {
                color: Color::rgba(0.0, 0.0, 0.0, 0.5),
                ..Default::default()
            },
        );
        b.save_layer_backdrop(Some(glass), &Paint::default(), Backdrop::blur(8.0));
        b.draw_rect(
            Rect::new(70.0, 90.0, 90.0, 30.0),
            &Paint::from_color(Color::rgba(1.0, 1.0, 1.0, 0.9)),
        );
        b.restore();
        b.restore();
        b.build()
    });

    assert_eq!(
        stats.layers_elided, 1,
        "the opacity group distributes its alpha onto the glass composite"
    );
    assert_eq!(stats.backdrops, 1, "one blur of what lies under the glass");
    assert_eq!(
        stats.layers_rendered, 1,
        "only the glass needs a texture — the group does not"
    );

    valo_harness::assert_golden(goldens_dir(), "backdrop_under_opacity", size, &rgba);
}

/// The Cupertino dialog's exact shape: fade -> path clip -> glass. The clip
/// must not forfeit the fade's elision; a materialized fade would hand the
/// glass a cleared offscreen to blur and the frost would vanish mid-fade.
#[test]
fn backdrop_under_opacity_with_clip_golden() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP backdrop_under_opacity_with_clip_golden: no GPU adapter");
        return;
    };
    let size = [320u32, 240u32];
    let glass = Rect::new(45.0, 65.0, 230.0, 105.0);

    let (rgba, stats) = render(&device, &queue, size, |_ctx| {
        let mut b = DisplayListBuilder::new();
        busy_background(&mut b);
        b.save_layer(
            None,
            &Paint {
                color: Color::rgba(0.0, 0.0, 0.0, 0.5),
                ..Default::default()
            },
        );
        b.save();
        let mut clip = valo::PathBuilder::new();
        clip.rrect(Rect::new(55.0, 72.0, 210.0, 91.0), 24.0);
        b.clip_path(&clip.build(), valo::FillRule::NonZero, ClipOp::Intersect);
        b.save_layer_backdrop(Some(glass), &Paint::default(), Backdrop::blur(8.0));
        b.draw_rect(
            Rect::new(70.0, 90.0, 90.0, 30.0),
            &Paint::from_color(Color::rgba(1.0, 1.0, 1.0, 0.9)),
        );
        b.restore();
        b.restore();
        b.restore();
        b.build()
    });

    assert_eq!(
        stats.layers_elided, 1,
        "the clip must not force the fade to materialize"
    );
    assert_eq!(stats.backdrops, 1);
    assert_eq!(stats.layers_rendered, 1, "only the glass needs a texture");
    assert_eq!(stats.clips, 1, "the rounded clip reaches the depth buffer");

    valo_harness::assert_golden(
        goldens_dir(),
        "backdrop_under_opacity_with_clip",
        size,
        &rgba,
    );
}
