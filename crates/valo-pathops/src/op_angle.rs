//! Port of Skia's src/pathops/SkOpAngle.{h,cpp}: the direction a segment leaves a span in, and
//! the sorting of all the angles at a span into a ring ordered around the point.
//!
//! `SkOpAngle` is a handle into `SkOpGlobalState`.

use std::ops::{Index, IndexMut};

use crate::core::float_bits::{
    sk_ieee_double_divide, sk_t_abs, std_max, std_min, SK_SCALAR_INFINITY,
};
use crate::core::path::SkPathVerb;
use crate::core::t_sort::sk_t_q_sort;
use crate::intersections::SkIntersections;
use crate::line_parameters::SkLineParameters;
use crate::op_segment::SkOpSegment;
use crate::op_span::{SkOpSpan, SkOpSpanBase};
use crate::path_ops_cubic::SkDCubic;
use crate::path_ops_curve::{
    SkDCurve, SkDCurveSweep, CURVE_D_POINT_AT_T, CURVE_D_SLOPE_AT_T, CURVE_INTERSECT_RAY,
};
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::{SkDPoint, SkDVector};
use crate::path_ops_types::{
    almost_bequal_ulps, almost_equal_ulps, approximately_between_orderable, approximately_equal,
    approximately_equal_orderable, approximately_zero, arena_handle, between,
    sk_path_ops_verb_to_points, SkOpGlobalState,
};

/// `SkOpAngle::IncludeType`, compared in Skia's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum IncludeType {
    UnaryWinding,
    UnaryXor,
    BinarySingle,
    BinaryOpp,
}

arena_handle! {
    /// Stands for `SkOpAngle*`.
    SkOpAngle
}

/// `SkOpAngle`'s fields. Skia leaves them uninitialized until `set`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkOpAngleData {
    /// the curve from start to end
    pub(crate) original_curve_part: SkDCurve,
    /// the curve from start to end offset as needed
    pub(crate) part: SkDCurveSweep,
    pub(crate) side: f64,
    /// used only to sort a pair of lines or line-like sections
    pub(crate) tangent_half: SkLineParameters,
    pub(crate) next: Option<SkOpAngle>,
    pub(crate) last_marked: Option<SkOpSpanBase>,
    pub(crate) start: SkOpSpanBase,
    pub(crate) end: SkOpSpanBase,
    pub(crate) computed_end: Option<SkOpSpanBase>,
    pub(crate) sector_mask: i32,
    /// in 32nds of a circle
    pub(crate) sector_start: i8,
    pub(crate) sector_end: i8,
    pub(crate) unorderable: bool,
    pub(crate) compute_sector: bool,
    pub(crate) computed_sector: bool,
    pub(crate) tangents_ambiguous: bool,
}

impl Index<SkOpAngle> for SkOpGlobalState {
    type Output = SkOpAngleData;

    fn index(&self, angle: SkOpAngle) -> &SkOpAngleData {
        &self.angles[angle.0 as usize]
    }
}

impl IndexMut<SkOpAngle> for SkOpGlobalState {
    fn index_mut(&mut self, angle: SkOpAngle) -> &mut SkOpAngleData {
        &mut self.angles[angle.0 as usize]
    }
}

/* Angles are sorted counterclockwise. The smallest angle has a positive x and the smallest
positive y. The largest angle has a positive x and a zero y. */

/*             quarter angle values for sector

31   x > 0, y == 0              horizontal line (to the right)
0    x > 0, y == epsilon        quad/cubic horizontal tangent eventually going +y
1    x > 0, y > 0, x > y        nearer horizontal angle
2                  x + e == y   quad/cubic 45 going horiz
3    x > 0, y > 0, x == y       45 angle
4                  x == y + e   quad/cubic 45 going vert
5    x > 0, y > 0, x < y        nearer vertical angle
6    x == epsilon, y > 0        quad/cubic vertical tangent eventually going +x
7    x == 0, y > 0              vertical line (to the top)

                                      8  7  6
                                 9       |       5
                              10         |          4
                            11           |            3
                          12  \          |           / 2
                         13              |              1
                        14               |               0
                        15 --------------+------------- 31
                        16               |              30
                         17              |             29
                          18  /          |          \ 28
                            19           |           27
                              20         |         26
                                 21      |      25
                                     22 23 24
*/

impl SkOpAngle {
    // OPTIMIZATION: longest can all be either lazily computed here or precomputed in setup
    pub(crate) fn dist_end_ratio(self, state: &SkOpGlobalState, dist: f64) -> f64 {
        let mut longest = 0.0;
        let segment = self.segment(state);
        let pt_count = sk_path_ops_verb_to_points(segment.verb(state));
        let pts = segment.pts(state);
        for idx1 in 0..=pt_count - 1 {
            for idx2 in idx1 + 1..=pt_count {
                if idx1 == idx2 {
                    continue;
                }
                let mut v = SkDVector::default();
                v.set(&(pts[idx2 as usize] - pts[idx1 as usize]));
                let len_sq = v.length_squared();
                longest = std_max(longest, len_sq);
            }
        }
        longest.sqrt() / dist
    }

    pub(crate) fn end(self, state: &SkOpGlobalState) -> SkOpSpanBase {
        state[self].end
    }

    // OPTIMIZE: if this loops to only one other angle, after first compare fails, insert on other side
    // OPTIMIZE: return where insertion succeeded. Then, start next insertion on opposite side
    pub(crate) fn insert(self, state: &mut SkOpGlobalState, angle: SkOpAngle) -> bool {
        if state[angle].next.is_some() {
            if self.loop_count(state) >= angle.loop_count(state) {
                if !self.merge(state, angle) {
                    return true;
                }
            } else if state[self].next.is_some() {
                if !angle.merge(state, self) {
                    return true;
                }
            } else {
                angle.insert(state, self);
            }
            return true;
        }
        let singleton = state[self].next.is_none();
        if singleton {
            state[self].next = Some(self);
        }
        let mut next = state[self].next.expect("set above");
        if state[next].next == Some(self) {
            if singleton || angle.after(state, self) {
                state[self].next = Some(angle);
                state[angle].next = Some(next);
            } else {
                state[next].next = Some(angle);
                state[angle].next = Some(self);
            }
            return true;
        }
        let mut last = self;
        let mut flip_ambiguity = false;
        loop {
            debug_assert!(state[last].next == Some(next));
            // Skia's operands of `^` are unsequenced; clang evaluates `after` first, and `after`
            // can set the ambiguity it is combined with.
            let after = angle.after(state, last);
            if after ^ (angle.tangents_ambiguous(state) & flip_ambiguity) {
                state[last].next = Some(angle);
                state[angle].next = Some(next);
                break;
            }
            last = next;
            if last == self {
                if flip_ambiguity {
                    return false;
                }
                // We're in a loop. If a sort was ambiguous, flip it to end the loop.
                flip_ambiguity = true;
            }
            next = state[next].next.expect("an angle in a loop has a next");
        }
        true
    }

    /// `const` in Skia, which marks the span chased through its pointer.
    pub(crate) fn last_marked(self, state: &mut SkOpGlobalState) -> Option<SkOpSpanBase> {
        if let Some(last_marked) = state[self].last_marked {
            if last_marked.chased(state) {
                return None;
            }
            last_marked.set_chased(state, true);
        }
        state[self].last_marked
    }

    /// Skia's `continue` in the do-while goes to the `fNext` step.
    pub(crate) fn loop_contains(self, state: &SkOpGlobalState, angle: SkOpAngle) -> bool {
        if state[self].next.is_none() {
            return false;
        }
        let first = self;
        let mut lp = self;
        let t_segment = state[angle].start.segment(state);
        let t_start = state[angle].start.t(state);
        let t_end = state[angle].end.t(state);
        loop {
            let l_segment = state[lp].start.segment(state);
            if l_segment == t_segment {
                let l_start = state[lp].start.t(state);
                if l_start == t_end {
                    let l_end = state[lp].end.t(state);
                    if l_end == t_start {
                        return true;
                    }
                }
            }
            lp = state[lp].next.expect("an angle in a loop has a next");
            if lp == first {
                break;
            }
        }
        false
    }

    pub(crate) fn loop_count(self, state: &SkOpGlobalState) -> i32 {
        let mut count = 0;
        let first = self;
        let mut next = Some(self);
        loop {
            next = state[next.expect("checked by the loop")].next;
            count += 1;
            match next {
                Some(angle) if angle != first => {}
                _ => break,
            }
        }
        count
    }

    pub(crate) fn next(self, state: &SkOpGlobalState) -> Option<SkOpAngle> {
        state[self].next
    }

    // OPTIMIZE: if this shows up in a profile, add a previous pointer
    // as is, this should be rarely called
    pub(crate) fn previous(self, state: &SkOpGlobalState) -> SkOpAngle {
        let mut last = state[self].next.expect("an angle in a loop has a next");
        loop {
            let next = state[last].next.expect("an angle in a loop has a next");
            if next == self {
                return last;
            }
            last = next;
        }
    }

    pub(crate) fn segment(self, state: &SkOpGlobalState) -> SkOpSegment {
        state[self].start.segment(state)
    }

    pub(crate) fn set(self, state: &mut SkOpGlobalState, start: SkOpSpanBase, end: SkOpSpanBase) {
        state[self].start = start;
        state[self].end = end;
        state[self].computed_end = Some(end);
        debug_assert!(start != end);
        state[self].next = None;
        let data = &mut state[self];
        data.compute_sector = false;
        data.computed_sector = false;
        data.tangents_ambiguous = false;
        self.set_spans(state);
        self.set_sector(state);
    }

    pub(crate) fn set_last_marked(self, state: &mut SkOpGlobalState, marked: Option<SkOpSpanBase>) {
        state[self].last_marked = marked;
    }

    pub(crate) fn start(self, state: &SkOpGlobalState) -> SkOpSpanBase {
        state[self].start
    }

    pub(crate) fn starter(self, state: &SkOpGlobalState) -> SkOpSpan {
        state[self].start.starter(state, state[self].end)
    }

    pub(crate) fn tangents_ambiguous(self, state: &SkOpGlobalState) -> bool {
        state[self].tangents_ambiguous
    }

    pub(crate) fn unorderable(self, state: &SkOpGlobalState) -> bool {
        state[self].unorderable
    }

    // return true if lh < this < rh
    pub(crate) fn after(self, state: &mut SkOpGlobalState, test: SkOpAngle) -> bool {
        let lh = test;
        let rh = state[lh].next.expect("the angle after is in a loop");
        debug_assert!(lh != rh);
        state[self].part.curve = state[self].original_curve_part;
        // Adjust lh and rh to share the same origin (floating point error in intersections can mean
        // they aren't exactly the same).
        let origin = state[self].part.curve[0];
        state[lh].part.curve = state[lh].original_curve_part;
        state[lh].part.curve[0] = origin;
        state[rh].part.curve = state[rh].original_curve_part;
        state[rh].part.curve[0] = origin;

        if state[lh].compute_sector && !lh.compute_sector(state) {
            return true;
        }
        if state[self].compute_sector && !self.compute_sector(state) {
            return true;
        }
        if state[rh].compute_sector && !rh.compute_sector(state) {
            return true;
        }
        let ltr_overlap =
            ((state[lh].sector_mask | state[rh].sector_mask) & state[self].sector_mask) != 0;
        let lr_overlap = (state[lh].sector_mask & state[rh].sector_mask) != 0;
        let lr_order; // set to -1 if either order works
        if !lr_overlap {
            // no lh/rh sector overlap
            if !ltr_overlap {
                // no lh/this/rh sector overlap
                return (state[lh].sector_end > state[rh].sector_start)
                    ^ (state[self].sector_start > state[lh].sector_end)
                    ^ (state[self].sector_start > state[rh].sector_start);
            }
            let lr_gap =
                (i32::from(state[rh].sector_start) - i32::from(state[lh].sector_start) + 32) & 0x1f;
            /* A tiny change can move the start +/- 4. The order can only be determined if
              lr gap is not 12 to 20 or -12 to -20.
                  -31 ..-21      1
                  -20 ..-12     -1
                  -11 .. -1      0
                    0          shouldn't get here
                   11 ..  1      1
                   12 .. 20     -1
                   21 .. 31      0
            */
            lr_order = if lr_gap > 20 {
                0
            } else if lr_gap > 11 {
                -1
            } else {
                1
            };
        } else {
            lr_order = lh.orderable(state, rh);
            if !ltr_overlap && lr_order >= 0 {
                return lr_order == 0;
            }
        }
        let mut lt_order;
        debug_assert!(
            (state[lh].sector_mask & state[self].sector_mask) != 0
                || (state[rh].sector_mask & state[self].sector_mask) != 0
                || -1 == lr_order
        );
        if (state[lh].sector_mask & state[self].sector_mask) != 0 {
            lt_order = lh.orderable(state, self);
        } else {
            let lt_gap = (i32::from(state[self].sector_start) - i32::from(state[lh].sector_start)
                + 32)
                & 0x1f;
            lt_order = if lt_gap > 20 {
                0
            } else if lt_gap > 11 {
                -1
            } else {
                1
            };
        }
        let mut tr_order;
        if (state[rh].sector_mask & state[self].sector_mask) != 0 {
            tr_order = self.orderable(state, rh);
        } else {
            let tr_gap = (i32::from(state[rh].sector_start) - i32::from(state[self].sector_start)
                + 32)
                & 0x1f;
            tr_order = if tr_gap > 20 {
                0
            } else if tr_gap > 11 {
                -1
            } else {
                1
            };
        }
        self.alignment_same_side(state, lh, &mut lt_order);
        self.alignment_same_side(state, rh, &mut tr_order);
        if lr_order >= 0 && lt_order >= 0 && tr_order >= 0 {
            return (if lr_order != 0 {
                lt_order & tr_order
            } else {
                lt_order | tr_order
            }) != 0;
        }
        // There's not enough information to sort. Get the pairs of angles in opposite planes.
        // If an order is < 0, the pair is already in an opposite plane. Check the remaining pairs.
        // FIXME : once all variants are understood, rewrite this more simply
        if lt_order == 0 && lr_order == 0 {
            debug_assert!(tr_order < 0);
            // FIXME : once this is verified to work, remove one opposite angle call
            let lt_opposite = lh.opposite_planes(state, self);
            return lt_opposite;
        } else if lt_order == 1 && tr_order == 0 {
            debug_assert!(lr_order < 0);
            let tr_opposite = self.opposite_planes(state, rh);
            return tr_opposite;
        } else if lr_order == 1 && tr_order == 1 {
            debug_assert!(lt_order < 0);
            let lr_opposite = lh.opposite_planes(state, rh);
            return lr_opposite;
        }
        // If a pair couldn't be ordered, there's not enough information to determine the sort.
        // Refer to:  https://docs.google.com/drawings/d/1KV-8SJTedku9fj4K6fd1SB-8divuV_uivHVsSgwXICQ
        if state[self].unorderable || state[lh].unorderable || state[rh].unorderable {
            // limit to lines; should work with curves, but wait for a failing test to verify
            if !state[self].part.is_curve()
                && !state[lh].part.is_curve()
                && !state[rh].part.is_curve()
            {
                // see if original raw data is orderable
                // if two share a point, check if third has both points in same half plane
                let lt_share = i32::from(
                    state[lh].original_curve_part[0] == state[self].original_curve_part[0],
                );
                let lr_share =
                    i32::from(state[lh].original_curve_part[0] == state[rh].original_curve_part[0]);
                let tr_share = i32::from(
                    state[self].original_curve_part[0] == state[rh].original_curve_part[0],
                );
                // if only one pair are the same, the third point touches neither of the pair
                if lt_share + lr_share + tr_share == 1 {
                    if lr_share != 0 {
                        let lt_o_order = lh.lines_on_original_side(state, self);
                        let rt_o_order = rh.lines_on_original_side(state, self);
                        if (rt_o_order ^ lt_o_order) == 1 {
                            return lt_o_order != 0;
                        }
                    } else if tr_share != 0 {
                        let tl_o_order = self.lines_on_original_side(state, lh);
                        let rl_o_order = rh.lines_on_original_side(state, lh);
                        if (tl_o_order ^ rl_o_order) == 1 {
                            return rl_o_order != 0;
                        }
                    } else {
                        debug_assert!(lt_share != 0);
                        let tr_o_order = rh.lines_on_original_side(state, self);
                        let lr_o_order = lh.lines_on_original_side(state, rh);
                        // result must be 0 and 1 or 1 and 0 to be valid
                        if (lr_o_order ^ tr_o_order) == 1 {
                            return tr_o_order != 0;
                        }
                    }
                }
            }
        }
        if lr_order < 0 {
            if lt_order < 0 {
                return tr_order != 0;
            }
            return lt_order != 0;
        }
        lr_order == 0
    }

    // To sort the angles, all curves are translated to have the same starting point.
    // If the curve's control point in its original position is on one side of a compared line,
    // and translated is on the opposite side, reverse the previously computed order.
    pub(crate) fn alignment_same_side(
        self,
        state: &SkOpGlobalState,
        test: SkOpAngle,
        order: &mut i32,
    ) {
        if *order < 0 {
            return;
        }
        if state[self].part.is_curve() {
            // This should support all curve types, but only bug that requires this has lines
            // Turning on for curves causes existing tests to fail
            return;
        }
        if state[test].part.is_curve() {
            return;
        }
        let x_origin = state[test].part.curve.as_line()[0];
        let o_origin = state[test].original_curve_part.as_line()[0];
        if x_origin == o_origin {
            return;
        }
        let i_max = sk_path_ops_verb_to_points(self.segment(state).verb(state));
        let x_line = state[test].part.curve.as_line()[1] - x_origin;
        let o_line = state[test].original_curve_part.as_line()[1] - o_origin;
        for index in 1..=i_max {
            let test_pt = state[self].part.curve[index as usize];
            let x_cross = o_line.cross_check(&(test_pt - x_origin));
            let o_cross = x_line.cross_check(&(test_pt - o_origin));
            if o_cross * x_cross < 0.0 {
                *order ^= 1;
                break;
            }
        }
    }

    pub(crate) fn check_crosses_zero(self, state: &SkOpGlobalState) -> bool {
        let start = i32::from(std_min(state[self].sector_start, state[self].sector_end));
        let end = i32::from(std_max(state[self].sector_start, state[self].sector_end));
        end - start > 16
    }

    pub(crate) fn check_parallel(self, state: &mut SkOpGlobalState, rh: SkOpAngle) -> bool {
        let mut scratch = [SkDVector::default(); 2];
        let sweep = if state[self].part.is_ordered() {
            state[self].part.sweep[0]
        } else {
            scratch[0] = state[self].part.curve[1] - state[self].part.curve[0];
            scratch[0]
        };
        let tweep = if state[rh].part.is_ordered() {
            state[rh].part.sweep[0]
        } else {
            scratch[1] = state[rh].part.curve[1] - state[rh].part.curve[0];
            scratch[1]
        };
        let s0xt0 = sweep.cross_check(&tweep);
        if self.tangents_diverge(state, rh, s0xt0) {
            return s0xt0 < 0.0;
        }
        // compute the perpendicular to the endpoints and see where it intersects the opposite curve
        // if the intersections within the t range, do a cross check on those
        let mut inside = false;
        if !state[self].end.contains_span(state, state[rh].end) {
            if self.end_to_side(state, rh, &mut inside) {
                return inside;
            }
            if rh.end_to_side(state, self, &mut inside) {
                return !inside;
            }
        }
        if self.mid_to_side(state, rh, &mut inside) {
            return inside;
        }
        if rh.mid_to_side(state, self, &mut inside) {
            return !inside;
        }
        // compute the cross check from the mid T values (last resort)
        let m0 =
            self.segment(state).d_pt_at_t(state, self.mid_t(state)) - state[self].part.curve[0];
        let m1 = rh.segment(state).d_pt_at_t(state, rh.mid_t(state)) - state[rh].part.curve[0];
        let m0xm1 = m0.cross_check(&m1);
        if m0xm1 == 0.0 {
            state[self].unorderable = true;
            state[rh].unorderable = true;
            return true;
        }
        m0xm1 < 0.0
    }

    // the original angle is too short to get meaningful sector information
    // lengthen it until it is long enough to be meaningful or leave it unset if lengthening it
    // would cause it to intersect one of the adjacent angles
    pub(crate) fn compute_sector(self, state: &mut SkOpGlobalState) -> bool {
        if state[self].computed_sector {
            return !state[self].unorderable;
        }
        state[self].computed_sector = true;
        let f_start = state[self].start;
        let f_end = state[self].end;
        let step_up = f_start.t(state) < f_end.t(state);
        let mut check_end = Some(f_end);
        if f_end.r#final(state) && step_up {
            state[self].unorderable = true;
            return false;
        }
        // Skia's `goto recomputeSector` leaves the labeled loop with `checkEnd` set.
        'recompute_sector: while let Some(current_end) = check_end {
            // advance end
            let other = current_end.segment(state);
            let mut o_span: SkOpSpanBase = other.head(state).into();
            loop {
                let skip = o_span.segment(state) != self.segment(state)
                    || o_span == current_end
                    || !approximately_equal(o_span.t(state), current_end.t(state));
                if !skip {
                    break 'recompute_sector;
                }
                if o_span.r#final(state) {
                    break;
                }
                match o_span.up_cast().next(state) {
                    Some(next) => o_span = next,
                    None => break,
                }
            }
            check_end = if step_up {
                if !current_end.r#final(state) {
                    current_end.up_cast().next(state)
                } else {
                    None
                }
            } else {
                current_end.prev(state).map(SkOpSpanBase::from)
            };
        }
        // recomputeSector:
        let computed_end = if step_up {
            match check_end {
                Some(check_end) => check_end.prev(state).map(SkOpSpanBase::from),
                None => Some(f_end.segment(state).head(state).into()),
            }
        } else {
            match check_end {
                Some(check_end) => check_end.up_cast().next(state),
                None => Some(f_end.segment(state).tail(state)),
            }
        };
        if check_end == Some(f_end) || computed_end == Some(f_end) || computed_end == Some(f_start)
        {
            state[self].unorderable = true;
            return false;
        }
        // Skia dereferences it without a check.
        let computed_end = computed_end.expect("the computed end is a span");
        if step_up != (f_start.t(state) < computed_end.t(state)) {
            state[self].unorderable = true;
            return false;
        }
        let save_end = state[self].end;
        state[self].end = computed_end;
        state[self].computed_end = Some(computed_end);
        self.set_spans(state);
        self.set_sector(state);
        state[self].end = save_end;
        !state[self].unorderable
    }

    /// Not `const` in Skia: `tangentsDiverge` marks the angle's tangents ambiguous.
    pub(crate) fn convex_hull_overlaps(self, state: &mut SkOpGlobalState, rh: SkOpAngle) -> i32 {
        let sweep = state[self].part.sweep;
        let tweep = state[rh].part.sweep;
        let s0xs1 = sweep[0].cross_check(&sweep[1]);
        let s0xt0 = sweep[0].cross_check(&tweep[0]);
        let s1xt0 = sweep[1].cross_check(&tweep[0]);
        let mut t_between_s = if s0xs1 > 0.0 {
            s0xt0 > 0.0 && s1xt0 < 0.0
        } else {
            s0xt0 < 0.0 && s1xt0 > 0.0
        };
        let s0xt1 = sweep[0].cross_check(&tweep[1]);
        let s1xt1 = sweep[1].cross_check(&tweep[1]);
        t_between_s |= if s0xs1 > 0.0 {
            s0xt1 > 0.0 && s1xt1 < 0.0
        } else {
            s0xt1 < 0.0 && s1xt1 > 0.0
        };
        let t0xt1 = tweep[0].cross_check(&tweep[1]);
        if t_between_s {
            return -1;
        }
        if (s0xt0 == 0.0 && s1xt1 == 0.0) || (s1xt0 == 0.0 && s0xt1 == 0.0) {
            // s0 to s1 equals t0 to t1
            return -1;
        }
        let mut s_between_t = if t0xt1 > 0.0 {
            s0xt0 < 0.0 && s0xt1 > 0.0
        } else {
            s0xt0 > 0.0 && s0xt1 < 0.0
        };
        s_between_t |= if t0xt1 > 0.0 {
            s1xt0 < 0.0 && s1xt1 > 0.0
        } else {
            s1xt0 > 0.0 && s1xt1 < 0.0
        };
        if s_between_t {
            return -1;
        }
        // if all of the sweeps are in the same half plane, then the order of any pair is enough
        if s0xt0 >= 0.0 && s0xt1 >= 0.0 && s1xt0 >= 0.0 && s1xt1 >= 0.0 {
            return 0;
        }
        if s0xt0 <= 0.0 && s0xt1 <= 0.0 && s1xt0 <= 0.0 && s1xt1 <= 0.0 {
            return 1;
        }
        // if the outside sweeps are greater than 180 degress:
        // first assume the inital tangents are the ordering
        // if the midpoint direction matches the inital order, that is enough
        let m0 =
            self.segment(state).d_pt_at_t(state, self.mid_t(state)) - state[self].part.curve[0];
        let m1 = rh.segment(state).d_pt_at_t(state, rh.mid_t(state)) - state[rh].part.curve[0];
        let m0xm1 = m0.cross_check(&m1);
        if s0xt0 > 0.0 && m0xm1 > 0.0 {
            return 0;
        }
        if s0xt0 < 0.0 && m0xm1 < 0.0 {
            return 1;
        }
        if self.tangents_diverge(state, rh, s0xt0) {
            return i32::from(s0xt0 < 0.0);
        }
        i32::from(m0xm1 < 0.0)
    }

    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    pub(crate) fn end_to_side(
        self,
        state: &SkOpGlobalState,
        rh: SkOpAngle,
        inside: &mut bool,
    ) -> bool {
        let segment = self.segment(state);
        let verb = segment.verb(state);
        let mut ray_end = SkDLine::default();
        ray_end[0].set(&state[self].end.pt(state));
        ray_end[1] = ray_end[0];
        let slope_at_end = CURVE_D_SLOPE_AT_T[verb as usize].expect("a segment's verb has a curve")(
            &segment.pts(state),
            segment.weight(state),
            state[self].end.t(state),
        );
        ray_end[1].x += slope_at_end.y;
        ray_end[1].y -= slope_at_end.x;
        let mut i_end = SkIntersections::new();
        let opp_segment = rh.segment(state);
        let opp_verb = opp_segment.verb(state);
        CURVE_INTERSECT_RAY[opp_verb as usize].expect("a segment's verb has a curve")(
            &opp_segment.pts(state),
            opp_segment.weight(state),
            &ray_end,
            &mut i_end,
        );
        let mut end_dist = 0.0;
        let closest_end = i_end.closest_to(
            state[rh].start.t(state),
            state[rh].end.t(state),
            &ray_end[0],
            &mut end_dist,
        );
        if closest_end < 0 {
            return false;
        }
        if end_dist == 0.0 {
            return false;
        }
        let mut start = SkDPoint::default();
        start.set(&state[self].start.pt(state));
        // OPTIMIZATION: multiple times in the code we find the max scalar
        let mut min_x = f64::from(SK_SCALAR_INFINITY);
        let mut min_y = min_x;
        let mut max_x = f64::from(-SK_SCALAR_INFINITY);
        let mut max_y = max_x;
        let curve = &state[rh].part.curve;
        let opp_pts = sk_path_ops_verb_to_points(opp_verb);
        for idx2 in 0..=opp_pts as usize {
            min_x = std_min(min_x, curve[idx2].x);
            min_y = std_min(min_y, curve[idx2].y);
            max_x = std_max(max_x, curve[idx2].x);
            max_y = std_max(max_y, curve[idx2].y);
        }
        let max_width = std_max(max_x - min_x, max_y - min_y);
        end_dist = sk_ieee_double_divide(end_dist, max_width);
        if !(end_dist >= 5e-12) {
            // empirically found
            return false; // ! above catches NaN
        }
        let end_pt = &ray_end[0];
        let opp_pt = *i_end.pt(closest_end as usize);
        let v_left = *end_pt - start;
        let v_right = opp_pt - start;
        let dir = v_left.cross_no_normal_check(&v_right);
        if dir == 0.0 {
            return false;
        }
        *inside = dir < 0.0;
        true
    }

    pub(crate) fn ends_intersect(self, state: &mut SkOpGlobalState, rh: SkOpAngle) -> bool {
        let l_verb = self.segment(state).verb(state);
        let r_verb = rh.segment(state).verb(state);
        let l_pts = sk_path_ops_verb_to_points(l_verb);
        let r_pts = sk_path_ops_verb_to_points(r_verb);
        let rays = [
            SkDLine {
                pts: [
                    state[self].part.curve[0],
                    state[rh].part.curve[r_pts as usize],
                ],
            },
            SkDLine {
                pts: [
                    state[self].part.curve[0],
                    state[self].part.curve[l_pts as usize],
                ],
            },
        ];
        if state[self].end.contains_span(state, state[rh].end) {
            return self.check_parallel(state, rh);
        }
        let mut small_ts = [-1.0f64, -1.0];
        let mut limited = [false, false];
        for index in 0..2 {
            let c_verb = if index != 0 { r_verb } else { l_verb };
            // if the curve is a line, then the line and the ray intersect only at their crossing
            if c_verb == SkPathVerb::Line {
                continue;
            }
            let segment = if index != 0 {
                rh.segment(state)
            } else {
                self.segment(state)
            };
            let mut i = SkIntersections::new();
            CURVE_INTERSECT_RAY[c_verb as usize].expect("a segment's verb has a curve")(
                &segment.pts(state),
                segment.weight(state),
                &rays[index],
                &mut i,
            );
            let t_start = if index != 0 {
                state[rh].start.t(state)
            } else {
                state[self].start.t(state)
            };
            let rh_computed_end = state[rh].computed_end.expect("set with the end");
            let computed_end = state[self].computed_end.expect("set with the end");
            let t_end = if index != 0 {
                rh_computed_end.t(state)
            } else {
                computed_end.t(state)
            };
            let test_ascends = t_start
                < (if index != 0 {
                    rh_computed_end.t(state)
                } else {
                    computed_end.t(state)
                });
            let mut t = if test_ascends { 0.0 } else { 1.0 };
            for &test_t in i[0].iter().take(i.used() as usize) {
                if !approximately_between_orderable(t_start, test_t, t_end) {
                    continue;
                }
                if approximately_equal_orderable(t_start, test_t) {
                    continue;
                }
                t = if test_ascends {
                    std_max(t, test_t)
                } else {
                    std_min(t, test_t)
                };
                small_ts[index] = t;
                limited[index] = approximately_equal_orderable(t, t_end);
            }
        }
        let mut s_ray_longer = false;
        let mut s_cept = SkDVector { x: 0.0, y: 0.0 };
        let mut s_cept_t = -1.0;
        let mut s_index = -1;
        let mut use_intersect = false;
        for index in 0..2usize {
            if small_ts[index] < 0.0 {
                continue;
            }
            let segment = if index != 0 {
                rh.segment(state)
            } else {
                self.segment(state)
            };
            let d_pt = segment.d_pt_at_t(state, small_ts[index]);
            let cept = d_pt - rays[index][0];
            // If this point is on the curve, it should have been detected earlier by ordinary
            // curve intersection. This may be hard to determine in general, but for lines,
            // the point could be close to or equal to its end, but shouldn't be near the start.
            if (if index != 0 { l_pts } else { r_pts }) == 1 {
                let total = rays[index][1] - rays[index][0];
                if cept.length_squared() * 2.0 < total.length_squared() {
                    continue;
                }
            }
            let end = rays[index][1] - rays[index][0];
            if cept.x * end.x < 0.0 || cept.y * end.y < 0.0 {
                continue;
            }
            let ray_dist = cept.length();
            let end_dist = end.length();
            let ray_longer = ray_dist > end_dist;
            if limited[0] && limited[1] && ray_longer {
                use_intersect = true;
                s_ray_longer = ray_longer;
                s_cept = cept;
                s_cept_t = small_ts[index];
                s_index = index as i32;
                break;
            }
            let mut delta = (ray_dist - end_dist).abs();
            let mut min_x = f64::from(SK_SCALAR_INFINITY);
            let mut min_y = min_x;
            let mut max_x = f64::from(-SK_SCALAR_INFINITY);
            let mut max_y = max_x;
            let curve = if index != 0 {
                state[rh].part.curve
            } else {
                state[self].part.curve
            };
            let pt_count = if index != 0 { r_pts } else { l_pts };
            for idx2 in 0..=pt_count as usize {
                min_x = std_min(min_x, curve[idx2].x);
                min_y = std_min(min_y, curve[idx2].y);
                max_x = std_max(max_x, curve[idx2].x);
                max_y = std_max(max_y, curve[idx2].y);
            }
            let max_width = std_max(max_x - min_x, max_y - min_y);
            delta = sk_ieee_double_divide(delta, max_width);
            // FIXME: move these magic numbers
            // This fixes skbug.com/40039654
            // Larger changes (like changing the constant in the next block) cause other
            // tests to fail as documented in the bug.
            // This could probably become a more general test: e.g., if translating the
            // curve causes the cross product of any control point or end point to change
            // sign with regard to the opposite curve's hull, treat the curves as parallel.

            // Moreso, this points to the general fragility of this approach of assigning
            // winding by sorting the angles of curves sharing a common point, as mentioned
            // in the bug.
            if delta < 4e-3
                && delta > 1e-3
                && !use_intersect
                && state[self].part.is_curve()
                && state[rh].part.is_curve()
                && state[self].original_curve_part[0] != state[self].part.curve.as_line()[0]
            {
                // see if original curve is on one side of hull; translated is on the other
                let origin = state[rh].original_curve_part[0];
                let count = sk_path_ops_verb_to_points(rh.segment(state).verb(state));
                let line = state[rh].original_curve_part[count as usize] - origin;
                let original_side = rh.line_on_one_side_origin(state, &origin, &line, self, true);
                if original_side >= 0 {
                    let translated_side =
                        rh.line_on_one_side_origin(state, &origin, &line, self, false);
                    if original_side != translated_side {
                        continue;
                    }
                }
            }
            if delta > 1e-3 && {
                use_intersect ^= true;
                use_intersect
            } {
                s_ray_longer = ray_longer;
                s_cept = cept;
                s_cept_t = small_ts[index];
                s_index = index as i32;
            }
        }
        if use_intersect {
            let curve = if s_index != 0 {
                state[rh].part.curve
            } else {
                state[self].part.curve
            };
            let segment = if s_index != 0 {
                rh.segment(state)
            } else {
                self.segment(state)
            };
            let t_start = if s_index != 0 {
                state[rh].start.t(state)
            } else {
                state[self].start.t(state)
            };
            let mid = segment.d_pt_at_t(state, t_start + (s_cept_t - t_start) / 2.0) - curve[0];
            let sept_dir = mid.cross_check(&s_cept);
            if sept_dir == 0.0 {
                return self.check_parallel(state, rh);
            }
            s_ray_longer ^ (s_index == 0) ^ (sept_dir < 0.0)
        } else {
            self.check_parallel(state, rh)
        }
    }

    /*      y<0 y==0 y>0  x<0 x==0 x>0 xy<0 xy==0 xy>0
        0    x                      x               x
        1    x                      x          x
        2    x                      x    x
        3    x                  x        x
        4    x             x             x
        5    x             x                   x
        6    x             x                        x
        7         x        x                        x
        8             x    x                        x
        9             x    x                   x
        10            x    x             x
        11            x         x        x
        12            x             x    x
        13            x             x          x
        14            x             x               x
        15        x                 x               x
    */
    pub(crate) fn find_sector(
        self,
        state: &SkOpGlobalState,
        verb: SkPathVerb,
        x: f64,
        y: f64,
    ) -> i32 {
        let _ = state;
        let abs_x = x.abs();
        let abs_y = y.abs();
        let xy = if SkPathVerb::Line == verb || !almost_equal_ulps(abs_x, abs_y) {
            abs_x - abs_y
        } else {
            0.0
        };
        // If there are four quadrants and eight octants, and since the Latin for sixteen is sedecim,
        // one could coin the term sedecimant for a space divided into 16 sections.
        // http://english.stackexchange.com/questions/133688/word-for-something-partitioned-into-16-parts
        const SEDECIMANT: [[[i32; 3]; 3]; 3] = [
            //       y<0           y==0           y>0
            //   x<0 x==0 x>0  x<0 x==0 x>0  x<0 x==0 x>0
            [[4, 3, 2], [7, -1, 15], [10, 11, 12]], // abs(x) <  abs(y)
            [[5, -1, 1], [-1, -1, -1], [9, -1, 13]], // abs(x) == abs(y)
            [[6, 3, 0], [7, -1, 15], [8, 11, 14]],  // abs(x) >  abs(y)
        ];
        SEDECIMANT[usize::from(xy >= 0.0) + usize::from(xy > 0.0)]
            [usize::from(y >= 0.0) + usize::from(y > 0.0)]
            [usize::from(x >= 0.0) + usize::from(x > 0.0)]
            * 2
            + 1
    }

    /// `lineOnOneSide(const SkDPoint& origin, const SkDVector& line, const SkOpAngle* test,
    /// bool useOriginal) const`.
    pub(crate) fn line_on_one_side_origin(
        self,
        state: &SkOpGlobalState,
        origin: &SkDPoint,
        line: &SkDVector,
        test: SkOpAngle,
        use_original: bool,
    ) -> i32 {
        // Skia leaves `crosses` uninitialized; every caller's test is a curve, which sets two.
        let mut crosses = [0.0f64; 3];
        let test_verb = test.segment(state).verb(state);
        let i_max = sk_path_ops_verb_to_points(test_verb);
        let test_curve = if use_original {
            &state[test].original_curve_part
        } else {
            &state[test].part.curve
        };
        for index in 1..=i_max as usize {
            let xy1 = line.x * (test_curve[index].y - origin.y);
            let xy2 = line.y * (test_curve[index].x - origin.x);
            crosses[index - 1] = if almost_bequal_ulps(xy1, xy2) {
                0.0
            } else {
                xy1 - xy2
            };
        }
        if crosses[0] * crosses[1] < 0.0 {
            return -1;
        }
        if SkPathVerb::Cubic == test_verb
            && (crosses[0] * crosses[2] < 0.0 || crosses[1] * crosses[2] < 0.0)
        {
            return -1;
        }
        if crosses[0] != 0.0 {
            return i32::from(crosses[0] < 0.0);
        }
        if crosses[1] != 0.0 {
            return i32::from(crosses[1] < 0.0);
        }
        if SkPathVerb::Cubic == test_verb && crosses[2] != 0.0 {
            return i32::from(crosses[2] < 0.0);
        }
        -2
    }

    // given a line, see if the opposite curve's convex hull is all on one side
    // returns -1=not on one side    0=this CW of test   1=this CCW of test
    /// `lineOnOneSide(const SkOpAngle* test, bool useOriginal)`.
    pub(crate) fn line_on_one_side(
        self,
        state: &mut SkOpGlobalState,
        test: SkOpAngle,
        use_original: bool,
    ) -> i32 {
        debug_assert!(!state[self].part.is_curve());
        debug_assert!(state[test].part.is_curve());
        let origin = state[self].part.curve[0];
        let line = state[self].part.curve[1] - origin;
        let mut result = self.line_on_one_side_origin(state, &origin, &line, test, use_original);
        if -2 == result {
            state[self].unorderable = true;
            result = -1;
        }
        result
    }

    // experiment works only with lines for now
    pub(crate) fn lines_on_original_side(
        self,
        state: &mut SkOpGlobalState,
        test: SkOpAngle,
    ) -> i32 {
        debug_assert!(!state[self].part.is_curve());
        debug_assert!(!state[test].part.is_curve());
        let origin = state[self].original_curve_part[0];
        let line = state[self].original_curve_part[1] - origin;
        let mut dots = [0.0f64; 2];
        let mut crosses = [0.0f64; 2];
        let test_curve = state[test].original_curve_part;
        for index in 0..2 {
            let test_line = test_curve[index] - origin;
            let xy1 = line.x * test_line.y;
            let xy2 = line.y * test_line.x;
            dots[index] = line.x * test_line.x + line.y * test_line.y;
            crosses[index] = if almost_bequal_ulps(xy1, xy2) {
                0.0
            } else {
                xy1 - xy2
            };
        }
        if crosses[0] * crosses[1] < 0.0 {
            return -1;
        }
        if crosses[0] != 0.0 {
            return i32::from(crosses[0] < 0.0);
        }
        if crosses[1] != 0.0 {
            return i32::from(crosses[1] < 0.0);
        }
        if (dots[0] == 0.0 && dots[1] < 0.0) || (dots[0] < 0.0 && dots[1] == 0.0) {
            return 2; // 180 degrees apart
        }
        state[self].unorderable = true;
        -1
    }

    pub(crate) fn merge(self, state: &mut SkOpGlobalState, angle: SkOpAngle) -> bool {
        debug_assert!(state[self].next.is_some());
        debug_assert!(state[angle].next.is_some());
        let mut working = angle;
        loop {
            if self == working {
                return false;
            }
            working = state[working].next.expect("an angle in a loop has a next");
            if working == angle {
                break;
            }
        }
        loop {
            let next = state[working].next.expect("an angle in a loop has a next");
            state[working].next = None;
            self.insert(state, working);
            working = next;
            if working == angle {
                break;
            }
        }
        // it's likely that a pair of the angles are unorderable
        true
    }

    pub(crate) fn mid_t(self, state: &SkOpGlobalState) -> f64 {
        (state[self].start.t(state) + state[self].end.t(state)) / 2.0
    }

    pub(crate) fn mid_to_side(
        self,
        state: &SkOpGlobalState,
        rh: SkOpAngle,
        inside: &mut bool,
    ) -> bool {
        let segment = self.segment(state);
        let verb = segment.verb(state);
        let start_pt = state[self].start.pt(state);
        let end_pt = state[self].end.pt(state);
        let mut d_start_pt = SkDPoint::default();
        d_start_pt.set(&start_pt);
        let mut ray_mid = SkDLine::default();
        ray_mid[0].x = f64::from((start_pt.x + end_pt.x) / 2.0);
        ray_mid[0].y = f64::from((start_pt.y + end_pt.y) / 2.0);
        ray_mid[1].x = ray_mid[0].x + f64::from(end_pt.y - start_pt.y);
        ray_mid[1].y = ray_mid[0].y - f64::from(end_pt.x - start_pt.x);
        let mut i_mid = SkIntersections::new();
        CURVE_INTERSECT_RAY[verb as usize].expect("a segment's verb has a curve")(
            &segment.pts(state),
            segment.weight(state),
            &ray_mid,
            &mut i_mid,
        );
        let i_outside = i_mid.most_outside(
            state[self].start.t(state),
            state[self].end.t(state),
            &d_start_pt,
        );
        if i_outside < 0 {
            return false;
        }
        let opp_segment = rh.segment(state);
        let opp_verb = opp_segment.verb(state);
        let mut opp_mid = SkIntersections::new();
        CURVE_INTERSECT_RAY[opp_verb as usize].expect("a segment's verb has a curve")(
            &opp_segment.pts(state),
            opp_segment.weight(state),
            &ray_mid,
            &mut opp_mid,
        );
        let opp_outside = opp_mid.most_outside(
            state[rh].start.t(state),
            state[rh].end.t(state),
            &d_start_pt,
        );
        if opp_outside < 0 {
            return false;
        }
        let i_side = *i_mid.pt(i_outside as usize) - d_start_pt;
        let opp_side = *opp_mid.pt(opp_outside as usize) - d_start_pt;
        let dir = i_side.cross_check(&opp_side);
        if dir == 0.0 {
            return false;
        }
        *inside = dir < 0.0;
        true
    }

    pub(crate) fn opposite_planes(self, state: &SkOpGlobalState, rh: SkOpAngle) -> bool {
        let start_span =
            sk_t_abs(i32::from(state[rh].sector_start) - i32::from(state[self].sector_start));
        start_span >= 8
    }

    /// false == this < rh ; true == this > rh; -1 == unorderable
    pub(crate) fn orderable(self, state: &mut SkOpGlobalState, rh: SkOpAngle) -> i32 {
        // Skia's `goto unorderable` leaves the labeled block.
        'unorderable: {
            if !state[self].part.is_curve() {
                if !state[rh].part.is_curve() {
                    let left_x = state[self].tangent_half.dx();
                    let left_y = state[self].tangent_half.dy();
                    let right_x = state[rh].tangent_half.dx();
                    let right_y = state[rh].tangent_half.dy();
                    let x_ry = left_x * right_y;
                    let rx_y = right_x * left_y;
                    if x_ry == rx_y {
                        if left_x * right_x < 0.0 || left_y * right_y < 0.0 {
                            return 1; // exactly 180 degrees apart
                        }
                        break 'unorderable;
                    }
                    debug_assert!(x_ry != rx_y); // indicates an undetected coincidence -- worth finding earlier
                    return if x_ry < rx_y { 1 } else { 0 };
                }
                let result = self.line_on_one_side(state, rh, false);
                if result >= 0 {
                    return result;
                }
                if state[self].unorderable || approximately_zero(state[rh].side) {
                    break 'unorderable;
                }
            } else if !state[rh].part.is_curve() {
                let result = rh.line_on_one_side(state, self, false);
                if result >= 0 {
                    return if result != 0 { 0 } else { 1 };
                }
                if state[rh].unorderable || approximately_zero(state[self].side) {
                    break 'unorderable;
                }
            } else {
                let result = self.convex_hull_overlaps(state, rh);
                if result >= 0 {
                    return result;
                }
            }
            return if self.ends_intersect(state, rh) { 1 } else { 0 };
        }
        // unorderable:
        state[self].unorderable = true;
        state[rh].unorderable = true;
        -1
    }

    pub(crate) fn set_sector(self, state: &mut SkOpGlobalState) {
        // Skia's `if (!fStart)` is left out: `set` is always given a span.
        let segment = state[self].start.segment(state);
        let verb = segment.verb(state);
        let sweep = state[self].part.sweep;
        state[self].sector_start = self.find_sector(state, verb, sweep[0].x, sweep[0].y) as i8;
        // Skia's `goto deferTilLater` jumps into the second check's block.
        let defer_til_later = if state[self].sector_start < 0 {
            true
        } else {
            if !state[self].part.is_curve() {
                // if it's a line or line-like, note that both sectors are the same
                debug_assert!(state[self].sector_start >= 0);
                state[self].sector_end = state[self].sector_start;
                state[self].sector_mask = 1 << state[self].sector_start;
                return;
            }
            debug_assert!(SkPathVerb::Line != verb);
            state[self].sector_end = self.find_sector(state, verb, sweep[1].x, sweep[1].y) as i8;
            state[self].sector_end < 0
        };
        if defer_til_later {
            // deferTilLater:
            state[self].sector_start = -1;
            state[self].sector_end = -1;
            state[self].sector_mask = 0;
            state[self].compute_sector = true; // can't determine sector until segment length can be found
            return;
        }
        let data = &mut state[self];
        if data.sector_end == data.sector_start && (data.sector_start & 3) != 3 {
            // if the sector has no span, it can't be an exact angle
            data.sector_mask = 1 << data.sector_start;
            return;
        }
        let mut crosses_zero = self.check_crosses_zero(state);
        let data = &mut state[self];
        let mut start = i32::from(std_min(data.sector_start, data.sector_end));
        let curve_bends_ccw = (i32::from(data.sector_start) == start) ^ crosses_zero;
        // bump the start and end of the sector span if they are on exact compass points
        if (data.sector_start & 3) == 3 {
            data.sector_start = ((i32::from(data.sector_start)
                + if curve_bends_ccw { 1 } else { 31 })
                & 0x1f) as i8;
        }
        if (data.sector_end & 3) == 3 {
            data.sector_end =
                ((i32::from(data.sector_end) + if curve_bends_ccw { 31 } else { 1 }) & 0x1f) as i8;
        }
        crosses_zero = self.check_crosses_zero(state);
        let data = &mut state[self];
        start = i32::from(std_min(data.sector_start, data.sector_end));
        let end = i32::from(std_max(data.sector_start, data.sector_end));
        if !crosses_zero {
            data.sector_mask = (u32::MAX >> (31 - end + start) << start) as i32;
        } else {
            data.sector_mask = (u32::MAX >> (31 - start) | (u32::MAX << end)) as i32;
        }
    }

    pub(crate) fn set_spans(self, state: &mut SkOpGlobalState) {
        state[self].unorderable = false;
        state[self].last_marked = None;
        // Skia's `if (!fStart)` is left out: `set` is always given a span.
        let f_start = state[self].start;
        let f_end = state[self].end;
        let segment = f_start.segment(state);
        let pts = segment.pts(state);
        let mut curve = state[self].part.curve;
        segment.sub_divide(state, f_start, f_end, &mut curve); //  set at least the line part if not more
        state[self].part.curve = curve;
        state[self].original_curve_part = curve;
        let verb = segment.verb(state);
        state[self].part.set_curve_hull_sweep(verb);
        if SkPathVerb::Line != verb && !state[self].part.is_curve() {
            let mut line_half = SkDLine::default();
            let data = &mut state[self];
            data.part.curve[1] = data.part.curve[sk_path_ops_verb_to_points(verb) as usize];
            data.original_curve_part[1] = data.part.curve[1];
            line_half[0].set(&data.part.curve[0].as_sk_point());
            line_half[1].set(&data.part.curve[1].as_sk_point());
            data.tangent_half.line_end_points(&line_half);
            data.side = 0.0;
        }
        match verb {
            SkPathVerb::Line => {
                debug_assert!(f_start != f_end);
                let c_p1 = pts[usize::from(f_start.t(state) < f_end.t(state))];
                let mut line_half = SkDLine::default();
                line_half[0].set(&f_start.pt(state));
                line_half[1].set(&c_p1);
                let data = &mut state[self];
                data.tangent_half.line_end_points(&line_half);
                data.side = 0.0;
            }
            SkPathVerb::Quad | SkPathVerb::Conic => {
                let mut tangent_part = SkLineParameters::default();
                let data = &mut state[self];
                let _ = tangent_part.quad_end_points(&data.part.curve.as_quad());
                data.side = -tangent_part.point_distance(&data.part.curve[2]); // not normalized -- compare sign only
            }
            SkPathVerb::Cubic => {
                let mut tangent_part = SkLineParameters::default();
                let part_cubic = state[self].part.curve.as_cubic();
                let _ = tangent_part.cubic_part(&part_cubic);
                state[self].side = -tangent_part.point_distance(&state[self].part.curve[3]);
                let mut test_ts = [0.0f64; 4];
                // OPTIMIZATION: keep inflections precomputed with cubic segment?
                let mut test_count = SkDCubic::find_inflections_pts(&pts, &mut test_ts) as usize;
                let start_t = f_start.t(state);
                let end_t = f_end.t(state);
                let limit_t = end_t;
                for test_t in test_ts.iter_mut().take(test_count) {
                    if !between(start_t, *test_t, limit_t) {
                        *test_t = -1.0;
                    }
                }
                test_ts[test_count] = start_t;
                test_count += 1;
                test_ts[test_count] = end_t;
                test_count += 1;
                sk_t_q_sort(&mut test_ts[..test_count]);
                let mut best_side = 0.0f64;
                let test_cases = (test_count << 1) - 1;
                let mut index = 0;
                while test_ts[index] < 0.0 {
                    index += 1;
                }
                index <<= 1;
                let weight = segment.weight(state);
                while index < test_cases {
                    let test_index = index >> 1;
                    let mut test_t = test_ts[test_index];
                    if (index & 1) != 0 {
                        test_t = (test_t + test_ts[test_index + 1]) / 2.0;
                    }
                    // OPTIMIZE: could avoid call for t == startT, endT
                    let pt = CURVE_D_POINT_AT_T[SkPathVerb::Cubic as usize]
                        .expect("the cubic entry is dcubic_xy_at_t")(
                        &pts, weight, test_t
                    );
                    let mut test_part = SkLineParameters::default();
                    let _ = test_part.cubic_end_points(&part_cubic);
                    let test_side = test_part.point_distance(&pt);
                    if best_side.abs() < test_side.abs() {
                        best_side = test_side;
                    }
                    index += 1;
                }
                state[self].side = -best_side; // compare sign only
            }
            _ => debug_assert!(false),
        }
    }

    pub(crate) fn tangents_diverge(
        self,
        state: &mut SkOpGlobalState,
        rh: SkOpAngle,
        s0xt0: f64,
    ) -> bool {
        if s0xt0 == 0.0 {
            return false;
        }
        // if the ctrl tangents are not nearly parallel, use them
        // solve for opposite direction displacement scale factor == m
        // initial dir = v1.cross(v2) == v2.x * v1.y - v2.y * v1.x
        // displacement of q1[1] : dq1 = { -m * v1.y, m * v1.x } + q1[1]
        // straight angle when : v2.x * (dq1.y - q1[0].y) == v2.y * (dq1.x - q1[0].x)
        //                       v2.x * (m * v1.x + v1.y) == v2.y * (-m * v1.y + v1.x)
        // - m * (v2.x * v1.x + v2.y * v1.y) == v2.x * v1.y - v2.y * v1.x
        // m = (v2.y * v1.x - v2.x * v1.y) / (v2.x * v1.x + v2.y * v1.y)
        // m = v1.cross(v2) / v1.dot(v2)
        let sweep = state[self].part.sweep;
        let tweep = state[rh].part.sweep;
        let s0dt0 = sweep[0].dot(&tweep[0]);
        if s0dt0 == 0.0 {
            return true;
        }
        debug_assert!(s0dt0 != 0.0);
        let m = s0xt0 / s0dt0;
        let s_dist = sweep[0].length() * m;
        let t_dist = tweep[0].length() * m;
        let use_s = s_dist.abs() < t_dist.abs();
        let m_factor = (if use_s {
            self.dist_end_ratio(state, s_dist)
        } else {
            rh.dist_end_ratio(state, t_dist)
        })
        .abs();
        #[allow(clippy::manual_range_contains)]
        let tangents_ambiguous = m_factor >= 50.0 && m_factor < 200.0;
        state[self].tangents_ambiguous = tangents_ambiguous;
        m_factor < 50.0 // empirically found limit
    }
}
