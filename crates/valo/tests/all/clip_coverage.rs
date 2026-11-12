//! Clips and the coverage they leave. A draw is cut by its scissor to the
//! bounds of the clips around it, a clip's own ceiling among them, and a
//! rectangle clip is that scissor alone, with nothing drawn for it, when it
//! lies on whole pixels or is not anti-aliased, which takes it to the
//! nearest ones. An anti-aliased clip shows the same pixels drawn or not,
//! and every clip ends with its scope and holds in a layer's own texels and
//! past a split of its target.

use std::sync::Arc;

use valo::{
    BlendMode, ClipOp, Color, Context, DisplayList, DisplayListBuilder, FillRule, Offscreen, Paint,
    Path, PathBuilder, Rect, RenderStats,
};

const SIZE: [u32; 2] = [64, 32];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn red() -> Paint {
    Paint::from_color(Color::rgb(1.0, 0.0, 0.0))
}

fn blue() -> Paint {
    Paint::from_color(Color::rgb(0.0, 0.0, 1.0))
}

fn whole_target() -> Rect {
    Rect::new(0.0, 0.0, SIZE[0] as f32, SIZE[1] as f32)
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE[0] + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

/// Renders the list `record` records over white, on a device of its own;
/// `None`, with a note that `test` is skipped, when the machine has none.
fn render(
    test: &str,
    record: impl FnOnce(&mut DisplayListBuilder),
) -> Option<(Vec<u8>, RenderStats)> {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP {test}: no GPU adapter");
        return None;
    };
    let mut context = Context::new(device.clone(), queue.clone());
    let offscreen = Offscreen::new(&device, SIZE);
    let mut builder = DisplayListBuilder::new();
    record(&mut builder);
    let stats = context.render(&builder.build(), &offscreen.target(Some(Color::WHITE)));
    let pixels = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), SIZE);
    Some((pixels, stats))
}

/// `rect` as a path that walks back to its first corner before it closes:
/// the same shape, which the renderer does not take for a rectangle, so its
/// clip is drawn.
fn rect_walked_round(rect: Rect) -> Arc<Path> {
    let mut path = PathBuilder::new();
    path.move_to((rect.x, rect.y))
        .line_to((rect.right(), rect.y))
        .line_to((rect.right(), rect.bottom()))
        .line_to((rect.x, rect.bottom()))
        .line_to((rect.x, rect.y))
        .close();
    path.build()
}

/// A scene that shows where a clip cuts: a fill, a circle across the
/// clip's edges and a faded layer, all inside `clip`, then a stripe after
/// the clip's scope.
fn clipped_scene(builder: &mut DisplayListBuilder, clip: impl FnOnce(&mut DisplayListBuilder)) {
    builder.save();
    clip(builder);
    builder.draw_rect(whole_target(), &red());
    builder.draw_circle((30.0, 16.0), 14.0, &blue());
    let faded = Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.5));
    builder.save_layer(None, &faded);
    builder.draw_rect(Rect::new(0.0, 10.0, 64.0, 8.0), &blue());
    builder.draw_rect(Rect::new(20.0, 0.0, 8.0, 32.0), &red());
    builder.restore();
    builder.restore();
    builder.draw_rect(Rect::new(0.0, 28.0, 64.0, 2.0), &blue());
}

/// A rectangle clip on whole pixels draws nothing itself, and one between
/// pixels draws its stencil and its ceiling: two draw calls more.
#[test]
fn a_rect_clip_on_whole_pixels_draws_nothing() {
    let test = "a_rect_clip_on_whole_pixels_draws_nothing";
    let clipped_to = |rect: Rect| {
        render(test, |builder| {
            builder.clip_rect(rect, ClipOp::Intersect, true);
            builder.draw_rect(whole_target(), &red());
        })
    };
    let Some((pixels, on_pixels)) = clipped_to(Rect::new(8.0, 4.0, 16.0, 8.0)) else {
        return;
    };
    assert_eq!(on_pixels.clips, 1);
    assert_eq!(on_pixels.draw_calls, 1, "the fill alone");
    assert_eq!(pixel(&pixels, 8, 4), RED, "the clip's first pixel");
    assert_eq!(pixel(&pixels, 23, 11), RED, "its last");
    assert_eq!(pixel(&pixels, 7, 4), WHITE);
    assert_eq!(pixel(&pixels, 8, 3), WHITE);
    assert_eq!(pixel(&pixels, 24, 11), WHITE);
    assert_eq!(pixel(&pixels, 23, 12), WHITE);

    let Some((pixels, between_pixels)) = clipped_to(Rect::new(8.5, 4.0, 16.0, 8.0)) else {
        return;
    };
    assert_eq!(between_pixels.clips, 1);
    assert_eq!(between_pixels.draw_calls, 3, "stencil, ceiling, fill");
    assert_eq!(pixel(&pixels, 12, 8), RED);
    assert_eq!(pixel(&pixels, 7, 8), WHITE);
    assert_eq!(pixel(&pixels, 8, 8), [255, 128, 128, 255], "half covered");
    assert_eq!(pixel(&pixels, 25, 8), WHITE);
}

/// A rectangle clip that is not anti-aliased is the scissor alone wherever
/// it lies: half a pixel off, it draws no clip and cuts at the nearest whole
/// pixels, where the same clip anti-aliased is drawn and half covers the
/// pixels its edges cross.
#[test]
fn a_hard_edged_rect_clip_between_pixels_draws_nothing_and_cuts_at_whole_pixels() {
    let test = "a_hard_edged_rect_clip_between_pixels_draws_nothing_and_cuts_at_whole_pixels";
    let between_pixels = Rect::from_ltrb(8.5, 4.5, 24.5, 12.5);
    let clipped = |anti_alias: bool| {
        render(test, |builder| {
            builder.clip_rect(between_pixels, ClipOp::Intersect, anti_alias);
            builder.draw_rect(whole_target(), &red());
        })
    };
    let Some((pixels, hard)) = clipped(false) else {
        return;
    };
    assert_eq!(hard.clips, 1);
    assert_eq!(hard.draw_calls, 1, "the fill alone");
    assert_eq!(
        pixel(&pixels, 9, 5),
        RED,
        "the first pixel the scissor keeps"
    );
    assert_eq!(pixel(&pixels, 24, 12), RED, "the last");
    assert_eq!(pixel(&pixels, 8, 8), WHITE, "left of the scissor");
    assert_eq!(pixel(&pixels, 25, 8), WHITE, "right of it");
    assert_eq!(pixel(&pixels, 16, 4), WHITE, "above it");
    assert_eq!(pixel(&pixels, 16, 13), WHITE, "below it");

    let Some((pixels, soft)) = clipped(true) else {
        return;
    };
    assert_eq!(soft.draw_calls, 3, "stencil, ceiling, fill");
    assert_eq!(pixel(&pixels, 16, 8), RED);
    assert_eq!(pixel(&pixels, 8, 8), [255, 128, 128, 255], "half covered");
    assert_eq!(pixel(&pixels, 24, 8), [255, 128, 128, 255], "half covered");
    assert_eq!(pixel(&pixels, 25, 8), WHITE);
}

/// The flag changes nothing for a clip that is no rectangle along the pixel
/// grid: a rounded rectangle and a turned rectangle are drawn, and show the
/// same pixels, with it and without.
#[test]
fn a_hard_edge_changes_nothing_for_a_rounded_or_turned_clip() {
    let test = "a_hard_edge_changes_nothing_for_a_rounded_or_turned_clip";
    let scene = |anti_alias: bool| {
        render(test, |builder| {
            builder.save();
            builder.clip_rrect(
                Rect::new(4.5, 4.5, 24.0, 20.0),
                6.0,
                ClipOp::Intersect,
                anti_alias,
            );
            builder.draw_rect(whole_target(), &red());
            builder.restore();
            builder.translate(48.0, 16.0);
            builder.rotate(0.4);
            builder.clip_rect(
                Rect::new(-8.5, -8.5, 17.0, 17.0),
                ClipOp::Intersect,
                anti_alias,
            );
            builder.draw_rect(Rect::new(-40.0, -40.0, 80.0, 80.0), &blue());
        })
    };
    let (Some((soft_pixels, soft)), Some((hard_pixels, hard))) = (scene(true), scene(false)) else {
        return;
    };
    assert_eq!(soft.draw_calls, 6, "two clips drawn, two fills");
    assert_eq!(hard.draw_calls, soft.draw_calls);
    assert!(hard_pixels == soft_pixels, "the flag changed a picture");
}

/// The scissor alone and a drawn clip show the same pixels, for a rectangle
/// on whole pixels and for one near them: the drawn clip's edge covers the
/// samples the whole pixel's does.
#[test]
fn the_scissor_alone_clips_as_a_drawn_clip_does() {
    let test = "the_scissor_alone_clips_as_a_drawn_clip_does";
    for rect in [
        Rect::new(8.0, 4.0, 40.0, 20.0),
        Rect::from_ltrb(8.1, 3.9, 47.9, 24.1),
    ] {
        let Some((by_scissor, scissored)) = render(test, |builder| {
            clipped_scene(builder, |builder| {
                builder.clip_rect(rect, ClipOp::Intersect, true)
            });
        }) else {
            return;
        };
        let Some((by_depth, drawn)) = render(test, |builder| {
            clipped_scene(builder, |builder| {
                builder.clip_path(
                    &rect_walked_round(rect),
                    FillRule::NonZero,
                    ClipOp::Intersect,
                    true,
                );
            });
        }) else {
            return;
        };
        assert_eq!(drawn.draw_calls, scissored.draw_calls + 2, "{rect:?}");
        assert!(by_scissor == by_depth, "{rect:?} clips differently");
        assert_eq!(pixel(&by_scissor, 4, 16), WHITE, "outside the clip");
        assert_eq!(pixel(&by_scissor, 4, 28), BLUE, "after its scope");
    }
}

/// A clip whose shape is not its bounds is drawn, inside its bounds: its
/// corners clip, and what follows its scope shows everywhere.
#[test]
fn a_rounded_clip_is_drawn_and_ends_with_its_scope() {
    let Some((pixels, stats)) = render("a_rounded_clip_is_drawn_and_ends_with_its_scope", |b| {
        b.save();
        b.clip_rrect(
            Rect::new(8.0, 4.0, 40.0, 24.0),
            12.0,
            ClipOp::Intersect,
            true,
        );
        b.draw_rect(whole_target(), &red());
        b.restore();
        b.draw_rect(Rect::new(0.0, 0.0, 4.0, 4.0), &blue());
    }) else {
        return;
    };
    assert_eq!(stats.draw_calls, 4, "stencil, ceiling and two fills");
    assert_eq!(pixel(&pixels, 28, 16), RED, "inside the shape");
    assert_eq!(pixel(&pixels, 9, 5), WHITE, "inside its bounds, outside it");
    assert_eq!(pixel(&pixels, 4, 16), WHITE, "outside its bounds");
    assert_eq!(pixel(&pixels, 2, 2), BLUE, "after its scope");
}

/// A rectangle that holds everything the clips around it leave changes
/// nothing, and one that misses it leaves its scope nowhere to show:
/// neither draws.
#[test]
fn a_rect_clip_around_its_coverage_changes_nothing_and_one_beside_it_hides_its_scope() {
    let test = "a_rect_clip_around_its_coverage_changes_nothing_and_one_beside_it_hides_its_scope";
    let Some((pixels, stats)) = render(test, |builder| {
        builder.clip_rect(
            Rect::new(-10.0, -10.0, 100.0, 100.0),
            ClipOp::Intersect,
            true,
        );
        builder.draw_rect(Rect::new(0.0, 0.0, 8.0, 8.0), &red());
        builder.save();
        builder.clip_rect(Rect::new(0.0, 0.0, 16.5, 16.5), ClipOp::Intersect, true);
        builder.clip_rect(Rect::new(0.0, 0.0, 32.0, 32.0), ClipOp::Intersect, true);
        builder.draw_rect(Rect::new(8.0, 8.0, 40.0, 4.0), &blue());
        builder.save();
        builder.clip_rrect(
            Rect::new(40.0, 0.0, 20.0, 20.0),
            4.0,
            ClipOp::Intersect,
            true,
        );
        builder.draw_rect(whole_target(), &red());
        builder.restore();
        builder.restore();
    }) else {
        return;
    };
    assert_eq!(
        stats.draw_calls, 4,
        "two fills, and the one clip between pixels: its stencil and its ceiling"
    );
    assert_eq!(pixel(&pixels, 4, 4), RED);
    assert_eq!(pixel(&pixels, 12, 10), BLUE);
    assert_eq!(
        pixel(&pixels, 20, 10),
        WHITE,
        "past the clip between pixels"
    );
    assert_eq!(
        pixel(&pixels, 50, 10),
        WHITE,
        "the scope with nowhere to show"
    );
}

/// A clip made inside a layer is in the layer's own texels, wherever the
/// layer lies in its parent, and the clips around the layer meet its
/// composite.
#[test]
fn a_clip_in_a_layer_is_in_the_layers_texels() {
    let Some((pixels, _)) = render("a_clip_in_a_layer_is_in_the_layers_texels", |builder| {
        builder.clip_rect(Rect::new(0.0, 0.0, 40.0, 32.0), ClipOp::Intersect, true);
        let faded = Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.5));
        builder.save_layer(Some(Rect::new(20.0, 8.0, 40.0, 20.0)), &faded);
        builder.clip_rect(Rect::new(24.0, 12.0, 30.0, 8.0), ClipOp::Intersect, true);
        builder.draw_rect(whole_target(), &blue());
        builder.draw_rect(Rect::new(24.0, 12.0, 4.0, 4.0), &blue());
        builder.restore();
    }) else {
        return;
    };
    let faded_blue = [128, 128, 255, 255];
    assert_eq!(pixel(&pixels, 24, 12), faded_blue, "the inner clip's first");
    assert_eq!(
        pixel(&pixels, 39, 19),
        faded_blue,
        "the last the outer leaves"
    );
    assert_eq!(pixel(&pixels, 23, 12), WHITE);
    assert_eq!(pixel(&pixels, 24, 11), WHITE);
    assert_eq!(pixel(&pixels, 39, 20), WHITE);
    assert_eq!(
        pixel(&pixels, 40, 19),
        WHITE,
        "the outer clip cuts the layer"
    );
}

/// A list embedded under a clip draws inside it, and a clip the embedded
/// list makes at its root ends with the list.
#[test]
fn an_embedded_lists_clips_end_with_it_and_its_embedders_hold_in_it() {
    let test = "an_embedded_lists_clips_end_with_it_and_its_embedders_hold_in_it";
    let mut nested = DisplayListBuilder::new();
    nested.clip_rect(Rect::new(0.0, 0.0, 16.0, 32.0), ClipOp::Intersect, true);
    nested.draw_rect(whole_target(), &red());
    let nested: Arc<DisplayList> = Arc::new(nested.build());
    let Some((pixels, stats)) = render(test, |builder| {
        builder.save();
        builder.clip_rect(Rect::new(0.0, 0.0, 64.0, 16.0), ClipOp::Intersect, true);
        builder.translate(8.0, 0.0);
        builder.draw_display_list(&nested);
        builder.restore();
        builder.draw_rect(Rect::new(40.0, 20.0, 8.0, 8.0), &blue());
    }) else {
        return;
    };
    assert_eq!(stats.draw_calls, 2, "both clips are the scissor's");
    assert_eq!(pixel(&pixels, 8, 0), RED);
    assert_eq!(pixel(&pixels, 23, 15), RED);
    assert_eq!(pixel(&pixels, 7, 8), WHITE, "left of the list's clip");
    assert_eq!(pixel(&pixels, 24, 8), WHITE, "right of it");
    assert_eq!(pixel(&pixels, 12, 16), WHITE, "below its embedder's");
    assert_eq!(pixel(&pixels, 44, 24), BLUE, "after both");
}

/// A split target starts its next pass from a copy of itself and replays
/// the clips still active inside their own coverage: what was drawn before
/// the split outside a clip stays, and what is drawn after it inside the
/// clip stays inside, for a drawn clip and for the scissor alone.
#[test]
fn clips_and_their_coverage_hold_past_a_split() {
    let multiply = Paint {
        blend_mode: BlendMode::Multiply,
        ..Paint::from_color(Color::rgb(0.5, 0.5, 0.5))
    };
    let Some((pixels, stats)) = render("clips_and_their_coverage_hold_past_a_split", |builder| {
        builder.draw_rect(whole_target(), &red());
        builder.save();
        builder.clip_rect(Rect::new(4.0, 4.0, 24.0, 24.0), ClipOp::Intersect, true);
        builder.clip_rrect(
            Rect::new(4.0, 4.0, 40.0, 24.0),
            12.0,
            ClipOp::Intersect,
            true,
        );
        builder.draw_rect(Rect::new(12.0, 12.0, 4.0, 4.0), &multiply);
        builder.draw_rect(whole_target(), &blue());
        builder.restore();
    }) else {
        return;
    };
    assert_eq!(stats.snapshots, 1, "the destination read splits the target");
    assert_eq!(pixel(&pixels, 16, 16), BLUE, "inside both clips");
    assert_eq!(pixel(&pixels, 5, 5), RED, "outside the rounded corner");
    assert_eq!(pixel(&pixels, 30, 16), RED, "outside the rectangle");
    assert_eq!(pixel(&pixels, 60, 30), RED, "what the first pass drew");
}

/// A difference is drawn inside the coverage around it, and cuts its hole
/// there.
#[test]
fn a_difference_cuts_its_hole_inside_the_coverage_around_it() {
    let test = "a_difference_cuts_its_hole_inside_the_coverage_around_it";
    let Some((pixels, _)) = render(test, |builder| {
        builder.clip_rect(Rect::new(8.0, 4.0, 32.0, 24.0), ClipOp::Intersect, true);
        builder.clip_rect(Rect::new(0.0, 8.0, 20.0, 8.0), ClipOp::Difference, true);
        builder.draw_rect(whole_target(), &red());
    }) else {
        return;
    };
    assert_eq!(pixel(&pixels, 12, 6), RED, "above the hole");
    assert_eq!(pixel(&pixels, 12, 12), WHITE, "in it");
    assert_eq!(pixel(&pixels, 24, 12), RED, "beside it");
    assert_eq!(pixel(&pixels, 4, 6), WHITE, "outside the clip around it");
}

/// A rectangle clip under a rotation is no scissor's: it is drawn, and it
/// clips to its turned shape inside its bounds.
#[test]
fn a_turned_rect_clip_is_drawn() {
    let Some((pixels, stats)) = render("a_turned_rect_clip_is_drawn", |builder| {
        builder.translate(32.0, 16.0);
        builder.rotate(std::f32::consts::FRAC_PI_4);
        builder.clip_rect(Rect::new(-10.0, -10.0, 20.0, 20.0), ClipOp::Intersect, true);
        builder.draw_rect(Rect::new(-40.0, -40.0, 80.0, 80.0), &red());
    }) else {
        return;
    };
    assert_eq!(stats.draw_calls, 3, "stencil, ceiling, fill");
    assert_eq!(pixel(&pixels, 32, 16), RED, "the diamond's middle");
    assert_eq!(pixel(&pixels, 32, 4), RED, "near its top corner");
    assert_eq!(
        pixel(&pixels, 21, 5),
        WHITE,
        "inside its bounds, outside it"
    );
    assert_eq!(pixel(&pixels, 10, 16), WHITE, "outside its bounds");
}
