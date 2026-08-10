//! The list raster cache's proof: a hinted embed drawn from its
//! cached texture is BYTE-IDENTICAL to unhinted inline replay at exact
//! scale — the integral snapping + full-texel sampling make it a hard
//! equality, not a fuzzy compare — and the lifecycle (first-sight fill,
//! hold reuse, settle refills EVERYTHING at once) behaves on scripted
//! frames. There is no gate and no quota: the embedder vouches for
//! stability by hinting (021d), so churn protection is its job, not ours.

use std::path::Path;
use std::sync::Arc;

use valo::{Color, Context, DisplayListBuilder, Matrix, Offscreen, Paint, Rect};

fn goldens_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens"))
}

/// A board-like list: comfortably past MIN_CACHED_DRAWS, with overlap and
/// alpha so compositing mistakes would show.
fn board() -> Arc<valo::DisplayList> {
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 220.0, 160.0),
        &Paint::from_color(Color::rgb(0.13, 0.15, 0.2)),
    );
    for i in 0..20 {
        let x = 10.0 + (i % 5) as f32 * 42.0;
        let y = 12.0 + (i / 5) as f32 * 36.0;
        b.draw_rect(
            Rect::new(x, y, 34.0, 26.0),
            &Paint::from_color(Color::rgba(
                0.2 + (i as f32) * 0.03,
                0.5,
                1.0 - (i as f32) * 0.02,
                0.85,
            )),
        );
    }
    Arc::new(b.build())
}

/// The wrapper a scene would record: the board embedded at an offset,
/// next to an uncached neighbour — `hinted` is the only difference
/// between the cache path and plain inline replay.
fn wrapper(board: &Arc<valo::DisplayList>, hinted: bool) -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(250.0, 20.0, 40.0, 40.0),
        &Paint::from_color(Color::rgb(0.9, 0.6, 0.2)),
    );
    b.save();
    b.translate(12.0, 14.0);
    if hinted {
        b.draw_display_list_cached(board);
    } else {
        b.draw_display_list(board);
    }
    b.restore();
    b.build()
}

fn frame_arc(
    ctx: &mut Context,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    list: &Arc<valo::DisplayList>,
    camera: Matrix,
) -> (Vec<u8>, valo::RenderStats) {
    let size = [320, 200];
    let offscreen = Offscreen::new(device, size);
    let mut wrapped = DisplayListBuilder::new();
    wrapped.save();
    wrapped.concat(&camera);
    wrapped.draw_display_list(list);
    wrapped.restore();
    let stats = ctx.render(
        &wrapped.build(),
        &offscreen.target(Some(Color::rgb(0.08, 0.08, 0.1))),
    );
    (
        valo_harness::read_texture_rgba(device, queue, offscreen.texture(), size),
        stats,
    )
}

#[test]
fn cached_quad_is_byte_identical_and_lifecycle_holds() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let board = board();
    // The truth to compare against: the SAME content, never hinted.
    let plain = Arc::new(wrapper(&board, false));
    let (inline_pixels, s0) = frame_arc(&mut ctx, &device, &queue, &plain, Matrix::IDENTITY);
    assert_eq!((s0.raster_quads, s0.raster_fills), (0, 0), "unhinted");

    let scene = Arc::new(wrapper(&board, true));
    let identity = Matrix::IDENTITY;

    // Frame 1 of the hinted scene: fills on FIRST sight and draws the quad
    // in the same frame (021d: no gate, no quota) — byte-identical to the
    // unhinted truth.
    let (first_frame, s1) = frame_arc(&mut ctx, &device, &queue, &scene, identity);
    assert_eq!(
        (s1.raster_quads, s1.raster_fills),
        (1, 1),
        "first sight: fill + quad, one frame"
    );
    assert_eq!(first_frame, inline_pixels, "fill-frame quad == inline");

    // Frame 2: the entry serves with no further fills.
    let (quad_frame, s2) = frame_arc(&mut ctx, &device, &queue, &scene, identity);
    assert_eq!((s2.raster_quads, s2.raster_fills), (1, 0), "quad serves");
    assert_eq!(quad_frame, inline_pixels, "cached quad == inline, exactly");
    valo_harness::assert_golden(goldens_dir(), "raster_cache_quad", [320, 200], &quad_frame);

    // Zoom out WITHIN the safe band (0.6 ≥ 0.5): the texture serves by
    // plain downsampling, even idle — no refill is ever due here.
    let out = zoom(0.6);
    let (_, s4) = frame_arc(&mut ctx, &device, &queue, &scene, out);
    assert_eq!((s4.raster_quads, s4.raster_fills), (1, 0), "band reuses");

    // Deep zoom-out (0.4 < 0.5) under HOLD: still reuses — gesture frames
    // accept transient shimmer over refills.
    ctx.set_raster_hold(true);
    let deep = zoom(0.4);
    let (_, s5) = frame_arc(&mut ctx, &device, &queue, &scene, deep);
    assert_eq!((s5.raster_quads, s5.raster_fills), (1, 0), "hold reuses");

    // Settle: past the band and idle, the refill lands (smaller, crisp)
    // and serves in the same frame.
    ctx.set_raster_hold(false);
    let (_, s6) = frame_arc(&mut ctx, &device, &queue, &scene, deep);
    assert_eq!(
        (s6.raster_quads, s6.raster_fills),
        (1, 1),
        "settle refills past the band and quads immediately"
    );
    let (_, s7) = frame_arc(&mut ctx, &device, &queue, &scene, deep);
    assert_eq!(
        (s7.raster_quads, s7.raster_fills),
        (1, 0),
        "refilled quad serves"
    );

    // Zoom IN past the tolerance: idle refill at the denser scale.
    let (_, s8) = frame_arc(&mut ctx, &device, &queue, &scene, zoom(1.5));
    assert_eq!(
        (s8.raster_quads, s8.raster_fills),
        (1, 1),
        "zoom-in refills denser"
    );
}

fn zoom(scale: f32) -> Matrix {
    Matrix::from_affine(scale, 0.0, 0.0, scale, 30.0, 20.0)
}

/// Churn protection is the EMBEDDER's job now (021d): a board under live
/// editing is embedded WITHOUT the hint, and an unhinted embed never
/// touches the cache — no texture per drag frame, by construction.
#[test]
fn unhinted_embeds_never_fill() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    for _ in 0..3 {
        let fresh = Arc::new(wrapper(&board(), false)); // churning ids, no hint
        let (_, stats) = frame_arc(&mut ctx, &device, &queue, &fresh, Matrix::IDENTITY);
        assert_eq!(
            (stats.raster_quads, stats.raster_fills),
            (0, 0),
            "no hint, no cache"
        );
    }
}

/// A zoom settle refills EVERY stale board in the ONE frame the host
/// renders on gesture release — no quota, no staleness, no follow-up
/// frames owed (021d).
#[test]
fn settle_refills_all_stale_boards_in_one_frame() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let (first, second) = (board(), board());
    let scene = {
        let mut b = DisplayListBuilder::new();
        b.draw_display_list_cached(&first);
        b.save();
        b.translate(0.0, 170.0);
        b.draw_display_list_cached(&second);
        b.restore();
        Arc::new(b.build())
    };
    // Warm both at identity, then zoom deep under hold (reuse), then
    // settle: BOTH refill on that single frame.
    let (_, warm) = frame_arc(&mut ctx, &device, &queue, &scene, Matrix::IDENTITY);
    assert_eq!((warm.raster_quads, warm.raster_fills), (2, 2));
    ctx.set_raster_hold(true);
    let deep = zoom(0.3);
    let (_, held) = frame_arc(&mut ctx, &device, &queue, &scene, deep);
    assert_eq!(
        (held.raster_quads, held.raster_fills),
        (2, 0),
        "hold reuses"
    );
    ctx.set_raster_hold(false);
    let (_, settled) = frame_arc(&mut ctx, &device, &queue, &scene, deep);
    assert_eq!(
        (settled.raster_quads, settled.raster_fills),
        (2, 2),
        "the settle frame refills everything at once"
    );
}

/// The liveness sweep is immediate (flutter's model): a board absent from
/// ONE frame loses its texture, and its return pays one fill — there is
/// no case in a canvas app where content vanishes for a frame and comes
/// back cheaper than the inline paint a fill costs anyway.
#[test]
fn an_unused_entry_dies_with_the_frame_that_dropped_it() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let board = board();
    let with_board = Arc::new(wrapper(&board, true));
    let without_board = Arc::new(wrapper(&self::board(), false));

    let (_, warm) = frame_arc(&mut ctx, &device, &queue, &with_board, Matrix::IDENTITY);
    assert_eq!((warm.raster_quads, warm.raster_fills), (1, 1));

    // One frame that never requests the board: its entry is swept.
    frame_arc(&mut ctx, &device, &queue, &without_board, Matrix::IDENTITY);

    // The board returns: a fresh fill, not a stale hit.
    let (_, back) = frame_arc(&mut ctx, &device, &queue, &with_board, Matrix::IDENTITY);
    assert_eq!(
        (back.raster_quads, back.raster_fills),
        (1, 1),
        "the comeback refills"
    );
}
