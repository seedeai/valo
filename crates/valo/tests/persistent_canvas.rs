//! The persistent canvas backing: pixels survive between frames, the work per
//! frame stays flat, and the restore that makes both true is pixel-exact.

use std::sync::Arc;

use valo::{Color, Context, DisplayList, DisplayListBuilder, Paint, PersistentCanvas, Rect};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn dot(x: f32, y: f32, color: Color) -> Arc<DisplayList> {
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(Rect::new(x, y, 8.0, 8.0), &Paint::from_color(color));
    Arc::new(builder.build())
}

/// The backing's raw PREMULTIPLIED bytes — what is actually stored, which is
/// what a drift check has to compare.
fn read(gpu: &(wgpu::Device, wgpu::Queue), canvas: &PersistentCanvas) -> Vec<u8> {
    valo_harness::read_texture_rgba(&gpu.0, &gpu.1, canvas.front().texture(), canvas.size())
}

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * width + x) * 4) as usize;
    [
        pixels[start],
        pixels[start + 1],
        pixels[start + 2],
        pixels[start + 3],
    ]
}

/// The guarantee the whole design exists for: draw, present, draw again
/// WITHOUT clearing, and the first drawing is still there.
#[test]
fn pixels_survive_across_presents() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP pixels_survive_across_presents");
        return;
    };
    let gpu = (device.clone(), queue.clone());
    let mut context = Context::new(device, queue);
    let mut canvas = PersistentCanvas::new(&mut context, [64, 64], FORMAT);

    canvas.draw(
        &mut context,
        &dot(8.0, 8.0, Color::rgb(1.0, 0.0, 0.0)),
        None,
    );
    canvas.draw(
        &mut context,
        &dot(40.0, 40.0, Color::rgb(0.0, 1.0, 0.0)),
        None,
    );

    let pixels = read(&gpu, &canvas);
    assert_eq!(
        pixel(&pixels, 64, 12, 12),
        [255, 0, 0, 255],
        "the first frame's ink must survive the second frame"
    );
    assert_eq!(pixel(&pixels, 64, 44, 44), [0, 255, 0, 255]);
    assert_eq!(pixel(&pixels, 64, 32, 32), [0, 0, 0, 0], "nothing else");
}

/// A clear DISCARDS what was there — the `reset` / `beginFrame` /
/// full-surface-clearRect path, and the one case that must skip the restore.
#[test]
fn a_clear_discards_what_came_before() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_clear_discards_what_came_before");
        return;
    };
    let gpu = (device.clone(), queue.clone());
    let mut context = Context::new(device, queue);
    let mut canvas = PersistentCanvas::new(&mut context, [64, 64], FORMAT);

    canvas.draw(
        &mut context,
        &dot(8.0, 8.0, Color::rgb(1.0, 0.0, 0.0)),
        None,
    );
    canvas.draw(
        &mut context,
        &dot(40.0, 40.0, Color::rgb(0.0, 1.0, 0.0)),
        Some(Color::TRANSPARENT),
    );

    let pixels = read(&gpu, &canvas);
    assert_eq!(
        pixel(&pixels, 64, 12, 12),
        [0, 0, 0, 0],
        "the cleared frame must not restore the old ink"
    );
    assert_eq!(pixel(&pixels, 64, 44, 44), [0, 255, 0, 255]);
}

/// The failure mode that would otherwise surface months later as "the canvas
/// looks blurry": any resampling in the restore compounds every frame.
///
/// 60 presents of an empty delta means 60 round trips through the restore. A
/// half-texel offset, a linear filter, or a premultiply round trip would each
/// leave a visible trail by then; exact restore leaves the buffer bit-identical.
#[test]
fn the_restore_does_not_drift_over_many_presents() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP the_restore_does_not_drift_over_many_presents");
        return;
    };
    let gpu = (device.clone(), queue.clone());
    let mut context = Context::new(device, queue);
    let mut canvas = PersistentCanvas::new(&mut context, [64, 64], FORMAT);

    // Content that reaches the CANVAS EDGE on all four sides, plus hard
    // interior edges and a translucent block.
    //
    // The edge coverage is the part that matters: a sub-pixel translation in
    // the restore erodes whatever sits against the boundary, and interior-only
    // content hides that completely — the eroded columns are transparent
    // either way. A translucent block catches a premultiply round trip going
    // wrong, and the interior edges catch a scale error.
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        Rect::new(0.0, 0.0, 64.0, 2.0),
        &Paint::from_color(Color::rgb(1.0, 1.0, 0.0)),
    );
    builder.draw_rect(
        Rect::new(0.0, 62.0, 64.0, 2.0),
        &Paint::from_color(Color::rgb(0.0, 1.0, 1.0)),
    );
    builder.draw_rect(
        Rect::new(0.0, 0.0, 2.0, 64.0),
        &Paint::from_color(Color::rgb(1.0, 0.0, 1.0)),
    );
    builder.draw_rect(
        Rect::new(62.0, 0.0, 2.0, 64.0),
        &Paint::from_color(Color::rgb(0.5, 0.5, 1.0)),
    );
    builder.draw_rect(
        Rect::new(3.0, 5.0, 17.0, 23.0),
        &Paint::from_color(Color::rgb(1.0, 0.25, 0.5)),
    );
    builder.draw_rect(
        Rect::new(30.0, 12.0, 21.0, 19.0),
        &Paint::from_color(Color::rgba(0.2, 0.9, 0.4, 0.6)),
    );
    canvas.draw(&mut context, &Arc::new(builder.build()), None);
    let first = read(&gpu, &canvas);

    let empty = Arc::new(DisplayListBuilder::new().build());
    for _ in 0..60 {
        canvas.draw(&mut context, &empty, None);
    }
    let after = read(&gpu, &canvas);

    assert_eq!(
        first, after,
        "60 restores must be bit-identical; any filtering compounds forever"
    );
}

/// The reason for the whole change: N incremental frames must cost O(N), not
/// O(N²).
///
/// The old model replayed every list ever recorded, so frame N encoded N
/// draws and the total was 1+2+…+N. Here each frame encodes its own delta
/// plus one restore, so the per-frame cost is FLAT and the total is linear.
/// Asserting on encoded draws rather than wall-clock keeps this a regression
/// test rather than a benchmark.
#[test]
fn per_frame_cost_stays_flat_as_the_canvas_accumulates() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP per_frame_cost_stays_flat_as_the_canvas_accumulates");
        return;
    };
    let mut context = Context::new(device, queue);
    let mut canvas = PersistentCanvas::new(&mut context, [128, 128], FORMAT);

    let mut draws = Vec::new();
    for frame in 0..24u32 {
        let x = (frame % 12) as f32 * 10.0;
        let y = (frame / 12) as f32 * 10.0;
        let stats = canvas.draw(&mut context, &dot(x, y, Color::rgb(1.0, 1.0, 1.0)), None);
        draws.push(stats.draws);
    }

    // Frame 0 has nothing to restore; every frame after is one restore plus
    // one delta draw, forever.
    assert_eq!(draws[0], 1, "the first frame restores nothing");
    assert!(
        draws[1..].iter().all(|&count| count == 2),
        "every later frame is restore + delta, got {draws:?}"
    );

    let total: u32 = draws.iter().sum();
    let quadratic = 24 * 25 / 2;
    assert!(
        total < quadratic / 4,
        "total work {total} should be linear, not the {quadratic} of cumulative replay"
    );
}

/// A resize reallocates rather than rescaling — scaling old pixels would be
/// the one place resampling could sneak back in.
#[test]
fn a_resize_starts_from_a_clear_canvas() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_resize_starts_from_a_clear_canvas");
        return;
    };
    let gpu = (device.clone(), queue.clone());
    let mut context = Context::new(device, queue);
    let mut canvas = PersistentCanvas::new(&mut context, [64, 64], FORMAT);
    canvas.draw(
        &mut context,
        &dot(8.0, 8.0, Color::rgb(1.0, 0.0, 0.0)),
        None,
    );

    canvas.resize(&mut context, [96, 48]);
    assert_eq!(canvas.size(), [96, 48]);

    let empty = Arc::new(DisplayListBuilder::new().build());
    canvas.draw(&mut context, &empty, None);
    let pixels = read(&gpu, &canvas);
    assert!(
        pixels.chunks_exact(4).all(|pixel| pixel[3] == 0),
        "a resized canvas starts empty"
    );
}
