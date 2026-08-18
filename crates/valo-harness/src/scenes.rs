//! Stress scenes shared by benches and the perf probe — one source of truth
//! so "the Figma board" means the same content everywhere.

use valo::{
    Backdrop, Color, DisplayList, DisplayListBuilder, DrawParagraphExt, FontCollection, MaskBlur,
    Paint, ParagraphBuilder, Rect, TextStyle,
};

/// A Figma-style board: ~100 artboard "frames" scattered in clusters over a
/// ~9600×5400 world — thumbnail grids, list rows, dialog stacks (each with a
/// real drop shadow), color-swatch tables, frosted headers. Roughly 3k
/// shapes, 700 text runs, 100 analytic shadows, 16 backdrop strips: the
/// design editor's worst realistic day.
pub fn figma_board(fonts: &mut FontCollection) -> DisplayList {
    let mut b = DisplayListBuilder::new();
    let mut rng = Lcg(0x5eed);
    for cluster in 0..8 {
        let cx = (cluster % 4) as f32 * 2400.0 + rng.range(0.0, 300.0);
        let cy = (cluster / 4) as f32 * 2700.0 + rng.range(0.0, 300.0);
        for i in 0..12 {
            let x = cx + (i % 4) as f32 * 560.0 + rng.range(0.0, 80.0);
            let y = cy + (i / 4) as f32 * 820.0 + rng.range(0.0, 120.0);
            frame(&mut b, fonts, &mut rng, x, y, cluster * 12 + i);
        }
    }
    b.build()
}

fn frame(
    b: &mut DisplayListBuilder,
    fonts: &mut FontCollection,
    rng: &mut Lcg,
    x: f32,
    y: f32,
    index: usize,
) {
    let dark = index.is_multiple_of(3);
    let (bg, fg) = if dark {
        (Color::rgb(0.11, 0.11, 0.13), Color::rgb(0.92, 0.92, 0.95))
    } else {
        (Color::rgb(0.99, 0.99, 1.0), Color::rgb(0.12, 0.12, 0.15))
    };
    let w = 480.0 + rng.range(0.0, 160.0);
    let h = match index % 4 {
        0 => 620.0, // thumbs
        1 => 560.0, // list
        2 => 760.0, // dialogs
        _ => 460.0, // swatches
    };
    text(b, fonts, format!("Frame {index}"), 22.0, fg, x, y - 34.0);
    b.draw_rrect(Rect::new(x, y, w, h), 6.0, &Paint::from_color(bg));
    if index.is_multiple_of(6) {
        b.save_layer_backdrop(
            Some(Rect::new(x, y, w, 64.0)),
            &Paint::default(),
            Backdrop::blur(7.0),
        );
        b.restore();
    }
    match index % 4 {
        0 => thumbs(b, fonts, rng, x, y, w, fg),
        1 => list(b, fonts, rng, x, y, w, fg),
        2 => dialogs(b, fonts, rng, x, y, w, dark),
        _ => swatches(b, fonts, rng, x, y, fg),
    }
}

/// 3×4 thumbnail grid + captions (the asset-library frame).
fn thumbs(
    b: &mut DisplayListBuilder,
    fonts: &mut FontCollection,
    rng: &mut Lcg,
    x: f32,
    y: f32,
    w: f32,
    fg: Color,
) {
    let cell = (w - 60.0) / 3.0;
    for i in 0..12 {
        let tx = x + 20.0 + (i % 3) as f32 * (cell + 10.0);
        let ty = y + 50.0 + (i / 3) as f32 * 130.0;
        b.draw_rrect(
            Rect::new(tx, ty, cell, 100.0),
            4.0,
            &Paint::from_color(rng.color(0.25, 0.7)),
        );
        b.draw_rect(
            Rect::new(tx + 8.0, ty + 8.0, cell - 16.0, 52.0),
            &Paint::from_color(rng.color(0.4, 0.95)),
        );
        b.draw_rect(
            Rect::new(tx + 8.0, ty + 68.0, cell * 0.6, 8.0),
            &Paint::from_color(rng.color(0.6, 0.9)),
        );
    }
    for row in 0..4 {
        text(
            b,
            fonts,
            format!("Asset group {row}"),
            12.0,
            fg,
            x + 20.0,
            y + 570.0 - row as f32 * 12.0,
        );
    }
}

/// Settings-list frame: rows of label + value + divider.
fn list(
    b: &mut DisplayListBuilder,
    fonts: &mut FontCollection,
    rng: &mut Lcg,
    x: f32,
    y: f32,
    w: f32,
    fg: Color,
) {
    for row in 0..10 {
        let ry = y + 56.0 + row as f32 * 48.0;
        b.draw_rect(
            Rect::new(x + 16.0, ry, w - 32.0, 40.0),
            &Paint::from_color(rng.color(0.88, 0.97).with_alpha(0.5)),
        );
        text(
            b,
            fonts,
            format!("Property row {row}"),
            13.0,
            fg,
            x + 28.0,
            ry + 12.0,
        );
        text(
            b,
            fonts,
            format!("{}px", 4 + row * 3),
            13.0,
            Color::rgb(0.45, 0.55, 0.9),
            x + w - 90.0,
            ry + 12.0,
        );
    }
}

/// Dialog-stack frame: cards with REAL drop shadows (analytic rrect blur).
fn dialogs(
    b: &mut DisplayListBuilder,
    fonts: &mut FontCollection,
    rng: &mut Lcg,
    x: f32,
    y: f32,
    w: f32,
    dark: bool,
) {
    for i in 0..4 {
        let dy = y + 60.0 + i as f32 * 170.0;
        let card = Rect::new(x + 30.0, dy, w - 60.0, 140.0);
        b.draw_rrect(
            Rect::new(card.x, card.y + 6.0, card.width, card.height),
            10.0,
            &Paint {
                color: Color::rgba(0.0, 0.0, 0.0, 0.45),
                mask_blur: Some(MaskBlur::new(9.0)),
                ..Default::default()
            },
        );
        let face = if dark {
            Color::rgb(0.17, 0.17, 0.2)
        } else {
            Color::WHITE
        };
        b.draw_rrect(card, 10.0, &Paint::from_color(face));
        let fg = if dark {
            Color::WHITE
        } else {
            Color::rgb(0.1, 0.1, 0.12)
        };
        text(
            b,
            fonts,
            format!("Dialog title {i}"),
            15.0,
            fg,
            card.x + 18.0,
            card.y + 16.0,
        );
        text(
            b,
            fonts,
            "Body copy explaining the action".into(),
            12.0,
            fg.with_alpha(0.7),
            card.x + 18.0,
            card.y + 44.0,
        );
        b.draw_rrect(
            Rect::new(card.x + card.width - 170.0, card.y + 92.0, 74.0, 30.0),
            6.0,
            &Paint::from_color(rng.color(0.8, 0.95)),
        );
        b.draw_rrect(
            Rect::new(card.x + card.width - 88.0, card.y + 92.0, 74.0, 30.0),
            6.0,
            &Paint::from_color(Color::rgb(0.25, 0.5, 0.95)),
        );
    }
}

/// Color-system frame: a table of swatches (Figma's palette pages).
fn swatches(
    b: &mut DisplayListBuilder,
    fonts: &mut FontCollection,
    rng: &mut Lcg,
    x: f32,
    y: f32,
    fg: Color,
) {
    for i in 0..30usize {
        let sx = x + 20.0 + (i % 6) as f32 * 76.0;
        let sy = y + 60.0 + (i / 6) as f32 * 64.0;
        b.draw_rect(
            Rect::new(sx, sy, 64.0, 40.0),
            &Paint::from_color(rng.color(0.2, 1.0)),
        );
        if i.is_multiple_of(6) {
            text(
                b,
                fonts,
                format!("Scale {}", i / 6),
                11.0,
                fg,
                sx,
                sy + 44.0,
            );
        }
    }
}

fn text(
    b: &mut DisplayListBuilder,
    fonts: &mut FontCollection,
    s: String,
    px: f32,
    color: Color,
    x: f32,
    y: f32,
) {
    let mut p = ParagraphBuilder::new(fonts);
    p.add_text(&s, &TextStyle::new("Fira Sans", px, color));
    let mut p = p.build();
    p.layout(f32::INFINITY);
    b.draw_paragraph(&p, (x, y));
}

/// Deterministic xorshift — benches must draw the same board every run.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next() * (hi - lo)
    }

    fn color(&mut self, lo: f32, hi: f32) -> Color {
        Color::rgb(self.range(lo, hi), self.range(lo, hi), self.range(lo, hi))
    }
}
