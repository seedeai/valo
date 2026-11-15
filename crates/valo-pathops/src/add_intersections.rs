//! Port of Skia's src/pathops/SkAddIntersections.{h,cpp}: finding every intersection between
//! the segments of two contours (or of one contour with itself) and recording each as t
//! values on both segments, or as a coincident run.

use crate::intersection_helper::{SegmentType, SkIntersectionHelper};
use crate::intersections::SkIntersections;
use crate::op_coincidence::SkOpCoincidence;
use crate::op_contour::SkOpContour;
use crate::op_span::SkOpPtT;
use crate::path_ops_bounds::SkPathOpsBounds;
use crate::path_ops_conic::SkDConic;
use crate::path_ops_cubic::SkDCubic;
use crate::path_ops_quad::SkDQuad;
use crate::path_ops_types::{almost_less_ulps_f32, SkOpGlobalState};

/// Skia's `continue` statements in its do-while loops go to the `advance()` steps.
pub(crate) fn add_intersect_ts(
    state: &mut SkOpGlobalState,
    test: SkOpContour,
    next: SkOpContour,
    coincidence: SkOpCoincidence,
) -> bool {
    if test != next {
        if almost_less_ulps_f32(test.bounds(state).bottom, next.bounds(state).top) {
            return false;
        }
        // OPTIMIZATION: outset contour bounds a smidgen instead?
        if !SkPathOpsBounds::intersects(test.bounds(state), next.bounds(state)) {
            return true;
        }
    }
    let mut wt = SkIntersectionHelper::default();
    wt.init(state, test);
    loop {
        let mut wn = SkIntersectionHelper::default();
        wn.init(state, next);
        if test != next || wn.start_after(state, &wt) {
            loop {
                add_segment_intersect_ts(state, &wt, &wn, coincidence);
                if !wn.advance(state) {
                    break;
                }
            }
        }
        if !wt.advance(state) {
            break;
        }
    }
    true
}

/// The body of `AddIntersectTs`'s inner do-while, for the pair of segments `wt` and `wn`.
fn add_segment_intersect_ts(
    state: &mut SkOpGlobalState,
    wt: &SkIntersectionHelper,
    wn: &SkIntersectionHelper,
    coincidence: SkOpCoincidence,
) {
    if !SkPathOpsBounds::intersects(wt.bounds(state), wn.bounds(state)) {
        return;
    }
    let pts: i32;
    let mut ts = SkIntersections::new();
    let mut swap = false;
    let mut quad1 = SkDQuad::default();
    let mut quad2 = SkDQuad::default();
    let mut conic1 = SkDConic::default();
    let mut conic2 = SkDConic::default();
    let mut cubic1 = SkDCubic::default();
    let mut cubic2 = SkDCubic::default();
    match wt.segment_type(state) {
        SegmentType::HorizontalLine => {
            swap = true;
            match wn.segment_type(state) {
                SegmentType::HorizontalLine | SegmentType::VerticalLine | SegmentType::Line => {
                    pts = ts.line_horizontal(
                        &wn.pts(state),
                        wt.left(state),
                        wt.right(state),
                        wt.y(state),
                        wt.x_flipped(state),
                    );
                }
                SegmentType::Quad => {
                    pts = ts.quad_horizontal(
                        &wn.pts(state),
                        wt.left(state),
                        wt.right(state),
                        wt.y(state),
                        wt.x_flipped(state),
                    );
                }
                SegmentType::Conic => {
                    pts = ts.conic_horizontal(
                        &wn.pts(state),
                        wn.weight(state),
                        wt.left(state),
                        wt.right(state),
                        wt.y(state),
                        wt.x_flipped(state),
                    );
                }
                SegmentType::Cubic => {
                    pts = ts.cubic_horizontal(
                        &wn.pts(state),
                        wt.left(state),
                        wt.right(state),
                        wt.y(state),
                        wt.x_flipped(state),
                    );
                }
            }
        }
        SegmentType::VerticalLine => {
            swap = true;
            match wn.segment_type(state) {
                SegmentType::HorizontalLine | SegmentType::VerticalLine | SegmentType::Line => {
                    pts = ts.line_vertical(
                        &wn.pts(state),
                        wt.top(state),
                        wt.bottom(state),
                        wt.x(state),
                        wt.y_flipped(state),
                    );
                }
                SegmentType::Quad => {
                    pts = ts.quad_vertical(
                        &wn.pts(state),
                        wt.top(state),
                        wt.bottom(state),
                        wt.x(state),
                        wt.y_flipped(state),
                    );
                }
                SegmentType::Conic => {
                    pts = ts.conic_vertical(
                        &wn.pts(state),
                        wn.weight(state),
                        wt.top(state),
                        wt.bottom(state),
                        wt.x(state),
                        wt.y_flipped(state),
                    );
                }
                SegmentType::Cubic => {
                    pts = ts.cubic_vertical(
                        &wn.pts(state),
                        wt.top(state),
                        wt.bottom(state),
                        wt.x(state),
                        wt.y_flipped(state),
                    );
                }
            }
        }
        SegmentType::Line => match wn.segment_type(state) {
            SegmentType::HorizontalLine => {
                pts = ts.line_horizontal(
                    &wt.pts(state),
                    wn.left(state),
                    wn.right(state),
                    wn.y(state),
                    wn.x_flipped(state),
                );
            }
            SegmentType::VerticalLine => {
                pts = ts.line_vertical(
                    &wt.pts(state),
                    wn.top(state),
                    wn.bottom(state),
                    wn.x(state),
                    wn.y_flipped(state),
                );
            }
            SegmentType::Line => {
                pts = ts.line_line(&wt.pts(state), &wn.pts(state));
            }
            SegmentType::Quad => {
                swap = true;
                pts = ts.quad_line(&wn.pts(state), &wt.pts(state));
            }
            SegmentType::Conic => {
                swap = true;
                pts = ts.conic_line(&wn.pts(state), wn.weight(state), &wt.pts(state));
            }
            SegmentType::Cubic => {
                swap = true;
                pts = ts.cubic_line(&wn.pts(state), &wt.pts(state));
            }
        },
        SegmentType::Quad => match wn.segment_type(state) {
            SegmentType::HorizontalLine => {
                pts = ts.quad_horizontal(
                    &wt.pts(state),
                    wn.left(state),
                    wn.right(state),
                    wn.y(state),
                    wn.x_flipped(state),
                );
            }
            SegmentType::VerticalLine => {
                pts = ts.quad_vertical(
                    &wt.pts(state),
                    wn.top(state),
                    wn.bottom(state),
                    wn.x(state),
                    wn.y_flipped(state),
                );
            }
            SegmentType::Line => {
                pts = ts.quad_line(&wt.pts(state), &wn.pts(state));
            }
            SegmentType::Quad => {
                pts = ts.intersect_quad_quad(quad1.set(&wt.pts(state)), quad2.set(&wn.pts(state)));
            }
            SegmentType::Conic => {
                swap = true;
                pts = ts.intersect_conic_quad(
                    conic2.set(&wn.pts(state), wn.weight(state)),
                    quad1.set(&wt.pts(state)),
                );
            }
            SegmentType::Cubic => {
                swap = true;
                pts =
                    ts.intersect_cubic_quad(cubic2.set(&wn.pts(state)), quad1.set(&wt.pts(state)));
            }
        },
        SegmentType::Conic => match wn.segment_type(state) {
            SegmentType::HorizontalLine => {
                pts = ts.conic_horizontal(
                    &wt.pts(state),
                    wt.weight(state),
                    wn.left(state),
                    wn.right(state),
                    wn.y(state),
                    wn.x_flipped(state),
                );
            }
            SegmentType::VerticalLine => {
                pts = ts.conic_vertical(
                    &wt.pts(state),
                    wt.weight(state),
                    wn.top(state),
                    wn.bottom(state),
                    wn.x(state),
                    wn.y_flipped(state),
                );
            }
            SegmentType::Line => {
                pts = ts.conic_line(&wt.pts(state), wt.weight(state), &wn.pts(state));
            }
            SegmentType::Quad => {
                pts = ts.intersect_conic_quad(
                    conic1.set(&wt.pts(state), wt.weight(state)),
                    quad2.set(&wn.pts(state)),
                );
            }
            SegmentType::Conic => {
                pts = ts.intersect_conic_conic(
                    conic1.set(&wt.pts(state), wt.weight(state)),
                    conic2.set(&wn.pts(state), wn.weight(state)),
                );
            }
            SegmentType::Cubic => {
                swap = true;
                pts = ts.intersect_cubic_conic(
                    cubic2.set(&wn.pts(state)),
                    conic1.set(&wt.pts(state), wt.weight(state)),
                );
            }
        },
        SegmentType::Cubic => match wn.segment_type(state) {
            SegmentType::HorizontalLine => {
                pts = ts.cubic_horizontal(
                    &wt.pts(state),
                    wn.left(state),
                    wn.right(state),
                    wn.y(state),
                    wn.x_flipped(state),
                );
            }
            SegmentType::VerticalLine => {
                pts = ts.cubic_vertical(
                    &wt.pts(state),
                    wn.top(state),
                    wn.bottom(state),
                    wn.x(state),
                    wn.y_flipped(state),
                );
            }
            SegmentType::Line => {
                pts = ts.cubic_line(&wt.pts(state), &wn.pts(state));
            }
            SegmentType::Quad => {
                pts =
                    ts.intersect_cubic_quad(cubic1.set(&wt.pts(state)), quad2.set(&wn.pts(state)));
            }
            SegmentType::Conic => {
                pts = ts.intersect_cubic_conic(
                    cubic1.set(&wt.pts(state)),
                    conic2.set(&wn.pts(state), wn.weight(state)),
                );
            }
            SegmentType::Cubic => {
                pts = ts
                    .intersect_cubic_cubic(cubic1.set(&wt.pts(state)), cubic2.set(&wn.pts(state)));
            }
        },
    }
    let mut coin_index = -1;
    let mut coin_pt_t: [Option<SkOpPtT>; 2] = [None; 2];
    for pt in 0..pts {
        let index = pt as usize;
        debug_assert!(ts[0][index] >= 0.0 && ts[0][index] <= 1.0);
        debug_assert!(ts[1][index] >= 0.0 && ts[1][index] <= 1.0);
        // if t value is used to compute pt in addT, error may creep in and
        // rect intersections may result in non-rects. if pt value from intersection
        // is passed in, current tests break. As a workaround, pass in pt
        // value from intersection only if pt.x and pt.y is integral
        let i_pt = ts.pt(index).as_sk_point();
        let i_pt_is_integral = i_pt.x == i_pt.x.floor() && i_pt.y == i_pt.y.floor();
        let test_t_at = if i_pt_is_integral {
            wt.segment()
                .add_t_pt(state, ts[usize::from(swap)][index], &i_pt)
        } else {
            wt.segment().add_t(state, ts[usize::from(swap)][index])
        };
        let next_t_at = if i_pt_is_integral {
            wn.segment()
                .add_t_pt(state, ts[usize::from(!swap)][index], &i_pt)
        } else {
            wn.segment().add_t(state, ts[usize::from(!swap)][index])
        };
        // Skia dereferences both without a check.
        let mut test_t_at = test_t_at.expect("addT found or made the t");
        let mut next_t_at = next_t_at.expect("addT found or made the t");
        if !test_t_at.contains_pt_t(state, next_t_at) {
            let opp_prev = test_t_at.opp_prev(state, next_t_at); //  Returns nullptr if pair
            if let Some(opp_prev) = opp_prev {
                //  already share a pt-t loop.
                let (test_span, next_span) = (test_t_at.span(state), next_t_at.span(state));
                test_span.merge_matches(state, next_span);
                test_t_at.add_opp(state, next_t_at, opp_prev);
            }
        }
        if !ts.is_coincident(pt) {
            continue;
        }
        if coin_index < 0 {
            coin_pt_t[0] = Some(test_t_at);
            coin_pt_t[1] = Some(next_t_at);
            coin_index = pt;
            continue;
        }
        let mut coin_start = coin_pt_t[0].expect("set with coinIndex");
        let mut coin_opp_start = coin_pt_t[1].expect("set with coinIndex");
        if coin_start.span(state) == test_t_at.span(state) {
            coin_index = -1;
            continue;
        }
        if coin_opp_start.span(state) == next_t_at.span(state) {
            coin_index = -1; // coincidence span collapsed
            continue;
        }
        if swap {
            std::mem::swap(&mut coin_start, &mut coin_opp_start);
            std::mem::swap(&mut test_t_at, &mut next_t_at);
            coin_pt_t = [Some(coin_start), Some(coin_opp_start)];
        }
        if coin_start.span(state).deleted(state) {
            coin_index = -1;
            continue;
        }
        if test_t_at.span(state).deleted(state) {
            coin_index = -1;
            continue;
        }
        coincidence.add(state, coin_start, test_t_at, coin_opp_start, next_t_at);
        coin_index = -1;
    }
}
