//! Port of Skia's src/pathops/SkPathOpsCubic.{h,cpp}: a cubic Bézier in doubles, and `SkTCubic`,
//! its `SkTCurve` for the TSect intersection.
//!
//! Skia passes `&fPts[0].fX` (or `.fY`) as a `const double*` with a stride of two to
//! `FindExtrema` and `Coefficients`; here those take a slice of the interleaved coordinates
//! `[x0, y0, x1, y1, ...]`, starting at the x or y of the first point.

// Skia indexes its arrays; the loops keep its indices.
#![allow(clippy::needless_range_loop)]

use std::ops::{Index, IndexMut};

use crate::core::float_bits::{sk_is_finite_f32, sk_scalar_abs, sk_t_pin, std_max, std_min};
use crate::core::geometry::{sk_classify_cubic, SkCubicType};
use crate::core::point::SkPoint;
use crate::core::t_sort::sk_t_q_sort;
use crate::intersections::SkIntersections;
use crate::line_parameters::SkLineParameters;
use crate::path_ops_conic::SkDConic;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::{SkDPoint, SkDVector};
use crate::path_ops_quad::SkDQuad;
use crate::path_ops_rect::SkDRect;
use crate::path_ops_t_curve::SkTCurve;
use crate::path_ops_types::{
    almost_bequal_ulps, almost_dequal_ulps, approximately_equal, approximately_equal_half,
    approximately_one_or_less, approximately_zero, approximately_zero_or_more,
    approximately_zero_when_compared_to, between, precisely_between, precisely_zero,
    roughly_between, sk_d_interp, zero_or_one, FLT_EPSILON_ORDERABLE_ERR,
};

/// `SkDCubic::SearchAxis`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchAxis {
    XAxis,
    YAxis,
}

/// `(&pt.fX)[xAxis]`: the point's x for `XAxis`, its y for `YAxis`.
fn axis_coord(pt: &SkDPoint, axis: SearchAxis) -> f64 {
    match axis {
        SearchAxis::XAxis => pt.x,
        SearchAxis::YAxis => pt.y,
    }
}

/* classic one t subdivision */
/// `interp_cubic_coords(const double* src, double* dst, double t)`: both at a stride of two.
fn interp_cubic_coords_chop(src: &[f64], dst: &mut [f64], t: f64) {
    let ab = sk_d_interp(src[0], src[2], t);
    let bc = sk_d_interp(src[2], src[4], t);
    let cd = sk_d_interp(src[4], src[6], t);
    let abc = sk_d_interp(ab, bc, t);
    let bcd = sk_d_interp(bc, cd, t);
    let abcd = sk_d_interp(abc, bcd, t);

    dst[0] = src[0];
    dst[2] = ab;
    dst[4] = abc;
    dst[6] = abcd;
    dst[8] = bcd;
    dst[10] = cd;
    dst[12] = src[6];
}

// Do a quick reject by rotating all points relative to a line formed by
// a pair of one cubic's points. If the 2nd cubic's points
// are on the line or on the opposite side from the 1st cubic's 'odd man', the
// curves at most intersect at the endpoints.
/* if returning true, check contains true if cubic's hull collapsed, making the cubic linear
   if returning false, check contains true if the the cubic pair have only the end point in common
*/

// from http://www.cs.sunysb.edu/~qin/courses/geometry/4.pdf
// c(t)  = a(1-t)^3 + 3bt(1-t)^2 + 3c(1-t)t^2 + dt^3
// c'(t) = -3a(1-t)^2 + 3b((1-t)^2 - 2t(1-t)) + 3c(2t(1-t) - t^2) + 3dt^2
//       = 3(b-a)(1-t)^2 + 6(c-b)t(1-t) + 3(d-c)t^2
fn derivative_at_t(src: &[f64], t: f64) -> f64 {
    let one_t = 1.0 - t;
    let a = src[0];
    let b = src[2];
    let c = src[4];
    let d = src[6];
    3.0 * ((b - a) * one_t * one_t + 2.0 * (c - b) * t * one_t + (d - c) * t * t)
}

fn formulate_f1_dot_f2(src: &[f64], coeff: &mut [f64; 4]) {
    let a = src[2] - src[0];
    let b = src[4] - 2.0 * src[2] + src[0];
    let c = src[6] + 3.0 * (src[2] - src[4]) - src[0];
    coeff[0] = c * c;
    coeff[1] = 3.0 * b * c;
    coeff[2] = 2.0 * b * b + c * a;
    coeff[3] = a * b;
}

/* SkDCubic'(t) = At^2 + Bt + C, where
    A = 3(-a + 3(b - c) + d)
    B = 6(a - 2b + c)
    C = 3(b - a)
    Solve for t, keeping only those that fit between 0 < t < 1
*/

/*  from SkGeometry.cpp
    Looking for F' dot F'' == 0

    A = b - a
    B = c - 2b + a
    C = d - 3c + 3b - a

    F' = 3Ct^2 + 6Bt + 3A
    F'' = 6Ct + 6B

    F' dot F'' -> CCt^3 + 3BCt^2 + (2BB + CA)t + AB
*/

/*
 Given a cubic c, t1, and t2, find a small cubic segment.

 The new cubic is defined as points A, B, C, and D, where
 s1 = 1 - t1
 s2 = 1 - t2
 A = c[0]*s1*s1*s1 + 3*c[1]*s1*s1*t1 + 3*c[2]*s1*t1*t1 + c[3]*t1*t1*t1
 D = c[0]*s2*s2*s2 + 3*c[1]*s2*s2*t2 + 3*c[2]*s2*t2*t2 + c[3]*t2*t2*t2

 We don't have B or C. So We define two equations to isolate them.
 First, compute two reference T values 1/3 and 2/3 from t1 to t2:

 c(at (2*t1 + t2)/3) == E
 c(at (t1 + 2*t2)/3) == F

 Next, compute where those values must be if we know the values of B and C:

 _12   =  A*2/3 + B*1/3
 12_   =  A*1/3 + B*2/3
 _23   =  B*2/3 + C*1/3
 23_   =  B*1/3 + C*2/3
 _34   =  C*2/3 + D*1/3
 34_   =  C*1/3 + D*2/3
 _123  = (A*2/3 + B*1/3)*2/3 + (B*2/3 + C*1/3)*1/3 = A*4/9 + B*4/9 + C*1/9
 123_  = (A*1/3 + B*2/3)*1/3 + (B*1/3 + C*2/3)*2/3 = A*1/9 + B*4/9 + C*4/9
 _234  = (B*2/3 + C*1/3)*2/3 + (C*2/3 + D*1/3)*1/3 = B*4/9 + C*4/9 + D*1/9
 234_  = (B*1/3 + C*2/3)*1/3 + (C*1/3 + D*2/3)*2/3 = B*1/9 + C*4/9 + D*4/9
 _1234 = (A*4/9 + B*4/9 + C*1/9)*2/3 + (B*4/9 + C*4/9 + D*1/9)*1/3
       =  A*8/27 + B*12/27 + C*6/27 + D*1/27
       =  E
 1234_ = (A*1/9 + B*4/9 + C*4/9)*1/3 + (B*1/9 + C*4/9 + D*4/9)*2/3
       =  A*1/27 + B*6/27 + C*12/27 + D*8/27
       =  F
 E*27  =  A*8    + B*12   + C*6     + D
 F*27  =  A      + B*6    + C*12    + D*8

Group the known values on one side:

 M       = E*27 - A*8 - D     = B*12 + C* 6
 N       = F*27 - A   - D*8   = B* 6 + C*12
 M*2 - N = B*18
 N*2 - M = C*18
 B       = (M*2 - N)/18
 C       = (N*2 - M)/18
 */

/// `interp_cubic_coords(const double* src, double t)`: `src` at a stride of two.
fn interp_cubic_coords(src: &[f64], t: f64) -> f64 {
    let ab = sk_d_interp(src[0], src[2], t);
    let bc = sk_d_interp(src[2], src[4], t);
    let cd = sk_d_interp(src[4], src[6], t);
    let abc = sk_d_interp(ab, bc, t);
    let bcd = sk_d_interp(bc, cd, t);
    sk_d_interp(abc, bcd, t)
}

/// `SK_DoublePI`.
const SK_DOUBLE_PI: f64 = std::f64::consts::PI;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkDCubic {
    pub(crate) pts: [SkDPoint; 4],
}

impl Index<usize> for SkDCubic {
    type Output = SkDPoint;

    fn index(&self, n: usize) -> &SkDPoint {
        debug_assert!(n < SkDCubic::K_POINT_COUNT as usize);
        &self.pts[n]
    }
}

impl IndexMut<usize> for SkDCubic {
    fn index_mut(&mut self, n: usize) -> &mut SkDPoint {
        debug_assert!(n < SkDCubic::K_POINT_COUNT as usize);
        &mut self.pts[n]
    }
}

impl SkDCubic {
    pub(crate) const K_POINT_COUNT: i32 = 4;
    pub(crate) const K_POINT_LAST: i32 = SkDCubic::K_POINT_COUNT - 1;
    pub(crate) const K_MAX_INTERSECTIONS: i32 = 9;
    /// FIXME: test different values in test framework
    pub(crate) const G_PRECISION_UNIT: i32 = 256;

    pub(crate) fn collapsed(&self) -> bool {
        self.pts[0].approximately_equal(&self.pts[1])
            && self.pts[0].approximately_equal(&self.pts[2])
            && self.pts[0].approximately_equal(&self.pts[3])
    }

    pub(crate) fn controls_inside(&self) -> bool {
        let v01 = self.pts[0] - self.pts[1];
        let v02 = self.pts[0] - self.pts[2];
        let v03 = self.pts[0] - self.pts[3];
        let v13 = self.pts[1] - self.pts[3];
        let v23 = self.pts[2] - self.pts[3];
        v03.dot(&v01) > 0.0 && v03.dot(&v02) > 0.0 && v03.dot(&v13) > 0.0 && v03.dot(&v23) > 0.0
    }

    pub(crate) fn align(&self, end_index: i32, ctrl_index: i32, dst_pt: &mut SkDPoint) {
        let (end_index, ctrl_index) = (end_index as usize, ctrl_index as usize);
        if self.pts[end_index].x == self.pts[ctrl_index].x {
            dst_pt.x = self.pts[end_index].x;
        }
        if self.pts[end_index].y == self.pts[ctrl_index].y {
            dst_pt.y = self.pts[end_index].y;
        }
    }

    pub(crate) fn binary_search(
        &self,
        min: f64,
        max: f64,
        axis_intercept: f64,
        x_axis: SearchAxis,
    ) -> f64 {
        let mut t = (min + max) / 2.0;
        let mut step = (t - min) / 2.0;
        let mut cubic_at_t = self.pt_at_t(t);
        let mut calc_pos = axis_coord(&cubic_at_t, x_axis);
        let mut calc_dist = calc_pos - axis_intercept;
        loop {
            let prior_t = std_max(min, t - step);
            let less_pt = self.pt_at_t(prior_t);
            if approximately_equal_half(less_pt.x, cubic_at_t.x)
                && approximately_equal_half(less_pt.y, cubic_at_t.y)
            {
                return -1.0; // binary search found no point at this axis intercept
            }
            let less_dist = axis_coord(&less_pt, x_axis) - axis_intercept;
            let last_step = step;
            step /= 2.0;
            // Skia's `continue` jumps to the loop's condition, skipping the new point.
            let mut moved = true;
            if if calc_dist > 0.0 {
                calc_dist > less_dist
            } else {
                calc_dist < less_dist
            } {
                t = prior_t;
            } else {
                let next_t = t + last_step;
                if next_t > max {
                    return -1.0;
                }
                let more_pt = self.pt_at_t(next_t);
                if approximately_equal_half(more_pt.x, cubic_at_t.x)
                    && approximately_equal_half(more_pt.y, cubic_at_t.y)
                {
                    return -1.0; // binary search found no point at this axis intercept
                }
                let more_dist = axis_coord(&more_pt, x_axis) - axis_intercept;
                if if calc_dist > 0.0 {
                    calc_dist <= more_dist
                } else {
                    calc_dist >= more_dist
                } {
                    moved = false;
                } else {
                    t = next_t;
                }
            }
            if moved {
                let test_at_t = self.pt_at_t(t);
                cubic_at_t = test_at_t;
                calc_pos = axis_coord(&cubic_at_t, x_axis);
                calc_dist = calc_pos - axis_intercept;
            }
            if approximately_equal(calc_pos, axis_intercept) {
                break;
            }
        }
        t
    }

    pub(crate) fn calc_precision(&self) -> f64 {
        ((self.pts[1] - self.pts[0]).length()
            + (self.pts[2] - self.pts[1]).length()
            + (self.pts[3] - self.pts[2]).length())
            / SkDCubic::G_PRECISION_UNIT as f64
    }

    pub(crate) fn chop_at(&self, t: f64) -> SkDCubicPair {
        let mut dst = SkDCubicPair::default();
        let p = &self.pts;
        if t == 0.5 {
            dst.pts[0] = p[0];
            dst.pts[1].x = (p[0].x + p[1].x) / 2.0;
            dst.pts[1].y = (p[0].y + p[1].y) / 2.0;
            dst.pts[2].x = (p[0].x + 2.0 * p[1].x + p[2].x) / 4.0;
            dst.pts[2].y = (p[0].y + 2.0 * p[1].y + p[2].y) / 4.0;
            dst.pts[3].x = (p[0].x + 3.0 * (p[1].x + p[2].x) + p[3].x) / 8.0;
            dst.pts[3].y = (p[0].y + 3.0 * (p[1].y + p[2].y) + p[3].y) / 8.0;
            dst.pts[4].x = (p[1].x + 2.0 * p[2].x + p[3].x) / 4.0;
            dst.pts[4].y = (p[1].y + 2.0 * p[2].y + p[3].y) / 4.0;
            dst.pts[5].x = (p[2].x + p[3].x) / 2.0;
            dst.pts[5].y = (p[2].y + p[3].y) / 2.0;
            dst.pts[6] = p[3];
            return dst;
        }
        let src = self.coords();
        let mut dst_coords = [0.0; 14];
        interp_cubic_coords_chop(&src[0..], &mut dst_coords[0..], t);
        interp_cubic_coords_chop(&src[1..], &mut dst_coords[1..], t);
        for (index, pt) in dst.pts.iter_mut().enumerate() {
            pt.x = dst_coords[index * 2];
            pt.y = dst_coords[index * 2 + 1];
        }
        dst
    }

    pub(crate) fn coefficients(cubic: &[f64], a: &mut f64, b: &mut f64, c: &mut f64, d: &mut f64) {
        let src = cubic;
        *a = src[6]; // d
        *b = src[4] * 3.0; // 3*c
        *c = src[2] * 3.0; // 3*b
        *d = src[0]; // a
        *a -= *d - *c + *b; // A =   -a + 3*b - 3*c + d
        *b += 3.0 * *d - 2.0 * *c; // B =  3*a - 6*b + 3*c
        *c -= 3.0 * *d; // C = -3*a + 3*b
    }

    pub(crate) fn complex_break(pts: &[SkPoint], t: &mut [f32]) -> i32 {
        let points_ptr = pts;
        let mut cubic = SkDCubic::default();
        cubic.set(points_ptr);
        if cubic.monotonic_in_x() && cubic.monotonic_in_y() {
            return 0;
        }
        let mut tt = [0.0; 2];
        let mut ss = [0.0; 2];
        let cubic_type = sk_classify_cubic(points_ptr, Some(&mut tt), Some(&mut ss), None);
        // Skia's `kLoop` case falls through to the others when it finds no t value.
        if cubic_type == SkCubicType::Loop {
            let (td, te, sd, se) = (tt[0], tt[1], ss[0], ss[1]);
            if roughly_between(0.0, td, sd) && roughly_between(0.0, te, se) {
                t[0] = ((td * se + te * sd) / (2.0 * sd * se)) as f32;
                return i32::from(t[0] > 0.0 && t[0] < 1.0);
            }
        }
        match cubic_type {
            SkCubicType::Loop
            | SkCubicType::Serpentine
            | SkCubicType::LocalCusp
            | SkCubicType::CuspAtInfinity => {
                let mut inflection_ts = [0.0; 2];
                let inf_t_count = cubic.find_inflections(&mut inflection_ts);
                let mut max_curvature = [0.0; 3];
                let roots = cubic.find_max_curvature(&mut max_curvature);
                if inf_t_count == 2 {
                    for index in 0..roots as usize {
                        if between(inflection_ts[0], max_curvature[index], inflection_ts[1]) {
                            t[0] = max_curvature[index] as f32;
                            return i32::from(t[0] > 0.0 && t[0] < 1.0);
                        }
                    }
                } else {
                    let mut result_count = 0;
                    // FIXME: constant found through experimentation -- maybe there's a better way....
                    let precision = cubic.calc_precision() * 2.0;
                    let coords = cubic.coords();
                    for index in 0..roots as usize {
                        let test_t = max_curvature[index];
                        if 0.0 >= test_t || test_t >= 1.0 {
                            continue;
                        }
                        // don't call dxdyAtT since we want (0,0) results
                        let d_pt = SkDVector {
                            x: derivative_at_t(&coords[0..], test_t),
                            y: derivative_at_t(&coords[1..], test_t),
                        };
                        let d_pt_len = d_pt.length();
                        if d_pt_len < precision {
                            t[result_count] = test_t as f32;
                            result_count += 1;
                        }
                    }
                    if result_count == 0 && inf_t_count == 1 {
                        t[0] = inflection_ts[0] as f32;
                        result_count = usize::from(t[0] > 0.0 && t[0] < 1.0);
                    }
                    return result_count as i32;
                }
            }
            _ => {}
        }
        0
    }

    pub(crate) fn dxdy_at_t(&self, t: f64) -> SkDVector {
        let coords = self.coords();
        let mut result = SkDVector {
            x: derivative_at_t(&coords[0..], t),
            y: derivative_at_t(&coords[1..], t),
        };
        if result.x == 0.0 && result.y == 0.0 {
            if t == 0.0 {
                result = self.pts[2] - self.pts[0];
            } else if t == 1.0 {
                result = self.pts[3] - self.pts[1];
            } else {
                // incomplete
                // Skia prints "!c" with SkDebugf here.
            }
            if result.x == 0.0 && result.y == 0.0 && zero_or_one(t) {
                result = self.pts[3] - self.pts[0];
            }
        }
        result
    }

    pub(crate) fn find_extrema(src: &[f64], t_value: &mut [f64]) -> i32 {
        // we divide A,B,C by 3 to simplify
        let a = src[0];
        let b = src[2];
        let c = src[4];
        let d = src[6];
        let a_coeff = d - a + 3.0 * (b - c);
        let b_coeff = 2.0 * (a - b - b + c);
        let c_coeff = b - a;

        SkDQuad::roots_valid_t(a_coeff, b_coeff, c_coeff, t_value)
    }

    pub(crate) fn find_inflections(&self, t_values: &mut [f64]) -> i32 {
        let p = &self.pts;
        let ax = p[1].x - p[0].x;
        let ay = p[1].y - p[0].y;
        let bx = p[2].x - 2.0 * p[1].x + p[0].x;
        let by = p[2].y - 2.0 * p[1].y + p[0].y;
        let cx = p[3].x + 3.0 * (p[1].x - p[2].x) - p[0].x;
        let cy = p[3].y + 3.0 * (p[1].y - p[2].y) - p[0].y;
        SkDQuad::roots_valid_t(
            bx * cy - by * cx,
            ax * cy - ay * cx,
            ax * by - ay * bx,
            t_values,
        )
    }

    /// `static FindInflections(const SkPoint a[kPointCount], double tValues[2])`.
    pub(crate) fn find_inflections_pts(a: &[SkPoint], t_values: &mut [f64]) -> i32 {
        let mut cubic = SkDCubic::default();
        cubic.set(a).find_inflections(t_values)
    }

    pub(crate) fn find_max_curvature(&self, t_values: &mut [f64]) -> i32 {
        let mut coeff_x = [0.0; 4];
        let mut coeff_y = [0.0; 4];
        let coords = self.coords();
        formulate_f1_dot_f2(&coords[0..], &mut coeff_x);
        formulate_f1_dot_f2(&coords[1..], &mut coeff_y);
        for i in 0..4 {
            coeff_x[i] += coeff_y[i];
        }
        SkDCubic::roots_valid_t(coeff_x[0], coeff_x[1], coeff_x[2], coeff_x[3], t_values)
    }

    pub(crate) fn hull_intersects_cubic(&self, c2: &SkDCubic, is_linear: &mut bool) -> bool {
        self.hull_intersects_pts(&c2.pts, SkDCubic::K_POINT_COUNT, is_linear)
    }

    pub(crate) fn hull_intersects_conic(&self, c: &SkDConic, is_linear: &mut bool) -> bool {
        self.hull_intersects_quad(&c.pts, is_linear)
    }

    pub(crate) fn hull_intersects_quad(&self, c2: &SkDQuad, is_linear: &mut bool) -> bool {
        self.hull_intersects_pts(&c2.pts, SkDQuad::K_POINT_COUNT, is_linear)
    }

    /// `hullIntersects(const SkDPoint* pts, int ptCount, bool* isLinear)`.
    pub(crate) fn hull_intersects_pts(
        &self,
        pts: &[SkDPoint],
        pt_count: i32,
        is_linear: &mut bool,
    ) -> bool {
        let mut linear = true;
        let mut hull_order = [0_i8; 4];
        let hull_count = self.convex_hull(&mut hull_order);
        let mut end1 = hull_order[0] as i32;
        let mut hull_index = 0;
        // Skia keeps `endPt[0]` and `endPt[1]` as pointers into `fPts`; here their indices.
        loop {
            hull_index = (hull_index + 1) % hull_count;
            let end2 = hull_order[hull_index as usize] as i32;
            let end_pt0 = self.pts[end1 as usize];
            let end_pt1 = self.pts[end2 as usize];
            let orig_x = end_pt0.x;
            let orig_y = end_pt0.y;
            let adj = end_pt1.x - orig_x;
            let opp = end_pt1.y - orig_y;
            let odd_man_mask = other_two(end1, end2);
            let odd_man = (end1 ^ odd_man_mask) as usize;
            let mut sign =
                (self.pts[odd_man].y - orig_y) * adj - (self.pts[odd_man].x - orig_x) * opp;
            let odd_man2 = (end2 ^ odd_man_mask) as usize;
            let sign2 =
                (self.pts[odd_man2].y - orig_y) * adj - (self.pts[odd_man2].x - orig_x) * opp;
            // Skia's `continue` jumps to the loop's condition, `while (hullIndex)`, leaving
            // `endPt[0]` and `end1` unchanged.
            'body: {
                if sign * sign2 < 0.0 {
                    break 'body;
                }
                if approximately_zero(sign) {
                    sign = sign2;
                    if approximately_zero(sign) {
                        break 'body;
                    }
                }
                linear = false;
                let mut found_outlier = false;
                for n in 0..pt_count as usize {
                    let test = (pts[n].y - orig_y) * adj - (pts[n].x - orig_x) * opp;
                    if test * sign > 0.0 && !precisely_zero(test) {
                        found_outlier = true;
                        break;
                    }
                }
                if !found_outlier {
                    return false;
                }
                end1 = end2;
            }
            if hull_index == 0 {
                break;
            }
        }
        *is_linear = linear;
        true
    }

    pub(crate) fn is_linear(&self, start_index: i32, end_index: i32) -> bool {
        if self.pts[0].approximately_d_equal(&self.pts[3]) {
            // Skia reads the cubic's first three points as a quad.
            let quad = SkDQuad {
                pts: [self.pts[0], self.pts[1], self.pts[2]],
            };
            return quad.is_linear(0, 2);
        }
        let mut line_parameters = SkLineParameters::default();
        line_parameters.cubic_end_points_range(self, start_index as usize, end_index as usize);
        // FIXME: maybe it's possible to avoid this and compare non-normalized
        line_parameters.normalize();
        let p = &self.pts;
        let tiniest = std_min(
            std_min(
                std_min(
                    std_min(
                        std_min(std_min(std_min(p[0].x, p[0].y), p[1].x), p[1].y),
                        p[2].x,
                    ),
                    p[2].y,
                ),
                p[3].x,
            ),
            p[3].y,
        );
        let mut largest = std_max(
            std_max(
                std_max(
                    std_max(
                        std_max(std_max(std_max(p[0].x, p[0].y), p[1].x), p[1].y),
                        p[2].x,
                    ),
                    p[2].y,
                ),
                p[3].x,
            ),
            p[3].y,
        );
        largest = std_max(largest, -tiniest);
        let mut distance = line_parameters.control_pt_distance_cubic(self, 1);
        if !approximately_zero_when_compared_to(distance, largest) {
            return false;
        }
        distance = line_parameters.control_pt_distance_cubic(self, 2);
        approximately_zero_when_compared_to(distance, largest)
    }

    pub(crate) fn monotonic_in_x(&self) -> bool {
        precisely_between(self.pts[0].x, self.pts[1].x, self.pts[3].x)
            && precisely_between(self.pts[0].x, self.pts[2].x, self.pts[3].x)
    }

    pub(crate) fn monotonic_in_y(&self) -> bool {
        precisely_between(self.pts[0].y, self.pts[1].y, self.pts[3].y)
            && precisely_between(self.pts[0].y, self.pts[2].y, self.pts[3].y)
    }

    /// Skia fills `o1Pts` with pointers into `fPts`; here with copies of those points.
    pub(crate) fn other_pts(&self, index: i32, o1_pts: &mut [SkDPoint]) {
        let mut offset = usize::from(index == 0);
        o1_pts[0] = self.pts[offset];
        offset += 1;
        o1_pts[1] = self.pts[offset];
        offset += 1;
        o1_pts[2] = self.pts[offset];
    }

    pub(crate) fn pt_at_t(&self, t: f64) -> SkDPoint {
        if 0.0 == t {
            return self.pts[0];
        }
        if 1.0 == t {
            return self.pts[3];
        }
        let one_t = 1.0 - t;
        let one_t2 = one_t * one_t;
        let a = one_t2 * one_t;
        let b = 3.0 * one_t2 * t;
        let t2 = t * t;
        let c = 3.0 * one_t * t2;
        let d = t2 * t;
        let p = &self.pts;
        SkDPoint {
            x: a * p[0].x + b * p[1].x + c * p[2].x + d * p[3].x,
            y: a * p[0].y + b * p[1].y + c * p[2].y + d * p[3].y,
        }
    }

    pub(crate) fn roots_real(a: f64, b: f64, c: f64, d: f64, t: &mut [f64]) -> i32 {
        let mut a_coeff = a;
        let (b_coeff, c_coeff, d_coeff) = (b, c, d);
        let s = t;
        if approximately_zero(a_coeff)
            && approximately_zero_when_compared_to(a_coeff, b_coeff)
            && approximately_zero_when_compared_to(a_coeff, c_coeff)
            && approximately_zero_when_compared_to(a_coeff, d_coeff)
        {
            // we're just a quadratic
            return SkDQuad::roots_real(b_coeff, c_coeff, d_coeff, s);
        }
        if approximately_zero_when_compared_to(d_coeff, a_coeff)
            && approximately_zero_when_compared_to(d_coeff, b_coeff)
            && approximately_zero_when_compared_to(d_coeff, c_coeff)
        {
            // 0 is one root
            let mut num = SkDQuad::roots_real(a_coeff, b_coeff, c_coeff, s);
            for i in 0..num as usize {
                if approximately_zero(s[i]) {
                    return num;
                }
            }
            s[num as usize] = 0.0;
            num += 1;
            return num;
        }
        if approximately_zero(a_coeff + b_coeff + c_coeff + d_coeff) {
            // 1 is one root
            let mut num = SkDQuad::roots_real(a_coeff, a_coeff + b_coeff, -d_coeff, s);
            for i in 0..num as usize {
                if almost_dequal_ulps(s[i], 1.0) {
                    return num;
                }
            }
            s[num as usize] = 1.0;
            num += 1;
            return num;
        }
        let a;
        let b;
        let c;
        {
            let inv_a = 1.0 / a_coeff;
            a = b_coeff * inv_a;
            b = c_coeff * inv_a;
            c = d_coeff * inv_a;
        }
        let a2 = a * a;
        let q = (a2 - b * 3.0) / 9.0;
        let r_coeff = (2.0 * a2 * a - 9.0 * a * b + 27.0 * c) / 54.0;
        let r2 = r_coeff * r_coeff;
        let q3 = q * q * q;
        let r2_minus_q3 = r2 - q3;
        let adiv3 = a / 3.0;
        let mut r;
        // Skia's `roots` pointer walks `s`; here its index.
        let mut roots = 0;
        if r2_minus_q3 < 0.0 {
            // we have 3 real roots
            // the divide/root can, due to finite precisions, be slightly outside of -1...1
            let theta = sk_t_pin(r_coeff / q3.sqrt(), -1.0, 1.0).acos();
            let neg2_root_q = -2.0 * q.sqrt();

            r = neg2_root_q * (theta / 3.0).cos() - adiv3;
            s[roots] = r;
            roots += 1;

            r = neg2_root_q * ((theta + 2.0 * SK_DOUBLE_PI) / 3.0).cos() - adiv3;
            if !almost_dequal_ulps(s[0], r) {
                s[roots] = r;
                roots += 1;
            }
            r = neg2_root_q * ((theta - 2.0 * SK_DOUBLE_PI) / 3.0).cos() - adiv3;
            if !almost_dequal_ulps(s[0], r) && (roots == 1 || !almost_dequal_ulps(s[1], r)) {
                s[roots] = r;
                roots += 1;
            }
        } else {
            // we have 1 real root
            let sqrt_r2_minus_q3 = r2_minus_q3.sqrt();
            a_coeff = r_coeff.abs() + sqrt_r2_minus_q3;
            a_coeff = a_coeff.cbrt(); // cube root
            if r_coeff > 0.0 {
                a_coeff = -a_coeff;
            }
            if a_coeff != 0.0 {
                a_coeff += q / a_coeff;
            }
            r = a_coeff - adiv3;
            s[roots] = r;
            roots += 1;
            if almost_dequal_ulps(r2, q3) {
                r = -a_coeff / 2.0 - adiv3;
                if !almost_dequal_ulps(s[0], r) {
                    s[roots] = r;
                    roots += 1;
                }
            }
        }
        roots as i32
    }

    pub(crate) fn roots_valid_t(a: f64, b: f64, c: f64, d: f64, s: &mut [f64]) -> i32 {
        let t = s;
        let mut s = [0.0; 3];
        let real_roots = SkDCubic::roots_real(a, b, c, d, &mut s);
        let mut found_roots = SkDQuad::add_valid_ts(&mut s, real_roots, t);
        'next_root: for index in 0..real_roots as usize {
            let t_value = s[index];
            if !approximately_one_or_less(t_value) && between(1.0, t_value, 1.00005) {
                for idx2 in 0..found_roots as usize {
                    if approximately_equal(t[idx2], 1.0) {
                        continue 'next_root;
                    }
                }
                debug_assert!(found_roots < 3);
                t[found_roots as usize] = 1.0;
                found_roots += 1;
            } else if !approximately_zero_or_more(t_value) && between(-0.00005, t_value, 0.0) {
                for idx2 in 0..found_roots as usize {
                    if approximately_equal(t[idx2], 0.0) {
                        continue 'next_root;
                    }
                }
                debug_assert!(found_roots < 3);
                t[found_roots as usize] = 0.0;
                found_roots += 1;
            }
        }
        found_roots
    }

    pub(crate) fn search_roots(
        &self,
        extremes: &mut [f64],
        extrema: i32,
        axis_intercept: f64,
        x_axis: SearchAxis,
        valid_roots: &mut [f64],
    ) -> i32 {
        let extreme_ts = extremes;
        let mut extrema = extrema as usize;
        extrema += self.find_inflections(&mut extreme_ts[extrema..]) as usize;
        extreme_ts[extrema] = 0.0;
        extrema += 1;
        extreme_ts[extrema] = 1.0;
        debug_assert!(extrema < 6);
        sk_t_q_sort(&mut extreme_ts[..extrema + 1]);
        let mut valid_count = 0;
        let mut index = 0;
        while index < extrema {
            let min = extreme_ts[index];
            index += 1;
            let max = extreme_ts[index];
            if min == max {
                continue;
            }
            let new_t = self.binary_search(min, max, axis_intercept, x_axis);
            if new_t >= 0.0 {
                if valid_count >= 3 {
                    return 0;
                }
                valid_roots[valid_count] = new_t;
                valid_count += 1;
            }
        }
        valid_count as i32
    }

    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn to_float_points(&self, pts: &mut [SkPoint]) -> bool {
        let d_cubic = self.coords();
        let mut cubic = [0.0_f32; 8];
        for index in 0..(SkDCubic::K_POINT_COUNT * 2) as usize {
            cubic[index] = d_cubic[index] as f32;
            if (sk_scalar_abs(cubic[index]) as f64) < FLT_EPSILON_ORDERABLE_ERR {
                cubic[index] = 0.0;
            }
        }
        for (index, pt) in pts
            .iter_mut()
            .take(SkDCubic::K_POINT_COUNT as usize)
            .enumerate()
        {
            pt.x = cubic[index * 2];
            pt.y = cubic[index * 2 + 1];
        }
        sk_is_finite_f32(&cubic)
    }

    // add debug only global pointer so asserts can be skipped by fuzzers
    pub(crate) fn set(&mut self, pts: &[SkPoint]) -> &SkDCubic {
        self.pts[0].set(&pts[0]);
        self.pts[1].set(&pts[1]);
        self.pts[2].set(&pts[2]);
        self.pts[3].set(&pts[3]);
        self
    }

    pub(crate) fn sub_divide(&self, t1: f64, t2: f64) -> SkDCubic {
        if t1 == 0.0 || t2 == 1.0 {
            if t1 == 0.0 && t2 == 1.0 {
                return *self;
            }
            let pair = self.chop_at(if t1 == 0.0 { t2 } else { t1 });
            let dst = if t1 == 0.0 {
                pair.first()
            } else {
                pair.second()
            };
            return dst;
        }
        let src = self.coords();
        let (px, py) = (&src[0..], &src[1..]);
        let mut dst = SkDCubic::default();
        dst[0].x = interp_cubic_coords(px, t1);
        let ax = dst[0].x;
        dst[0].y = interp_cubic_coords(py, t1);
        let ay = dst[0].y;
        let ex = interp_cubic_coords(px, (t1 * 2.0 + t2) / 3.0);
        let ey = interp_cubic_coords(py, (t1 * 2.0 + t2) / 3.0);
        let fx = interp_cubic_coords(px, (t1 + t2 * 2.0) / 3.0);
        let fy = interp_cubic_coords(py, (t1 + t2 * 2.0) / 3.0);
        dst[3].x = interp_cubic_coords(px, t2);
        let dx = dst[3].x;
        dst[3].y = interp_cubic_coords(py, t2);
        let dy = dst[3].y;
        let mx = ex * 27.0 - ax * 8.0 - dx;
        let my = ey * 27.0 - ay * 8.0 - dy;
        let nx = fx * 27.0 - ax - dx * 8.0;
        let ny = fy * 27.0 - ay - dy * 8.0;
        /* bx = */
        dst[1].x = (mx * 2.0 - nx) / 18.0;
        /* by = */
        dst[1].y = (my * 2.0 - ny) / 18.0;
        /* cx = */
        dst[2].x = (nx * 2.0 - mx) / 18.0;
        /* cy = */
        dst[2].y = (ny * 2.0 - my) / 18.0;
        // FIXME: call align() ?
        dst
    }

    /// `static SubDivide(const SkPoint a[kPointCount], double t1, double t2)`.
    pub(crate) fn sub_divide_pts(a: &[SkPoint], t1: f64, t2: f64) -> SkDCubic {
        let mut cubic = SkDCubic::default();
        cubic.set(a).sub_divide(t1, t2)
    }

    /// `subDivide(const SkDPoint& a, const SkDPoint& d, double t1, double t2, SkDPoint p[2])`.
    pub(crate) fn sub_divide_control(
        &self,
        a: &SkDPoint,
        d: &SkDPoint,
        t1: f64,
        t2: f64,
        p: &mut [SkDPoint],
    ) {
        let dst = p;
        debug_assert!(t1 != t2);
        // this approach assumes that the control points computed directly are accurate enough
        let sub = self.sub_divide(t1, t2);
        dst[0] = sub[1] + (*a - sub[0]);
        dst[1] = sub[2] + (*d - sub[3]);
        if t1 == 0.0 || t2 == 0.0 {
            self.align(0, 1, if t1 == 0.0 { &mut dst[0] } else { &mut dst[1] });
        }
        if t1 == 1.0 || t2 == 1.0 {
            self.align(3, 2, if t1 == 1.0 { &mut dst[0] } else { &mut dst[1] });
        }
        if almost_bequal_ulps(dst[0].x, a.x) {
            dst[0].x = a.x;
        }
        if almost_bequal_ulps(dst[0].y, a.y) {
            dst[0].y = a.y;
        }
        if almost_bequal_ulps(dst[1].x, d.x) {
            dst[1].x = d.x;
        }
        if almost_bequal_ulps(dst[1].y, d.y) {
            dst[1].y = d.y;
        }
    }

    /// The points' coordinates interleaved, `[x0, y0, ..., x3, y3]`: what Skia reads through
    /// `&fPts[0].fX` with a stride of two.
    pub(crate) fn coords(&self) -> [f64; 8] {
        let p = &self.pts;
        [
            p[0].x, p[0].y, p[1].x, p[1].y, p[2].x, p[2].y, p[3].x, p[3].y,
        ]
    }

    /// `static SubDivide(const SkPoint pts[kPointCount], const SkDPoint& a, const SkDPoint& d,
    /// double t1, double t2, SkDPoint p[2])`.
    pub(crate) fn sub_divide_pts_control(
        pts: &[SkPoint],
        a: &SkDPoint,
        d: &SkDPoint,
        t1: f64,
        t2: f64,
        p: &mut [SkDPoint],
    ) {
        let mut cubic = SkDCubic::default();
        cubic.set(pts).sub_divide_control(a, d, t1, t2, p);
    }
}

/* Given a pair of indices into a cubic's points, other_two computes the other two.

   one ^ two == 3 for (0, 3), (1, 2)
   one ^ two <  3 for (0, 1), (0, 2), (1, 3), (2, 3)
   3 - (one ^ two) is either 0, 1, or 2
   1 >> (3 - (one ^ two)) is either 0 or 1
thus:
   returned == 2 for (0, 3), (1, 2)
   returned == 3 for (0, 1), (0, 2), (1, 3), (2, 3)
given that:
   (0, 3) ^ 2 -> (2, 1)  (1, 2) ^ 2 -> (3, 0)
   (0, 1) ^ 3 -> (3, 2)  (0, 2) ^ 3 -> (3, 1)  (1, 3) ^ 3 -> (2, 0)  (2, 3) ^ 3 -> (1, 0)
*/
pub(crate) fn other_two(one: i32, two: i32) -> i32 {
    (1 >> (3 - (one ^ two))) ^ 3
}

/// `SkDCubicPair`: two cubics sharing their middle point, `first` at `pts[0..4]` and
/// `second` at `pts[3..7]` (Skia casts the array to an `SkDCubic` at either offset).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkDCubicPair {
    pub(crate) pts: [SkDPoint; 7],
}

impl SkDCubicPair {
    pub(crate) fn first(&self) -> SkDCubic {
        SkDCubic {
            pts: [self.pts[0], self.pts[1], self.pts[2], self.pts[3]],
        }
    }

    pub(crate) fn second(&self) -> SkDCubic {
        SkDCubic {
            pts: [self.pts[3], self.pts[4], self.pts[5], self.pts[6]],
        }
    }
}

/// `SkTCubic`: an `SkDCubic` as an `SkTCurve`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkTCubic {
    pub(crate) cubic: SkDCubic,
}

impl SkTCubic {
    pub(crate) fn new(c: &SkDCubic) -> SkTCubic {
        SkTCubic { cubic: *c }
    }

    pub(crate) fn collapsed(&self) -> bool {
        self.cubic.collapsed()
    }

    pub(crate) fn controls_inside(&self) -> bool {
        self.cubic.controls_inside()
    }

    pub(crate) fn dxdy_at_t(&self, t: f64) -> SkDVector {
        self.cubic.dxdy_at_t(t)
    }

    pub(crate) fn hull_intersects_quad(&self, quad: &SkDQuad, is_linear: &mut bool) -> bool {
        quad.hull_intersects_cubic(&self.cubic, is_linear)
    }

    pub(crate) fn hull_intersects_conic(&self, conic: &SkDConic, is_linear: &mut bool) -> bool {
        conic.hull_intersects_cubic(&self.cubic, is_linear)
    }

    pub(crate) fn hull_intersects_cubic(&self, cubic: &SkDCubic, is_linear: &mut bool) -> bool {
        cubic.hull_intersects_cubic(&self.cubic, is_linear)
    }

    pub(crate) fn hull_intersects_t_curve(&self, curve: &SkTCurve, is_linear: &mut bool) -> bool {
        curve.hull_intersects_cubic(&self.cubic, is_linear)
    }

    pub(crate) fn intersect_ray(&self, i: &mut SkIntersections, line: &SkDLine) -> i32 {
        i.intersect_ray_cubic(&self.cubic, line)
    }

    pub(crate) fn max_intersections(&self) -> i32 {
        SkDCubic::K_MAX_INTERSECTIONS
    }

    /// Skia passes a two-element `endPt` to the cubic's three-element `otherPts` (which writes
    /// past it); nothing calls this. The slice makes that overrun a panic.
    pub(crate) fn other_pts(&self, odd_man: i32, end_pt: &mut [SkDPoint]) {
        self.cubic.other_pts(odd_man, end_pt);
    }

    pub(crate) fn point_count(&self) -> i32 {
        SkDCubic::K_POINT_COUNT
    }

    pub(crate) fn point_last(&self) -> i32 {
        SkDCubic::K_POINT_LAST
    }

    pub(crate) fn pt_at_t(&self, t: f64) -> SkDPoint {
        self.cubic.pt_at_t(t)
    }

    pub(crate) fn set_bounds(&self, rect: &mut SkDRect) {
        rect.set_bounds_cubic(&self.cubic);
    }
}
