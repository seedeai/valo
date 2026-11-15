//! Port of Skia's src/pathops/SkPathWriter.{h,cpp}: building the result path from the
//! segments the walk keeps, one contour at a time.

use crate::core::float_bits::SK_MAX_S32;
use crate::core::path::{SkPath, SkPathAddPathMode, SkPathFillType, SkPathPriv};
use crate::core::path_builder::{SkPathBuilder, SkPathBuilderReserve};
use crate::core::point::{SkPoint, SkVector};
use crate::core::t_sort::sk_t_q_sort_by;
use crate::op_span::{SkOpPtT, SkOpSpanBase};
use crate::path_ops_types::{zero_or_one, SkOpGlobalState};

// Construct the path one contour at a time.
// If the contour is closed, copy it to the final output.
// Otherwise, keep the partial contour for later assembly.

pub(crate) struct SkPathWriter {
    pub(crate) builder: SkPathBuilder,
    /// contour under construction
    pub(crate) current: SkPathBuilder,
    /// contours with mismatched starts and ends
    pub(crate) partials: Vec<SkPathBuilder>,
    /// possible pt values for partial starts and ends
    pub(crate) end_pt_ts: Vec<SkOpPtT>,
    /// [0] deferred move, [1] deferred line
    pub(crate) defer: [Option<SkOpPtT>; 2],
    /// first in current contour
    pub(crate) first_pt_t: Option<SkOpPtT>,
}

impl SkPathWriter {
    pub(crate) fn new(fill_type: SkPathFillType) -> SkPathWriter {
        let mut writer = SkPathWriter {
            builder: SkPathBuilder::with_fill_type(fill_type),
            current: SkPathBuilder::new(),
            partials: Vec::new(),
            end_pt_ts: Vec::new(),
            defer: [None, None],
            first_pt_t: None,
        };
        writer.init();
        writer
    }

    /*
        check start and end of each contour
        if not the same, record them
        match them up
        connect closest
        reassemble contour pieces into new path
    */
    /// Lengthening partial contours follows their segments, marking the curves it adds.
    pub(crate) fn assemble(&mut self, state: &mut SkOpGlobalState) {
        if !self.some_assembly_required(state) {
            return;
        }
        // starts, ends of partial contours: `self.end_pt_ts`
        let end_count = self.end_pt_ts.len() as i32; // all starts and ends
        debug_assert!(end_count > 0);
        debug_assert!(end_count == self.partials.len() as i32 * 2);

        // Limit the number of partial contours to avoid O(N^2) complexity and integer overflows.
        // 10,000 partial contours results in 20,000 ends and ~200,000,000 distance entries.
        const K_MAX_PARTIAL_CONTOURS: i32 = 10000;
        if end_count > K_MAX_PARTIAL_CONTOURS * 2 {
            return;
        }

        // lengthen any partial contour adjacent to a simple segment
        for p_index in 0..end_count {
            let mut op_pt_t = self.end_pt_ts[p_index as usize];
            let mut part_writer = SkPathWriter::new(SkPathFillType::Winding);
            loop {
                if !zero_or_one(state[op_pt_t].t) {
                    break;
                }
                let op_span_base = op_pt_t.span(state);
                let mut start: Option<SkOpSpanBase> = if state[op_pt_t].t != 0.0 {
                    op_span_base.prev(state).map(SkOpSpanBase::from)
                } else {
                    op_span_base.up_cast().next(state)
                };
                let mut step = if state[op_pt_t].t != 0.0 { 1 } else { -1 };
                let op_segment = op_span_base.segment(state);
                let Some(next_segment) = op_segment.is_simple(state, &mut start, &mut step) else {
                    break;
                };
                let start = start.expect("a simple segment's chase ends at a span");
                let op_span_end: SkOpSpanBase = if start.t(state) != 0.0 {
                    start
                        .prev(state)
                        .map(SkOpSpanBase::from)
                        .expect("a span past t=0 has a previous span")
                } else {
                    start
                        .up_cast()
                        .next(state)
                        .expect("the span at t=0 has a next span")
                };
                if start.starter(state, op_span_end).already_added(state) {
                    break;
                }
                let _ = next_segment.add_curve_to(state, start, op_span_end, &mut part_writer);
                op_pt_t = op_span_end.pt_t();
                self.end_pt_ts[p_index as usize] = op_pt_t;
            }
            part_writer.finish_contour(state);
            let part_partials = &part_writer.partials;
            if part_partials.is_empty() {
                continue;
            }
            // if pIndex is even, reverse and prepend to fPartials; otherwise, append
            let partial = &mut self.partials[(p_index >> 1) as usize];
            let part = part_partials[0].snapshot();
            if p_index & 1 != 0 {
                partial.add_path(&part, SkPathAddPathMode::Extend);
            } else {
                let mut reverse = SkPathBuilder::new();
                SkPathPriv::reverse_add_path(&mut reverse, &part);
                reverse.add_path(&partial.detach(), SkPathAddPathMode::Extend);
                *partial = reverse;
            }
        }
        let link_count = end_count / 2; // number of partial contours
        let mut s_link: Vec<i32> = Vec::with_capacity(link_count as usize);
        let mut e_link: Vec<i32> = Vec::with_capacity(link_count as usize);
        for _ in 0..link_count {
            s_link.push(SK_MAX_S32);
            e_link.push(SK_MAX_S32);
        }
        let entries = end_count * (end_count - 1) / 2; // folded triangle
        let mut distances: Vec<f64> = Vec::with_capacity(entries as usize);
        let mut sorted_dist: Vec<i32> = Vec::with_capacity(entries as usize);
        let mut dist_lookup: Vec<i32> = Vec::with_capacity(entries as usize);
        let mut r_row = 0;
        let mut d_index = 0;
        for r_index in 0..end_count - 1 {
            let o_pt_t = self.end_pt_ts[r_index as usize];
            for i_index in r_index + 1..end_count {
                let i_pt_t = self.end_pt_ts[i_index as usize];
                // Skia subtracts the floats, then widens the difference.
                let dx = (state[i_pt_t].pt.x - state[o_pt_t].pt.x) as f64;
                let dy = (state[i_pt_t].pt.y - state[o_pt_t].pt.y) as f64;
                let dist = dx * dx + dy * dy;
                dist_lookup.push(r_row + i_index);
                distances.push(dist); // oStart distance from iStart
                sorted_dist.push(d_index);
                d_index += 1;
            }
            r_row += end_count;
        }
        debug_assert!(d_index == entries);
        sk_t_q_sort_by(&mut sorted_dist, |one: &i32, two: &i32| {
            distances[*one as usize] < distances[*two as usize]
        });
        let mut remaining = link_count; // number of start/end pairs
        for r_index in 0..entries {
            let mut pair = sorted_dist[r_index as usize];
            pair = dist_lookup[pair as usize];
            let row = pair / end_count;
            let col = pair - row * end_count;
            let ndx_one = row >> 1;
            let end_one = (row & 1) != 0;
            if links(end_one, &mut s_link, &mut e_link)[ndx_one as usize] != SK_MAX_S32 {
                continue;
            }
            let ndx_two = col >> 1;
            let end_two = (col & 1) != 0;
            if links(end_two, &mut s_link, &mut e_link)[ndx_two as usize] != SK_MAX_S32 {
                continue;
            }
            debug_assert!(end_one != end_two || ndx_one != ndx_two);
            let flip = end_one == end_two;
            links(end_one, &mut s_link, &mut e_link)[ndx_one as usize] =
                if flip { !ndx_two } else { ndx_two };
            links(end_two, &mut s_link, &mut e_link)[ndx_two as usize] =
                if flip { !ndx_one } else { ndx_one };
            remaining -= 1;
            if remaining == 0 {
                break;
            }
        }
        debug_assert!(remaining == 0);
        let mut r_index: i32 = 0;
        loop {
            let mut forward = true;
            let mut first = true;
            let s_index = s_link[r_index as usize];
            debug_assert!(s_index != SK_MAX_S32);
            s_link[r_index as usize] = SK_MAX_S32;
            let mut e_index;
            if s_index < 0 {
                e_index = s_link[!s_index as usize];
                s_link[!s_index as usize] = SK_MAX_S32;
            } else {
                e_index = e_link[s_index as usize];
                e_link[s_index as usize] = SK_MAX_S32;
            }
            debug_assert!(e_index != SK_MAX_S32);
            loop {
                let contour = self.partials[r_index as usize].snapshot();
                if !first {
                    let Some(prior) = self.builder.get_last_pt() else {
                        return;
                    };
                    let contour_pts = contour.points();
                    let next = if forward {
                        contour_pts
                            .first()
                            .copied()
                            .unwrap_or(SkPoint::new(0.0, 0.0))
                    } else {
                        let Some(&back) = contour_pts.last() else {
                            return;
                        };
                        back
                    };
                    // TODO: if there is a gap between open path written so far and path to come,
                    // connect by following segments from one to the other, rather than
                    // introducing a diagonal to connect the two. (Skia's `if (*prior != next)`
                    // has only this comment for a body.)
                    let _ = (prior, next);
                }
                if forward {
                    self.builder.add_path(
                        &contour,
                        if first {
                            SkPathAddPathMode::Append
                        } else {
                            SkPathAddPathMode::Extend
                        },
                    );
                } else {
                    debug_assert!(!first);
                    SkPathPriv::reverse_path_to(&mut self.builder, &contour);
                }
                if first {
                    first = false;
                }
                if s_index
                    == (if (r_index != e_index) ^ forward {
                        e_index
                    } else {
                        !e_index
                    })
                {
                    self.builder.close();
                    break;
                }
                if forward {
                    e_index = e_link[r_index as usize];
                    debug_assert!(e_index != SK_MAX_S32);
                    e_link[r_index as usize] = SK_MAX_S32;
                    if e_index >= 0 {
                        debug_assert!(s_link[e_index as usize] == r_index);
                        s_link[e_index as usize] = SK_MAX_S32;
                    } else {
                        debug_assert!(e_link[!e_index as usize] == !r_index);
                        e_link[!e_index as usize] = SK_MAX_S32;
                    }
                } else {
                    e_index = s_link[r_index as usize];
                    debug_assert!(e_index != SK_MAX_S32);
                    s_link[r_index as usize] = SK_MAX_S32;
                    if e_index >= 0 {
                        debug_assert!(e_link[e_index as usize] == r_index);
                        e_link[e_index as usize] = SK_MAX_S32;
                    } else {
                        debug_assert!(s_link[!e_index as usize] == !r_index);
                        s_link[!e_index as usize] = SK_MAX_S32;
                    }
                }
                r_index = e_index;
                if r_index < 0 {
                    forward ^= true;
                    r_index = !r_index;
                }
            }
            r_index = 0;
            while r_index < link_count {
                if s_link[r_index as usize] != SK_MAX_S32 {
                    break;
                }
                r_index += 1;
            }
            if r_index >= link_count {
                break;
            }
        }
    }

    pub(crate) fn conic_to(
        &mut self,
        state: &SkOpGlobalState,
        pt1: &SkPoint,
        pt2: SkOpPtT,
        weight: f32,
    ) {
        let pt2pt = self.update(state, pt2);
        self.current.conic_to(*pt1, pt2pt, weight);
    }

    pub(crate) fn cubic_to(
        &mut self,
        state: &SkOpGlobalState,
        pt1: &SkPoint,
        pt2: &SkPoint,
        pt3: SkOpPtT,
    ) {
        let pt3pt = self.update(state, pt3);
        self.current.cubic_to(*pt1, *pt2, pt3pt);
    }

    pub(crate) fn deferred_line(&mut self, state: &SkOpGlobalState, pt: SkOpPtT) -> bool {
        debug_assert!(self.first_pt_t.is_some());
        let defer0 = self.defer[0].expect("a deferred line follows a deferred move");
        if defer0 == pt {
            // FIXME: why we're adding a degenerate line? Caller should have preflighted this.
            return true;
        }
        if pt.contains_pt_t(state, defer0) {
            // FIXME: why we're adding a degenerate line?
            return true;
        }
        if self.matched_last(state, Some(pt)) {
            return false;
        }
        if self.defer[1].is_some() && self.changed_slopes(state, pt) {
            self.line_to(state);
            self.defer[0] = self.defer[1];
        }
        self.defer[1] = Some(pt);
        true
    }

    pub(crate) fn deferred_move(&mut self, state: &SkOpGlobalState, pt: SkOpPtT) {
        if self.defer[1].is_none() {
            self.defer[0] = Some(pt);
            self.first_pt_t = Some(pt);
            return;
        }
        debug_assert!(self.defer[0].is_some());
        if !self.matched_last(state, Some(pt)) {
            self.finish_contour(state);
            self.defer[0] = Some(pt);
            self.first_pt_t = Some(pt);
        }
    }

    pub(crate) fn finish_contour(&mut self, state: &SkOpGlobalState) {
        if !self.matched_last(state, self.defer[0]) {
            if self.defer[1].is_none() {
                return;
            }
            self.line_to(state);
        }
        if self.current.is_empty() {
            return;
        }
        if self.is_closed(state) {
            self.close(state);
        } else {
            debug_assert!(self.defer[1].is_some());
            self.end_pt_ts
                .push(self.first_pt_t.expect("a partial contour has a first pt-t"));
            self.end_pt_ts
                .push(self.defer[1].expect("a partial contour has a last pt-t"));
            self.partials.push(self.current.clone());
            self.init();
        }
    }

    pub(crate) fn has_move(&self) -> bool {
        self.first_pt_t.is_none()
    }

    pub(crate) fn init(&mut self) {
        self.current.reset();
        self.first_pt_t = None;
        self.defer[0] = None;
        self.defer[1] = None;
    }

    pub(crate) fn is_closed(&self, state: &SkOpGlobalState) -> bool {
        self.matched_last(state, self.first_pt_t)
    }

    pub(crate) fn native_path(&mut self) -> SkPath {
        self.builder.detach()
    }

    pub(crate) fn quad_to(&mut self, state: &SkOpGlobalState, pt1: &SkPoint, pt2: SkOpPtT) {
        let pt2pt = self.update(state, pt2);
        self.current.quad_to(*pt1, pt2pt);
    }

    pub(crate) fn changed_slopes(&self, state: &SkOpGlobalState, pt: SkOpPtT) -> bool {
        if self.matched_last(state, self.defer[0]) {
            return false;
        }
        let defer0 = self.defer[0].expect("a deferred line has a deferred move");
        let defer1 = self.defer[1].expect("changed slopes follows a deferred line");
        let defer_dxdy: SkVector = state[defer1].pt - state[defer0].pt;
        let line_dxdy: SkVector = state[pt].pt - state[defer1].pt;
        defer_dxdy.x * line_dxdy.y != defer_dxdy.y * line_dxdy.x
    }

    /// Skia asserts `isClosed()`, which reads the pt-ts.
    pub(crate) fn close(&mut self, state: &SkOpGlobalState) {
        if self.current.is_empty() {
            return;
        }
        debug_assert!(self.is_closed(state));
        self.current.close();
        if let Some(raw) = SkPathPriv::raw_builder(&self.current) {
            self.builder.add_raw(&raw, SkPathBuilderReserve::Exact);
        }
        self.init();
    }

    pub(crate) fn line_to(&mut self, state: &SkOpGlobalState) {
        if self.current.is_empty() {
            self.move_to(state);
        }
        let defer1 = self.defer[1].expect("a line goes to the deferred line's end");
        self.current.line_to(state[defer1].pt);
    }

    /// `test` may be null in Skia.
    pub(crate) fn matched_last(&self, state: &SkOpGlobalState, test: Option<SkOpPtT>) -> bool {
        if test == self.defer[1] {
            return true;
        }
        let Some(test) = test else {
            return false;
        };
        let Some(defer1) = self.defer[1] else {
            return false;
        };
        test.contains_pt_t(state, defer1)
    }

    pub(crate) fn move_to(&mut self, state: &SkOpGlobalState) {
        let first_pt_t = self.first_pt_t.expect("a contour moves to its first pt-t");
        self.current.move_to(state[first_pt_t].pt);
    }

    pub(crate) fn some_assembly_required(&mut self, state: &SkOpGlobalState) -> bool {
        self.finish_contour(state);
        !self.end_pt_ts.is_empty()
    }

    // if last point to be written matches the current path's first point, alter the
    // last to avoid writing a degenerate lineTo when the path is closed
    pub(crate) fn update(&mut self, state: &SkOpGlobalState, pt: SkOpPtT) -> SkPoint {
        if self.defer[1].is_none() {
            self.move_to(state);
        } else if !self.matched_last(state, self.defer[0]) {
            self.line_to(state);
        }
        let mut result = state[pt].pt;
        if let Some(first_pt_t) = self.first_pt_t {
            if result != state[first_pt_t].pt && first_pt_t.contains_pt_t(state, pt) {
                result = state[first_pt_t].pt;
            }
        }
        // set both to know that there is not a pending deferred line
        self.defer[0] = Some(pt);
        self.defer[1] = Some(pt);
        result
    }
}

/// `int* linkOne = endOne ? eLink.begin() : sLink.begin();`: the link array an end indexes.
fn links<'a>(end: bool, s_link: &'a mut [i32], e_link: &'a mut [i32]) -> &'a mut [i32] {
    if end {
        e_link
    } else {
        s_link
    }
}
