//! CPU geometry micro-benches: flattening and the stroker (with dashing) on
//! a 1k-segment path — the per-frame cost a pan pays WITHOUT the caches.

use criterion::{criterion_group, criterion_main, Criterion};
use valo::PathBuilder;
use valo_geometry::{dash_contours, stroke_strip, Dash, Stroke};

fn geometry_benches(c: &mut Criterion) {
    let path = {
        let mut p = PathBuilder::new();
        p.move_to((0.0, 0.0));
        for k in 1..1000 {
            let x = k as f32 * 2.0;
            p.line_to((x, (k as f32 * 0.11).sin() * 60.0));
            if k % 4 == 0 {
                p.quad_to((x + 1.0, 30.0), (x + 2.0, 0.0));
            }
        }
        p.build()
    };
    c.bench_function("geometry/flatten_1k", |b| b.iter(|| path.flatten(0.25)));

    let contours = path.flatten(0.25);
    let stroke = Stroke::new(6.0);
    c.bench_function("geometry/stroke_strip_1k", |b| {
        b.iter(|| stroke_strip(&contours, &stroke, 0.25))
    });

    let dash = Dash {
        intervals: vec![14.0, 8.0],
        phase: 0.0,
    };
    c.bench_function("geometry/dash_then_stroke_1k", |b| {
        b.iter(|| stroke_strip(&dash_contours(&contours, &dash), &stroke, 0.25))
    });
}

criterion_group!(benches, geometry_benches);
criterion_main!(benches);
