//! Port of Skia's src/pathops/SkOpSegment.{h,cpp}: one line or curve of a contour, its spans,
//! and the walk that marks which of its pieces belong to the result.
//!
//! `SkOpSegment` is a handle into `SkOpGlobalState`. Skia embeds a segment's head span and
//! tail span base; here they are allocated with the segment and recorded by handle. Skia
//! points `fPts` at points the contour builder copied into the arena and never writes them
//! again; here the segment holds those points itself (unused entries past the verb's count
//! stay zero).

use std::ops::{Index, IndexMut};

use crate::core::float_bits::{
    sk_t_abs, std_max, FLT_EPSILON, SK_MAX_S32, SK_MIN_S32, SK_NAN_32, SK_SCALAR_MAX,
};
use crate::core::path::SkPathVerb;
use crate::core::point::{SkPoint, SkPointPriv};
use crate::intersections::SkIntersections;
use crate::op_angle::{IncludeType, SkOpAngle};
use crate::op_contour::SkOpContour;
use crate::op_span::{Collapsed, SkOpPtT, SkOpSpan, SkOpSpanBase};
use crate::path_ops_bounds::SkPathOpsBounds;
use crate::path_ops_conic::SkDConic;
use crate::path_ops_cubic::SkDCubic;
use crate::path_ops_curve::{
    SkDCurve, SkDCurveSweep, CURVE_DD_POINT_AT_T, CURVE_DD_SLOPE_AT_T, CURVE_D_INTERSECT_RAY,
    CURVE_D_POINT_AT_T, CURVE_D_SLOPE_AT_T, CURVE_INTERSECT_RAY, CURVE_POINT_AT_T,
};
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::{SkDPoint, SkDVector};
use crate::path_ops_quad::SkDQuad;
use crate::path_ops_types::{
    arena_handle, between, precisely_equal, roughly_equal, sk_path_ops_verb_to_points, zero_or_one,
    SkOpGlobalState, SkPathOp,
};
use crate::path_writer::SkPathWriter;

arena_handle! {
    /// Stands for `SkOpSegment*`.
    SkOpSegment
}

/// `SkOpSegment`'s fields.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkOpSegmentData {
    /// the head span always has its t set to zero
    pub(crate) head: SkOpSpan,
    /// the tail span always has its t set to one
    pub(crate) tail: SkOpSpanBase,
    pub(crate) contour: SkOpContour,
    /// forward-only linked list used by contour to walk the segments
    pub(crate) next: Option<SkOpSegment>,
    /// Skia's `SkPoint* fPts`, pointing at points the contour builder owns: copied here.
    pub(crate) pts: [SkPoint; 4],
    /// tight bounds
    pub(crate) bounds: SkPathOpsBounds,
    pub(crate) weight: f32,
    /// number of spans (one for a non-intersecting segment)
    pub(crate) count: i32,
    /// number of processed spans (zero initially)
    pub(crate) done_count: i32,
    pub(crate) verb: SkPathVerb,
    /// used by missing coincidence check
    pub(crate) visited: bool,
}

impl Index<SkOpSegment> for SkOpGlobalState {
    type Output = SkOpSegmentData;

    fn index(&self, segment: SkOpSegment) -> &SkOpSegmentData {
        &self.segments[segment.0 as usize]
    }
}

impl IndexMut<SkOpSegment> for SkOpGlobalState {
    fn index_mut(&mut self, segment: SkOpSegment) -> &mut SkOpSegmentData {
        &mut self.segments[segment.0 as usize]
    }
}

/*
After computing raw intersections, post process all segments to:
- find small collections of points that can be collapsed to a single point
- find missing intersections to resolve differences caused by different algorithms

Consider segments containing tiny or small intervals. Consider coincident segments
because coincidence finds intersections through distance measurement that non-coincident
intersection tests cannot.
 */

// false discards the edge, true keeps it
const K_UNARY_ACTIVE_EDGE: [[bool; 2]; 2] = [
    //  from=0  from=1
    //  to=0,1  to=0,1
    [false, true],
    [true, false],
];

// indexed by [op][miFrom][miTo][suFrom][suTo], for the ops up to kXOR_SkPathOp
#[rustfmt::skip]
const K_ACTIVE_EDGE: [[[[[bool; 2]; 2]; 2]; 2]; SkPathOp::Xor as usize + 1] = [
    //                 miFrom=0                              miFrom=1
    //         miTo=0             miTo=1             miTo=0             miTo=1
    //     suFrom=0    1      suFrom=0    1      suFrom=0    1      suFrom=0    1
    //   suTo=0,1 suTo=0,1  suTo=0,1 suTo=0,1  suTo=0,1 suTo=0,1  suTo=0,1 suTo=0,1
    [[[[false, false], [false, false]], [[true, false], [true, false]]], [[[true, true], [false, false]], [[false, true], [true, false]]]],  // mi - su
    [[[[false, false], [false, false]], [[false, true], [false, true]]], [[[false, false], [true, true]], [[false, true], [true, false]]]],  // mi & su
    [[[[false, true], [true, false]], [[true, true], [false, false]]], [[[true, false], [true, false]], [[false, false], [false, false]]]],  // mi | su
    [[[[false, true], [true, false]], [[true, false], [false, true]]], [[[true, false], [false, true]], [[false, true], [true, false]]]],  // mi ^ su
];

fn set_last(
    last: Option<&mut Option<SkOpSpanBase>>,
    end_span: SkOpSpanBase,
) -> Option<SkOpSegment> {
    if let Some(last) = last {
        *last = Some(end_span);
    }
    None
}

impl SkOpSegment {
    pub(crate) fn active_angle(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        start_ptr: &mut Option<SkOpSpanBase>,
        end_ptr: &mut Option<SkOpSpanBase>,
        done: &mut bool,
    ) -> Option<SkOpAngle> {
        if let Some(result) = self.active_angle_inner(state, start, start_ptr, end_ptr, done) {
            return Some(result);
        }
        if let Some(result) = self.active_angle_other(state, start, start_ptr, end_ptr, done) {
            return Some(result);
        }
        None
    }

    pub(crate) fn active_angle_inner(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        start_ptr: &mut Option<SkOpSpanBase>,
        end_ptr: &mut Option<SkOpSpanBase>,
        done: &mut bool,
    ) -> Option<SkOpAngle> {
        let up_span = start.up_castable(state);
        if let Some(up_span) = up_span {
            if up_span.wind_value(state) != 0 || up_span.opp_value(state) != 0 {
                let next = up_span.next(state);
                if end_ptr.is_none() {
                    *start_ptr = Some(start);
                    *end_ptr = next;
                }
                if !up_span.done(state) {
                    if up_span.wind_sum(state) != SK_MIN_S32 {
                        return self.span_to_angle(
                            state,
                            start,
                            next.expect("a span has a next span"),
                        );
                    }
                    *done = false;
                }
            } else {
                debug_assert!(up_span.done(state));
            }
        }
        let down_span = start.prev(state);
        // edge leading into junction
        if let Some(down_span) = down_span {
            if down_span.wind_value(state) != 0 || down_span.opp_value(state) != 0 {
                if end_ptr.is_none() {
                    *start_ptr = Some(start);
                    *end_ptr = Some(down_span.0);
                }
                if !down_span.done(state) {
                    if down_span.wind_sum(state) != SK_MIN_S32 {
                        return self.span_to_angle(state, start, down_span.0);
                    }
                    *done = false;
                }
            } else {
                debug_assert!(down_span.done(state));
            }
        }
        None
    }

    pub(crate) fn active_angle_other(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        start_ptr: &mut Option<SkOpSpanBase>,
        end_ptr: &mut Option<SkOpSpanBase>,
        done: &mut bool,
    ) -> Option<SkOpAngle> {
        let o_pt_t = start.pt_t().next(state);
        let other = o_pt_t.segment(state);
        let o_span = o_pt_t.span(state);
        other.active_angle_inner(state, o_span, start_ptr, end_ptr, done)
    }

    /// `activeOp(SkOpSpanBase* start, SkOpSpanBase* end, int xorMiMask, int xorSuMask,
    /// SkPathOp op)`.
    pub(crate) fn active_op(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        xor_mi_mask: i32,
        xor_su_mask: i32,
        op: SkPathOp,
    ) -> bool {
        let mut sum_mi_winding = self.update_winding(state, end, start);
        let mut sum_su_winding = self.update_opp_winding(state, end, start);
        if self.operand(state) {
            std::mem::swap(&mut sum_mi_winding, &mut sum_su_winding);
        }
        self.active_op_sum(
            state,
            xor_mi_mask,
            xor_su_mask,
            start,
            end,
            op,
            &mut sum_mi_winding,
            &mut sum_su_winding,
        )
    }

    /// `activeOp(int xorMiMask, int xorSuMask, SkOpSpanBase* start, SkOpSpanBase* end,
    /// SkPathOp op, int* sumMiWinding, int* sumSuWinding)`.
    pub(crate) fn active_op_sum(
        self,
        state: &mut SkOpGlobalState,
        xor_mi_mask: i32,
        xor_su_mask: i32,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        op: SkPathOp,
        sum_mi_winding: &mut i32,
        sum_su_winding: &mut i32,
    ) -> bool {
        let mut max_winding = 0;
        let mut sum_winding = 0;
        let mut opp_max_winding = 0;
        let mut opp_sum_winding = 0;
        self.set_up_windings_opp(
            state,
            start,
            end,
            sum_mi_winding,
            sum_su_winding,
            &mut max_winding,
            &mut sum_winding,
            &mut opp_max_winding,
            &mut opp_sum_winding,
        );
        let (mi_from, mi_to, su_from, su_to) = if self.operand(state) {
            (
                (opp_max_winding & xor_mi_mask) != 0,
                (opp_sum_winding & xor_mi_mask) != 0,
                (max_winding & xor_su_mask) != 0,
                (sum_winding & xor_su_mask) != 0,
            )
        } else {
            (
                (max_winding & xor_mi_mask) != 0,
                (sum_winding & xor_mi_mask) != 0,
                (opp_max_winding & xor_su_mask) != 0,
                (opp_sum_winding & xor_su_mask) != 0,
            )
        };
        K_ACTIVE_EDGE[op as usize][mi_from as usize][mi_to as usize][su_from as usize]
            [su_to as usize]
    }

    /// `activeWinding(SkOpSpanBase* start, SkOpSpanBase* end)`.
    pub(crate) fn active_winding(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
    ) -> bool {
        let mut sum_winding = self.update_winding(state, end, start);
        self.active_winding_sum(state, start, end, &mut sum_winding)
    }

    /// `activeWinding(SkOpSpanBase* start, SkOpSpanBase* end, int* sumWinding)`.
    pub(crate) fn active_winding_sum(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        sum_winding: &mut i32,
    ) -> bool {
        let mut max_winding = 0;
        self.set_up_winding(state, start, end, &mut max_winding, sum_winding);
        let from = max_winding != 0;
        let to = *sum_winding != 0;
        K_UNARY_ACTIVE_EDGE[from as usize][to as usize]
    }

    pub(crate) fn add_conic(
        self,
        state: &mut SkOpGlobalState,
        pts: &[SkPoint],
        weight: f32,
        parent: SkOpContour,
    ) -> SkOpSegment {
        self.init(state, pts, weight, parent, SkPathVerb::Conic);
        let mut curve = SkDCurve::default();
        let mut conic = curve.as_conic();
        conic.set(pts, weight);
        curve.set_conic(&conic);
        curve.set_conic_bounds(pts, weight, 0.0, 1.0, &mut state[self].bounds);
        self
    }

    pub(crate) fn add_cubic(
        self,
        state: &mut SkOpGlobalState,
        pts: &[SkPoint],
        parent: SkOpContour,
    ) -> SkOpSegment {
        self.init(state, pts, 1.0, parent, SkPathVerb::Cubic);
        let mut curve = SkDCurve::default();
        let mut cubic = curve.as_cubic();
        cubic.set(pts);
        curve.set_cubic(&cubic);
        curve.set_cubic_bounds(pts, 1.0, 0.0, 1.0, &mut state[self].bounds);
        self
    }

    /// `const` in Skia, which marks the span added through a `const_cast`.
    pub(crate) fn add_curve_to(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        path: &mut SkPathWriter,
    ) -> bool {
        let span_start = start.starter(state, end);
        if span_start.already_added(state) {
            return false;
        }
        span_start.mark_added(state);
        let mut curve_part = SkDCurveSweep::default();
        start
            .segment(state)
            .sub_divide(state, start, end, &mut curve_part.curve);
        let own_verb = state[self].verb;
        curve_part.set_curve_hull_sweep(own_verb);
        let verb = if curve_part.is_curve() {
            own_verb
        } else {
            SkPathVerb::Line
        };
        path.deferred_move(state, start.pt_t());
        match verb {
            SkPathVerb::Line => {
                if !path.deferred_line(state, end.pt_t()) {
                    return false;
                }
            }
            SkPathVerb::Quad => {
                path.quad_to(
                    state,
                    &curve_part.curve.as_quad()[1].as_sk_point(),
                    end.pt_t(),
                );
            }
            SkPathVerb::Conic => {
                path.conic_to(
                    state,
                    &curve_part.curve.as_conic()[1].as_sk_point(),
                    end.pt_t(),
                    curve_part.curve.as_conic().weight,
                );
            }
            SkPathVerb::Cubic => {
                path.cubic_to(
                    state,
                    &curve_part.curve.as_cubic()[1].as_sk_point(),
                    &curve_part.curve.as_cubic()[2].as_sk_point(),
                    end.pt_t(),
                );
            }
            _ => debug_assert!(false),
        }
        true
    }

    pub(crate) fn add_end_span(self, state: &mut SkOpGlobalState) -> SkOpAngle {
        let angle = state.make_angle();
        let tail = state[self].tail;
        let tail_prev = tail
            .prev(state)
            .expect("a segment's tail has a previous span");
        angle.set(state, tail, tail_prev.0);
        tail.set_from_angle(state, Some(angle));
        angle
    }

    // break the span so that the coincident part does not change the angle of the remainder
    pub(crate) fn add_expanded(
        self,
        state: &mut SkOpGlobalState,
        new_t: f64,
        test: SkOpSpanBase,
        start_over: &mut bool,
    ) -> bool {
        if self.contains(state, new_t) {
            return true;
        }
        state.reset_allocated_op_span();
        if !between(0.0, new_t, 1.0) {
            return false;
        }
        let new_pt_t = self.add_t(state, new_t);
        *start_over |= state.allocated_op_span();
        let Some(new_pt_t) = new_pt_t else {
            return false;
        };
        let new_pt = self.pt_at_t(state, new_t);
        state[new_pt_t].pt = new_pt;
        let opp_prev = test.pt_t().opp_prev(state, new_pt_t);
        if let Some(opp_prev) = opp_prev {
            // const cast away to change linked list; pt/t values stays unchanged
            let writable_test = test;
            let new_span = new_pt_t.span(state);
            writable_test.merge_matches(state, new_span);
            writable_test.pt_t().add_opp(state, new_pt_t, opp_prev);
            writable_test.check_for_collapsed_coincidence(state);
        }
        true
    }

    pub(crate) fn add_line(
        self,
        state: &mut SkOpGlobalState,
        pts: &[SkPoint],
        parent: SkOpContour,
    ) -> SkOpSegment {
        debug_assert!(pts[0] != pts[1]);
        self.init(state, pts, 1.0, parent, SkPathVerb::Line);
        state[self].bounds.set_bounds(&pts[..2]);
        self
    }

    // `addMissing` is declared in SkOpSegment.h but defined nowhere in Skia: left out.

    pub(crate) fn add_start_span(self, state: &mut SkOpGlobalState) -> SkOpAngle {
        let angle = state.make_angle();
        let head = state[self].head;
        let head_next = head.next(state).expect("a segment's head has a next span");
        angle.set(state, head.0, head_next);
        head.set_to_angle(state, Some(angle));
        angle
    }

    pub(crate) fn add_quad(
        self,
        state: &mut SkOpGlobalState,
        pts: &[SkPoint],
        parent: SkOpContour,
    ) -> SkOpSegment {
        self.init(state, pts, 1.0, parent, SkPathVerb::Quad);
        let mut curve = SkDCurve::default();
        let mut quad = curve.as_quad();
        quad.set(pts);
        curve.set_quad(&quad);
        curve.set_quad_bounds(pts, 1.0, 0.0, 1.0, &mut state[self].bounds);
        self
    }

    /// `addT(double t)`.
    pub(crate) fn add_t(self, state: &mut SkOpGlobalState, t: f64) -> Option<SkOpPtT> {
        let pt = self.pt_at_t(state, t);
        self.add_t_pt(state, t, &pt)
    }

    /// `addT(double t, const SkPoint& pt)`.
    // Please keep this in sync with debugAddT()
    pub(crate) fn add_t_pt(
        self,
        state: &mut SkOpGlobalState,
        t: f64,
        pt: &SkPoint,
    ) -> Option<SkOpPtT> {
        let mut span_base: SkOpSpanBase = state[self].head.0;
        loop {
            let result = span_base.pt_t();
            if t == state[result].t || (!zero_or_one(t) && self.r#match(state, result, self, t, pt))
            {
                span_base.bump_span_adds(state);
                return Some(result);
            }
            if t < state[result].t {
                let prev = result.span(state).prev(state)?;
                // marks in global state that new op span has been allocated
                let span = self.insert(state, prev);
                span.init(state, self, Some(prev), t, pt);
                span.bump_span_adds(state);
                return Some(span.pt_t());
            }
            if span_base == state[self].tail {
                return None;
            }
            match span_base.up_cast().next(state) {
                Some(next) => span_base = next,
                None => break,
            }
        }
        debug_assert!(false);
        None // we never get here, but need this to satisfy compiler
    }

    pub(crate) fn bounds(self, state: &SkOpGlobalState) -> &SkPathOpsBounds {
        &state[self].bounds
    }

    pub(crate) fn bump_count(self, state: &mut SkOpGlobalState) {
        state[self].count += 1;
    }

    pub(crate) fn calc_angles(self, state: &mut SkOpGlobalState) {
        let head = state[self].head;
        let tail = state[self].tail;
        let mut active_prior = !head.is_canceled(state);
        if active_prior && !head.simple(state) {
            self.add_start_span(state);
        }
        let mut prior = head;
        let mut span_base = head.next(state).expect("a segment's head has a next span");
        while span_base != tail {
            if active_prior {
                let prior_angle = state.make_angle();
                prior_angle.set(state, span_base, prior.0);
                span_base.set_from_angle(state, Some(prior_angle));
            }
            let span = span_base.up_cast();
            let active = !span.is_canceled(state);
            let next = span.next(state).expect("a span has a next span");
            if active {
                let angle = state.make_angle();
                angle.set(state, span.0, next);
                span.set_to_angle(state, Some(angle));
            }
            active_prior = active;
            prior = span;
            span_base = next;
        }
        if active_prior && !tail.simple(state) {
            self.add_end_span(state);
        }
    }

    pub(crate) fn collapsed(self, state: &SkOpGlobalState, start_t: f64, end_t: f64) -> Collapsed {
        let mut span: SkOpSpanBase = state[self].head.0;
        loop {
            let result = span.collapsed(state, start_t, end_t);
            if Collapsed::No != result {
                return result;
            }
            if span.up_castable(state).is_none() {
                break;
            }
            match span.up_cast().next(state) {
                Some(next) => span = next,
                None => break,
            }
        }
        Collapsed::No
    }

    pub(crate) fn compute_one_sum(
        state: &mut SkOpGlobalState,
        base_angle: SkOpAngle,
        next_angle: SkOpAngle,
        include_type: IncludeType,
    ) -> bool {
        let base_segment = base_angle.segment(state);
        let mut sum_mi_winding = base_segment.update_winding_reverse(state, base_angle);
        // uninitialized in Skia; read only when binary
        let mut sum_su_winding = 0;
        let binary = include_type >= IncludeType::BinarySingle;
        if binary {
            sum_su_winding = base_segment.update_opp_winding_reverse(state, base_angle);
            if base_segment.operand(state) {
                std::mem::swap(&mut sum_mi_winding, &mut sum_su_winding);
            }
        }
        let next_segment = next_angle.segment(state);
        let mut max_winding = 0;
        let mut sum_winding = 0;
        let mut last: Option<SkOpSpanBase> = None;
        if binary {
            let mut opp_max_winding = 0;
            let mut opp_sum_winding = 0;
            next_segment.set_up_windings_opp(
                state,
                next_angle.start(state),
                next_angle.end(state),
                &mut sum_mi_winding,
                &mut sum_su_winding,
                &mut max_winding,
                &mut sum_winding,
                &mut opp_max_winding,
                &mut opp_sum_winding,
            );
            if !next_segment.mark_angle_opp(
                state,
                max_winding,
                sum_winding,
                opp_max_winding,
                opp_sum_winding,
                next_angle,
                Some(&mut last),
            ) {
                return false;
            }
        } else {
            next_segment.set_up_windings(
                state,
                next_angle.start(state),
                next_angle.end(state),
                &mut sum_mi_winding,
                &mut max_winding,
                &mut sum_winding,
            );
            if !next_segment.mark_angle(
                state,
                max_winding,
                sum_winding,
                next_angle,
                Some(&mut last),
            ) {
                return false;
            }
        }
        next_angle.set_last_marked(state, last);
        true
    }

    pub(crate) fn compute_one_sum_reverse(
        state: &mut SkOpGlobalState,
        base_angle: SkOpAngle,
        next_angle: SkOpAngle,
        include_type: IncludeType,
    ) -> bool {
        let base_segment = base_angle.segment(state);
        let mut sum_mi_winding = base_segment.update_winding_angle(state, base_angle);
        // uninitialized in Skia; read only when binary
        let mut sum_su_winding = 0;
        let binary = include_type >= IncludeType::BinarySingle;
        if binary {
            sum_su_winding = base_segment.update_opp_winding_angle(state, base_angle);
            if base_segment.operand(state) {
                std::mem::swap(&mut sum_mi_winding, &mut sum_su_winding);
            }
        }
        let next_segment = next_angle.segment(state);
        let mut max_winding = 0;
        let mut sum_winding = 0;
        let mut last: Option<SkOpSpanBase> = None;
        if binary {
            let mut opp_max_winding = 0;
            let mut opp_sum_winding = 0;
            next_segment.set_up_windings_opp(
                state,
                next_angle.end(state),
                next_angle.start(state),
                &mut sum_mi_winding,
                &mut sum_su_winding,
                &mut max_winding,
                &mut sum_winding,
                &mut opp_max_winding,
                &mut opp_sum_winding,
            );
            if !next_segment.mark_angle_opp(
                state,
                max_winding,
                sum_winding,
                opp_max_winding,
                opp_sum_winding,
                next_angle,
                Some(&mut last),
            ) {
                return false;
            }
        } else {
            next_segment.set_up_windings(
                state,
                next_angle.end(state),
                next_angle.start(state),
                &mut sum_mi_winding,
                &mut max_winding,
                &mut sum_winding,
            );
            if !next_segment.mark_angle(
                state,
                max_winding,
                sum_winding,
                next_angle,
                Some(&mut last),
            ) {
                return false;
            }
        }
        next_angle.set_last_marked(state, last);
        true
    }

    // at this point, the span is already ordered, or unorderable
    pub(crate) fn compute_sum(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        include_type: IncludeType,
    ) -> i32 {
        debug_assert!(include_type != IncludeType::UnaryXor);
        let Some(first_angle) = self.span_to_angle(state, end, start) else {
            return SK_NAN_32;
        };
        if first_angle.next(state).is_none() {
            return SK_NAN_32;
        }
        // if all angles have a computed winding,
        //  or if no adjacent angles are orderable,
        //  or if adjacent orderable angles have no computed winding,
        //  there's nothing to do
        // if two orderable angles are adjacent, and both are next to orderable angles,
        //  and one has winding computed, transfer to the other
        let mut base_angle: Option<SkOpAngle> = None;
        let mut try_reverse = false;
        // look for counterclockwise transfers
        let mut angle = first_angle.previous(state);
        let mut next = angle.next(state).expect("an angle in a loop has a next");
        let mut first_angle = next;
        loop {
            let prior = angle;
            angle = next;
            next = angle.next(state).expect("an angle in a loop has a next");
            debug_assert!(prior.next(state) == Some(angle));
            debug_assert!(angle.next(state) == Some(next));
            'next_angle: {
                if prior.unorderable(state) || angle.unorderable(state) || next.unorderable(state) {
                    base_angle = None;
                    break 'next_angle;
                }
                let test_winding = angle.starter(state).wind_sum(state);
                if SK_MIN_S32 != test_winding {
                    base_angle = Some(angle);
                    try_reverse = true;
                    break 'next_angle;
                }
                if let Some(base) = base_angle {
                    SkOpSegment::compute_one_sum(state, base, angle, include_type);
                    base_angle = if SK_MIN_S32 != angle.starter(state).wind_sum(state) {
                        Some(angle)
                    } else {
                        None
                    };
                }
            }
            if next == first_angle {
                break;
            }
        }
        if let Some(base) = base_angle {
            if SK_MIN_S32 == first_angle.starter(state).wind_sum(state) {
                first_angle = base;
                try_reverse = true;
            }
        }
        if try_reverse {
            base_angle = None;
            let mut prior = first_angle;
            loop {
                angle = prior;
                prior = angle.previous(state);
                debug_assert!(prior.next(state) == Some(angle));
                next = angle.next(state).expect("an angle in a loop has a next");
                'prior_angle: {
                    if prior.unorderable(state)
                        || angle.unorderable(state)
                        || next.unorderable(state)
                    {
                        base_angle = None;
                        break 'prior_angle;
                    }
                    let test_winding = angle.starter(state).wind_sum(state);
                    if SK_MIN_S32 != test_winding {
                        base_angle = Some(angle);
                        break 'prior_angle;
                    }
                    if let Some(base) = base_angle {
                        SkOpSegment::compute_one_sum_reverse(state, base, angle, include_type);
                        base_angle = if SK_MIN_S32 != angle.starter(state).wind_sum(state) {
                            Some(angle)
                        } else {
                            None
                        };
                    }
                }
                if prior == first_angle {
                    break;
                }
            }
        }
        start.starter(state, end).wind_sum(state)
    }

    // Please keep this in sync with debugClearAll()
    pub(crate) fn clear_all(self, state: &mut SkOpGlobalState) {
        let mut span = state[self].head;
        loop {
            self.clear_one(state, span);
            match span
                .next(state)
                .expect("a span has a next span")
                .up_castable(state)
            {
                Some(next) => span = next,
                None => break,
            }
        }
        state
            .coincidence()
            .expect("the global state has a coincidence")
            .release(state, self);
    }

    // Please keep this in sync with debugClearOne()
    pub(crate) fn clear_one(self, state: &mut SkOpGlobalState, span: SkOpSpan) {
        span.set_wind_value(state, 0);
        span.set_opp_value(state, 0);
        self.mark_done(state, span);
    }

    // Please keep this in sync with DebugClearVisited()
    pub(crate) fn clear_visited(state: &mut SkOpGlobalState, span: SkOpSpanBase) {
        // reset visited flag back to false
        let mut span = span;
        loop {
            let stop_pt_t = span.pt_t();
            let mut pt_t = stop_pt_t;
            loop {
                pt_t = pt_t.next(state);
                if pt_t == stop_pt_t {
                    break;
                }
                let opp = pt_t.segment(state);
                opp.reset_visited(state);
            }
            if span.r#final(state) {
                break;
            }
            match span.up_cast().next(state) {
                Some(next) => span = next,
                None => break,
            }
        }
    }

    pub(crate) fn contains(self, state: &SkOpGlobalState, t: f64) -> bool {
        let tail = state[self].tail;
        let mut span_base: SkOpSpanBase = state[self].head.0;
        loop {
            if span_base.pt_t().contains_segment_t(state, self, t) {
                return true;
            }
            if span_base == tail {
                break;
            }
            span_base = span_base
                .up_cast()
                .next(state)
                .expect("a span before the tail has a next span");
        }
        false
    }

    pub(crate) fn done(self, state: &SkOpGlobalState) -> bool {
        state[self].done_count == state[self].count
    }

    /// `done(const SkOpAngle* angle)`.
    pub(crate) fn done_angle(self, state: &SkOpGlobalState, angle: SkOpAngle) -> bool {
        angle
            .start(state)
            .starter(state, angle.end(state))
            .done(state)
    }

    pub(crate) fn d_pt_at_t(self, state: &SkOpGlobalState, mid: f64) -> SkDPoint {
        let data = &state[self];
        CURVE_D_POINT_AT_T[data.verb as usize].expect("a segment's verb has a curve")(
            &data.pts,
            data.weight,
            mid,
        )
    }

    pub(crate) fn d_slope_at_t(self, state: &SkOpGlobalState, mid: f64) -> SkDVector {
        let data = &state[self];
        CURVE_D_SLOPE_AT_T[data.verb as usize].expect("a segment's verb has a curve")(
            &data.pts,
            data.weight,
            mid,
        )
    }

    pub(crate) fn existing(
        self,
        state: &SkOpGlobalState,
        t: f64,
        opp: Option<SkOpSegment>,
    ) -> Option<SkOpPtT> {
        let mut test = Some(state[self].head.0);
        let mut test_pt_t;
        let pt = self.pt_at_t(state, t);
        'found_match: loop {
            let test_span = test.expect("a span before the tail has a next span");
            test_pt_t = test_span.pt_t();
            if state[test_pt_t].t == t {
                break;
            }
            'next_test: {
                if !self.r#match(state, test_pt_t, self, t, &pt) {
                    if t < state[test_pt_t].t {
                        return None;
                    }
                    break 'next_test;
                }
                if opp.is_none() {
                    return Some(test_pt_t);
                }
                let mut r#loop = test_pt_t.next(state);
                while r#loop != test_pt_t {
                    if r#loop.segment(state) == self
                        && state[r#loop].t == t
                        && state[r#loop].pt == pt
                    {
                        break 'found_match;
                    }
                    r#loop = r#loop.next(state);
                }
                return None;
            }
            test = test_span.up_cast().next(state);
            if test.is_none() {
                break;
            }
        }
        // foundMatch:
        if let Some(opp) = opp {
            test.expect("the matched span exists")
                .contains_segment(state, opp)?;
        }
        Some(test_pt_t)
    }

    /*
    The M and S variable name parts stand for the operators.
      Mi stands for Minuend (see wiki subtraction, analogous to difference)
      Su stands for Subtrahend
    The Opp variable name part designates that the value is for the Opposite operator.
    Opposite values result from combining coincident spans.
    */
    pub(crate) fn find_next_op(
        self,
        state: &mut SkOpGlobalState,
        chase: &mut Vec<SkOpSpanBase>,
        next_start: &mut Option<SkOpSpanBase>,
        next_end: &mut Option<SkOpSpanBase>,
        unsortable: &mut bool,
        simple: &mut bool,
        op: SkPathOp,
        xor_mi_mask: i32,
        xor_su_mask: i32,
    ) -> Option<SkOpSegment> {
        let start = next_start.expect("findNextOp has a start");
        let end = next_end.expect("findNextOp has an end");
        debug_assert!(start != end);
        let mut step = start.step(state, end);
        let other = self.is_simple(state, next_start, &mut step); // advances nextStart
        *simple = other.is_some();
        if *simple {
            // mark the smaller of startIndex, endIndex done, and all adjacent
            // spans with the same T value (but not 'other' spans)
            let start_span = start.starter(state, end);
            if start_span.done(state) {
                return None;
            }
            self.mark_done(state, start_span);
            let advanced = next_start.expect("isSimple leaves a start");
            *next_end = if step > 0 {
                advanced.up_cast().next(state)
            } else {
                advanced.prev(state).map(SkOpSpanBase::from)
            };
            return other;
        }
        let advanced = next_start.expect("isSimple leaves a start");
        let end_near = if step > 0 {
            advanced.up_cast().next(state)
        } else {
            advanced.prev(state).map(SkOpSpanBase::from)
        };
        debug_assert!(end_near == Some(end)); // is this ever not end?
        let end_near = end_near.expect("the span next to start exists");
        debug_assert!(start != end_near);
        debug_assert!((start.t(state) < end_near.t(state)) ^ (step < 0));
        // more than one viable candidate -- measure angles to find best
        let calc_winding = self.compute_sum(state, start, end_near, IncludeType::BinaryOpp);
        let sortable = calc_winding != SK_NAN_32;
        if !sortable {
            *unsortable = true;
            let starter = start.starter(state, end);
            self.mark_done(state, starter);
            return None;
        }
        let angle = self
            .span_to_angle(state, end, start)
            .expect("a sortable span has an angle");
        if angle.unorderable(state) {
            *unsortable = true;
            let starter = start.starter(state, end);
            self.mark_done(state, starter);
            return None;
        }
        let mut sum_mi_winding = self.update_winding(state, end, start);
        if sum_mi_winding == SK_MIN_S32 {
            *unsortable = true;
            let starter = start.starter(state, end);
            self.mark_done(state, starter);
            return None;
        }
        let mut sum_su_winding = self.update_opp_winding(state, end, start);
        if self.operand(state) {
            std::mem::swap(&mut sum_mi_winding, &mut sum_su_winding);
        }
        let mut next_angle = angle.next(state).expect("an angle in a loop has a next");
        let mut found_angle: Option<SkOpAngle> = None;
        let mut found_done = false;
        // iterate through the angle, and compute everyone's winding
        let mut next_segment;
        let mut active_count = 0;
        loop {
            next_segment = next_angle.segment(state);
            let active_angle = next_segment.active_op_sum(
                state,
                xor_mi_mask,
                xor_su_mask,
                next_angle.start(state),
                next_angle.end(state),
                op,
                &mut sum_mi_winding,
                &mut sum_su_winding,
            );
            if active_angle {
                active_count += 1;
                if found_angle.is_none() || (found_done && active_count & 1 != 0) {
                    found_angle = Some(next_angle);
                    found_done = next_segment.done_angle(state, next_angle);
                }
            }
            if !next_segment.done(state) {
                if !active_angle {
                    let _ = next_segment.mark_and_chase_done(
                        state,
                        next_angle.start(state),
                        next_angle.end(state),
                        None,
                    );
                }
                let last = next_angle.last_marked(state);
                if let Some(last) = last {
                    chase.push(last);
                }
            }
            next_angle = next_angle
                .next(state)
                .expect("an angle in a loop has a next");
            if next_angle == angle {
                break;
            }
        }
        let start_segment = start.segment(state);
        let starter = start.starter(state, end);
        start_segment.mark_done(state, starter);
        let found_angle = found_angle?;
        *next_start = Some(found_angle.start(state));
        *next_end = Some(found_angle.end(state));
        next_segment = found_angle.segment(state);
        Some(next_segment)
    }

    pub(crate) fn find_next_winding(
        self,
        state: &mut SkOpGlobalState,
        chase: &mut Vec<SkOpSpanBase>,
        next_start: &mut Option<SkOpSpanBase>,
        next_end: &mut Option<SkOpSpanBase>,
        unsortable: &mut bool,
    ) -> Option<SkOpSegment> {
        let start = next_start.expect("findNextWinding has a start");
        let end = next_end.expect("findNextWinding has an end");
        debug_assert!(start != end);
        let mut step = start.step(state, end);
        let other = self.is_simple(state, next_start, &mut step); // advances nextStart
        if other.is_some() {
            // mark the smaller of startIndex, endIndex done, and all adjacent
            // spans with the same T value (but not 'other' spans)
            let start_span = start.starter(state, end);
            if start_span.done(state) {
                return None;
            }
            self.mark_done(state, start_span);
            let advanced = next_start.expect("isSimple leaves a start");
            *next_end = if step > 0 {
                advanced.up_cast().next(state)
            } else {
                advanced.prev(state).map(SkOpSpanBase::from)
            };
            return other;
        }
        let advanced = next_start.expect("isSimple leaves a start");
        let end_near = if step > 0 {
            advanced.up_cast().next(state)
        } else {
            advanced.prev(state).map(SkOpSpanBase::from)
        };
        debug_assert!(end_near == Some(end)); // is this ever not end?
        let end_near = end_near.expect("the span next to start exists");
        debug_assert!(start != end_near);
        debug_assert!((start.t(state) < end_near.t(state)) ^ (step < 0));
        // more than one viable candidate -- measure angles to find best
        let calc_winding = self.compute_sum(state, start, end_near, IncludeType::UnaryWinding);
        let sortable = calc_winding != SK_NAN_32;
        if !sortable {
            *unsortable = true;
            let starter = start.starter(state, end);
            self.mark_done(state, starter);
            return None;
        }
        let angle = self
            .span_to_angle(state, end, start)
            .expect("a sortable span has an angle");
        if angle.unorderable(state) {
            *unsortable = true;
            let starter = start.starter(state, end);
            self.mark_done(state, starter);
            return None;
        }
        let mut sum_winding = self.update_winding(state, end, start);
        let mut next_angle = angle.next(state).expect("an angle in a loop has a next");
        let mut found_angle: Option<SkOpAngle> = None;
        let mut found_done = false;
        // iterate through the angle, and compute everyone's winding
        let mut next_segment;
        let mut active_count = 0;
        loop {
            next_segment = next_angle.segment(state);
            let active_angle = next_segment.active_winding_sum(
                state,
                next_angle.start(state),
                next_angle.end(state),
                &mut sum_winding,
            );
            if active_angle {
                active_count += 1;
                if found_angle.is_none() || (found_done && active_count & 1 != 0) {
                    found_angle = Some(next_angle);
                    found_done = next_segment.done_angle(state, next_angle);
                }
            }
            if !next_segment.done(state) {
                if !active_angle {
                    let _ = next_segment.mark_and_chase_done(
                        state,
                        next_angle.start(state),
                        next_angle.end(state),
                        None,
                    );
                }
                let last = next_angle.last_marked(state);
                if let Some(last) = last {
                    chase.push(last);
                }
            }
            next_angle = next_angle
                .next(state)
                .expect("an angle in a loop has a next");
            if next_angle == angle {
                break;
            }
        }
        let start_segment = start.segment(state);
        let starter = start.starter(state, end);
        start_segment.mark_done(state, starter);
        let found_angle = found_angle?;
        *next_start = Some(found_angle.start(state));
        *next_end = Some(found_angle.end(state));
        next_segment = found_angle.segment(state);
        Some(next_segment)
    }

    pub(crate) fn find_next_xor(
        self,
        state: &mut SkOpGlobalState,
        next_start: &mut Option<SkOpSpanBase>,
        next_end: &mut Option<SkOpSpanBase>,
        unsortable: &mut bool,
    ) -> Option<SkOpSegment> {
        let start = next_start.expect("findNextXor has a start");
        let end = next_end.expect("findNextXor has an end");
        debug_assert!(start != end);
        let mut step = start.step(state, end);
        let other = self.is_simple(state, next_start, &mut step); // advances nextStart
        if other.is_some() {
            // mark the smaller of startIndex, endIndex done, and all adjacent
            // spans with the same T value (but not 'other' spans)
            let start_span = start.starter(state, end);
            if start_span.done(state) {
                return None;
            }
            self.mark_done(state, start_span);
            let advanced = next_start.expect("isSimple leaves a start");
            *next_end = if step > 0 {
                advanced.up_cast().next(state)
            } else {
                advanced.prev(state).map(SkOpSpanBase::from)
            };
            return other;
        }
        let angle = self.span_to_angle(state, end, start);
        let angle = match angle {
            Some(angle) if !angle.unorderable(state) => angle,
            _ => {
                *unsortable = true;
                let starter = start.starter(state, end);
                self.mark_done(state, starter);
                return None;
            }
        };
        let mut next_angle = angle.next(state);
        let mut found_angle: Option<SkOpAngle> = None;
        let mut found_done = false;
        // iterate through the angle, and compute everyone's winding
        let mut next_segment;
        let mut active_count = 0;
        loop {
            let current = next_angle?;
            next_segment = current.segment(state);
            active_count += 1;
            if found_angle.is_none() || (found_done && active_count & 1 != 0) {
                found_angle = Some(current);
                found_done = next_segment.done_angle(state, current);
                if !found_done {
                    break;
                }
            }
            next_angle = current.next(state);
            if next_angle == Some(angle) {
                break;
            }
        }
        let start_segment = start.segment(state);
        let starter = start.starter(state, end);
        start_segment.mark_done(state, starter);
        let found_angle = found_angle?;
        *next_start = Some(found_angle.start(state));
        *next_end = Some(found_angle.end(state));
        next_segment = found_angle.segment(state);
        Some(next_segment)
    }

    // `findSortableTop` is defined in SkPathOpsWinding.cpp: see `path_ops_winding.rs`.

    pub(crate) fn head(self, state: &SkOpGlobalState) -> SkOpSpan {
        state[self].head
    }

    /// Copies the verb's points from `pts` (Skia keeps the pointer).
    pub(crate) fn init(
        self,
        state: &mut SkOpGlobalState,
        pts: &[SkPoint],
        weight: f32,
        parent: SkOpContour,
        verb: SkPathVerb,
    ) {
        let point_count = sk_path_ops_verb_to_points(verb) as usize + 1;
        let data = &mut state[self];
        data.contour = parent;
        data.next = None;
        data.pts[..point_count].copy_from_slice(&pts[..point_count]);
        data.weight = weight;
        data.verb = verb;
        data.count = 0;
        data.done_count = 0;
        data.visited = false;
        let zero_span = data.head;
        let one_span = data.tail;
        let first_pt = data.pts[0];
        let last_pt = data.pts[sk_path_ops_verb_to_points(verb) as usize];
        zero_span.init(state, self, None, 0.0, &first_pt);
        zero_span.set_next(state, Some(one_span));
        one_span.init_base(state, self, Some(zero_span), 1.0, &last_pt);
    }

    pub(crate) fn insert(self, state: &mut SkOpGlobalState, prev: SkOpSpan) -> SkOpSpan {
        state.set_allocated_op_span();
        let result = state.make_span();
        let next = prev.next(state);
        result.set_prev(state, Some(prev));
        prev.set_next(state, Some(result.0));
        result.set_next(state, next);
        if let Some(next) = next {
            next.set_prev(state, Some(result));
        }
        result
    }

    pub(crate) fn is_close(self, state: &SkOpGlobalState, t: f64, opp: SkOpSegment) -> bool {
        let c_pt = self.d_pt_at_t(state, t);
        let data = &state[self];
        let dxdy = CURVE_D_SLOPE_AT_T[data.verb as usize].expect("a segment's verb has a curve")(
            &data.pts,
            data.weight,
            t,
        );
        let perp = SkDLine {
            pts: [
                c_pt,
                SkDPoint {
                    x: c_pt.x + dxdy.y,
                    y: c_pt.y - dxdy.x,
                },
            ],
        };
        let mut i = SkIntersections::new();
        let opp_data = &state[opp];
        CURVE_INTERSECT_RAY[opp_data.verb as usize].expect("a segment's verb has a curve")(
            &opp_data.pts,
            opp_data.weight,
            &perp,
            &mut i,
        );
        let used = i.used();
        for index in 0..used {
            if c_pt.roughly_equal(i.pt(index as usize)) {
                return true;
            }
        }
        false
    }

    pub(crate) fn is_horizontal(self, state: &SkOpGlobalState) -> bool {
        state[self].bounds.top == state[self].bounds.bottom
    }

    pub(crate) fn is_simple(
        self,
        state: &SkOpGlobalState,
        end: &mut Option<SkOpSpanBase>,
        step: &mut i32,
    ) -> Option<SkOpSegment> {
        self.next_chase(state, end, step, None, None)
    }

    pub(crate) fn is_vertical(self, state: &SkOpGlobalState) -> bool {
        state[self].bounds.left == state[self].bounds.right
    }

    pub(crate) fn is_xor(self, state: &SkOpGlobalState) -> bool {
        state[self].contour.is_xor(state)
    }

    pub(crate) fn mark_all_done(self, state: &mut SkOpGlobalState) {
        let mut span = state[self].head;
        loop {
            self.mark_done(state, span);
            match span
                .next(state)
                .expect("a span has a next span")
                .up_castable(state)
            {
                Some(next) => span = next,
                None => break,
            }
        }
    }

    /// `found` may be null in Skia.
    pub(crate) fn mark_and_chase_done(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        found: Option<&mut Option<SkOpSpanBase>>,
    ) -> bool {
        let mut step = start.step(state, end);
        let mut min_span = start.starter(state, end);
        self.mark_done(state, min_span);
        let mut last: Option<SkOpSpanBase> = None;
        let mut other = self;
        let mut prior_done: Option<SkOpSpan> = None;
        let mut last_done: Option<SkOpSpan> = None;
        let mut safety_net = 1000;
        let mut start = Some(start);
        while let Some(next_other) = other.next_chase(
            state,
            &mut start,
            &mut step,
            Some(&mut min_span),
            Some(&mut last),
        ) {
            other = next_other;
            safety_net -= 1;
            if safety_net == 0 {
                return false;
            }
            if other.done(state) {
                debug_assert!(last.is_none());
                break;
            }
            if last_done == Some(min_span) || prior_done == Some(min_span) {
                if let Some(found) = found {
                    *found = None;
                }
                return true;
            }
            other.mark_done(state, min_span);
            prior_done = last_done;
            last_done = Some(min_span);
        }
        if let Some(found) = found {
            *found = last;
        }
        true
    }

    /// `markAndChaseWinding(start, end, winding, lastPtr)`; `lastPtr` may be null in Skia.
    pub(crate) fn mark_and_chase_winding(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        winding: i32,
        last_ptr: Option<&mut Option<SkOpSpanBase>>,
    ) -> bool {
        let mut span_start = start.starter(state, end);
        let mut step = start.step(state, end);
        let success = self.mark_winding(state, span_start, winding);
        let mut last: Option<SkOpSpanBase> = None;
        let mut other = self;
        let mut safety_net = 1000;
        let mut start = Some(start);
        while let Some(next_other) = other.next_chase(
            state,
            &mut start,
            &mut step,
            Some(&mut span_start),
            Some(&mut last),
        ) {
            other = next_other;
            safety_net -= 1;
            if safety_net == 0 {
                return false;
            }
            if span_start.wind_sum(state) != SK_MIN_S32 {
                debug_assert!(last.is_none());
                break;
            }
            let _ = other.mark_winding(state, span_start, winding);
        }
        if let Some(last_ptr) = last_ptr {
            *last_ptr = last;
        }
        success
    }

    /// `markAndChaseWinding(start, end, winding, oppWinding, lastPtr)`; `lastPtr` may be null.
    pub(crate) fn mark_and_chase_winding_opp(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        winding: i32,
        opp_winding: i32,
        last_ptr: Option<&mut Option<SkOpSpanBase>>,
    ) -> bool {
        let mut span_start = start.starter(state, end);
        let mut step = start.step(state, end);
        let success = self.mark_winding_opp(state, span_start, winding, opp_winding);
        let mut last: Option<SkOpSpanBase> = None;
        let mut other = self;
        let mut safety_net = 1000;
        let mut start = Some(start);
        while let Some(next_other) = other.next_chase(
            state,
            &mut start,
            &mut step,
            Some(&mut span_start),
            Some(&mut last),
        ) {
            other = next_other;
            safety_net -= 1;
            if safety_net == 0 {
                return false;
            }
            if span_start.wind_sum(state) != SK_MIN_S32 {
                if self.operand(state) == other.operand(state) {
                    if span_start.wind_sum(state) != winding
                        || span_start.opp_sum(state) != opp_winding
                    {
                        return true; // ... but let it succeed anyway
                    }
                } else {
                    if span_start.wind_sum(state) != opp_winding {
                        return false;
                    }
                    if span_start.opp_sum(state) != winding {
                        return false;
                    }
                }
                debug_assert!(last.is_none());
                break;
            }
            if self.operand(state) == other.operand(state) {
                let _ = other.mark_winding_opp(state, span_start, winding, opp_winding);
            } else {
                let _ = other.mark_winding_opp(state, span_start, opp_winding, winding);
            }
        }
        if let Some(last_ptr) = last_ptr {
            *last_ptr = last;
        }
        success
    }

    /// `markAngle(maxWinding, sumWinding, angle, result)`; `result` may be null in Skia.
    pub(crate) fn mark_angle(
        self,
        state: &mut SkOpGlobalState,
        max_winding: i32,
        sum_winding: i32,
        angle: SkOpAngle,
        result: Option<&mut Option<SkOpSpanBase>>,
    ) -> bool {
        debug_assert!(angle.segment(state) == self);
        let mut max_winding = max_winding;
        if SkOpSegment::use_inner_winding(max_winding, sum_winding) {
            max_winding = sum_winding;
        }
        self.mark_and_chase_winding(
            state,
            angle.start(state),
            angle.end(state),
            max_winding,
            result,
        )
    }

    /// `markAngle(maxWinding, sumWinding, oppMaxWinding, oppSumWinding, angle, result)`.
    pub(crate) fn mark_angle_opp(
        self,
        state: &mut SkOpGlobalState,
        max_winding: i32,
        sum_winding: i32,
        opp_max_winding: i32,
        opp_sum_winding: i32,
        angle: SkOpAngle,
        result: Option<&mut Option<SkOpSpanBase>>,
    ) -> bool {
        debug_assert!(angle.segment(state) == self);
        let mut max_winding = max_winding;
        let mut opp_max_winding = opp_max_winding;
        if SkOpSegment::use_inner_winding(max_winding, sum_winding) {
            max_winding = sum_winding;
        }
        if opp_max_winding != opp_sum_winding
            && SkOpSegment::use_inner_winding(opp_max_winding, opp_sum_winding)
        {
            opp_max_winding = opp_sum_winding;
        }
        // caller doesn't require that this marks anything
        self.mark_and_chase_winding_opp(
            state,
            angle.start(state),
            angle.end(state),
            max_winding,
            opp_max_winding,
            result,
        )
    }

    pub(crate) fn mark_done(self, state: &mut SkOpGlobalState, span: SkOpSpan) {
        debug_assert!(self == span.segment(state));
        if span.done(state) {
            return;
        }
        span.set_done(state, true);
        state[self].done_count += 1;
    }

    /// `markWinding(SkOpSpan*, int winding)`.
    pub(crate) fn mark_winding(
        self,
        state: &mut SkOpGlobalState,
        span: SkOpSpan,
        winding: i32,
    ) -> bool {
        debug_assert!(self == span.segment(state));
        debug_assert!(winding != 0);
        if span.done(state) {
            return false;
        }
        span.set_wind_sum(state, winding);
        true
    }

    /// `markWinding(SkOpSpan*, int winding, int oppWinding)`.
    pub(crate) fn mark_winding_opp(
        self,
        state: &mut SkOpGlobalState,
        span: SkOpSpan,
        winding: i32,
        opp_winding: i32,
    ) -> bool {
        debug_assert!(self == span.segment(state));
        debug_assert!(winding != 0 || opp_winding != 0);
        if span.done(state) {
            return false;
        }
        span.set_wind_sum(state, winding);
        span.set_opp_sum(state, opp_winding);
        true
    }

    pub(crate) fn r#match(
        self,
        state: &SkOpGlobalState,
        span: SkOpPtT,
        parent: SkOpSegment,
        t: f64,
        pt: &SkPoint,
    ) -> bool {
        debug_assert!(self == span.segment(state));
        if self == parent && precisely_equal(state[span].t, t) {
            return true;
        }
        if !SkDPoint::approximately_equal_sk_points(pt, &state[span].pt) {
            return false;
        }
        self != parent || !self.pts_disjoint(state, state[span].t, &state[span].pt, t, pt)
    }

    // Please keep this in sync with debugMissingCoincidence()
    // look for pairs of undetected coincident curves
    // assumes that segments going in have visited flag clear
    // Even though pairs of curves correct detect coincident runs, a run may be missed
    // if the coincidence is a product of multiple intersections. For instance, given
    // curves A, B, and C:
    // A-B intersect at a point 1; A-C and B-C intersect at point 2, so near
    // the end of C that the intersection is replaced with the end of C.
    // Even though A-B correctly do not detect an intersection at point 2,
    // the resulting run from point 1 to point 2 is coincident on A and B.
    pub(crate) fn missing_coincidence(self, state: &mut SkOpGlobalState) -> bool {
        if self.done(state) {
            return false;
        }
        let head = state[self].head;
        let mut prior: Option<SkOpSpan> = None;
        let mut span_base: SkOpSpanBase = head.0;
        let mut result = false;
        let mut safety_net = 1000;
        loop {
            let span_stop_pt_t = span_base.pt_t();
            let mut pt_t = span_stop_pt_t;
            loop {
                pt_t = pt_t.next(state);
                if pt_t == span_stop_pt_t {
                    break;
                }
                safety_net -= 1;
                if safety_net == 0 {
                    return false;
                }
                if pt_t.deleted(state) {
                    continue;
                }
                let opp = pt_t.span(state).segment(state);
                if opp.done(state) {
                    continue;
                }
                // when opp is encounted the 1st time, continue; on 2nd encounter, look for
                // coincidence
                if !opp.visited(state) {
                    continue;
                }
                if span_base == head {
                    continue;
                }
                if pt_t.segment(state) == self {
                    continue;
                }
                let span = span_base.up_castable(state);
                // FIXME?: this assumes that if the opposite segment is coincident then no more
                // coincidence needs to be detected. This may not be true.
                if let Some(span) = span {
                    if span.contains_coincidence_segment(state, opp) {
                        continue;
                    }
                }
                if span_base.contains_coin_end_segment(state, opp) {
                    continue;
                }
                let mut prior_pt_t: Option<SkOpPtT> = None;
                // find prior span containing opp segment
                let mut prior_opp: Option<SkOpSegment> = None;
                let mut prior_test = span_base.prev(state);
                while prior_opp.is_none() {
                    let Some(prior_test_span) = prior_test else {
                        break;
                    };
                    let prior_stop_pt_t = prior_test_span.pt_t();
                    let mut walk = prior_stop_pt_t;
                    loop {
                        walk = walk.next(state);
                        prior_pt_t = Some(walk);
                        if walk == prior_stop_pt_t {
                            break;
                        }
                        if walk.deleted(state) {
                            continue;
                        }
                        let segment = walk.span(state).segment(state);
                        if segment == opp {
                            prior = Some(prior_test_span);
                            prior_opp = Some(opp);
                            break;
                        }
                    }
                    prior_test = prior_test_span.prev(state);
                }
                if prior_opp.is_none() {
                    continue;
                }
                let mut prior_pt_t = prior_pt_t.expect("the prior span's loop was walked");
                if prior_pt_t == pt_t {
                    continue;
                }
                let prior_span = prior.expect("the prior span was found");
                let mut opp_start = prior_span.pt_t();
                let mut opp_end = span_base.pt_t();
                let swapped = state[prior_pt_t].t > state[pt_t].t;
                if swapped {
                    std::mem::swap(&mut prior_pt_t, &mut pt_t);
                    std::mem::swap(&mut opp_start, &mut opp_end);
                }
                let coincidences = state
                    .coincidence()
                    .expect("the global state has a coincidence");
                let root_prior_pt_t = prior_pt_t.span(state).pt_t();
                let root_pt_t = pt_t.span(state).pt_t();
                let root_opp_start = opp_start.span(state).pt_t();
                let root_opp_end = opp_end.span(state).pt_t();
                // goto swapBack when already contained
                if !coincidences.contains(
                    state,
                    root_prior_pt_t,
                    root_pt_t,
                    root_opp_start,
                    root_opp_end,
                ) && self.test_for_coincidence(
                    state,
                    root_prior_pt_t,
                    root_pt_t,
                    prior_span.0,
                    span_base,
                    opp,
                ) {
                    // mark coincidence
                    if !coincidences.extend(
                        state,
                        root_prior_pt_t,
                        root_pt_t,
                        root_opp_start,
                        root_opp_end,
                    ) {
                        coincidences.add(
                            state,
                            root_prior_pt_t,
                            root_pt_t,
                            root_opp_start,
                            root_opp_end,
                        );
                    }
                    result = true;
                }
                // swapBack:
                if swapped {
                    std::mem::swap(&mut prior_pt_t, &mut pt_t);
                }
            }
            if span_base.r#final(state) {
                break;
            }
            match span_base.up_cast().next(state) {
                Some(next) => span_base = next,
                None => break,
            }
        }
        SkOpSegment::clear_visited(state, head.0);
        result
    }

    // please keep this in sync with debugMoveMultiples()
    // if a span has more than one intersection, merge the other segments' span as needed
    // Skia's innermost `while` ends every pass in a `goto`, so it runs at most once.
    #[allow(clippy::never_loop)]
    pub(crate) fn move_multiples(self, state: &mut SkOpGlobalState) -> bool {
        let mut test: SkOpSpanBase = state[self].head.0;
        loop {
            'check_next_span: {
                let add_count = test.span_adds_count(state);
                if add_count <= 1 {
                    break 'check_next_span;
                }
                let start_pt_t = test.pt_t();
                let mut test_pt_t = start_pt_t;
                let mut safety_hatch = 1000;
                loop {
                    // iterate through all spans associated with start
                    'next_test_pt_t: {
                        safety_hatch -= 1;
                        if safety_hatch == 0 {
                            return false;
                        }
                        let opp_span = test_pt_t.span(state);
                        if opp_span.span_adds_count(state) == add_count {
                            break 'next_test_pt_t;
                        }
                        if opp_span.deleted(state) {
                            break 'next_test_pt_t;
                        }
                        let opp_segment = opp_span.segment(state);
                        if opp_segment == self {
                            break 'next_test_pt_t;
                        }
                        // find range of spans to consider merging
                        let mut opp_prev = opp_span;
                        let mut opp_first = opp_span;
                        while let Some(prev) = opp_prev.prev(state) {
                            opp_prev = prev.0;
                            if !roughly_equal(opp_prev.t(state), opp_span.t(state)) {
                                break;
                            }
                            if opp_prev.span_adds_count(state) == add_count {
                                continue;
                            }
                            if opp_prev.deleted(state) {
                                continue;
                            }
                            opp_first = opp_prev;
                        }
                        let mut opp_next = opp_span;
                        let mut opp_last = opp_span;
                        loop {
                            let next = if opp_next.r#final(state) {
                                None
                            } else {
                                opp_next.up_cast().next(state)
                            };
                            let Some(next) = next else {
                                break;
                            };
                            opp_next = next;
                            if !roughly_equal(opp_next.t(state), opp_span.t(state)) {
                                break;
                            }
                            if opp_next.span_adds_count(state) == add_count {
                                continue;
                            }
                            if opp_next.deleted(state) {
                                continue;
                            }
                            opp_last = opp_next;
                        }
                        if opp_first == opp_last {
                            break 'next_test_pt_t;
                        }
                        let mut opp_test = opp_first;
                        loop {
                            'try_next_span: {
                                if opp_test == opp_span {
                                    break 'try_next_span;
                                }
                                // check to see if the candidate meets specific criteria:
                                // it contains spans of segments in test's loop but not including
                                // 'this'
                                let opp_start_pt_t = opp_test.pt_t();
                                let mut opp_pt_t = opp_start_pt_t;
                                loop {
                                    opp_pt_t = opp_pt_t.next(state);
                                    if opp_pt_t == opp_start_pt_t {
                                        break;
                                    }
                                    let opp_pt_t_segment = opp_pt_t.segment(state);
                                    if opp_pt_t_segment == self {
                                        break 'try_next_span;
                                    }
                                    let mut match_pt_t = start_pt_t;
                                    let mut found_match = false;
                                    loop {
                                        if match_pt_t.segment(state) == opp_pt_t_segment {
                                            found_match = true;
                                            break;
                                        }
                                        match_pt_t = match_pt_t.next(state);
                                        if match_pt_t == start_pt_t {
                                            break;
                                        }
                                    }
                                    if !found_match {
                                        break 'try_next_span;
                                    }
                                    // foundMatch: merge oppTest and oppSpan
                                    opp_test.merge_matches(state, opp_span);
                                    opp_test.add_opp(state, opp_span);
                                    break 'check_next_span;
                                }
                            }
                            // tryNextSpan:
                            if opp_test == opp_last {
                                break;
                            }
                            match opp_test.up_cast().next(state) {
                                Some(next) => opp_test = next,
                                None => break,
                            }
                        }
                    }
                    test_pt_t = test_pt_t.next(state);
                    if test_pt_t == start_pt_t {
                        break;
                    }
                }
            }
            // checkNextSpan:
            if test.r#final(state) {
                break;
            }
            match test.up_cast().next(state) {
                Some(next) => test = next,
                None => break,
            }
        }
        true
    }

    // Please keep this function in sync with debugMoveNearby()
    // Move nearby t values and pts so they all hang off the same span. Alignment happens later.
    pub(crate) fn move_nearby(self, state: &mut SkOpGlobalState) -> bool {
        // release undeleted spans pointing to this seg that are linked to the primary span
        let head = state[self].head;
        let mut span_base: SkOpSpanBase = head.0;
        let mut escape_hatch = 9999; // the largest count for a regular test is 50; for a fuzzer, 500
        loop {
            let head_pt_t = span_base.pt_t();
            let mut pt_t = head_pt_t;
            loop {
                pt_t = pt_t.next(state);
                if pt_t == head_pt_t {
                    break;
                }
                escape_hatch -= 1;
                if escape_hatch == 0 {
                    return false;
                }
                let test = pt_t.span(state);
                if pt_t.segment(state) == self
                    && !pt_t.deleted(state)
                    && test != span_base
                    && test.pt_t() == pt_t
                {
                    if test.r#final(state) {
                        if span_base == head {
                            self.clear_all(state);
                            return true;
                        }
                        span_base.up_cast().release(state, pt_t);
                    } else if test.prev(state).is_some() {
                        test.up_cast().release(state, head_pt_t);
                    }
                    break;
                }
            }
            span_base = span_base
                .up_cast()
                .next(state)
                .expect("a span before the tail has a next span");
            if span_base.r#final(state) {
                break;
            }
        }
        // This loop looks for adjacent spans which are near by
        span_base = head.0;
        loop {
            // iterate through all spans associated with start
            let test = span_base
                .up_cast()
                .next(state)
                .expect("a span before the tail has a next span");
            let mut found = false;
            if !self.spans_nearby(state, span_base, test, &mut found) {
                return false;
            }
            if found {
                if test.r#final(state) {
                    if span_base.prev(state).is_some() {
                        test.merge(state, span_base.up_cast());
                    } else {
                        self.clear_all(state);
                        return true;
                    }
                } else {
                    span_base.merge(state, test.up_cast());
                }
            }
            span_base = test;
            if span_base.r#final(state) {
                break;
            }
        }
        true
    }

    pub(crate) fn next(self, state: &SkOpGlobalState) -> Option<SkOpSegment> {
        state[self].next
    }

    /// `minPtr` and `last` may be null in Skia.
    pub(crate) fn next_chase(
        self,
        state: &SkOpGlobalState,
        start_ptr: &mut Option<SkOpSpanBase>,
        step: &mut i32,
        min_ptr: Option<&mut SkOpSpan>,
        last: Option<&mut Option<SkOpSpanBase>>,
    ) -> Option<SkOpSegment> {
        let step_ptr = step;
        let orig_start = start_ptr.expect("nextChase has a start");
        let step = *step_ptr;
        let end_span = if step > 0 {
            orig_start.up_cast().next(state)
        } else {
            orig_start.prev(state).map(SkOpSpanBase::from)
        };
        let mut end_span = end_span.expect("the span next to start exists");
        let angle = if step > 0 {
            end_span.from_angle(state)
        } else {
            end_span.up_cast().to_angle(state)
        };
        let found_span;
        let other_end;
        let other;
        match angle {
            None => {
                if end_span.t(state) != 0.0 && end_span.t(state) != 1.0 {
                    return None;
                }
                let other_pt_t = end_span.pt_t().next(state);
                other = other_pt_t.segment(state);
                found_span = other_pt_t.span(state);
                other_end = if step > 0 {
                    if found_span.up_castable(state).is_some() {
                        found_span.up_cast().next(state)
                    } else {
                        None
                    }
                } else {
                    found_span.prev(state).map(SkOpSpanBase::from)
                };
            }
            Some(angle) => {
                let loop_count = angle.loop_count(state);
                if loop_count > 2 {
                    return set_last(last, end_span);
                }
                let next = angle.next(state)?;
                other = next.segment(state);
                found_span = next.start(state);
                end_span = found_span;
                other_end = Some(next.end(state));
            }
        }
        let other_end = other_end?;
        let found_step = found_span.step(state, other_end);
        if *step_ptr != found_step {
            return set_last(last, end_span);
        }
        let orig_min = if step < 0 {
            orig_start
                .prev(state)
                .expect("the span before start exists")
        } else {
            orig_start.up_cast()
        };
        let found_min = found_span.starter(state, other_end);
        if found_min.wind_value(state) != orig_min.wind_value(state)
            || found_min.opp_value(state) != orig_min.opp_value(state)
        {
            return set_last(last, end_span);
        }
        *start_ptr = Some(found_span);
        *step_ptr = found_step;
        if let Some(min_ptr) = min_ptr {
            *min_ptr = found_min;
        }
        Some(other)
    }

    pub(crate) fn operand(self, state: &SkOpGlobalState) -> bool {
        state[self].contour.operand(state)
    }

    pub(crate) fn opp_sign(state: &SkOpGlobalState, start: SkOpSpanBase, end: SkOpSpanBase) -> i32 {
        if start.t(state) < end.t(state) {
            -start.up_cast().opp_value(state)
        } else {
            end.up_cast().opp_value(state)
        }
    }

    pub(crate) fn opp_xor(self, state: &SkOpGlobalState) -> bool {
        state[self].contour.opp_xor(state)
    }

    pub(crate) fn pt_at_t(self, state: &SkOpGlobalState, mid: f64) -> SkPoint {
        let data = &state[self];
        CURVE_POINT_AT_T[data.verb as usize].expect("a segment's verb has a curve")(
            &data.pts,
            data.weight,
            mid,
        )
    }

    /// `pts()`: the segment's points (Skia returns its pointer).
    pub(crate) fn pts(self, state: &SkOpGlobalState) -> [SkPoint; 4] {
        state[self].pts
    }

    /// `ptsDisjoint(const SkOpPtT& span, const SkOpPtT& test)`.
    pub(crate) fn pts_disjoint_pt_t(
        self,
        state: &SkOpGlobalState,
        span: SkOpPtT,
        test: SkOpPtT,
    ) -> bool {
        debug_assert!(self == span.segment(state));
        debug_assert!(self == test.segment(state));
        self.pts_disjoint(
            state,
            state[span].t,
            &state[span].pt,
            state[test].t,
            &state[test].pt,
        )
    }

    /// `ptsDisjoint(double t1, const SkPoint& pt1, double t2, const SkPoint& pt2)`.
    pub(crate) fn pts_disjoint(
        self,
        state: &SkOpGlobalState,
        t1: f64,
        pt1: &SkPoint,
        t2: f64,
        pt2: &SkPoint,
    ) -> bool {
        if state[self].verb == SkPathVerb::Line {
            return false;
        }
        // quads (and cubics) can loop back to nearly a line so that an opposite curve
        // hits in two places with very different t values.
        // OPTIMIZATION: curves could be preflighted so that, for example, something like
        // 'controls contained by ends' could avoid this check for common curves
        // 'ends are extremes in x or y' is cheaper to compute and real-world common
        // on the other hand, the below check is relatively inexpensive
        let mid_t = (t1 + t2) / 2.0;
        let mid_pt = self.pt_at_t(state, mid_t);
        let se_dist_sq = std_max(
            SkPointPriv::distance_to_sqd(pt1, pt2) * 2.0,
            FLT_EPSILON * 2.0,
        ) as f64;
        SkPointPriv::distance_to_sqd(&mid_pt, pt1) as f64 > se_dist_sq
            || SkPointPriv::distance_to_sqd(&mid_pt, pt2) as f64 > se_dist_sq
    }

    // `rayCheck` is defined in SkPathOpsWinding.cpp: see `path_ops_winding.rs`.

    pub(crate) fn release(self, state: &mut SkOpGlobalState, span: SkOpSpan) {
        if span.done(state) {
            state[self].done_count -= 1;
        }
        state[self].count -= 1;
    }

    pub(crate) fn reset_visited(self, state: &mut SkOpGlobalState) {
        state[self].visited = false;
    }

    pub(crate) fn set_next(self, state: &mut SkOpGlobalState, next: Option<SkOpSegment>) {
        state[self].next = next;
    }

    pub(crate) fn set_up_winding(
        self,
        state: &SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        max_winding: &mut i32,
        sum_winding: &mut i32,
    ) {
        let delta_sum = SkOpSegment::span_sign(state, start, end);
        *max_winding = *sum_winding;
        if *sum_winding == SK_MIN_S32 {
            return;
        }
        *sum_winding -= delta_sum;
    }

    /// `setUpWindings(start, end, sumMiWinding, maxWinding, sumWinding)`.
    pub(crate) fn set_up_windings(
        self,
        state: &SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        sum_mi_winding: &mut i32,
        max_winding: &mut i32,
        sum_winding: &mut i32,
    ) {
        let delta_sum = SkOpSegment::span_sign(state, start, end);
        *max_winding = *sum_mi_winding;
        *sum_mi_winding -= delta_sum;
        *sum_winding = *sum_mi_winding;
    }

    /// `setUpWindings(start, end, sumMiWinding, sumSuWinding, maxWinding, sumWinding,
    /// oppMaxWinding, oppSumWinding)`.
    pub(crate) fn set_up_windings_opp(
        self,
        state: &SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        sum_mi_winding: &mut i32,
        sum_su_winding: &mut i32,
        max_winding: &mut i32,
        sum_winding: &mut i32,
        opp_max_winding: &mut i32,
        opp_sum_winding: &mut i32,
    ) {
        let delta_sum = SkOpSegment::span_sign(state, start, end);
        let opp_delta_sum = SkOpSegment::opp_sign(state, start, end);
        if self.operand(state) {
            *max_winding = *sum_su_winding;
            *sum_su_winding -= delta_sum;
            *sum_winding = *sum_su_winding;
            *opp_max_winding = *sum_mi_winding;
            *sum_mi_winding -= opp_delta_sum;
            *opp_sum_winding = *sum_mi_winding;
        } else {
            *max_winding = *sum_mi_winding;
            *sum_mi_winding -= delta_sum;
            *sum_winding = *sum_mi_winding;
            *opp_max_winding = *sum_su_winding;
            *sum_su_winding -= opp_delta_sum;
            *opp_sum_winding = *sum_su_winding;
        }
    }

    pub(crate) fn sort_angles(self, state: &mut SkOpGlobalState) -> bool {
        let mut span: SkOpSpanBase = state[self].head.0;
        loop {
            'next_span: {
                let from_angle = span.from_angle(state);
                let to_angle = if span.r#final(state) {
                    None
                } else {
                    span.up_cast().to_angle(state)
                };
                let base_angle = match (from_angle, to_angle) {
                    (None, None) => break 'next_span,
                    (Some(from_angle), Some(to_angle)) => {
                        if !from_angle.insert(state, to_angle) {
                            return false;
                        }
                        from_angle
                    }
                    (Some(from_angle), None) => from_angle,
                    (None, Some(to_angle)) => to_angle,
                };
                let stop_pt_t = span.pt_t();
                let mut pt_t = stop_pt_t;
                let mut safety_net = 1000;
                loop {
                    'next_pt_t: {
                        safety_net -= 1;
                        if safety_net == 0 {
                            return false;
                        }
                        let o_span = pt_t.span(state);
                        if o_span == span {
                            break 'next_pt_t;
                        }
                        if let Some(o_angle) = o_span.from_angle(state) {
                            if !o_angle.loop_contains(state, base_angle) {
                                base_angle.insert(state, o_angle);
                            }
                        }
                        if !o_span.r#final(state) {
                            if let Some(o_angle) = o_span.up_cast().to_angle(state) {
                                if !o_angle.loop_contains(state, base_angle) {
                                    base_angle.insert(state, o_angle);
                                }
                            }
                        }
                    }
                    pt_t = pt_t.next(state);
                    if pt_t == stop_pt_t {
                        break;
                    }
                }
                if base_angle.loop_count(state) == 1 {
                    span.set_from_angle(state, None);
                    if to_angle.is_some() {
                        span.up_cast().set_to_angle(state, None);
                    }
                }
            }
            if span.r#final(state) {
                break;
            }
            match span.up_cast().next(state) {
                Some(next) => span = next,
                None => break,
            }
        }
        true
    }

    // adjacent spans may have points close by
    pub(crate) fn spans_nearby(
        self,
        state: &SkOpGlobalState,
        ref_span: SkOpSpanBase,
        check: SkOpSpanBase,
        found: &mut bool,
    ) -> bool {
        let ref_head = ref_span.pt_t();
        let check_head = check.pt_t();
        // if the first pt pair from adjacent spans are far apart, assume that all are far enough
        // apart
        if !SkDPoint::way_roughly_equal(&state[ref_head].pt, &state[check_head].pt) {
            *found = false;
            return true;
        }
        // check only unique points
        let mut dist_sq_best = SK_SCALAR_MAX;
        let mut ref_best: Option<SkOpPtT> = None;
        let mut check_best: Option<SkOpPtT> = None;
        let mut r#ref = ref_head;
        'done_checking_distance: loop {
            'next_ref: {
                if r#ref.deleted(state) {
                    break 'next_ref;
                }
                while r#ref.pt_already_seen(state, ref_head) {
                    r#ref = r#ref.next(state);
                    if r#ref == ref_head {
                        break 'done_checking_distance;
                    }
                }
                let mut check = check_head;
                let ref_seg = r#ref.segment(state);
                let mut escape_hatch = 100; // defend against infinite loops
                loop {
                    'next_check: {
                        if check.deleted(state) {
                            break 'next_check;
                        }
                        while check.pt_already_seen(state, check_head) {
                            check = check.next(state);
                            if check == check_head {
                                break 'next_ref;
                            }
                        }
                        let dist_sq =
                            SkPointPriv::distance_to_sqd(&state[r#ref].pt, &state[check].pt);
                        if dist_sq_best > dist_sq
                            && (ref_seg != check.segment(state)
                                || !ref_seg.pts_disjoint_pt_t(state, r#ref, check))
                        {
                            dist_sq_best = dist_sq;
                            ref_best = Some(r#ref);
                            check_best = Some(check);
                        }
                        escape_hatch -= 1;
                        if escape_hatch <= 0 {
                            return false;
                        }
                    }
                    check = check.next(state);
                    if check == check_head {
                        break;
                    }
                }
            }
            // nextRef:
            r#ref = r#ref.next(state);
            if r#ref == ref_head {
                break;
            }
        }
        // doneCheckingDistance:
        *found = match (check_best, ref_best) {
            (Some(check_best), Some(ref_best)) => ref_best.segment(state).r#match(
                state,
                ref_best,
                check_best.segment(state),
                state[check_best].t,
                &state[check_best].pt,
            ),
            _ => false,
        };
        true
    }

    pub(crate) fn span_sign(
        state: &SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
    ) -> i32 {
        if start.t(state) < end.t(state) {
            -start.up_cast().wind_value(state)
        } else {
            end.up_cast().wind_value(state)
        }
    }

    pub(crate) fn span_to_angle(
        self,
        state: &SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
    ) -> Option<SkOpAngle> {
        debug_assert!(start != end);
        if start.t(state) < end.t(state) {
            start.up_cast().to_angle(state)
        } else {
            start.from_angle(state)
        }
    }

    pub(crate) fn sub_divide(
        self,
        state: &SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
        result: &mut SkDCurve,
    ) -> bool {
        debug_assert!(start != end);
        let start_pt_t = &state[start.pt_t()];
        let end_pt_t = &state[end.pt_t()];
        let data = &state[self];
        let edge = result;
        edge[0].set(&start_pt_t.pt);
        let points = sk_path_ops_verb_to_points(data.verb) as usize;
        edge[points].set(&end_pt_t.pt);
        if data.verb == SkPathVerb::Line {
            return false;
        }
        let start_t = start_pt_t.t;
        let end_t = end_pt_t.t;
        if (start_t == 0.0 || end_t == 0.0) && (start_t == 1.0 || end_t == 1.0) {
            // don't compute midpoints if we already have them
            if data.verb == SkPathVerb::Quad {
                edge[1].set(&data.pts[1]);
                return false;
            }
            if data.verb == SkPathVerb::Conic {
                edge[1].set(&data.pts[1]);
                edge.weight = data.weight;
                return false;
            }
            debug_assert!(data.verb == SkPathVerb::Cubic);
            if start_t == 0.0 {
                edge[1].set(&data.pts[1]);
                edge[2].set(&data.pts[2]);
                return false;
            }
            edge[1].set(&data.pts[2]);
            edge[2].set(&data.pts[1]);
            return false;
        }
        if data.verb == SkPathVerb::Quad {
            edge[1] =
                SkDQuad::sub_divide_pts_control(&data.pts, &edge[0], &edge[2], start_t, end_t);
        } else if data.verb == SkPathVerb::Conic {
            let (a, c) = (edge[0], edge[2]);
            edge[1] = SkDConic::sub_divide_pts_control(
                &data.pts,
                data.weight,
                &a,
                &c,
                start_t,
                end_t,
                &mut edge.weight,
            );
        } else {
            debug_assert!(data.verb == SkPathVerb::Cubic);
            let (a, d) = (edge[0], edge[3]);
            SkDCubic::sub_divide_pts_control(
                &data.pts,
                &a,
                &d,
                start_t,
                end_t,
                &mut edge.pts[1..3],
            );
        }
        true
    }

    pub(crate) fn tail(self, state: &SkOpGlobalState) -> SkOpSpanBase {
        state[self].tail
    }

    pub(crate) fn test_for_coincidence(
        self,
        state: &SkOpGlobalState,
        prior_pt_t: SkOpPtT,
        pt_t: SkOpPtT,
        prior: SkOpSpanBase,
        span_base: SkOpSpanBase,
        opp: SkOpSegment,
    ) -> bool {
        // average t, find mid pt
        let mid_t = (prior.t(state) + span_base.t(state)) / 2.0;
        let mid_pt = self.pt_at_t(state, mid_t);
        let mut coincident = true;
        // if the mid pt is not near either end pt, project perpendicular through opp seg
        if !SkDPoint::approximately_equal_sk_points(&state[prior_pt_t].pt, &mid_pt)
            && !SkDPoint::approximately_equal_sk_points(&state[pt_t].pt, &mid_pt)
        {
            if prior_pt_t.span(state) == pt_t.span(state) {
                return false;
            }
            coincident = false;
            let mut i = SkIntersections::new();
            let mut curve_part = SkDCurve::default();
            self.sub_divide(state, prior, span_base, &mut curve_part);
            let verb = state[self].verb as usize;
            let dxdy = CURVE_DD_SLOPE_AT_T[verb].expect("a segment's verb has a curve")(
                &curve_part,
                0.5f32 as f64,
            );
            let part_mid_pt = CURVE_DD_POINT_AT_T[verb].expect("a segment's verb has a curve")(
                &curve_part,
                0.5f32 as f64,
            );
            let ray = SkDLine {
                pts: [
                    SkDPoint {
                        x: mid_pt.x as f64,
                        y: mid_pt.y as f64,
                    },
                    SkDPoint {
                        x: part_mid_pt.x + dxdy.y,
                        y: part_mid_pt.y - dxdy.x,
                    },
                ],
            };
            let mut opp_part = SkDCurve::default();
            opp.sub_divide(
                state,
                prior_pt_t.span(state),
                pt_t.span(state),
                &mut opp_part,
            );
            CURVE_D_INTERSECT_RAY[state[opp].verb as usize].expect("a segment's verb has a curve")(
                &opp_part, &ray, &mut i,
            );
            // measure distance and see if it's small enough to denote coincidence
            for index in 0..i.used() as usize {
                if !between(0.0, i[0][index], 1.0) {
                    continue;
                }
                let opp_pt = *i.pt(index);
                if opp_pt.approximately_d_equal_sk_point(&mid_pt) {
                    // the coincidence can occur at almost any angle
                    coincident = true;
                }
            }
        }
        coincident
    }

    pub(crate) fn undone_span(self, state: &SkOpGlobalState) -> Option<SkOpSpan> {
        let mut span = state[self].head;
        loop {
            let next = span.next(state).expect("a span has a next span");
            if !span.done(state) {
                return Some(span);
            }
            if next.r#final(state) {
                break;
            }
            span = next.up_cast();
        }
        None
    }

    /// `updateOppWinding(const SkOpSpanBase* start, const SkOpSpanBase* end)`.
    pub(crate) fn update_opp_winding(
        self,
        state: &SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
    ) -> i32 {
        let lesser = start.starter(state, end);
        let mut opp_winding = lesser.opp_sum(state);
        let opp_span_winding = SkOpSegment::opp_sign(state, start, end);
        if opp_span_winding != 0
            && SkOpSegment::use_inner_winding(opp_winding - opp_span_winding, opp_winding)
            && opp_winding != SK_MAX_S32
        {
            opp_winding -= opp_span_winding;
        }
        opp_winding
    }

    /// `updateOppWinding(const SkOpAngle* angle)`.
    pub(crate) fn update_opp_winding_angle(self, state: &SkOpGlobalState, angle: SkOpAngle) -> i32 {
        let start_span = angle.start(state);
        let end_span = angle.end(state);
        self.update_opp_winding(state, end_span, start_span)
    }

    pub(crate) fn update_opp_winding_reverse(
        self,
        state: &SkOpGlobalState,
        angle: SkOpAngle,
    ) -> i32 {
        let start_span = angle.start(state);
        let end_span = angle.end(state);
        self.update_opp_winding(state, start_span, end_span)
    }

    /// `updateWinding(SkOpSpanBase* start, SkOpSpanBase* end)`: may compute the wind sum.
    pub(crate) fn update_winding(
        self,
        state: &mut SkOpGlobalState,
        start: SkOpSpanBase,
        end: SkOpSpanBase,
    ) -> i32 {
        let lesser = start.starter(state, end);
        let mut winding = lesser.wind_sum(state);
        if winding == SK_MIN_S32 {
            winding = lesser.compute_wind_sum(state);
        }
        if winding == SK_MIN_S32 {
            return winding;
        }
        let span_winding = SkOpSegment::span_sign(state, start, end);
        if winding != 0
            && SkOpSegment::use_inner_winding(winding - span_winding, winding)
            && winding != SK_MAX_S32
        {
            winding -= span_winding;
        }
        winding
    }

    /// `updateWinding(SkOpAngle* angle)`.
    pub(crate) fn update_winding_angle(self, state: &mut SkOpGlobalState, angle: SkOpAngle) -> i32 {
        let start_span = angle.start(state);
        let end_span = angle.end(state);
        self.update_winding(state, end_span, start_span)
    }

    pub(crate) fn update_winding_reverse(
        self,
        state: &mut SkOpGlobalState,
        angle: SkOpAngle,
    ) -> i32 {
        let start_span = angle.start(state);
        let end_span = angle.end(state);
        self.update_winding(state, start_span, end_span)
    }

    // OPTIMIZATION: does the following also work, and is it any faster?
    // return outerWinding * innerWinding > 0
    //      || ((outerWinding + innerWinding < 0) ^ ((outerWinding - innerWinding) < 0)))
    pub(crate) fn use_inner_winding(outer_winding: i32, inner_winding: i32) -> bool {
        debug_assert!(outer_winding != SK_MAX_S32);
        debug_assert!(inner_winding != SK_MAX_S32);
        let abs_out = sk_t_abs(outer_winding);
        let abs_in = sk_t_abs(inner_winding);
        if abs_out == abs_in {
            outer_winding < 0
        } else {
            abs_out < abs_in
        }
    }

    pub(crate) fn verb(self, state: &SkOpGlobalState) -> SkPathVerb {
        state[self].verb
    }

    // look for two different spans that point to the same opposite segment
    pub(crate) fn visited(self, state: &mut SkOpGlobalState) -> bool {
        if !state[self].visited {
            state[self].visited = true;
            return false;
        }
        true
    }

    pub(crate) fn weight(self, state: &SkOpGlobalState) -> f32 {
        state[self].weight
    }

    // `windingSpanAtT` is defined in SkPathOpsWinding.cpp: see `path_ops_winding.rs`.

    pub(crate) fn wind_sum(self, state: &SkOpGlobalState, angle: SkOpAngle) -> i32 {
        let min_span = angle.start(state).starter(state, angle.end(state));
        min_span.wind_sum(state)
    }
}
