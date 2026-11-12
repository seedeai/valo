//! Clip coverage: the rect that the clips around a draw leave it to show in,
//! kept as the walk meets clips (Impeller's clip coverage, in
//! `EntityPassClipStack`). A depth ceiling alone makes a clip cost the whole
//! target, since an Intersect ceiling covers everything outside its shape.
//! The coverage does what the ceiling then need not:
//!
//! - every draw under a clip is cut to the coverage by its scissor, the
//!   clip's own ceiling among them, so the ceiling is written only inside
//!   the coverage and a clip costs its own area (Impeller's
//!   `SetClipScissor`);
//! - a rectangle that the scissor states draws nothing: one that holds all
//!   the coverage there was changes nothing, one on whole pixels is the
//!   scissor alone, and so is one that is not anti-aliased, wherever it
//!   lies, cut at the nearest whole pixels
//!   (`EntityPassClipStack::RecordClip`).
//!
//! What becomes of a clip is decided first, as a value ([`ClipPlan`]), from
//! rects alone, so it needs no GPU.
//!
//! A clip's shape is recorded with its op: its bounds, whether it is a
//! rectangle along the axes of its list, whether its edge is anti-aliased.
//! What is kept here, at replay, is what only the replay knows: the
//! coverage around the clip (an embedder's clips, a layer's own texels),
//! and whether what the list is drawn under leaves the rectangle along the
//! target's pixel grid.
//!
//! The invariant a later editor will break: a ceiling exists only inside its
//! clip's coverage, so every draw in the clip's scope must be handed that
//! coverage or a narrower one. A draw handed a wider one shows outside the
//! clip.

use valo_dl::ClipOp;
use valo_geometry::{Matrix, Rect};

use crate::frame::Scissor;

use super::draw_context::PixelArea;

/// `WHOLE_PIXEL_TOLERANCE` is how far from a whole pixel an anti-aliased
/// rectangle clip's edge may lie and the scissor still state it: Impeller's
/// threshold in `RecordClip`. With four samples a pixel, an edge under an
/// eighth of a pixel from a whole one covers the same samples.
const WHOLE_PIXEL_TOLERANCE: f32 = 0.124;

/// `HARD_EDGE_TOLERANCE` is the same for a rectangle clip that is not
/// anti-aliased: its edge goes to the nearest whole pixel wherever it lies,
/// never more than half a pixel away (Impeller's `Rect::Round` of the
/// coverage when `is_aa` is false).
const HARD_EDGE_TOLERANCE: f32 = 0.5;

/// `ClipCoverage` is where the clips around a draw let it show, in the
/// replay coordinates of the context it is drawn into.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ClipCoverage {
    /// No clip bounds the draw: anywhere in its context.
    Unclipped,
    /// Inside this rect only; nowhere, when the rect is empty.
    Within(Rect),
}

/// `ClipShape` is what a clip's planning reads of its shape.
#[derive(Clone, Copy, Debug)]
pub(super) struct ClipShape {
    /// The shape's bounds in replay coordinates.
    pub bounds: Rect,
    /// The shape is exactly `bounds`: a rectangle with its sides along the
    /// pixel grid (Impeller's `is_axis_aligned_rect`).
    pub is_rect: bool,
    /// Whether the clip's edge is anti-aliased (Impeller's `is_aa`).
    pub anti_alias: bool,
}

impl ClipShape {
    /// `whole_pixel_tolerance` is how far from a whole pixel the edges of
    /// what the clip leaves may lie for the scissor to state it; `None`
    /// for a shape no scissor states.
    fn whole_pixel_tolerance(&self) -> Option<f32> {
        match (self.is_rect, self.anti_alias) {
            (false, _) => None,
            (true, true) => Some(WHOLE_PIXEL_TOLERANCE),
            (true, false) => Some(HARD_EDGE_TOLERANCE),
        }
    }
}

/// `scales_and_moves` reports whether `transform` only scales and moves, so
/// a rectangle's sides stay along the axes under it: Impeller's
/// `IsTranslationScaleOnly`, a mirror included.
pub(super) fn scales_and_moves(transform: &Matrix) -> bool {
    let [scale_x, shear_y, shear_x, scale_y, ..] = transform.to_affine();
    transform.is_affine() && shear_y == 0.0 && shear_x == 0.0 && scale_x != 0.0 && scale_y != 0.0
}

/// `ClipPlan` is what becomes of one clip, decided before anything is
/// drawn: Impeller's `EntityPassClipStack::RecordClip`. Either way it
/// carries the coverage the clipped draws take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ClipPlan {
    /// The scissor alone clips: nothing is drawn.
    Scissor(ClipCoverage),
    /// The shape is marked and its ceiling written, inside the coverage.
    Draw(ClipCoverage),
}

impl ClipCoverage {
    /// `NOWHERE` is the coverage of draws that no clip lets show.
    pub const NOWHERE: Self = ClipCoverage::Within(Rect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    });

    /// `plan_clip` is what becomes of a clip by `op` to `shape`, made under
    /// this coverage in a context over `area`.
    pub fn plan_clip(&self, op: ClipOp, shape: &ClipShape, area: &PixelArea) -> ClipPlan {
        if op == ClipOp::Difference {
            // What a difference leaves has the bounds of what there was.
            return ClipPlan::Draw(*self);
        }
        let current = self.rect_in(area);
        if shape.is_rect && holds(&shape.bounds, &current) {
            return ClipPlan::Scissor(*self);
        }
        let Some(remaining) = current.intersect(&shape.bounds) else {
            return ClipPlan::Scissor(ClipCoverage::NOWHERE);
        };
        let on_whole_pixels = shape
            .whole_pixel_tolerance()
            .and_then(|tolerance| area.snapped(&remaining, tolerance));
        match on_whole_pixels {
            Some(snapped) => ClipPlan::Scissor(ClipCoverage::Within(snapped)),
            None => ClipPlan::Draw(ClipCoverage::Within(remaining)),
        }
    }

    /// `scissor` is the texels of a context over `area` that a draw under
    /// this coverage may write; `None` when it may write none.
    pub fn scissor(&self, area: &PixelArea) -> Option<Scissor> {
        match self {
            ClipCoverage::Unclipped => Some(Scissor::whole(area.size())),
            ClipCoverage::Within(rect) => area.scissor(rect),
        }
    }

    /// `rect_in` is the coverage as a rect, in a context over `area`: all
    /// its texels when nothing clips.
    fn rect_in(&self, area: &PixelArea) -> Rect {
        match self {
            ClipCoverage::Unclipped => area.texels(),
            ClipCoverage::Within(rect) => *rect,
        }
    }
}

/// `holds` reports whether `outer` holds all of `inner`.
fn holds(outer: &Rect, inner: &Rect) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && outer.right() >= inner.right()
        && outer.bottom() >= inner.bottom()
}

/// Impeller's `clip_stack_unittests.cc`, for what a clip's plan shares with
/// `RecordClip`: `should_render` is [`ClipPlan::Draw`], and the coverage is
/// `CurrentClipCoverage` after it. A subpass is a context over the
/// subpass's coverage, whose draws start unclipped.
#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> PixelArea {
        PixelArea::of_size([100, 100])
    }

    /// An anti-aliased rectangle along the pixel grid.
    fn rect(bounds: Rect) -> ClipShape {
        ClipShape {
            bounds,
            is_rect: true,
            anti_alias: true,
        }
    }

    /// The same, not anti-aliased.
    fn hard_rect(bounds: Rect) -> ClipShape {
        ClipShape {
            anti_alias: false,
            ..rect(bounds)
        }
    }

    /// An anti-aliased shape that is not its bounds.
    fn shape(bounds: Rect) -> ClipShape {
        ClipShape {
            is_rect: false,
            ..rect(bounds)
        }
    }

    fn intersect(coverage: ClipCoverage, shape: &ClipShape) -> ClipPlan {
        coverage.plan_clip(ClipOp::Intersect, shape, &area())
    }

    fn ltrb(left: f32, top: f32, right: f32, bottom: f32) -> Rect {
        Rect::from_ltrb(left, top, right, bottom)
    }

    /// A rectangle between pixels is drawn, inside itself.
    #[test]
    fn append_and_restore_clip_coverage() {
        let between_pixels = ltrb(50.0, 50.0, 55.5, 55.5);
        assert_eq!(
            intersect(ClipCoverage::Unclipped, &rect(between_pixels)),
            ClipPlan::Draw(ClipCoverage::Within(between_pixels))
        );
    }

    /// A rectangle between pixels that is not anti-aliased is the scissor
    /// alone, at the nearest whole pixels.
    #[test]
    fn append_and_restore_clip_coverage_non_aa() {
        assert_eq!(
            intersect(
                ClipCoverage::Unclipped,
                &hard_rect(ltrb(50.0, 50.0, 55.4, 55.4))
            ),
            ClipPlan::Scissor(ClipCoverage::Within(ltrb(50.0, 50.0, 55.0, 55.0)))
        );
    }

    /// A clip made in a subpass is planned in the subpass's coverage, and
    /// drawn when it lies between pixels.
    #[test]
    fn clip_and_restore_with_subpasses() {
        let subpass = PixelArea::over(ltrb(50.0, 50.0, 55.0, 55.0));
        let between_pixels = ltrb(54.0, 54.0, 54.5, 54.5);
        let plan =
            ClipCoverage::Unclipped.plan_clip(ClipOp::Intersect, &rect(between_pixels), &subpass);
        assert_eq!(plan, ClipPlan::Draw(ClipCoverage::Within(between_pixels)));
    }

    /// The same clip not anti-aliased is the scissor alone: what the
    /// subpass leaves of it, at the nearest whole pixels.
    #[test]
    fn clip_and_restore_with_subpasses_non_aa() {
        let subpass = PixelArea::over(ltrb(50.0, 50.0, 55.0, 55.0));
        let plan = ClipCoverage::Unclipped.plan_clip(
            ClipOp::Intersect,
            &hard_rect(ltrb(54.0, 54.0, 55.4, 55.4)),
            &subpass,
        );
        assert_eq!(
            plan,
            ClipPlan::Scissor(ClipCoverage::Within(ltrb(54.0, 54.0, 55.0, 55.0)))
        );
    }

    /// The flag changes nothing for a shape that is not a rectangle along
    /// the pixel grid, nor for a difference: both are drawn.
    #[test]
    fn a_hard_edge_changes_only_an_intersecting_rect() {
        let between_pixels = ltrb(10.5, 10.5, 20.5, 20.5);
        let hard_shape = ClipShape {
            is_rect: false,
            ..hard_rect(between_pixels)
        };
        assert_eq!(
            intersect(ClipCoverage::Unclipped, &hard_shape),
            ClipPlan::Draw(ClipCoverage::Within(between_pixels))
        );
        assert_eq!(
            ClipCoverage::Unclipped.plan_clip(
                ClipOp::Difference,
                &hard_rect(between_pixels),
                &area()
            ),
            ClipPlan::Draw(ClipCoverage::Unclipped)
        );
    }

    /// A hard-edged rectangle thinner than half a pixel, between two pixel
    /// edges, rounds to nothing: its scope shows nowhere.
    #[test]
    fn a_hard_edged_sliver_rounds_to_nowhere() {
        let plan = intersect(
            ClipCoverage::Unclipped,
            &hard_rect(ltrb(10.1, 0.0, 10.4, 50.0)),
        );
        let ClipPlan::Scissor(coverage) = plan else {
            panic!("a hard-edged rectangle is never drawn: {plan:?}");
        };
        assert_eq!(coverage.scissor(&area()), None);
    }

    /// A second clip larger than the first needs no update.
    #[test]
    fn append_larger_clip_coverage() {
        let first = ClipCoverage::Within(ltrb(50.0, 50.0, 55.5, 55.5));
        assert_eq!(
            intersect(first, &rect(ltrb(0.0, 0.0, 100.5, 100.5))),
            ClipPlan::Scissor(first)
        );
    }

    /// A rectangle on whole pixels is the scissor alone; a larger shape
    /// that is no rectangle is drawn all the same, since its bounds hold
    /// more than it does.
    #[test]
    fn append_larger_clip_coverage_with_difference_or_non_square() {
        let on_pixels = ltrb(50.0, 50.0, 55.0, 55.0);
        let first = ClipCoverage::Within(on_pixels);
        assert_eq!(
            intersect(ClipCoverage::Unclipped, &rect(on_pixels)),
            ClipPlan::Scissor(first)
        );
        assert_eq!(
            intersect(first, &shape(ltrb(0.0, 0.0, 100.0, 100.0))),
            ClipPlan::Draw(first)
        );
    }

    /// Clips that shrink are all applied.
    #[test]
    fn append_decreasing_size_clip_coverage() {
        let mut coverage = ClipCoverage::Unclipped;
        for step in 1..20 {
            let inset = step as f32;
            let bounds = ltrb(inset, inset, 99.6 - inset, 99.6 - inset);
            let plan = intersect(coverage, &rect(bounds));
            assert_eq!(plan, ClipPlan::Draw(ClipCoverage::Within(bounds)));
            coverage = ClipCoverage::Within(bounds);
        }
    }

    /// Clips that grow are all skipped.
    #[test]
    fn append_increasing_size_clip_coverage() {
        for step in 1..20 {
            let outset = step as f32;
            let bounds = ltrb(-outset, -outset, 100.0 + outset, 100.0 + outset);
            assert_eq!(
                intersect(ClipCoverage::Unclipped, &rect(bounds)),
                ClipPlan::Scissor(ClipCoverage::Unclipped)
            );
        }
    }

    /// An edge under an eighth of a pixel from a whole one goes to the
    /// whole one, and the scissor states the rectangle.
    #[test]
    fn a_rect_nearly_on_whole_pixels_is_the_scissor_alone() {
        assert_eq!(
            intersect(
                ClipCoverage::Unclipped,
                &rect(ltrb(10.1, 19.9, 40.12, 60.0))
            ),
            ClipPlan::Scissor(ClipCoverage::Within(ltrb(10.0, 20.0, 40.0, 60.0)))
        );
    }

    /// A shape is drawn inside what it leaves of the coverage around it.
    #[test]
    fn a_shape_is_drawn_inside_what_it_leaves_of_the_coverage() {
        let around = ClipCoverage::Within(ltrb(0.0, 0.0, 25.0, 100.0));
        assert_eq!(
            intersect(around, &shape(ltrb(10.0, 20.0, 40.0, 60.0))),
            ClipPlan::Draw(ClipCoverage::Within(ltrb(10.0, 20.0, 25.0, 60.0)))
        );
    }

    /// A clip that misses the coverage leaves nowhere to show, and draws
    /// nothing itself.
    #[test]
    fn a_clip_that_misses_the_coverage_leaves_nowhere() {
        let around = ClipCoverage::Within(ltrb(0.0, 0.0, 10.0, 10.0));
        let plan = intersect(around, &shape(ltrb(50.0, 50.0, 60.0, 60.0)));
        assert_eq!(plan, ClipPlan::Scissor(ClipCoverage::NOWHERE));
        assert_eq!(ClipCoverage::NOWHERE.scissor(&area()), None);
    }

    /// A difference is always drawn, and leaves the coverage as it was.
    #[test]
    fn a_difference_is_drawn_and_keeps_the_coverage() {
        let around = ClipCoverage::Within(ltrb(0.0, 0.0, 50.0, 50.0));
        let hole = rect(ltrb(10.0, 10.0, 20.0, 20.0));
        assert_eq!(
            around.plan_clip(ClipOp::Difference, &hole, &area()),
            ClipPlan::Draw(around)
        );
    }

    /// The scissor is the whole texels that hold the coverage, in the
    /// context's own texels, cut to the texture.
    #[test]
    fn the_scissor_holds_the_coverage_in_the_contexts_texels() {
        let layer = PixelArea::over(Rect::new(20.0, 30.0, 50.0, 40.0));
        let coverage = ClipCoverage::Within(ltrb(25.5, 28.0, 90.0, 41.2));
        assert_eq!(
            coverage.scissor(&layer),
            Some(Scissor {
                x: 5,
                y: 0,
                width: 45,
                height: 12,
            })
        );
        assert_eq!(
            ClipCoverage::Unclipped.scissor(&layer),
            Some(Scissor::whole([50, 40]))
        );
    }
}
