//! Strokes: caps, joins, miter limit, dash, hairline, gradients.
//! `cargo run -p valo --example strokes`
//!
//! Strokes are CPU triangle strips along the flattened path (Impeller's
//! StrokePathGeometry shape): joins fan around the pivot, caps close open
//! ends, dashing splits contours before stroking. Any fragment family
//! composes — the gradient ring costs the same as a solid one.

use valo::{
    Cap, Color, Dash, DisplayListBuilder, Join, Paint, PaintStyle, PathBuilder, Point, Rect,
    Shader, Stroke,
};

fn stroke_paint(color: Color, stroke: Stroke) -> Paint {
    Paint {
        color,
        style: PaintStyle::Stroke(stroke),
        ..Default::default()
    }
}

fn zigzag(at: Point) -> std::sync::Arc<valo::Path> {
    let mut p = PathBuilder::new();
    p.move_to((at.x, at.y + 40.0))
        .line_to((at.x + 45.0, at.y))
        .line_to((at.x + 90.0, at.y + 40.0));
    p.build()
}

fn scene() -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    let ink = Color::rgb(0.9, 0.91, 0.95);

    // Caps × joins matrix on a zigzag (open contour).
    let caps = [Cap::Butt, Cap::Round, Cap::Square];
    let joins = [Join::Miter, Join::Round, Join::Bevel];
    for (row, join) in joins.into_iter().enumerate() {
        for (col, cap) in caps.into_iter().enumerate() {
            let at = Point::new(50.0 + col as f32 * 140.0, 45.0 + row as f32 * 90.0);
            b.draw_path(
                &zigzag(at),
                valo::FillRule::NonZero,
                &stroke_paint(
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

    // Miter limit: a sharp chevron spikes at limit 10, bevels at 1.5.
    for (i, limit) in [10.0f32, 1.5].into_iter().enumerate() {
        let mut p = PathBuilder::new();
        let x = 500.0 + i as f32 * 80.0;
        p.move_to((x, 130.0))
            .line_to((x + 30.0, 45.0))
            .line_to((x + 60.0, 130.0));
        b.draw_path(
            &p.build(),
            valo::FillRule::NonZero,
            &stroke_paint(
                Color::rgb(0.95, 0.75, 0.3),
                Stroke {
                    miter_limit: limit,
                    ..Stroke::new(12.0)
                },
            ),
        );
    }

    // Dashed rrect border — the card frame.
    let mut frame = PathBuilder::new();
    frame.rrect(Rect::new(50.0, 330.0, 250.0, 130.0), 20.0);
    b.draw_path(
        &frame.build(),
        valo::FillRule::NonZero,
        &stroke_paint(
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

    // Gradient-stroked circle: shaders compose with strips for free.
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

    // Hairline: width 0.1 floors to one device pixel.
    let mut hair = PathBuilder::new();
    hair.move_to((530.0, 340.0)).line_to((620.0, 450.0));
    b.draw_path(
        &hair.build(),
        valo::FillRule::NonZero,
        &stroke_paint(ink, Stroke::new(0.1)),
    );

    b.build()
}

fn main() {
    valo_harness::run_example("strokes", [660, 500], Color::rgb(0.09, 0.1, 0.13), |_ctx| {
        scene()
    });
}
