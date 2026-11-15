//! Port of Skia's src/pathops/SkPathOpsSimplify.cpp: rewriting one path as non-overlapping
//! contours that fill the same area.

use crate::add_intersections::add_intersect_ts;
use crate::core::path::{SkPath, SkPathFillType, SkPathIter, SkPathVerb};
use crate::core::point::{SkPoint, SkVector};
use crate::op_coincidence::SkOpCoincidence;
use crate::op_contour::SkOpContourHead;
use crate::op_edge_builder::SkOpEdgeBuilder;
use crate::op_span::SkOpSpanBase;
use crate::path_ops_common::{find_chase, find_undone, handle_coincidence, sort_contour_list};
use crate::path_ops_types::{PathOpsError, SkOpGlobalState, SkPathOpsMask};
use crate::path_ops_winding::find_sortable_top;
use crate::path_writer::SkPathWriter;

fn bridge_winding(
    state: &mut SkOpGlobalState,
    contour_list: SkOpContourHead,
    writer: &mut SkPathWriter,
) -> bool {
    let mut unsortable = false;
    while let Some(span) = find_sortable_top(state, contour_list) {
        let mut current = span.segment(state);
        let mut start: SkOpSpanBase = span.next(state).expect("a sortable top has a next span");
        let mut end: SkOpSpanBase = SkOpSpanBase::from(span);
        let mut chase: Vec<SkOpSpanBase> = Vec::new();
        loop {
            if current.active_winding(state, start, end) {
                loop {
                    if !unsortable && current.done(state) {
                        break;
                    }
                    debug_assert!(unsortable || !current.done(state));
                    let mut next_start = Some(start);
                    let mut next_end = Some(end);
                    let Some(next) = current.find_next_winding(
                        state,
                        &mut chase,
                        &mut next_start,
                        &mut next_end,
                        &mut unsortable,
                    ) else {
                        break;
                    };
                    if !current.add_curve_to(state, start, end, writer) {
                        return false;
                    }
                    current = next;
                    start = next_start.expect("the next segment has a start");
                    end = next_end.expect("the next segment has an end");
                    if writer.is_closed(state)
                        || (unsortable && start.starter(state, end).done(state))
                    {
                        break;
                    }
                }
                if current.active_winding(state, start, end) && !writer.is_closed(state) {
                    let span_start = start.starter(state, end);
                    if !span_start.done(state) {
                        if !current.add_curve_to(state, start, end, writer) {
                            return false;
                        }
                        current.mark_done(state, span_start);
                    }
                }
                writer.finish_contour(state);
            } else {
                let mut last: Option<SkOpSpanBase> = None;
                if !current.mark_and_chase_done(state, start, end, Some(&mut last)) {
                    return false;
                }
                if let Some(last) = last {
                    if !last.chased(state) {
                        last.set_chased(state, true);
                        chase.push(last);
                    }
                }
            }
            let mut start_ptr = Some(start);
            let mut end_ptr = Some(end);
            let Some(next_current) = find_chase(state, &mut chase, &mut start_ptr, &mut end_ptr)
            else {
                break;
            };
            current = next_current;
            start = start_ptr.expect("a chased segment has a start");
            end = end_ptr.expect("a chased segment has an end");
        }
    }
    true
}

// returns true if all edges were processed
fn bridge_xor(
    state: &mut SkOpGlobalState,
    contour_list: SkOpContourHead,
    writer: &mut SkPathWriter,
) -> bool {
    let mut unsortable = false;
    let mut safety_net = 1000000;
    while let Some(span) = find_undone(state, contour_list) {
        let mut current = span.segment(state);
        let mut start: SkOpSpanBase = span.next(state).expect("an undone span has a next span");
        let mut end: SkOpSpanBase = SkOpSpanBase::from(span);
        loop {
            safety_net -= 1;
            if safety_net < 0 {
                return false;
            }
            if !unsortable && current.done(state) {
                break;
            }
            debug_assert!(unsortable || !current.done(state));
            let mut next_start = Some(start);
            let mut next_end = Some(end);
            let Some(next) =
                current.find_next_xor(state, &mut next_start, &mut next_end, &mut unsortable)
            else {
                break;
            };
            if !current.add_curve_to(state, start, end, writer) {
                return false;
            }
            current = next;
            start = next_start.expect("the next segment has a start");
            end = next_end.expect("the next segment has an end");
            if writer.is_closed(state) || (unsortable && start.starter(state, end).done(state)) {
                break;
            }
        }
        if !writer.is_closed(state) {
            let span_start = start.starter(state, end);
            if !span_start.done(state) {
                return false;
            }
        }
        writer.finish_contour(state);
    }
    true
}

/// `Trivializer`, the class local to `path_is_trivial`.
struct Trivializer {
    prev_pt: SkPoint,
    prev_vec: SkVector,
}

impl Trivializer {
    fn move_to(&mut self, curr_pt: &SkPoint) {
        self.prev_pt = *curr_pt;
        self.prev_vec = SkVector::new(0.0, 0.0);
    }

    fn add_trivial_contour_point(&mut self, curr_pt: &SkPoint) -> bool {
        if *curr_pt == self.prev_pt {
            return true;
        }
        // There are more numericaly stable ways of determining if many points are co-linear.
        // However, this mirrors SkPath's Convexicator for consistency.
        let curr_vec: SkVector = *curr_pt - self.prev_pt;
        if SkPoint::cross_product(&self.prev_vec, &curr_vec) != 0.0 {
            return false;
        }
        self.prev_vec = curr_vec;
        self.prev_pt = *curr_pt;
        true
    }
}

fn path_is_trivial(path: &SkPath) -> bool {
    let mut iter = SkPathIter::new(path, true);

    let mut triv = Trivializer {
        prev_pt: SkPoint::new(0.0, 0.0),
        prev_vec: SkVector::new(0.0, 0.0),
    };

    while let Some(rec) = iter.next() {
        let points = rec.points();
        // Skia's switch falls through from the cubic to the quad and conic, then to the line.
        match rec.verb {
            SkPathVerb::Move => {
                triv.move_to(&points[0]);
            }
            SkPathVerb::Cubic | SkPathVerb::Conic | SkPathVerb::Quad | SkPathVerb::Line => {
                if rec.verb == SkPathVerb::Cubic && !triv.add_trivial_contour_point(&points[3]) {
                    return false;
                }
                if rec.verb != SkPathVerb::Line && !triv.add_trivial_contour_point(&points[2]) {
                    return false;
                }
                if !triv.add_trivial_contour_point(&points[1]) {
                    return false;
                }
                if !triv.add_trivial_contour_point(&points[0]) {
                    return false;
                }
            }
            SkPathVerb::Close => {}
        }
    }
    true
}

/// `simplify` rewrites a path as contours that do not overlap, as Skia's `Simplify` does.
///
/// The result fills the same area as `path` under its own fill type, but with no
/// self-intersections and no overlapping contours, so it is filled with the even-odd rule, or
/// with the inverse even-odd rule when `path` has an inverse fill type. A convex path is
/// returned with its geometry unchanged, or empty when all its points lie on one line.
/// Simplifying fails with [`PathOpsError`] exactly where Skia's `Simplify` returns no path:
/// when a path that is not convex is not finite, or when Skia cannot resolve the curves'
/// intersections or windings.
pub fn simplify(path: &SkPath) -> Result<SkPath, PathOpsError> {
    simplify_debug(path).ok_or(PathOpsError)
}

/// `SimplifyDebug`, declared in SkPathOpsCommon.h, with Skia's debug-only parameters left out.
pub(crate) fn simplify_debug(path: &SkPath) -> Option<SkPath> {
    // returns 1 for evenodd, -1 for winding, regardless of inverse-ness
    let fill_type = if path.is_inverse_fill_type() {
        SkPathFillType::InverseEvenOdd
    } else {
        SkPathFillType::EvenOdd
    };

    if path.is_convex() {
        let mut result = SkPath::new();
        // If the path is trivially convex, simplify to empty, else copy
        if !path_is_trivial(path) {
            result = path.clone();
        }
        return Some(result.make_fill_type(fill_type));
    }
    // turn path into list of segments
    // FIXME: constant-ize, tune
    let mut global_state = SkOpGlobalState::new();
    let state = &mut global_state;
    let mut contour_list = state.contour_head();
    let coincidence = SkOpCoincidence::new(state);
    let mut builder = SkOpEdgeBuilder::new(path, contour_list);
    if !builder.finish(state) {
        return None;
    }
    if !sort_contour_list(state, &mut contour_list, false, false) {
        return Some(SkPath::new().make_fill_type(fill_type));
    }
    // find all intersections between segments
    let mut current = Some(contour_list.0);
    while let Some(test) = current {
        let mut next = Some(test);
        while let Some(other) = next {
            if !add_intersect_ts(state, test, other, coincidence) {
                break;
            }
            next = other.next(state);
        }
        current = test.next(state);
    }
    let success = handle_coincidence(state, contour_list, coincidence);
    if !success {
        return None;
    }
    // construct closed contours
    let mut wrapper = SkPathWriter::new(fill_type);
    let bridged = if builder.xor_mask() == SkPathOpsMask::Winding {
        bridge_winding(state, contour_list, &mut wrapper)
    } else {
        bridge_xor(state, contour_list, &mut wrapper)
    };
    if !bridged {
        return None;
    }
    wrapper.assemble(state); // if some edges could not be resolved, assemble remaining
    Some(wrapper.native_path())
}
