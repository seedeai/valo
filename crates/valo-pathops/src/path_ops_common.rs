//! Port of Skia's src/pathops/SkPathOpsCommon.{h,cpp}: the steps `Op` and `Simplify` share,
//! from sorting the contours to resolving coincidence and chasing the spans the walk left.
//!
//! The header also declares `FindSortableTop` (defined in SkPathOpsWinding.cpp, so in
//! `path_ops_winding.rs`), `OpDebug` and `SimplifyDebug` (in `path_ops_op.rs` and
//! `path_ops_simplify.rs`); `FixWinding` and `ComputeTightBounds` belong to files this port
//! leaves out.

use crate::core::float_bits::SK_MIN_S32;
use crate::core::t_sort::sk_t_q_sort_by;
use crate::op_angle::SkOpAngle;
use crate::op_coincidence::SkOpCoincidence;
use crate::op_contour::{SkOpContour, SkOpContourHead};
use crate::op_segment::SkOpSegment;
use crate::op_span::{SkOpSpan, SkOpSpanBase};
use crate::path_ops_types::SkOpGlobalState;

/// May compute wind sums on the way.
pub(crate) fn angle_winding(
    state: &mut SkOpGlobalState,
    start: SkOpSpanBase,
    end: SkOpSpanBase,
    winding_ptr: &mut i32,
    sortable: &mut bool,
) -> Option<SkOpAngle> {
    // find first angle, initialize winding to computed fWindSum
    let segment = start.segment(state);
    let Some(mut angle) = segment.span_to_angle(state, start, end) else {
        *winding_ptr = SK_MIN_S32;
        return None;
    };
    let mut compute_winding;
    let mut first_angle = angle;
    let mut looped = false;
    let mut unorderable = false;
    let mut winding = SK_MIN_S32;
    loop {
        angle = angle.next(state)?;
        unorderable |= angle.unorderable(state);
        compute_winding = unorderable || (angle == first_angle && looped);
        if compute_winding {
            break; // if we get here, there's no winding, loop is unorderable
        }
        looped |= angle == first_angle;
        let segment = angle.segment(state);
        winding = segment.wind_sum(state, angle);
        if winding != SK_MIN_S32 {
            break;
        }
    }
    // if the angle loop contains an unorderable span, the angle order may be useless
    // directly compute the winding in this case for each span
    if compute_winding {
        first_angle = angle;
        winding = SK_MIN_S32;
        loop {
            let start_span = angle.start(state);
            let end_span = angle.end(state);
            let lesser = start_span.starter(state, end_span);
            let mut test_winding = lesser.wind_sum(state);
            if test_winding == SK_MIN_S32 {
                test_winding = lesser.compute_wind_sum(state);
            }
            if test_winding != SK_MIN_S32 {
                winding = test_winding;
            }
            angle = angle.next(state).expect("an angle loop is a ring");
            if angle == first_angle {
                break;
            }
        }
    }
    *sortable = !unorderable;
    *winding_ptr = winding;
    Some(angle)
}

pub(crate) fn find_chase(
    state: &mut SkOpGlobalState,
    chase: &mut Vec<SkOpSpanBase>,
    start_ptr: &mut Option<SkOpSpanBase>,
    end_ptr: &mut Option<SkOpSpanBase>,
) -> Option<SkOpSegment> {
    while let Some(span) = chase.pop() {
        let mut segment = span.segment(state);
        let start = span.pt_t().next(state).span(state);
        *start_ptr = Some(start);
        let mut done = true;
        *end_ptr = None;
        if let Some(last) = segment.active_angle(state, start, start_ptr, end_ptr, &mut done) {
            *start_ptr = Some(last.start(state));
            *end_ptr = Some(last.end(state));
            chase.push(span);
            return Some(last.segment(state));
        }
        if done {
            continue;
        }
        // find first angle, initialize winding to computed wind sum
        let mut winding = 0;
        let mut sortable = false;
        let mut angle = angle_winding(
            state,
            start_ptr.expect("a chased span has a start"),
            end_ptr.expect("an undone chased span has an end"),
            &mut winding,
            &mut sortable,
        )?;
        if winding == SK_MIN_S32 {
            continue;
        }
        let mut sum_winding = 0;
        if sortable {
            segment = angle.segment(state);
            sum_winding = segment.update_winding_reverse(state, angle);
        }
        let mut first: Option<SkOpSegment> = None;
        let first_angle = angle;
        loop {
            angle = angle.next(state).expect("an angle loop is a ring");
            if angle == first_angle {
                break;
            }
            segment = angle.segment(state);
            let start = angle.start(state);
            let end = angle.end(state);
            let mut max_winding = 0;
            if sortable {
                segment.set_up_winding(state, start, end, &mut max_winding, &mut sum_winding);
            }
            if !segment.done_angle(state, angle) {
                if first.is_none()
                    && (sortable || start.starter(state, end).wind_sum(state) != SK_MIN_S32)
                {
                    first = Some(segment);
                    *start_ptr = Some(start);
                    *end_ptr = Some(end);
                }
                // OPTIMIZATION: should this also add to the chase?
                if sortable {
                    // TODO: add error handling
                    let marked = segment.mark_angle(state, max_winding, sum_winding, angle, None);
                    debug_assert!(marked);
                }
            }
        }
        if let Some(first) = first {
            chase.push(span);
            return Some(first);
        }
    }
    None
}

/// Marks a contour done when it has no undone span.
pub(crate) fn find_undone(
    state: &mut SkOpGlobalState,
    contour_head: SkOpContourHead,
) -> Option<SkOpSpan> {
    let mut contour = Some(contour_head.0);
    while let Some(current) = contour {
        if !current.done(state) {
            let result = current.undone_span(state);
            if result.is_some() {
                return result;
            }
        }
        contour = current.next(state);
    }
    None
}

/// Skia's `SkOpContourHead**` is the `&mut` head: the sorted list's first contour replaces it.
pub(crate) fn sort_contour_list(
    state: &mut SkOpGlobalState,
    contour_list: &mut SkOpContourHead,
    even_odd: bool,
    opp_even_odd: bool,
) -> bool {
    let mut list: Vec<SkOpContour> = Vec::new();
    let mut contour = Some(contour_list.0);
    while let Some(current) = contour {
        if current.count(state) != 0 {
            let opp_xor = if current.operand(state) {
                even_odd
            } else {
                opp_even_odd
            };
            current.set_opp_xor(state, opp_xor);
            list.push(current);
        }
        contour = current.next(state);
    }
    let count = list.len();
    if count == 0 {
        return false;
    }
    if count > 1 {
        let state: &SkOpGlobalState = state;
        sk_t_q_sort_by(&mut list, |a: &SkOpContour, b: &SkOpContour| {
            a.less_than(state, *b)
        });
    }
    let mut contour = list[0];
    let contour_head = SkOpContourHead(contour);
    state.set_contour_head(contour_head);
    *contour_list = contour_head;
    for &next in &list[1..count] {
        contour.set_next(state, Some(next));
        contour = next;
    }
    contour.set_next(state, None);
    true
}

fn calc_angles(state: &mut SkOpGlobalState, contour_list: SkOpContourHead) {
    let mut contour = Some(contour_list.0);
    while let Some(current) = contour {
        current.calc_angles(state);
        contour = current.next(state);
    }
}

fn missing_coincidence(state: &mut SkOpGlobalState, contour_list: SkOpContourHead) -> bool {
    let mut contour = Some(contour_list.0);
    let mut result = false;
    while let Some(current) = contour {
        result |= current.missing_coincidence(state);
        contour = current.next(state);
    }
    result
}

fn move_multiples(state: &mut SkOpGlobalState, contour_list: SkOpContourHead) -> bool {
    let mut contour = Some(contour_list.0);
    while let Some(current) = contour {
        if !current.move_multiples(state) {
            return false;
        }
        contour = current.next(state);
    }
    true
}

fn move_nearby(state: &mut SkOpGlobalState, contour_list: SkOpContourHead) -> bool {
    let mut contour = Some(contour_list.0);
    while let Some(current) = contour {
        if !current.move_nearby(state) {
            return false;
        }
        contour = current.next(state);
    }
    true
}

fn sort_angles(state: &mut SkOpGlobalState, contour_list: SkOpContourHead) -> bool {
    let mut contour = Some(contour_list.0);
    while let Some(current) = contour {
        if !current.sort_angles(state) {
            return false;
        }
        contour = current.next(state);
    }
    true
}

pub(crate) fn handle_coincidence(
    state: &mut SkOpGlobalState,
    contour_list: SkOpContourHead,
    coincidence: SkOpCoincidence,
) -> bool {
    // match up points within the coincident runs
    if !coincidence.add_expanded(state) {
        return false;
    }
    // combine t values when multiple intersections occur on some segments but not others
    if !move_multiples(state, contour_list) {
        return false;
    }
    // move t values and points together to eliminate small/tiny gaps
    if !move_nearby(state, contour_list) {
        return false;
    }
    // add coincidence formed by pairing on curve points and endpoints
    coincidence.correct_ends(state);
    if !coincidence.add_end_moved_spans(state) {
        return false;
    }
    const SAFETY_COUNT: i32 = 3;
    let mut safety_hatch = SAFETY_COUNT;
    // look for coincidence present in A-B and A-C but missing in B-C
    loop {
        let mut added = false;
        if !coincidence.add_missing(state, &mut added) {
            return false;
        }
        if !added {
            break;
        }
        safety_hatch -= 1;
        if safety_hatch == 0 {
            return false;
        }
        move_nearby(state, contour_list);
    }
    // check to see if, loosely, coincident ranges may be expanded
    if coincidence.expand(state) {
        let mut added = false;
        if !coincidence.add_missing(state, &mut added) {
            return false;
        }
        if !coincidence.add_expanded(state) {
            return false;
        }
        if !move_multiples(state, contour_list) {
            return false;
        }
        move_nearby(state, contour_list);
    }
    // the expanded ranges may not align -- add the missing spans
    if !coincidence.add_expanded(state) {
        return false;
    }
    // mark spans of coincident segments as coincident
    coincidence.mark(state);
    // look for coincidence lines and curves undetected by intersection
    if missing_coincidence(state, contour_list) {
        let _ = coincidence.expand(state);
        if !coincidence.add_expanded(state) {
            return false;
        }
        if !coincidence.mark(state) {
            return false;
        }
    } else {
        let _ = coincidence.expand(state);
    }
    let _ = coincidence.expand(state);

    let overlaps = SkOpCoincidence::new(state);
    safety_hatch = SAFETY_COUNT;
    loop {
        let pairs = if overlaps.is_empty(state) {
            coincidence
        } else {
            overlaps
        };
        // adjust the winding value to account for coincident edges
        if !pairs.apply(state) {
            return false;
        }
        // For each coincident pair that overlaps another, when the receivers (the 1st of the pair)
        // are different, construct a new pair to resolve their mutual span
        if !pairs.find_overlaps(state, overlaps) {
            return false;
        }
        safety_hatch -= 1;
        if safety_hatch == 0 {
            return false;
        }
        if overlaps.is_empty(state) {
            break;
        }
    }
    calc_angles(state, contour_list);
    sort_angles(state, contour_list)
}
