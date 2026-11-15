//! Port of Skia's src/pathops/SkPathOpsOp.cpp: the boolean operation on two paths.

use crate::add_intersections::add_intersect_ts;
use crate::core::float_bits::SK_MIN_S32;
use crate::core::path::{SkPath, SkPathFillType, SkPathVerb};
use crate::core::rect::SkRect;
use crate::op_coincidence::SkOpCoincidence;
use crate::op_contour::SkOpContourHead;
use crate::op_edge_builder::SkOpEdgeBuilder;
use crate::op_segment::SkOpSegment;
use crate::op_span::SkOpSpanBase;
use crate::path_ops_common::{angle_winding, handle_coincidence, sort_contour_list};
use crate::path_ops_simplify::simplify_debug;
use crate::path_ops_types::{PathOpsError, SkOpGlobalState, SkPathOp, SkPathOpsMask};
use crate::path_ops_winding::find_sortable_top;
use crate::path_writer::SkPathWriter;

fn find_chase_op(
    state: &mut SkOpGlobalState,
    chase: &mut Vec<SkOpSpanBase>,
    start_ptr: &mut Option<SkOpSpanBase>,
    end_ptr: &mut Option<SkOpSpanBase>,
    result: &mut Option<SkOpSegment>,
) -> bool {
    while let Some(span) = chase.pop() {
        // OPTIMIZE: prev makes this compatible with old code -- but is it necessary?
        let start = span.pt_t().prev(state).span(state);
        *start_ptr = Some(start);
        let mut segment = start.segment(state);
        let mut done = true;
        *end_ptr = None;
        if let Some(last) = segment.active_angle(state, start, start_ptr, end_ptr, &mut done) {
            *start_ptr = Some(last.start(state));
            *end_ptr = Some(last.end(state));
            chase.push(span);
            *result = Some(last.segment(state));
            return true;
        }
        if done {
            continue;
        }
        let mut winding = 0;
        let mut sortable = false;
        let Some(mut angle) = angle_winding(
            state,
            start_ptr.expect("a chased span has a start"),
            end_ptr.expect("an undone chased span has an end"),
            &mut winding,
            &mut sortable,
        ) else {
            *result = None;
            return true;
        };
        if winding == SK_MIN_S32 {
            continue;
        }
        // Skia leaves both uninitialized when the angles are unsortable, and then reads them
        // only when they are sortable.
        let mut sum_mi_winding = 0;
        let mut sum_su_winding = 0;
        if sortable {
            segment = angle.segment(state);
            sum_mi_winding = segment.update_winding_reverse(state, angle);
            if sum_mi_winding == SK_MIN_S32 {
                *result = None;
                return true;
            }
            sum_su_winding = segment.update_opp_winding_reverse(state, angle);
            if sum_su_winding == SK_MIN_S32 {
                *result = None;
                return true;
            }
            if segment.operand(state) {
                std::mem::swap(&mut sum_mi_winding, &mut sum_su_winding);
            }
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
            let mut sum_winding = 0;
            let mut opp_max_winding = 0;
            let mut opp_sum_winding = 0;
            if sortable {
                segment.set_up_windings_opp(
                    state,
                    start,
                    end,
                    &mut sum_mi_winding,
                    &mut sum_su_winding,
                    &mut max_winding,
                    &mut sum_winding,
                    &mut opp_max_winding,
                    &mut opp_sum_winding,
                );
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
                if sortable
                    && !segment.mark_angle_opp(
                        state,
                        max_winding,
                        sum_winding,
                        opp_max_winding,
                        opp_sum_winding,
                        angle,
                        None,
                    )
                {
                    return false;
                }
            }
        }
        if first.is_some() {
            chase.push(span);
            *result = first;
            return true;
        }
    }
    *result = None;
    true
}

fn bridge_op(
    state: &mut SkOpGlobalState,
    contour_list: SkOpContourHead,
    op: SkPathOp,
    xor_mask: i32,
    xor_op_mask: i32,
    writer: &mut SkPathWriter,
) -> bool {
    let mut unsortable = false;
    let mut last_simple;
    let mut simple = false;
    while let Some(span) = find_sortable_top(state, contour_list) {
        let mut current = span.segment(state);
        let mut start: SkOpSpanBase = span.next(state).expect("a sortable top has a next span");
        let mut end: SkOpSpanBase = SkOpSpanBase::from(span);
        let mut chase: Vec<SkOpSpanBase> = Vec::new();
        loop {
            if current.active_op(state, start, end, xor_mask, xor_op_mask, op) {
                loop {
                    if !unsortable && current.done(state) {
                        break;
                    }
                    debug_assert!(unsortable || !current.done(state));
                    let mut next_start = Some(start);
                    let mut next_end = Some(end);
                    last_simple = simple;
                    let next = current.find_next_op(
                        state,
                        &mut chase,
                        &mut next_start,
                        &mut next_end,
                        &mut unsortable,
                        &mut simple,
                        op,
                        xor_mask,
                        xor_op_mask,
                    );
                    let Some(next) = next else {
                        if !unsortable
                            && writer.has_move()
                            && current.verb(state) != SkPathVerb::Line
                            && !writer.is_closed(state)
                        {
                            if !current.add_curve_to(state, start, end, writer) {
                                return false;
                            }
                        } else if last_simple && !current.add_curve_to(state, start, end, writer) {
                            return false;
                        }
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
            let mut next_current = None;
            if !find_chase_op(
                state,
                &mut chase,
                &mut start_ptr,
                &mut end_ptr,
                &mut next_current,
            ) {
                return false;
            }
            let Some(next_current) = next_current else {
                break;
            };
            current = next_current;
            start = start_ptr.expect("a chased segment has a start");
            end = end_ptr.expect("a chased segment has an end");
        }
    }
    true
}

// diagram of why this simplifcation is possible is here:
// https://skia.org/dev/present/pathops link at bottom of the page
// https://drive.google.com/file/d/0BwoLUwz9PYkHLWpsaXd0UDdaN00/view?usp=sharing
#[rustfmt::skip]
const G_OP_INVERSE: [[[SkPathOp; 2]; 2]; SkPathOp::ReverseDifference as usize + 1] = {
    use SkPathOp::{Difference, Intersect, ReverseDifference, Union, Xor};
    [
//                  inside minuend                               outside minuend
//     inside subtrahend     outside subtrahend      inside subtrahend     outside subtrahend
[[ Difference,         Intersect ],         [ Union,             ReverseDifference ]],
[[ Intersect,          Difference ],        [ ReverseDifference, Union ]],
[[ Union,              ReverseDifference ], [ Difference,        Intersect ]],
[[ Xor,                Xor ],               [ Xor,               Xor ]],
[[ ReverseDifference,  Union ],             [ Intersect,         Difference ]],
    ]
};

#[rustfmt::skip]
const G_OUT_INVERSE: [[[bool; 2]; 2]; SkPathOp::ReverseDifference as usize + 1] = [
    [[ false, false ], [ true, false ]],  // diff
    [[ false, false ], [ false, true ]],  // sect
    [[ false, true ], [ true, true ]],    // union
    [[ false, true ], [ true, false ]],   // xor
    [[ false, true ], [ false, false ]],  // rev diff
];

/// `op` combines two paths with a boolean operation, as Skia's `Op` does.
///
/// Each path is read with its own fill type, so a winding path and an even-odd path combine
/// as they are drawn, and the result is a new path that bounds the combined area with no
/// overlapping contours. The result is filled with the even-odd rule, or with the inverse
/// even-odd rule when the combined area reaches infinitely far from both paths, which only an
/// input with an inverse fill type can give. The operation fails with [`PathOpsError`] exactly
/// where Skia's `Op` returns no path: when a path it has to read is not finite, or when Skia
/// cannot resolve the curves' intersections or windings.
pub fn op(one: &SkPath, two: &SkPath, op: SkPathOp) -> Result<SkPath, PathOpsError> {
    op_debug(one, two, op).ok_or(PathOpsError)
}

/// `OpDebug`, declared in SkPathOpsCommon.h, with Skia's debug-only parameters left out.
pub(crate) fn op_debug(one: &SkPath, two: &SkPath, op: SkPathOp) -> Option<SkPath> {
    let one_inverse = usize::from(one.is_inverse_fill_type());
    let two_inverse = usize::from(two.is_inverse_fill_type());
    let mut op = G_OP_INVERSE[op as usize][one_inverse][two_inverse];
    let inverse_fill = G_OUT_INVERSE[op as usize][one_inverse][two_inverse];
    let fill_type = if inverse_fill {
        SkPathFillType::InverseEvenOdd
    } else {
        SkPathFillType::EvenOdd
    };
    let mut rect1 = SkRect::default();
    let mut rect2 = SkRect::default();
    if SkPathOp::Intersect == op && one.is_rect(&mut rect1) && two.is_rect(&mut rect2) {
        let result = if rect1.intersect(&rect2) {
            SkPath::rect(&rect1)
        } else {
            SkPath::new()
        };
        return Some(result.make_fill_type(fill_type));
    }
    if one.is_empty() || two.is_empty() {
        let mut work = SkPath::new();
        match op {
            SkPathOp::Intersect => {}
            SkPathOp::Union | SkPathOp::Xor => {
                work = if one.is_empty() {
                    two.clone()
                } else {
                    one.clone()
                };
            }
            SkPathOp::Difference => {
                if !one.is_empty() {
                    work = one.clone();
                }
            }
            SkPathOp::ReverseDifference => {
                if !two.is_empty() {
                    work = two.clone();
                }
            }
        }
        if inverse_fill != work.is_inverse_fill_type() {
            work.toggle_inverse_fill_type();
        }
        return simplify_debug(&work);
    }
    // FIXME: add a constant expression here, tune
    let mut global_state = SkOpGlobalState::new();
    let state = &mut global_state;
    let mut contour_list = state.contour_head();
    let coincidence = SkOpCoincidence::new(state);
    let mut minuend = one;
    let mut subtrahend = two;
    if op == SkPathOp::ReverseDifference {
        std::mem::swap(&mut minuend, &mut subtrahend);
        op = SkPathOp::Difference;
    }
    // turn path into list of segments
    let mut builder = SkOpEdgeBuilder::new(minuend, contour_list);
    if builder.unparseable() {
        return None;
    }
    let xor_mask = builder.xor_mask() as i32;
    builder.add_operand(subtrahend);
    if !builder.finish(state) {
        return None;
    }

    let xor_op_mask = builder.xor_mask() as i32;
    if !sort_contour_list(
        state,
        &mut contour_list,
        xor_mask == SkPathOpsMask::EvenOdd as i32,
        xor_op_mask == SkPathOpsMask::EvenOdd as i32,
    ) {
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
    if !bridge_op(state, contour_list, op, xor_mask, xor_op_mask, &mut wrapper) {
        return None;
    }
    wrapper.assemble(state); // if some edges could not be resolved, assemble remaining
    Some(wrapper.native_path())
}
