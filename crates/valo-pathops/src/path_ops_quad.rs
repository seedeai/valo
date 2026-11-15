//! Port of Skia's src/pathops/SkPathOpsQuad.{h,cpp}: a quadratic Bézier in doubles, and
//! `SkTQuad`, its `SkTCurve` for the TSect intersection.
//!
//! Skia passes `&fPts[0].fX` (or `.fY`) as a `const double*` with a stride of two to
//! `FindExtrema` and `SetABC`; here those take a slice of the interleaved coordinates
//! `[x0, y0, x1, y1, ...]`, starting at the x or y of the first point.

// Skia indexes its arrays; the loops keep its indices.
#![allow(clippy::needless_range_loop)]

use std::ops::{Index, IndexMut};

use crate::core::float_bits::{std_max, std_min};
use crate::core::point::SkPoint;
use crate::intersections::SkIntersections;
use crate::line_parameters::SkLineParameters;
use crate::path_ops_conic::SkDConic;
use crate::path_ops_cubic::SkDCubic;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::{SkDPoint, SkDVector};
use crate::path_ops_rect::SkDRect;
use crate::path_ops_t_curve::SkTCurve;
use crate::path_ops_types::{
    almost_bequal_ulps, almost_dequal_ulps, approximately_equal, approximately_greater_than_one,
    approximately_less_than_zero, approximately_one_or_less, approximately_zero,
    approximately_zero_inverse, approximately_zero_or_more, approximately_zero_when_compared_to,
    between, precisely_zero, sk_d_interp, zero_or_one,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkDQuad {
    pub(crate) pts: [SkDPoint; 3],
}

impl Index<usize> for SkDQuad {
    type Output = SkDPoint;

    fn index(&self, n: usize) -> &SkDPoint {
        debug_assert!(n < SkDQuad::K_POINT_COUNT as usize);
        &self.pts[n]
    }
}

impl IndexMut<usize> for SkDQuad {
    fn index_mut(&mut self, n: usize) -> &mut SkDPoint {
        debug_assert!(n < SkDQuad::K_POINT_COUNT as usize);
        &mut self.pts[n]
    }
}

impl SkDQuad {
    pub(crate) const K_POINT_COUNT: i32 = 3;
    pub(crate) const K_POINT_LAST: i32 = SkDQuad::K_POINT_COUNT - 1;
    pub(crate) const K_MAX_INTERSECTIONS: i32 = 4;

    pub(crate) fn collapsed(&self) -> bool {
        self.pts[0].approximately_equal(&self.pts[1])
            && self.pts[0].approximately_equal(&self.pts[2])
    }

    pub(crate) fn controls_inside(&self) -> bool {
        let v01 = self.pts[0] - self.pts[1];
        let v02 = self.pts[0] - self.pts[2];
        let v12 = self.pts[1] - self.pts[2];
        v02.dot(&v01) > 0.0 && v02.dot(&v12) > 0.0
    }

    pub(crate) fn set(&mut self, pts: &[SkPoint]) -> &SkDQuad {
        self.pts[0].set(&pts[0]);
        self.pts[1].set(&pts[1]);
        self.pts[2].set(&pts[2]);
        self
    }

    pub(crate) fn add_valid_ts(s: &mut [f64], real_roots: i32, t: &mut [f64]) -> i32 {
        let mut found_roots = 0;
        'next_root: for index in 0..real_roots as usize {
            let mut t_value = s[index];
            if approximately_zero_or_more(t_value) && approximately_one_or_less(t_value) {
                if approximately_less_than_zero(t_value) {
                    t_value = 0.0;
                } else if approximately_greater_than_one(t_value) {
                    t_value = 1.0;
                }
                for idx2 in 0..found_roots as usize {
                    if approximately_equal(t[idx2], t_value) {
                        continue 'next_root;
                    }
                }
                t[found_roots as usize] = t_value;
                found_roots += 1;
            }
        }
        found_roots
    }

    pub(crate) fn align(&self, end_index: i32, dst_pt: &mut SkDPoint) {
        let end_index = end_index as usize;
        if self.pts[end_index].x == self.pts[1].x {
            dst_pt.x = self.pts[end_index].x;
        }
        if self.pts[end_index].y == self.pts[1].y {
            dst_pt.y = self.pts[end_index].y;
        }
    }

    pub(crate) fn dxdy_at_t(&self, t: f64) -> SkDVector {
        let a = t - 1.0;
        let b = 1.0 - 2.0 * t;
        let c = t;
        let mut result = SkDVector {
            x: a * self.pts[0].x + b * self.pts[1].x + c * self.pts[2].x,
            y: a * self.pts[0].y + b * self.pts[1].y + c * self.pts[2].y,
        };
        if result.x == 0.0 && result.y == 0.0 {
            if zero_or_one(t) {
                result = self.pts[2] - self.pts[0];
            } else {
                // incomplete
                // Skia prints "!q" with SkDebugf here.
            }
        }
        result
    }

    pub(crate) fn find_extrema(src: &[f64], t_value: &mut [f64]) -> i32 {
        /*  At + B == 0
            t = -B / A
        */
        let a = src[0];
        let b = src[2];
        let c = src[4];
        valid_unit_divide(a - b, a - b - b + c, &mut t_value[0])
    }

    pub(crate) fn hull_intersects_quad(&self, q2: &SkDQuad, is_linear: &mut bool) -> bool {
        let mut linear = true;
        for odd_man in 0..SkDQuad::K_POINT_COUNT {
            let mut end_pt = [SkDPoint::default(); 2];
            self.other_pts(odd_man, &mut end_pt);
            let orig_x = end_pt[0].x;
            let orig_y = end_pt[0].y;
            let adj = end_pt[1].x - orig_x;
            let opp = end_pt[1].y - orig_y;
            let odd = &self.pts[odd_man as usize];
            let sign = (odd.y - orig_y) * adj - (odd.x - orig_x) * opp;
            if approximately_zero(sign) {
                continue;
            }
            linear = false;
            let mut found_outlier = false;
            for n in 0..SkDQuad::K_POINT_COUNT as usize {
                let test = (q2[n].y - orig_y) * adj - (q2[n].x - orig_x) * opp;
                if test * sign > 0.0 && !precisely_zero(test) {
                    found_outlier = true;
                    break;
                }
            }
            if !found_outlier {
                return false;
            }
        }
        if linear && !matches_end(&self.pts, &q2.pts[0]) && !matches_end(&self.pts, &q2.pts[2]) {
            // if the end point of the opposite quad is inside the hull that is nearly a line,
            // then representing the quad as a line may cause the intersection to be missed.
            // Check to see if the endpoint is in the triangle.
            if point_in_triangle(&self.pts, &q2.pts[0]) || point_in_triangle(&self.pts, &q2.pts[2])
            {
                linear = false;
            }
        }
        *is_linear = linear;
        true
    }

    pub(crate) fn hull_intersects_conic(&self, conic: &SkDConic, is_linear: &mut bool) -> bool {
        conic.hull_intersects_quad(self, is_linear)
    }

    pub(crate) fn hull_intersects_cubic(&self, cubic: &SkDCubic, is_linear: &mut bool) -> bool {
        cubic.hull_intersects_quad(self, is_linear)
    }

    pub(crate) fn is_linear(&self, start_index: i32, end_index: i32) -> bool {
        let mut line_parameters = SkLineParameters::default();
        line_parameters.quad_end_points_range(self, start_index as usize, end_index as usize);
        // FIXME: maybe it's possible to avoid this and compare non-normalized
        line_parameters.normalize();
        let distance = line_parameters.control_pt_distance_quad(self);
        let p = &self.pts;
        let tiniest = std_min(
            std_min(
                std_min(std_min(std_min(p[0].x, p[0].y), p[1].x), p[1].y),
                p[2].x,
            ),
            p[2].y,
        );
        let mut largest = std_max(
            std_max(
                std_max(std_max(std_max(p[0].x, p[0].y), p[1].x), p[1].y),
                p[2].x,
            ),
            p[2].y,
        );
        largest = std_max(largest, -tiniest);
        approximately_zero_when_compared_to(distance, largest)
    }

    pub(crate) fn monotonic_in_x(&self) -> bool {
        between(self.pts[0].x, self.pts[1].x, self.pts[2].x)
    }

    pub(crate) fn monotonic_in_y(&self) -> bool {
        between(self.pts[0].y, self.pts[1].y, self.pts[2].y)
    }

    /// Skia fills `endPt` with pointers into `fPts`; here with copies of those points.
    pub(crate) fn other_pts(&self, odd_man: i32, end_pt: &mut [SkDPoint]) {
        for opp in 1..SkDQuad::K_POINT_COUNT {
            let mut end = (odd_man ^ opp) - odd_man; // choose a value not equal to oddMan
            end &= !(end >> 2); // if the value went negative, set it to zero
            end_pt[(opp - 1) as usize] = self.pts[end as usize];
        }
    }

    pub(crate) fn pt_at_t(&self, t: f64) -> SkDPoint {
        if 0.0 == t {
            return self.pts[0];
        }
        if 1.0 == t {
            return self.pts[2];
        }
        let one_t = 1.0 - t;
        let a = one_t * one_t;
        let b = 2.0 * one_t * t;
        let c = t * t;
        SkDPoint {
            x: a * self.pts[0].x + b * self.pts[1].x + c * self.pts[2].x,
            y: a * self.pts[0].y + b * self.pts[1].y + c * self.pts[2].y,
        }
    }

    pub(crate) fn roots_real(a: f64, b: f64, c: f64, t: &mut [f64]) -> i32 {
        let s = t;
        if a == 0.0 {
            return handle_zero(b, c, s);
        }
        let p = b / (2.0 * a);
        let q = c / a;
        if approximately_zero(a) && (approximately_zero_inverse(p) || approximately_zero_inverse(q))
        {
            return handle_zero(b, c, s);
        }
        /* normal form: x^2 + px + q = 0 */
        let p2 = p * p;
        if !almost_dequal_ulps(p2, q) && p2 < q {
            return 0;
        }
        let mut sqrt_d = 0.0;
        if p2 > q {
            sqrt_d = (p2 - q).sqrt();
        }
        s[0] = sqrt_d - p;
        s[1] = -sqrt_d - p;
        1 + i32::from(!almost_dequal_ulps(s[0], s[1]))
    }

    pub(crate) fn roots_valid_t(a: f64, b: f64, c: f64, s: &mut [f64]) -> i32 {
        let t = s;
        let mut s = [0.0; 2];
        let real_roots = SkDQuad::roots_real(a, b, c, &mut s);
        SkDQuad::add_valid_ts(&mut s, real_roots, t)
    }

    pub(crate) fn sub_divide(&self, t1: f64, t2: f64) -> SkDQuad {
        if 0.0 == t1 && 1.0 == t2 {
            return *self;
        }
        let src = self.coords();
        let mut dst = SkDQuad::default();
        dst[0].x = interp_quad_coords(&src[0..], t1);
        let ax = dst[0].x;
        dst[0].y = interp_quad_coords(&src[1..], t1);
        let ay = dst[0].y;
        let dx = interp_quad_coords(&src[0..], (t1 + t2) / 2.0);
        let dy = interp_quad_coords(&src[1..], (t1 + t2) / 2.0);
        dst[2].x = interp_quad_coords(&src[0..], t2);
        let cx = dst[2].x;
        dst[2].y = interp_quad_coords(&src[1..], t2);
        let cy = dst[2].y;
        /* bx = */
        dst[1].x = 2.0 * dx - (ax + cx) / 2.0;
        /* by = */
        dst[1].y = 2.0 * dy - (ay + cy) / 2.0;
        dst
    }

    /// `subDivide(const SkDPoint& a, const SkDPoint& c, double t1, double t2)`.
    pub(crate) fn sub_divide_control(
        &self,
        a: &SkDPoint,
        c: &SkDPoint,
        t1: f64,
        t2: f64,
    ) -> SkDPoint {
        debug_assert!(t1 != t2);
        let mut b;
        let sub = self.sub_divide(t1, t2);
        let b0 = SkDLine {
            pts: [*a, sub[1] + (*a - sub[0])],
        };
        let b1 = SkDLine {
            pts: [*c, sub[1] + (*c - sub[2])],
        };
        let mut i = SkIntersections::new();
        i.intersect_ray_line(&b0, &b1);
        if i.used() == 1 && i[0][0] >= 0.0 && i[1][0] >= 0.0 {
            b = *i.pt(0);
        } else {
            debug_assert!(i.used() <= 2);
            return SkDPoint::mid(&b0[1], &b1[1]);
        }
        if t1 == 0.0 || t2 == 0.0 {
            self.align(0, &mut b);
        }
        if t1 == 1.0 || t2 == 1.0 {
            self.align(2, &mut b);
        }
        if almost_bequal_ulps(b.x, a.x) {
            b.x = a.x;
        } else if almost_bequal_ulps(b.x, c.x) {
            b.x = c.x;
        }
        if almost_bequal_ulps(b.y, a.y) {
            b.y = a.y;
        } else if almost_bequal_ulps(b.y, c.y) {
            b.y = c.y;
        }
        b
    }

    /// The points' coordinates interleaved, `[x0, y0, x1, y1, x2, y2]`: what Skia reads through
    /// `&fPts[0].fX` with a stride of two.
    pub(crate) fn coords(&self) -> [f64; 6] {
        let p = &self.pts;
        [p[0].x, p[0].y, p[1].x, p[1].y, p[2].x, p[2].y]
    }

    /// `static SubDivide(const SkPoint pts[kPointCount], const SkDPoint& a, const SkDPoint& c,
    /// double t1, double t2)`.
    pub(crate) fn sub_divide_pts_control(
        pts: &[SkPoint],
        a: &SkDPoint,
        c: &SkDPoint,
        t1: f64,
        t2: f64,
    ) -> SkDPoint {
        let mut quad = SkDQuad::default();
        quad.set(pts);
        quad.sub_divide_control(a, c, t1, t2)
    }
}

// from blackpawn.com/texts/pointinpoly
fn point_in_triangle(f_pts: &[SkDPoint; 3], test: &SkDPoint) -> bool {
    let v0 = f_pts[2] - f_pts[0];
    let v1 = f_pts[1] - f_pts[0];
    let v2 = *test - f_pts[0];
    let dot00 = v0.dot(&v0);
    let dot01 = v0.dot(&v1);
    let dot02 = v0.dot(&v2);
    let dot11 = v1.dot(&v1);
    let dot12 = v1.dot(&v2);
    // Compute barycentric coordinates
    let denom = dot00 * dot11 - dot01 * dot01;
    let u = dot11 * dot02 - dot01 * dot12;
    let v = dot00 * dot12 - dot01 * dot02;
    // Check if point is in triangle
    if denom >= 0.0 {
        return u >= 0.0 && v >= 0.0 && u + v < denom;
    }
    u <= 0.0 && v <= 0.0 && u + v > denom
}

fn matches_end(f_pts: &[SkDPoint; 3], test: &SkDPoint) -> bool {
    f_pts[0] == *test || f_pts[2] == *test
}

/* started with at_most_end_pts_in_common from SkDQuadIntersection.cpp */
// Do a quick reject by rotating all points relative to a line formed by
// a pair of one quad's points. If the 2nd quad's points
// are on the line or on the opposite side from the 1st quad's 'odd man', the
// curves at most intersect at the endpoints.
/* if returning true, check contains true if quad's hull collapsed, making the cubic linear
   if returning false, check contains true if the the quad pair have only the end point in common
*/

/* bit twiddling for finding the off curve index (x&~m is the pair in [0,1,2] excluding oddMan)
oddMan    opp   x=oddMan^opp  x=x-oddMan  m=x>>2   x&~m
    0       1         1            1         0       1
            2         2            2         0       2
    1       1         0           -1        -1       0
            2         3            2         0       2
    2       1         3            1         0       1
            2         0           -2        -1       0
*/

fn handle_zero(b: f64, c: f64, s: &mut [f64]) -> i32 {
    if approximately_zero(b) {
        s[0] = 0.0;
        return i32::from(c == 0.0);
    }
    s[0] = -c / b;
    1
}

/*
Numeric Solutions (5.6) suggests to solve the quadratic by computing
       Q = -1/2(B + sgn(B)Sqrt(B^2 - 4 A C))
and using the roots
      t1 = Q / A
      t2 = C / Q
*/
// this does not discard real roots <= 0 or >= 1
// TODO(skbug.com/40045140) Deduplicate with SkQuads::RootsReal

/// `interp_quad_coords(const double* src, double t)`: `src` at a stride of two.
fn interp_quad_coords(src: &[f64], t: f64) -> f64 {
    if 0.0 == t {
        return src[0];
    }
    if 1.0 == t {
        return src[4];
    }
    let ab = sk_d_interp(src[0], src[2], t);
    let bc = sk_d_interp(src[2], src[4], t);
    sk_d_interp(ab, bc, t)
}

/*
Given a quadratic q, t1, and t2, find a small quadratic segment.

The new quadratic is defined by A, B, and C, where
 A = c[0]*(1 - t1)*(1 - t1) + 2*c[1]*t1*(1 - t1) + c[2]*t1*t1
 C = c[3]*(1 - t1)*(1 - t1) + 2*c[2]*t1*(1 - t1) + c[1]*t1*t1

To find B, compute the point halfway between t1 and t2:

q(at (t1 + t2)/2) == D

Next, compute where D must be if we know the value of B:

_12 = A/2 + B/2
12_ = B/2 + C/2
123 = A/4 + B/2 + C/4
    = D

Group the known values on one side:

B   = D*2 - A/2 - C/2
*/

/* classic one t subdivision */

fn valid_unit_divide(mut numer: f64, mut denom: f64, ratio: &mut f64) -> i32 {
    if numer < 0.0 {
        numer = -numer;
        denom = -denom;
    }
    if denom == 0.0 || numer == 0.0 || numer >= denom {
        return 0;
    }
    let r = numer / denom;
    if r == 0.0 {
        // catch underflow if numer <<<< denom
        return 0;
    }
    *ratio = r;
    1
}

/* Quad'(t) = At + B, where
    A = 2(a - 2b + c)
    B = 2(b - a)
    Solve for t, only if it fits between 0 < t < 1
*/

/* Parameterization form, given A*t*t + 2*B*t*(1-t) + C*(1-t)*(1-t)
 *
 * a = A - 2*B +   C
 * b =     2*B - 2*C
 * c =             C
 */

/// `SkTQuad`: an `SkDQuad` as an `SkTCurve`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkTQuad {
    pub(crate) quad: SkDQuad,
}

impl SkTQuad {
    pub(crate) fn new(q: &SkDQuad) -> SkTQuad {
        SkTQuad { quad: *q }
    }

    pub(crate) fn collapsed(&self) -> bool {
        self.quad.collapsed()
    }

    pub(crate) fn controls_inside(&self) -> bool {
        self.quad.controls_inside()
    }

    pub(crate) fn dxdy_at_t(&self, t: f64) -> SkDVector {
        self.quad.dxdy_at_t(t)
    }

    pub(crate) fn hull_intersects_quad(&self, quad: &SkDQuad, is_linear: &mut bool) -> bool {
        quad.hull_intersects_quad(&self.quad, is_linear)
    }

    pub(crate) fn hull_intersects_conic(&self, conic: &SkDConic, is_linear: &mut bool) -> bool {
        conic.hull_intersects_quad(&self.quad, is_linear)
    }

    pub(crate) fn hull_intersects_cubic(&self, cubic: &SkDCubic, is_linear: &mut bool) -> bool {
        cubic.hull_intersects_quad(&self.quad, is_linear)
    }

    pub(crate) fn hull_intersects_t_curve(&self, curve: &SkTCurve, is_linear: &mut bool) -> bool {
        curve.hull_intersects_quad(&self.quad, is_linear)
    }

    pub(crate) fn intersect_ray(&self, i: &mut SkIntersections, line: &SkDLine) -> i32 {
        i.intersect_ray_quad(&self.quad, line)
    }

    pub(crate) fn max_intersections(&self) -> i32 {
        SkDQuad::K_MAX_INTERSECTIONS
    }

    pub(crate) fn other_pts(&self, odd_man: i32, end_pt: &mut [SkDPoint]) {
        self.quad.other_pts(odd_man, end_pt);
    }

    pub(crate) fn point_count(&self) -> i32 {
        SkDQuad::K_POINT_COUNT
    }

    pub(crate) fn point_last(&self) -> i32 {
        SkDQuad::K_POINT_LAST
    }

    pub(crate) fn pt_at_t(&self, t: f64) -> SkDPoint {
        self.quad.pt_at_t(t)
    }

    pub(crate) fn set_bounds(&self, rect: &mut SkDRect) {
        rect.set_bounds_quad(&self.quad);
    }
}
