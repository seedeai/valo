//! Port of Skia's src/pathops/SkPathOpsWinding.cpp: computing a span's winding by casting a
//! ray from it and summing the windings of the edges it crosses.
//!
//! `SkOpRayDir` and `SkOpRayHit` are defined in this file but named by SkOpSegment.h and
//! SkOpContour.h, so their declarations are here with the methods those headers declare and
//! this file defines. Skia links the hits into a list allocated from a local arena, ending at
//! the stack-allocated `hitBase`; here the arena is a `Vec<SkOpRayHit>` the caller owns,
//! `fNext` is an index into it, and the list's head is an `Option<usize>`.

// given a prospective edge, compute its initial winding by projecting a ray
// if the ray hits another edge
// if the edge doesn't have a winding yet, hop up to that edge and start over
// concern : check for hops forming a loop
// if the edge is unsortable, or
// the intersection is nearly at the ends, or
// the tangent at the intersection is nearly coincident to the ray,
// choose a different ray and try again
// concern : if it is unable to succeed after N tries, try another edge? direction?
// if no edge is hit, compute the winding directly

// given the top span, project the most perpendicular ray and look for intersections
// let's try up and then down. What the hey

// bestXY is initialized by caller with basePt

use crate::core::float_bits::SK_MIN_S32;
use crate::core::path::SkPathVerb;
use crate::core::point::SkPoint;
use crate::core::rect::SkRect;
use crate::core::t_sort::sk_t_q_sort_by;
use crate::op_contour::{SkOpContour, SkOpContourHead};
use crate::op_segment::SkOpSegment;
use crate::op_span::{SkOpSpan, SkOpSpanBase};
use crate::path_ops_curve::CURVE_INTERCEPT;
use crate::path_ops_point::{SkDPoint, SkDVector};
use crate::path_ops_types::{
    approximately_between, approximately_equal, approximately_zero, between, roughly_equal,
    sk_path_ops_verb_to_points, SkOpGlobalState,
};

/// `SkOpRayDir`: the direction of the ray, whose value `& 1` is the axis it runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkOpRayDir {
    Left,
    Top,
    Right,
    Bottom,
}

impl SkOpRayDir {
    /// `static_cast<SkOpRayDir>(int)`.
    fn from_i32(value: i32) -> SkOpRayDir {
        match value {
            0 => SkOpRayDir::Left,
            1 => SkOpRayDir::Top,
            2 => SkOpRayDir::Right,
            3 => SkOpRayDir::Bottom,
            _ => unreachable!("SkOpRayDir has four values"),
        }
    }
}

fn xy_index(dir: SkOpRayDir) -> i32 {
    dir as i32 & 1
}

fn pt_xy(pt: &SkPoint, dir: SkOpRayDir) -> f32 {
    if xy_index(dir) == 0 {
        pt.x
    } else {
        pt.y
    }
}

fn pt_yx(pt: &SkPoint, dir: SkOpRayDir) -> f32 {
    if xy_index(dir) == 0 {
        pt.y
    } else {
        pt.x
    }
}

fn pt_dxdy(v: &SkDVector, dir: SkOpRayDir) -> f64 {
    if xy_index(dir) == 0 {
        v.x
    } else {
        v.y
    }
}

fn pt_dydx(v: &SkDVector, dir: SkOpRayDir) -> f64 {
    if xy_index(dir) == 0 {
        v.y
    } else {
        v.x
    }
}

/// `(&r.fLeft)[static_cast<int>(dir)]`.
fn rect_side(r: &SkRect, dir: SkOpRayDir) -> f32 {
    match dir {
        SkOpRayDir::Left => r.left,
        SkOpRayDir::Top => r.top,
        SkOpRayDir::Right => r.right,
        SkOpRayDir::Bottom => r.bottom,
    }
}

fn sideways_overlap(rect: &SkRect, pt: &SkPoint, dir: SkOpRayDir) -> bool {
    // `i = !xy_index(dir)` picks x (left, right) at 0 and y (top, bottom) at 1.
    if xy_index(dir) == 0 {
        approximately_between(rect.top as f64, pt.y as f64, rect.bottom as f64)
    } else {
        approximately_between(rect.left as f64, pt.x as f64, rect.right as f64)
    }
}

fn less_than(dir: SkOpRayDir) -> bool {
    (dir as i32 & 2) == 0
}

fn ccw_dxdy(v: &SkDVector, dir: SkOpRayDir) -> bool {
    let v_part_pos = pt_dydx(v, dir) > 0.0;
    let left_bottom = ((dir as i32 + 1) & 2) != 0;
    v_part_pos == left_bottom
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkOpRayHit {
    /// Index of the next hit in the hits arena.
    pub(crate) next: Option<usize>,
    pub(crate) span: Option<SkOpSpan>,
    pub(crate) pt: SkPoint,
    pub(crate) t: f64,
    pub(crate) slope: SkDVector,
    pub(crate) valid: bool,
}

impl SkOpRayHit {
    pub(crate) fn make_test_base(
        &mut self,
        state: &SkOpGlobalState,
        span: SkOpSpan,
        t: f64,
    ) -> SkOpRayDir {
        self.next = None;
        self.span = Some(span);
        let span_next = span.next(state).expect("a span has a next span");
        self.t = span.t(state) * (1.0 - t) + span_next.t(state) * t;
        let segment = span.segment(state);
        self.slope = segment.d_slope_at_t(state, self.t);
        self.pt = segment.pt_at_t(state, self.t);
        self.valid = true;
        if self.slope.x.abs() < self.slope.y.abs() {
            SkOpRayDir::Left
        } else {
            SkOpRayDir::Top
        }
    }
}

impl SkOpContour {
    pub(crate) fn ray_check(
        self,
        state: &SkOpGlobalState,
        base: &SkOpRayHit,
        dir: SkOpRayDir,
        hits: &mut Option<usize>,
        allocator: &mut Vec<SkOpRayHit>,
    ) {
        // if the bounds extreme is outside the best, we're done
        let base_xy = pt_xy(&base.pt, dir);
        let bounds_xy = rect_side(&state[self].bounds, dir);
        let check_less_than = less_than(dir);
        if !approximately_equal(base_xy as f64, bounds_xy as f64)
            && (base_xy < bounds_xy) == check_less_than
        {
            return;
        }
        let mut test_segment = Some(state[self].head);
        while let Some(segment) = test_segment {
            segment.ray_check(state, base, dir, hits, allocator);
            test_segment = segment.next(state);
        }
    }

    pub(crate) fn find_sortable_top(
        self,
        state: &mut SkOpGlobalState,
        contour_head: SkOpContour,
    ) -> Option<SkOpSpan> {
        let mut all_done = true;
        if state[self].count != 0 {
            let mut test_segment = Some(state[self].head);
            while let Some(segment) = test_segment {
                if !segment.done(state) {
                    all_done = false;
                    let result = segment.find_sortable_top(state, contour_head);
                    if result.is_some() {
                        return result;
                    }
                }
                test_segment = segment.next(state);
            }
        }
        if all_done {
            state[self].done = true;
        }
        None
    }
}

impl SkOpSegment {
    pub(crate) fn ray_check(
        self,
        state: &SkOpGlobalState,
        base: &SkOpRayHit,
        dir: SkOpRayDir,
        hits: &mut Option<usize>,
        allocator: &mut Vec<SkOpRayHit>,
    ) {
        if !sideways_overlap(&state[self].bounds, &base.pt, dir) {
            return;
        }
        let base_xy = pt_xy(&base.pt, dir);
        let bounds_xy = rect_side(&state[self].bounds, dir);
        let check_less_than = less_than(dir);
        if !approximately_equal(base_xy as f64, bounds_xy as f64)
            && (base_xy < bounds_xy) == check_less_than
        {
            return;
        }
        let mut t_vals = [0.0f64; 3];
        let base_yx = pt_yx(&base.pt, dir);
        let verb = state[self].verb;
        let intercept = CURVE_INTERCEPT[(verb as i32 * 2 + xy_index(dir)) as usize]
            .expect("a segment's verb has an intercept");
        let roots = intercept(&state[self].pts, state[self].weight, base_yx, &mut t_vals);
        let base_segment = base.span.map(|span| span.segment(state));
        for &t in &t_vals[..roots as usize] {
            if base_segment == Some(self) && approximately_equal(base.t, t) {
                continue;
            }
            let mut slope = SkDVector::default();
            let pt;
            let mut valid = false;
            if approximately_zero(t) {
                pt = state[self].pts[0];
            } else if approximately_equal(t, 1.0) {
                pt = state[self].pts[sk_path_ops_verb_to_points(verb) as usize];
            } else {
                debug_assert!(between(0.0, t, 1.0));
                pt = self.pt_at_t(state, t);
                if SkDPoint::approximately_equal_sk_points(&pt, &base.pt) {
                    if base_segment == Some(self) {
                        continue;
                    }
                } else {
                    let pt_xy = pt_xy(&pt, dir);
                    if !approximately_equal(base_xy as f64, pt_xy as f64)
                        && (base_xy < pt_xy) == check_less_than
                    {
                        continue;
                    }
                    slope = self.d_slope_at_t(state, t);
                    if verb == SkPathVerb::Cubic
                        && base_segment == Some(self)
                        && roughly_equal(base.t, t)
                        && SkDPoint::roughly_equal_sk_points(&pt, &base.pt)
                    {
                        continue;
                    }
                    if (pt_dydx(&slope, dir) * 10000.0).abs() > pt_dxdy(&slope, dir).abs() {
                        valid = true;
                    }
                }
            }
            let span = self.winding_span_at_t(state, t);
            match span {
                None => valid = false,
                Some(span) => {
                    if span.wind_value(state) == 0 && span.opp_value(state) == 0 {
                        continue;
                    }
                }
            }
            let new_hit = SkOpRayHit {
                next: *hits,
                pt,
                slope,
                span,
                t,
                valid,
            };
            allocator.push(new_hit);
            *hits = Some(allocator.len() - 1);
        }
    }

    pub(crate) fn winding_span_at_t(self, state: &SkOpGlobalState, t_hit: f64) -> Option<SkOpSpan> {
        let mut span = state[self].head;
        loop {
            let next = span.next(state).expect("a span has a next span");
            if approximately_equal(t_hit, next.t(state)) {
                return None;
            }
            if t_hit < next.t(state) {
                return Some(span);
            }
            if next.r#final(state) {
                return None;
            }
            span = next.up_cast();
        }
    }

    pub(crate) fn find_sortable_top(
        self,
        state: &mut SkOpGlobalState,
        contour_head: SkOpContour,
    ) -> Option<SkOpSpan> {
        let mut span = state[self].head;
        loop {
            let next = span.next(state).expect("a span has a next span");
            if !span.done(state) {
                if span.wind_sum(state) != SK_MIN_S32 {
                    return Some(span);
                }
                if span.sortable_top(state, contour_head) {
                    return Some(span);
                }
            }
            if next.r#final(state) {
                return None;
            }
            span = next.up_cast();
        }
    }
}

fn hit_compare_x(a: &SkOpRayHit, b: &SkOpRayHit) -> bool {
    a.pt.x < b.pt.x
}

fn reverse_hit_compare_x(a: &SkOpRayHit, b: &SkOpRayHit) -> bool {
    b.pt.x < a.pt.x
}

fn hit_compare_y(a: &SkOpRayHit, b: &SkOpRayHit) -> bool {
    a.pt.y < b.pt.y
}

fn reverse_hit_compare_y(a: &SkOpRayHit, b: &SkOpRayHit) -> bool {
    b.pt.y < a.pt.y
}

fn get_t_guess(mut t_try: i32, dir_offset: &mut i32) -> f64 {
    let mut t = 0.5;
    *dir_offset = t_try & 1;
    let t_base = t_try >> 1;
    let mut t_bits = 0;
    loop {
        t_try >>= 1;
        if t_try == 0 {
            break;
        }
        t /= 2.0;
        t_bits += 1;
    }
    if t_bits != 0 {
        let t_index = (t_base - 1) & ((1 << t_bits) - 1);
        t += t * 2.0 * t_index as f64;
    }
    t
}

impl SkOpSpan {
    pub(crate) fn sortable_top(
        self,
        state: &mut SkOpGlobalState,
        contour_head: SkOpContour,
    ) -> bool {
        let mut allocator: Vec<SkOpRayHit> = Vec::new();
        let mut dir_offset = 0;
        let t_try = state[self].top_t_try;
        state[self].top_t_try += 1;
        let t = get_t_guess(t_try, &mut dir_offset);
        let mut hit_base = SkOpRayHit::default();
        let mut dir = hit_base.make_test_base(state, self, t);
        if hit_base.slope.x == 0.0 && hit_base.slope.y == 0.0 {
            return false;
        }
        // `hitBase` is the list's last hit: index 0 of the arena.
        allocator.push(hit_base);
        let mut hit_head = Some(0);
        dir = SkOpRayDir::from_i32(dir as i32 + dir_offset);
        if let Some(base_span) = hit_base.span {
            if base_span.segment(state).verb(state) as i32 > SkPathVerb::Line as i32
                && pt_dydx(&hit_base.slope, dir) == 0.0
            {
                return false;
            }
        }
        let mut contour = Some(contour_head);
        while let Some(current) = contour {
            if current.count(state) != 0 {
                current.ray_check(state, &hit_base, dir, &mut hit_head, &mut allocator);
            }
            contour = current.next(state);
        }
        // sort hits
        let mut sorted: Vec<usize> = Vec::with_capacity(1);
        let mut hit = hit_head;
        while let Some(index) = hit {
            sorted.push(index);
            hit = allocator[index].next;
        }
        let count = sorted.len();
        let compare: fn(&SkOpRayHit, &SkOpRayHit) -> bool = if xy_index(dir) != 0 {
            if less_than(dir) {
                hit_compare_y
            } else {
                reverse_hit_compare_y
            }
        } else if less_than(dir) {
            hit_compare_x
        } else {
            reverse_hit_compare_x
        };
        sk_t_q_sort_by(&mut sorted, |a: &usize, b: &usize| {
            compare(&allocator[*a], &allocator[*b])
        });
        // verify windings
        let mut last: Option<SkPoint> = None;
        let mut wind = 0;
        let mut opp_wind = 0;
        for index in 0..count {
            let hit = allocator[sorted[index]];
            if !hit.valid {
                return false;
            }
            let ccw = ccw_dxdy(&hit.slope, dir);
            let Some(span) = hit.span else {
                return false;
            };
            let hit_segment = span.segment(state);
            if span.wind_value(state) == 0 && span.opp_value(state) == 0 {
                continue;
            }
            if let Some(last) = last {
                if SkDPoint::approximately_equal_sk_points(&last, &hit.pt) {
                    return false;
                }
            }
            if index < count - 1 {
                let next = allocator[sorted[index + 1]].pt;
                if SkDPoint::approximately_equal_sk_points(&next, &hit.pt) {
                    return false;
                }
            }
            let operand = hit_segment.operand(state);
            if operand {
                std::mem::swap(&mut wind, &mut opp_wind);
            }
            let last_wind = wind;
            let last_opp = opp_wind;
            let wind_value = if ccw {
                -span.wind_value(state)
            } else {
                span.wind_value(state)
            };
            let opp_value = if ccw {
                -span.opp_value(state)
            } else {
                span.opp_value(state)
            };
            wind += wind_value;
            opp_wind += opp_value;
            let mut sum_set = false;
            let span_sum = span.wind_sum(state);
            let wind_sum = if SkOpSegment::use_inner_winding(last_wind, wind) {
                wind
            } else {
                last_wind
            };
            if span_sum == SK_MIN_S32 {
                span.set_wind_sum(state, wind_sum);
                sum_set = true;
            }
            // else: the need for this condition suggests that UseInnerWinding is flawed
            // happened when last = 1 wind = -1
            let o_span_sum = span.opp_sum(state);
            let opp_sum = if SkOpSegment::use_inner_winding(last_opp, opp_wind) {
                opp_wind
            } else {
                last_opp
            };
            if o_span_sum == SK_MIN_S32 {
                span.set_opp_sum(state, opp_sum);
            }
            // Skia records the contour's direction here instead when its phase is
            // `kFixWinding`, which only `SkOpBuilder` (left out of this port) sets.
            if sum_set {
                let span_next = span.next(state).expect("a span has a next span");
                let _ = hit_segment.mark_and_chase_winding_opp(
                    state,
                    SkOpSpanBase::from(span),
                    span_next,
                    wind_sum,
                    opp_sum,
                    None,
                );
                let _ = hit_segment.mark_and_chase_winding_opp(
                    state,
                    span_next,
                    SkOpSpanBase::from(span),
                    wind_sum,
                    opp_sum,
                    None,
                );
            }
            if operand {
                std::mem::swap(&mut wind, &mut opp_wind);
            }
            last = Some(hit.pt);
        }
        true
    }
}

/// Declared in SkPathOpsCommon.h.
pub(crate) fn find_sortable_top(
    state: &mut SkOpGlobalState,
    contour_head: SkOpContourHead,
) -> Option<SkOpSpan> {
    for _ in 0..SkOpGlobalState::K_MAX_WINDING_TRIES {
        let mut contour = Some(contour_head.0);
        while let Some(current) = contour {
            if !current.done(state) {
                let result = current.find_sortable_top(state, contour_head.0);
                if result.is_some() {
                    return result;
                }
            }
            contour = current.next(state);
        }
    }
    None
}
