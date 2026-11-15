//! Port of Skia's src/pathops/SkReduceOrder.{h,cpp}: lowering a curve whose points are
//! degenerate to a curve of fewer points (a cubic to a quad or line, a quad to a line).
//!
//! `SkReduceOrder` is a C++ union of a line, quad and cubic sharing one point array, written as
//! one member and read as another (`Quad` reads `fLine` after `reduce` wrote `fQuad`). Here it
//! is that point array, read through `as_line`/`as_quad`.
//!
//! Skia's callers pass the same array as `a` and `reducePts` (`SkReduceOrder::Quad(curve,
//! curve)`); the functions read `a` completely before writing `reducePts`, so a Rust caller
//! passes a copy of the input.

use crate::core::float_bits::std_max;
use crate::core::geometry::SkConic;
use crate::core::path::SkPathVerb;
use crate::core::point::SkPoint;
use crate::path_ops_cubic::SkDCubic;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::SkDPoint;
use crate::path_ops_quad::SkDQuad;
use crate::path_ops_types::{
    almost_equal_ulps, almost_equal_ulps_pin, approximately_equal, approximately_equal_half,
    approximately_zero, sk_path_ops_points_to_verb,
};

/// `SkReduceOrder::Quadratics`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Quadratics {
    AllowQuadratics,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkReduceOrder {
    /// The union's shared points: `fLine.fPts`, `fQuad.fPts`, `fCubic.fPts`.
    pub(crate) pts: [SkDPoint; 4],
}

impl SkReduceOrder {
    /// `fLine`.
    pub(crate) fn as_line(&self) -> SkDLine {
        SkDLine {
            pts: [self.pts[0], self.pts[1]],
        }
    }

    /// `fQuad`.
    pub(crate) fn as_quad(&self) -> SkDQuad {
        SkDQuad {
            pts: [self.pts[0], self.pts[1], self.pts[2]],
        }
    }

    /// `reduce(const SkDCubic& cubic, Quadratics)`.
    pub(crate) fn reduce_cubic(&mut self, cubic: &SkDCubic, allow_quadratics: Quadratics) -> i32 {
        let mut min_x = 0;
        let mut max_x = 0;
        let mut min_y = 0;
        let mut max_y = 0;
        let mut min_x_set = 0;
        let mut min_y_set = 0;
        for index in 1..4 {
            if cubic[min_x].x > cubic[index].x {
                min_x = index;
            }
            if cubic[min_y].y > cubic[index].y {
                min_y = index;
            }
            if cubic[max_x].x < cubic[index].x {
                max_x = index;
            }
            if cubic[max_y].y < cubic[index].y {
                max_y = index;
            }
        }
        for index in 0..4 {
            let cx = cubic[index].x;
            let cy = cubic[index].y;
            let denom = std_max(
                cx.abs(),
                std_max(
                    cy.abs(),
                    std_max(cubic[min_x].x.abs(), cubic[min_y].y.abs()),
                ),
            );
            if denom == 0.0 {
                min_x_set |= 1 << index;
                min_y_set |= 1 << index;
                continue;
            }
            let inv = 1.0 / denom;
            if approximately_equal_half(cx * inv, cubic[min_x].x * inv) {
                min_x_set |= 1 << index;
            }
            if approximately_equal_half(cy * inv, cubic[min_y].y * inv) {
                min_y_set |= 1 << index;
            }
        }
        if min_x_set == 0xF {
            // test for vertical line
            if min_y_set == 0xF {
                // return 1 if all four are coincident
                return coincident_line_cubic(cubic, &mut self.pts);
            }
            return vertical_line_cubic(cubic, &mut self.pts);
        }
        if min_y_set == 0xF {
            // test for horizontal line
            return horizontal_line_cubic(cubic, &mut self.pts);
        }
        let mut result = check_linear_cubic(cubic, min_x, max_x, min_y, max_y, &mut self.pts);
        if result != 0 {
            return result;
        }
        if allow_quadratics == Quadratics::AllowQuadratics {
            result = check_quadratic(cubic, &mut self.pts);
            if result != 0 {
                return result;
            }
        }
        self.pts = cubic.pts;
        4
    }

    /// `reduce(const SkDQuad& quad)`.
    pub(crate) fn reduce_quad(&mut self, quad: &SkDQuad) -> i32 {
        let mut min_x = 0;
        let mut max_x = 0;
        let mut min_y = 0;
        let mut max_y = 0;
        let mut min_x_set = 0;
        let mut min_y_set = 0;
        for index in 1..3 {
            if quad[min_x].x > quad[index].x {
                min_x = index;
            }
            if quad[min_y].y > quad[index].y {
                min_y = index;
            }
            if quad[max_x].x < quad[index].x {
                max_x = index;
            }
            if quad[max_y].y < quad[index].y {
                max_y = index;
            }
        }
        for index in 0..3 {
            if almost_equal_ulps(quad[index].x, quad[min_x].x) {
                min_x_set |= 1 << index;
            }
            if almost_equal_ulps(quad[index].y, quad[min_y].y) {
                min_y_set |= 1 << index;
            }
        }
        if (min_x_set & 0x05) == 0x5 && (min_y_set & 0x05) == 0x5 {
            // test for degenerate
            // this quad starts and ends at the same place, so never contributes
            // to the fill
            return coincident_line_quad(quad, &mut self.pts);
        }
        if min_x_set == 0x7 {
            // test for vertical line
            return vertical_line_quad(quad, &mut self.pts);
        }
        if min_y_set == 0x7 {
            // test for horizontal line
            return horizontal_line_quad(quad, &mut self.pts);
        }
        let result = check_linear_quad(quad, min_x, max_x, min_y, max_y, &mut self.pts);
        if result != 0 {
            return result;
        }
        self.pts[..3].copy_from_slice(&quad.pts);
        3
    }

    /// `static Conic(const SkConic& conic, SkPoint* reducePts)`.
    pub(crate) fn conic(conic: &SkConic, reduce_pts: &mut [SkPoint]) -> SkPathVerb {
        let c = conic;
        let verb = SkReduceOrder::quad(&c.pts, reduce_pts);
        if verb > SkPathVerb::Line && c.w == 1.0 {
            return SkPathVerb::Quad;
        }
        if verb == SkPathVerb::Quad {
            SkPathVerb::Conic
        } else {
            verb
        }
    }

    /// `static Cubic(const SkPoint pts[4], SkPoint* reducePts)`.
    pub(crate) fn cubic(pts: &[SkPoint], reduce_pts: &mut [SkPoint]) -> SkPathVerb {
        let a = pts;
        if SkDPoint::approximately_equal_sk_points(&a[0], &a[1])
            && SkDPoint::approximately_equal_sk_points(&a[0], &a[2])
            && SkDPoint::approximately_equal_sk_points(&a[0], &a[3])
        {
            reduce_pts[0] = a[0];
            return SkPathVerb::Move;
        }
        let mut cubic = SkDCubic::default();
        cubic.set(a);
        let mut reducer = SkReduceOrder::default();
        let order = reducer.reduce_cubic(&cubic, Quadratics::AllowQuadratics);
        if order == 2 || order == 3 {
            // cubic became line or quad
            let quad = reducer.as_quad();
            for index in 0..order as usize {
                reduce_pts[index] = quad[index].as_sk_point();
            }
        }
        sk_path_ops_points_to_verb(order - 1)
    }

    /// `static Quad(const SkPoint pts[3], SkPoint* reducePts)`.
    pub(crate) fn quad(pts: &[SkPoint], reduce_pts: &mut [SkPoint]) -> SkPathVerb {
        let a = pts;
        let mut quad = SkDQuad::default();
        quad.set(a);
        let mut reducer = SkReduceOrder::default();
        let order = reducer.reduce_quad(&quad);
        if order == 2 {
            // quad became line
            let line = reducer.as_line();
            for index in 0..order as usize {
                reduce_pts[index] = line[index].as_sk_point();
            }
        }
        sk_path_ops_points_to_verb(order - 1)
    }
}

// Skia's helpers take the reduction as an `SkDQuad&` or `SkDCubic&` member of the union; here
// they take the union's shared points.

fn coincident_line_quad(quad: &SkDQuad, reduction: &mut [SkDPoint; 4]) -> i32 {
    reduction[0] = quad[0];
    reduction[1] = quad[0];
    1
}

fn reduction_line_count_quad(reduction: &[SkDPoint; 4]) -> i32 {
    1 + i32::from(!reduction[0].approximately_equal(&reduction[1]))
}

fn vertical_line_quad(quad: &SkDQuad, reduction: &mut [SkDPoint; 4]) -> i32 {
    reduction[0] = quad[0];
    reduction[1] = quad[2];
    reduction_line_count_quad(reduction)
}

fn horizontal_line_quad(quad: &SkDQuad, reduction: &mut [SkDPoint; 4]) -> i32 {
    reduction[0] = quad[0];
    reduction[1] = quad[2];
    reduction_line_count_quad(reduction)
}

fn check_linear_quad(
    quad: &SkDQuad,
    _min_x: usize,
    _max_x: usize,
    _min_y: usize,
    _max_y: usize,
    reduction: &mut [SkDPoint; 4],
) -> i32 {
    if !quad.is_linear(0, 2) {
        return 0;
    }
    // four are colinear: return line formed by outside
    reduction[0] = quad[0];
    reduction[1] = quad[2];
    reduction_line_count_quad(reduction)
}

// reduce to a quadratic or smaller
// look for identical points
// look for all four points in a line
// note that three points in a line doesn't simplify a cubic
// look for approximation with single quadratic
// save approximation with multiple quadratics for later

////////////////////////////////////////////////////////////////////////////////////

fn coincident_line_cubic(cubic: &SkDCubic, reduction: &mut [SkDPoint; 4]) -> i32 {
    reduction[0] = cubic[0];
    reduction[1] = cubic[0];
    1
}

fn reduction_line_count_cubic(reduction: &[SkDPoint; 4]) -> i32 {
    1 + i32::from(!reduction[0].approximately_equal(&reduction[1]))
}

fn vertical_line_cubic(cubic: &SkDCubic, reduction: &mut [SkDPoint; 4]) -> i32 {
    reduction[0] = cubic[0];
    reduction[1] = cubic[3];
    reduction_line_count_cubic(reduction)
}

fn horizontal_line_cubic(cubic: &SkDCubic, reduction: &mut [SkDPoint; 4]) -> i32 {
    reduction[0] = cubic[0];
    reduction[1] = cubic[3];
    reduction_line_count_cubic(reduction)
}

// check to see if it is a quadratic or a line
fn check_quadratic(cubic: &SkDCubic, reduction: &mut [SkDPoint; 4]) -> i32 {
    let dx10 = cubic[1].x - cubic[0].x;
    let dx23 = cubic[2].x - cubic[3].x;
    let mid_x = cubic[0].x + dx10 * 3.0 / 2.0;
    let side_ax = mid_x - cubic[3].x;
    let side_bx = dx23 * 3.0 / 2.0;
    if if approximately_zero(side_ax) {
        !approximately_equal(side_ax, side_bx)
    } else {
        !almost_equal_ulps_pin(side_ax, side_bx)
    } {
        return 0;
    }
    let dy10 = cubic[1].y - cubic[0].y;
    let dy23 = cubic[2].y - cubic[3].y;
    let mid_y = cubic[0].y + dy10 * 3.0 / 2.0;
    let side_ay = mid_y - cubic[3].y;
    let side_by = dy23 * 3.0 / 2.0;
    if if approximately_zero(side_ay) {
        !approximately_equal(side_ay, side_by)
    } else {
        !almost_equal_ulps_pin(side_ay, side_by)
    } {
        return 0;
    }
    reduction[0] = cubic[0];
    reduction[1].x = mid_x;
    reduction[1].y = mid_y;
    reduction[2] = cubic[3];
    3
}

fn check_linear_cubic(
    cubic: &SkDCubic,
    _min_x: usize,
    _max_x: usize,
    _min_y: usize,
    _max_y: usize,
    reduction: &mut [SkDPoint; 4],
) -> i32 {
    if !cubic.is_linear(0, 3) {
        return 0;
    }
    // four are colinear: return line formed by outside
    reduction[0] = cubic[0];
    reduction[1] = cubic[3];
    reduction_line_count_cubic(reduction)
}

/* food for thought:
http://objectmix.com/graphics/132906-fast-precision-driven-cubic-quadratic-piecewise-degree-reduction-algos-2-a.html

Given points c1, c2, c3 and c4 of a cubic Bezier, the points of the
corresponding quadratic Bezier are (given in convex combinations of
points):

q1 = (11/13)c1 + (3/13)c2 -(3/13)c3 + (2/13)c4
q2 = -c1 + (3/2)c2 + (3/2)c3 - c4
q3 = (2/13)c1 - (3/13)c2 + (3/13)c3 + (11/13)c4

Of course, this curve does not interpolate the end-points, but it would
be interesting to see the behaviour of such a curve in an applet.

--
Kalle Rutanen
http://kaba.hilvi.org

*/
