//! The no-growth invariant. 300 varied frames (animated text
//! size cycling the outline tier, periodic vertex spikes, pan) must not
//! accumulate resources — the pools that self-evict plateau, and the two
//! that didn't (glyph paths, spike-sized HostBuffer blocks) are pinned here.

use std::sync::Arc;

use valo::{
    Color, Context, DisplayListBuilder, DrawParagraphExt, FontCollection, Offscreen, Paint,
    ParagraphBuilder, PathBuilder, Rect, TextStyle,
};

fn fonts() -> Arc<FontCollection> {
    valo_harness::example_fonts()
}

/// One frame of the churn scene. `i` animates everything that stresses a
/// cache: text size (unique outline-tier rasters per frame), a pan, and a
/// heavyweight path every 50th frame (a dedicated vertex block).
fn scene(fonts: &Arc<FontCollection>, i: usize) -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, 400.0, 300.0),
        &Paint::from_color(Color::rgb(0.1, 0.1, 0.14)),
    );
    b.save();
    b.translate((i % 40) as f32, 0.0);
    let mut p = ParagraphBuilder::new(fonts);
    let size = 340.0 + (i as f32) * 0.5; // outline tier, a fresh size every frame
    p.add_text("Zg", &TextStyle::new("Fira Sans", size, Color::WHITE));
    let mut p = p.build();
    p.layout(f32::INFINITY);
    b.draw_paragraph(&p, (10.0, -120.0));
    b.restore();
    if i.is_multiple_of(50) {
        // ~17k points: forces a dedicated (larger than default) vertex block.
        let mut big = PathBuilder::new();
        big.move_to((0.0, 150.0));
        for k in 0..17000 {
            let x = k as f32 * 0.02;
            big.line_to((x, 150.0 + (k as f32 * 0.7).sin() * 40.0));
        }
        b.draw_path(
            &big.build(),
            valo::FillRule::NonZero,
            &Paint::from_color(Color::rgb(0.3, 0.6, 0.4)),
        );
    }
    b.build()
}

#[test]
fn no_resource_growth_over_300_frames() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP no_resource_growth_over_300_frames: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let fonts = fonts();
    ctx.set_fonts(fonts.clone());
    let offscreen = Offscreen::new(&device, [400, 300]);

    // Warm-up covers first-sight allocations AND one vertex spike, so the
    // baseline already contains everything a steady state legitimately holds.
    for i in 0..60 {
        ctx.render(&scene(&fonts, i), &offscreen.target(Some(Color::BLACK)));
    }
    let base = ctx.memory_report();

    let mut blocks_created_warm = 0u32;
    for i in 60..300 {
        let stats = ctx.render(&scene(&fonts, i), &offscreen.target(Some(Color::BLACK)));
        // Spike frames legitimately re-create their dedicated block: it
        // DRAINED while idle (that's the point — memory isn't pinned
        // between spikes). Warm frames must never allocate.
        if i >= 120 && i % 50 != 0 {
            blocks_created_warm += stats.blocks_created;
        }
    }
    let end = ctx.memory_report();

    // Self-evicting pools plateau…
    assert!(
        end.contours.count <= base.contours.count + 4,
        "contours: {} → {}",
        base.contours.count,
        end.contours.count
    );
    assert!(
        end.targets.count <= base.targets.count + 4,
        "targets: {} → {}",
        base.targets.count,
        end.targets.count
    );
    // …and the two audited leaks stay fixed:
    assert!(
        end.glyph_paths.count <= 32,
        "outline-path cache must evict idle sizes: {} entries",
        end.glyph_paths.count
    );
    assert!(
        end.host_buffer.count <= base.host_buffer.count,
        "spike blocks must drain: {} → {}",
        base.host_buffer.count,
        end.host_buffer.count
    );
    assert_eq!(blocks_created_warm, 0, "warm frames allocate no new blocks");
    // Driver-side cross-check (all zeros unless built with `counters`).
    if end.wgpu.enabled {
        assert!(end.wgpu.textures <= base.wgpu.textures + 4);
        assert!(end.wgpu.buffers <= base.wgpu.buffers + 4);
    }
}
