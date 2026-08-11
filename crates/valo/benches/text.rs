//! The retained-paragraph tiers: what each editing operation
//! costs — full rebuild (shape), re-wrap (layout), recolor (place).

use criterion::{criterion_group, criterion_main, Criterion};
use valo::{Color, ParagraphBuilder, TextStyle};

mod bench_fonts;

const BODY: &str = "Grumpy wizards make toxic brew for the evil queen and jack; \
    a quick movement of the enemy will jeopardize five gunboats. The five \
    boxing wizards jump quickly over the lazy dog while vexed daft zebras run.";

fn text_benches(c: &mut Criterion) {
    let mut fonts = bench_fonts::fonts();
    let style = TextStyle::new("Fira Sans", 17.0, Color::WHITE);

    c.bench_function("text/build_reshape", |b| {
        b.iter(|| {
            let mut p = ParagraphBuilder::new(&mut fonts);
            p.add_text(BODY, &style);
            let mut p = p.build();
            p.layout(360.0);
            p.height()
        })
    });

    let mut para = ParagraphBuilder::new(&mut fonts);
    para.add_text(BODY, &style);
    let mut para = para.build();
    para.layout(360.0);
    let mut width = 360.0f32;
    c.bench_function("text/relayout_width", |b| {
        b.iter(|| {
            width = if width > 360.5 { 360.0 } else { 361.0 };
            para.layout(width);
            para.height()
        })
    });

    let mut flip = false;
    c.bench_function("text/update_color_replace", |b| {
        b.iter(|| {
            flip = !flip;
            let tint = if flip {
                Color::WHITE
            } else {
                Color::rgb(1.0, 0.9, 0.6)
            };
            para.update_color(0, tint);
            para.lines().len()
        })
    });
}

criterion_group!(benches, text_benches);
criterion_main!(benches);
