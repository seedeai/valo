//! Images: upload, premultiply, mips, sampling, tiling.
//! `cargo run -p valo --example images` → target/examples/images.png
//!
//! Pixels are premultiplied at the upload boundary (Skia's kPremul
//! convention) and get a full GPU-rendered mip chain by default.
//!
//! What to look at:
//! - top row: the same 64² checker upscaled with Nearest (crisp blocks)
//!   vs Linear (soft) — sampler choice, one texture
//! - middle: a busy 256² pattern downscaled to 64² WITH mips (smooth grey
//!   average) vs WITHOUT (aliased shimmer picked from sparse samples)
//! - tiling: one small texture, `src` larger than the texture, Repeat vs
//!   Mirror address modes
//! - opacity: paint.color's ALPHA scales the draw (RGB is ignored for
//!   image sources — Skia semantics; tinting arrives with color filters)

use valo::{
    Color, Context, DisplayListBuilder, Filter, Image, ImageDesc, Paint, Rect, Sampling, TileMode,
};

/// A procedural checkerboard (no decode deps — examples stay I/O-free).
fn checker(size: u32, cell: u32, a: [u8; 4], b: [u8; 4]) -> Vec<u8> {
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let on = ((x / cell) + (y / cell)).is_multiple_of(2);
            px.extend_from_slice(if on { &a } else { &b });
        }
    }
    px
}

fn upload_checker(ctx: &mut Context, size: u32, cell: u32, mips: bool) -> Image {
    let pixels = checker(size, cell, [235, 235, 240, 255], [40, 45, 60, 255]);
    ctx.upload_image(
        ImageDesc {
            size: [size, size],
            premultiplied: true,
            mips,
        },
        &pixels,
    )
}

fn scene(ctx: &mut Context) -> valo::DisplayList {
    let small = upload_checker(ctx, 64, 8, true);
    let busy_mips = upload_checker(ctx, 256, 2, true);
    let busy_flat = upload_checker(ctx, 256, 2, false);

    let mut b = DisplayListBuilder::new();
    let paint = Paint::default();

    // Sampler choice on upscale: Nearest vs Linear.
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

    // Mips on downscale: smooth vs shimmer (2px cells at 1/4 scale).
    b.draw_image(&busy_mips, Rect::new(440.0, 40.0, 64.0, 64.0), &paint);
    b.draw_image(&busy_flat, Rect::new(530.0, 40.0, 64.0, 64.0), &paint);

    // Tiling: src spans 3×2 texture sizes; the sampler wraps.
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

    // Opacity: alpha 0.5 over the background (RGB of the color is ignored).
    b.draw_image(
        &small,
        Rect::new(440.0, 130.0, 64.0, 64.0),
        &Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.5)),
    );

    b.build()
}

fn main() {
    valo_harness::run_example("images", [640, 480], Color::rgb(0.07, 0.07, 0.09), scene);
}
