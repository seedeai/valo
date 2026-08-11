//! Recording cost, no GPU (Skia nanobench kNonRendering class): what the
//! oracle (bounds, slots, elision proofs) adds per op.

use criterion::{criterion_group, criterion_main, Criterion};
use valo::{Color, DisplayListBuilder, DrawParagraphExt, Paint, PathBuilder, Rect};

fn thousand_rects(c: &mut Criterion) {
    c.bench_function("record/1k_rects", |b| {
        b.iter(|| {
            let mut dl = DisplayListBuilder::new();
            for i in 0..1000 {
                let x = (i % 40) as f32 * 20.0;
                let y = (i / 40) as f32 * 24.0;
                dl.draw_rect(
                    Rect::new(x, y, 18.0, 20.0),
                    &Paint::from_color(Color::rgb(0.3, 0.5, 0.7)),
                );
            }
            dl.build()
        })
    });
}

fn stroked_paths(c: &mut Criterion) {
    let paths: Vec<_> = (0..200)
        .map(|i| {
            let mut p = PathBuilder::new();
            p.move_to((0.0, i as f32));
            for k in 1..24 {
                p.line_to((
                    k as f32 * 12.0,
                    i as f32 + if k % 2 == 0 { 0.0 } else { 9.0 },
                ));
            }
            p.build()
        })
        .collect();
    let paint = Paint {
        color: Color::rgb(0.9, 0.6, 0.2),
        style: valo::PaintStyle::Stroke(valo::Stroke::new(4.0)),
        ..Default::default()
    };
    c.bench_function("record/200_stroked_paths", |b| {
        b.iter(|| {
            let mut dl = DisplayListBuilder::new();
            for p in &paths {
                dl.draw_path(p, valo::FillRule::NonZero, &paint);
            }
            dl.build()
        })
    });
}

fn paragraph_stamps(c: &mut Criterion) {
    let mut fonts = bench_fonts::fonts();
    let mut para = valo::ParagraphBuilder::new(&mut fonts);
    para.add_text(
        "The quick brown fox jumps over the lazy dog",
        &valo::TextStyle::new("Fira Sans", 18.0, Color::WHITE),
    );
    let mut para = para.build();
    para.layout(400.0);
    c.bench_function("record/50_paragraph_stamps", |b| {
        b.iter(|| {
            let mut dl = DisplayListBuilder::new();
            for i in 0..50 {
                dl.draw_paragraph(&para, (10.0, i as f32 * 60.0));
            }
            dl.build()
        })
    });
}

mod bench_fonts;

criterion_group!(benches, thousand_rects, stroked_paths, paragraph_stamps);
criterion_main!(benches);
