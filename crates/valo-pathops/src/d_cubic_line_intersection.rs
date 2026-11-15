//! Port of Skia's src/pathops/SkDCubicLineIntersection.cpp: where a cubic meets a line, or a
//! horizontal or vertical line. The file-local `LineCubicIntersections` that does the work is
//! not declared in a header, so its port comes with these bodies.
// Skia indexes its arrays in its loops; the port keeps the indices.
#![allow(clippy::needless_range_loop)]

use crate::core::path::SkPathVerb;
use crate::intersections::SkIntersections;
use crate::path_ops_cubic::{SearchAxis, SkDCubic};
use crate::path_ops_curve::SkDCurve;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::SkDPoint;
use crate::path_ops_types::{
    approximately_equal, approximately_one_or_less, approximately_zero, approximately_zero_or_more,
    sk_pin_t,
};

/*
Find the intersection of a line and cubic by solving for valid t values.

Analogous to line-quadratic intersection, solve line-cubic intersection by
representing the cubic as:
  x = a(1-t)^3 + 2b(1-t)^2t + c(1-t)t^2 + dt^3
  y = e(1-t)^3 + 2f(1-t)^2t + g(1-t)t^2 + ht^3
and the line as:
  y = i*x + j  (if the line is more horizontal)
or:
  x = i*y + j  (if the line is more vertical)

Then using Mathematica, solve for the values of t where the cubic intersects the
line:

  (in) Resultant[
        a*(1 - t)^3 + 3*b*(1 - t)^2*t + 3*c*(1 - t)*t^2 + d*t^3 - x,
        e*(1 - t)^3 + 3*f*(1 - t)^2*t + 3*g*(1 - t)*t^2 + h*t^3 - i*x - j, x]
  (out) -e     +   j     +
       3 e t   - 3 f t   -
       3 e t^2 + 6 f t^2 - 3 g t^2 +
         e t^3 - 3 f t^3 + 3 g t^3 - h t^3 +
     i ( a     -
       3 a t + 3 b t +
       3 a t^2 - 6 b t^2 + 3 c t^2 -
         a t^3 + 3 b t^3 - 3 c t^3 + d t^3 )

if i goes to infinity, we can rewrite the line in terms of x. Mathematica:

  (in) Resultant[
        a*(1 - t)^3 + 3*b*(1 - t)^2*t + 3*c*(1 - t)*t^2 + d*t^3 - i*y - j,
        e*(1 - t)^3 + 3*f*(1 - t)^2*t + 3*g*(1 - t)*t^2 + h*t^3 - y,       y]
  (out)  a     -   j     -
       3 a t   + 3 b t   +
       3 a t^2 - 6 b t^2 + 3 c t^2 -
         a t^3 + 3 b t^3 - 3 c t^3 + d t^3 -
     i ( e     -
       3 e t   + 3 f t   +
       3 e t^2 - 6 f t^2 + 3 g t^2 -
         e t^3 + 3 f t^3 - 3 g t^3 + h t^3 )

Solving this with Mathematica produces an expression with hundreds of terms;
instead, use Numeric Solutions recipe to solve the cubic.

The near-horizontal case, in terms of:  Ax^3 + Bx^2 + Cx + D == 0
    A =   (-(-e + 3*f - 3*g + h) + i*(-a + 3*b - 3*c + d)     )
    B = 3*(-( e - 2*f +   g    ) + i*( a - 2*b +   c    )     )
    C = 3*(-(-e +   f          ) + i*(-a +   b          )     )
    D =   (-( e                ) + i*( a                ) + j )

The near-vertical case, in terms of:  Ax^3 + Bx^2 + Cx + D == 0
    A =   ( (-a + 3*b - 3*c + d) - i*(-e + 3*f - 3*g + h)     )
    B = 3*( ( a - 2*b +   c    ) - i*( e - 2*f +   g    )     )
    C = 3*( (-a +   b          ) - i*(-e +   f          )     )
    D =   ( ( a                ) - i*( e                ) - j )

For horizontal lines:
(in) Resultant[
      a*(1 - t)^3 + 3*b*(1 - t)^2*t + 3*c*(1 - t)*t^2 + d*t^3 - j,
      e*(1 - t)^3 + 3*f*(1 - t)^2*t + 3*g*(1 - t)*t^2 + h*t^3 - y, y]
(out)  e     -   j     -
     3 e t   + 3 f t   +
     3 e t^2 - 6 f t^2 + 3 g t^2 -
       e t^3 + 3 f t^3 - 3 g t^3 + h t^3
 */

/// The cubic's coordinates interleaved, `[x0, y0, x1, y1, ...]`: Skia passes `&c[0].fX` (from
/// index 0) or `&c[0].fY` (from index 1) with a stride of two.
fn interleaved_coords(c: &SkDCubic) -> [f64; 8] {
    [
        c[0].x, c[0].y, c[1].x, c[1].y, c[2].x, c[2].y, c[3].x, c[3].y,
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PinTPoint {
    PointUninitialized,
    PointInitialized,
}

struct LineCubicIntersections<'a> {
    cubic: &'a SkDCubic,
    line: &'a SkDLine,
    intersections: &'a mut SkIntersections,
    allow_near: bool,
}

impl<'a> LineCubicIntersections<'a> {
    fn new(
        c: &'a SkDCubic,
        l: &'a SkDLine,
        i: &'a mut SkIntersections,
    ) -> LineCubicIntersections<'a> {
        i.set_max(4);
        LineCubicIntersections {
            cubic: c,
            line: l,
            intersections: i,
            allow_near: true,
        }
    }

    fn allow_near(&mut self, allow: bool) {
        self.allow_near = allow;
    }

    fn check_coincident(&mut self) {
        let mut last = self.intersections.used() - 1;
        let mut index = 0;
        while index < last {
            let cubic_mid_t = (self.intersections[0][index as usize]
                + self.intersections[0][index as usize + 1])
                / 2.0;
            let cubic_mid_pt = self.cubic.pt_at_t(cubic_mid_t);
            let t = self.line.near_point(&cubic_mid_pt, None);
            if t < 0.0 {
                index += 1;
                continue;
            }
            if self.intersections.is_coincident(index) {
                self.intersections.remove_one(index);
                last -= 1;
            } else if self.intersections.is_coincident(index + 1) {
                self.intersections.remove_one(index + 1);
                last -= 1;
            } else {
                self.intersections.set_coincident(index);
                index += 1;
            }
            self.intersections.set_coincident(index);
        }
    }

    // see parallel routine in line quadratic intersections
    fn intersect_ray(&self, roots: &mut [f64]) -> i32 {
        let adj = self.line[1].x - self.line[0].x;
        let opp = self.line[1].y - self.line[0].y;
        // Skia leaves `c`'s y coordinates uninitialized until the search below sets them; the
        // only read of them before is the unused y of `calcPt`.
        let mut c = SkDCubic::default();
        for n in 0..4 {
            c[n].x =
                (self.cubic[n].y - self.line[0].y) * adj - (self.cubic[n].x - self.line[0].x) * opp;
        }
        let (mut a, mut b, mut cc, mut d) = (0.0, 0.0, 0.0, 0.0);
        SkDCubic::coefficients(&interleaved_coords(&c), &mut a, &mut b, &mut cc, &mut d);
        let mut count = SkDCubic::roots_valid_t(a, b, cc, d, roots);
        for index in 0..count as usize {
            let calc_pt = c.pt_at_t(roots[index]);
            if !approximately_zero(calc_pt.x) {
                for n in 0..4 {
                    c[n].y = (self.cubic[n].y - self.line[0].y) * opp
                        + (self.cubic[n].x - self.line[0].x) * adj;
                }
                let mut extreme_ts = [0.0f64; 6];
                let extrema = SkDCubic::find_extrema(&interleaved_coords(&c), &mut extreme_ts);
                count = c.search_roots(&mut extreme_ts, extrema, 0.0, SearchAxis::XAxis, roots);
                break;
            }
        }
        count
    }

    fn intersect(&mut self) -> i32 {
        self.add_exact_end_points();
        if self.allow_near {
            self.add_near_end_points();
        }
        let mut root_vals = [0.0f64; 3];
        let roots = self.intersect_ray(&mut root_vals);
        for index in 0..roots as usize {
            let mut cubic_t = root_vals[index];
            let mut line_t = self.find_line_t(cubic_t);
            let mut pt = SkDPoint::default();
            if self.pin_ts(
                &mut cubic_t,
                &mut line_t,
                &mut pt,
                PinTPoint::PointUninitialized,
            ) && self.unique_answer(cubic_t, &pt)
            {
                self.intersections.insert(cubic_t, line_t, &pt);
            }
        }
        self.check_coincident();
        self.intersections.used()
    }

    /// `static HorizontalIntersect(const SkDCubic& c, double axisIntercept, double roots[3])`.
    fn horizontal_intersect_roots(c: &SkDCubic, axis_intercept: f64, roots: &mut [f64]) -> i32 {
        let (mut a, mut b, mut cc, mut d) = (0.0, 0.0, 0.0, 0.0);
        let coords = interleaved_coords(c);
        SkDCubic::coefficients(&coords[1..], &mut a, &mut b, &mut cc, &mut d);
        d -= axis_intercept;
        let mut count = SkDCubic::roots_valid_t(a, b, cc, d, roots);
        for index in 0..count as usize {
            let calc_pt = c.pt_at_t(roots[index]);
            if !approximately_equal(calc_pt.y, axis_intercept) {
                let mut extreme_ts = [0.0f64; 6];
                let extrema = SkDCubic::find_extrema(&coords[1..], &mut extreme_ts);
                count = c.search_roots(
                    &mut extreme_ts,
                    extrema,
                    axis_intercept,
                    SearchAxis::YAxis,
                    roots,
                );
                break;
            }
        }
        count
    }

    fn horizontal_intersect(
        &mut self,
        axis_intercept: f64,
        left: f64,
        right: f64,
        flipped: bool,
    ) -> i32 {
        self.add_exact_horizontal_end_points(left, right, axis_intercept);
        if self.allow_near {
            self.add_near_horizontal_end_points(left, right, axis_intercept);
        }
        let mut roots = [0.0f64; 3];
        let count = Self::horizontal_intersect_roots(self.cubic, axis_intercept, &mut roots);
        for index in 0..count as usize {
            let mut cubic_t = roots[index];
            let mut pt = SkDPoint {
                x: self.cubic.pt_at_t(cubic_t).x,
                y: axis_intercept,
            };
            let mut line_t = (pt.x - left) / (right - left);
            if self.pin_ts(
                &mut cubic_t,
                &mut line_t,
                &mut pt,
                PinTPoint::PointInitialized,
            ) && self.unique_answer(cubic_t, &pt)
            {
                self.intersections.insert(cubic_t, line_t, &pt);
            }
        }
        if flipped {
            self.intersections.flip();
        }
        self.check_coincident();
        self.intersections.used()
    }

    fn unique_answer(&self, cubic_t: f64, pt: &SkDPoint) -> bool {
        for inner in 0..self.intersections.used() as usize {
            if self.intersections.pt(inner) != pt {
                continue;
            }
            let existing_cubic_t = self.intersections[0][inner];
            if cubic_t == existing_cubic_t {
                return false;
            }
            // check if midway on cubic is also same point. If so, discard this
            let cubic_mid_t = (existing_cubic_t + cubic_t) / 2.0;
            let cubic_mid_pt = self.cubic.pt_at_t(cubic_mid_t);
            if cubic_mid_pt.approximately_equal(pt) {
                return false;
            }
        }
        true
    }

    /// `static VerticalIntersect(const SkDCubic& c, double axisIntercept, double roots[3])`.
    fn vertical_intersect_roots(c: &SkDCubic, axis_intercept: f64, roots: &mut [f64]) -> i32 {
        let (mut a, mut b, mut cc, mut d) = (0.0, 0.0, 0.0, 0.0);
        let coords = interleaved_coords(c);
        SkDCubic::coefficients(&coords, &mut a, &mut b, &mut cc, &mut d);
        d -= axis_intercept;
        let mut count = SkDCubic::roots_valid_t(a, b, cc, d, roots);
        for index in 0..count as usize {
            let calc_pt = c.pt_at_t(roots[index]);
            if !approximately_equal(calc_pt.x, axis_intercept) {
                let mut extreme_ts = [0.0f64; 6];
                let extrema = SkDCubic::find_extrema(&coords, &mut extreme_ts);
                count = c.search_roots(
                    &mut extreme_ts,
                    extrema,
                    axis_intercept,
                    SearchAxis::XAxis,
                    roots,
                );
                break;
            }
        }
        count
    }

    fn vertical_intersect(
        &mut self,
        axis_intercept: f64,
        top: f64,
        bottom: f64,
        flipped: bool,
    ) -> i32 {
        self.add_exact_vertical_end_points(top, bottom, axis_intercept);
        if self.allow_near {
            self.add_near_vertical_end_points(top, bottom, axis_intercept);
        }
        let mut roots = [0.0f64; 3];
        let count = Self::vertical_intersect_roots(self.cubic, axis_intercept, &mut roots);
        for index in 0..count as usize {
            let mut cubic_t = roots[index];
            let mut pt = SkDPoint {
                x: axis_intercept,
                y: self.cubic.pt_at_t(cubic_t).y,
            };
            let mut line_t = (pt.y - top) / (bottom - top);
            if self.pin_ts(
                &mut cubic_t,
                &mut line_t,
                &mut pt,
                PinTPoint::PointInitialized,
            ) && self.unique_answer(cubic_t, &pt)
            {
                self.intersections.insert(cubic_t, line_t, &pt);
            }
        }
        if flipped {
            self.intersections.flip();
        }
        self.check_coincident();
        self.intersections.used()
    }

    fn add_exact_end_points(&mut self) {
        for c_index in (0..4).step_by(3) {
            let line_t = self.line.exact_point(&self.cubic[c_index]);
            if line_t < 0.0 {
                continue;
            }
            let cubic_t = (c_index >> 1) as f64;
            self.intersections
                .insert(cubic_t, line_t, &self.cubic[c_index]);
        }
    }

    /* Note that this does not look for endpoints of the line that are near the cubic.
    These points are found later when check ends looks for missing points */
    fn add_near_end_points(&mut self) {
        for c_index in (0..4).step_by(3) {
            let cubic_t = (c_index >> 1) as f64;
            if self.intersections.has_t(cubic_t) {
                continue;
            }
            let line_t = self.line.near_point(&self.cubic[c_index], None);
            if line_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(cubic_t, line_t, &self.cubic[c_index]);
        }
        self.add_line_near_end_points();
    }

    fn add_line_near_end_points(&mut self) {
        // Skia casts the cubic to an `SkDCurve`, whose cubic member shares its points.
        let mut curve = SkDCurve::default();
        curve.set_cubic(self.cubic);
        for l_index in 0..2usize {
            let line_t = l_index as f64;
            if self.intersections.has_opp_t(line_t) {
                continue;
            }
            let cubic_t = curve.near_point(
                SkPathVerb::Cubic,
                &self.line[l_index],
                &self.line[usize::from(l_index == 0)],
            );
            if cubic_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(cubic_t, line_t, &self.line[l_index]);
        }
    }

    fn add_exact_horizontal_end_points(&mut self, left: f64, right: f64, y: f64) {
        for c_index in (0..4).step_by(3) {
            let line_t = SkDLine::exact_point_h(&self.cubic[c_index], left, right, y);
            if line_t < 0.0 {
                continue;
            }
            let cubic_t = (c_index >> 1) as f64;
            self.intersections
                .insert(cubic_t, line_t, &self.cubic[c_index]);
        }
    }

    fn add_near_horizontal_end_points(&mut self, left: f64, right: f64, y: f64) {
        for c_index in (0..4).step_by(3) {
            let cubic_t = (c_index >> 1) as f64;
            if self.intersections.has_t(cubic_t) {
                continue;
            }
            let line_t = SkDLine::near_point_h(&self.cubic[c_index], left, right, y);
            if line_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(cubic_t, line_t, &self.cubic[c_index]);
        }
        self.add_line_near_end_points();
    }

    fn add_exact_vertical_end_points(&mut self, top: f64, bottom: f64, x: f64) {
        for c_index in (0..4).step_by(3) {
            let line_t = SkDLine::exact_point_v(&self.cubic[c_index], top, bottom, x);
            if line_t < 0.0 {
                continue;
            }
            let cubic_t = (c_index >> 1) as f64;
            self.intersections
                .insert(cubic_t, line_t, &self.cubic[c_index]);
        }
    }

    fn add_near_vertical_end_points(&mut self, top: f64, bottom: f64, x: f64) {
        for c_index in (0..4).step_by(3) {
            let cubic_t = (c_index >> 1) as f64;
            if self.intersections.has_t(cubic_t) {
                continue;
            }
            let line_t = SkDLine::near_point_v(&self.cubic[c_index], top, bottom, x);
            if line_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(cubic_t, line_t, &self.cubic[c_index]);
        }
        self.add_line_near_end_points();
    }

    fn find_line_t(&self, t: f64) -> f64 {
        let xy = self.cubic.pt_at_t(t);
        let dx = self.line[1].x - self.line[0].x;
        let dy = self.line[1].y - self.line[0].y;
        if dx.abs() > dy.abs() {
            return (xy.x - self.line[0].x) / dx;
        }
        (xy.y - self.line[0].y) / dy
    }

    fn pin_ts(
        &self,
        cubic_t: &mut f64,
        line_t: &mut f64,
        pt: &mut SkDPoint,
        pt_set: PinTPoint,
    ) -> bool {
        if !approximately_one_or_less(*line_t) {
            return false;
        }
        if !approximately_zero_or_more(*line_t) {
            return false;
        }
        *cubic_t = sk_pin_t(*cubic_t);
        let c_t = *cubic_t;
        *line_t = sk_pin_t(*line_t);
        let l_t = *line_t;
        let l_pt = self.line.pt_at_t(l_t);
        let c_pt = self.cubic.pt_at_t(c_t);
        if !l_pt.roughly_equal(&c_pt) {
            return false;
        }
        // FIXME: if points are roughly equal but not approximately equal, need to do
        // a binary search like quad/quad intersection to find more precise t values
        if l_t == 0.0
            || l_t == 1.0
            || (pt_set == PinTPoint::PointUninitialized && c_t != 0.0 && c_t != 1.0)
        {
            *pt = l_pt;
        } else if pt_set == PinTPoint::PointUninitialized {
            *pt = c_pt;
        }
        let grid_pt = pt.as_sk_point();
        if grid_pt == self.line[0].as_sk_point() {
            *line_t = 0.0;
        } else if grid_pt == self.line[1].as_sk_point() {
            *line_t = 1.0;
        }
        if grid_pt == self.cubic[0].as_sk_point() && approximately_equal(*cubic_t, 0.0) {
            *cubic_t = 0.0;
        } else if grid_pt == self.cubic[3].as_sk_point() && approximately_equal(*cubic_t, 1.0) {
            *cubic_t = 1.0;
        }
        true
    }
}

impl SkIntersections {
    /// `horizontal(const SkDCubic&, double left, double right, double y, bool flipped)`.
    pub(crate) fn horizontal_cubic(
        &mut self,
        cubic: &SkDCubic,
        left: f64,
        right: f64,
        y: f64,
        flipped: bool,
    ) -> i32 {
        let line = SkDLine {
            pts: [SkDPoint { x: left, y }, SkDPoint { x: right, y }],
        };
        let mut c = LineCubicIntersections::new(cubic, &line, self);
        c.horizontal_intersect(y, left, right, flipped)
    }

    /// `vertical(const SkDCubic&, double top, double bottom, double x, bool flipped)`.
    pub(crate) fn vertical_cubic(
        &mut self,
        cubic: &SkDCubic,
        top: f64,
        bottom: f64,
        x: f64,
        flipped: bool,
    ) -> i32 {
        let line = SkDLine {
            pts: [SkDPoint { x, y: top }, SkDPoint { x, y: bottom }],
        };
        let mut c = LineCubicIntersections::new(cubic, &line, self);
        c.vertical_intersect(x, top, bottom, flipped)
    }

    /// `intersect(const SkDCubic& cubic, const SkDLine& line)`.
    pub(crate) fn intersect_cubic_line(&mut self, cubic: &SkDCubic, line: &SkDLine) -> i32 {
        let allow_near = self.allow_near;
        let mut c = LineCubicIntersections::new(cubic, line, self);
        c.allow_near(allow_near);
        c.intersect()
    }

    /// `intersectRay(const SkDCubic& cubic, const SkDLine& line)`.
    pub(crate) fn intersect_ray_cubic(&mut self, cubic: &SkDCubic, line: &SkDLine) -> i32 {
        let c = LineCubicIntersections::new(cubic, line, self);
        // Skia passes `fT[0]` as the roots; `intersectRay` writes them and reads nothing else of
        // the intersections, so they are written to a copy that replaces `fT[0]` after.
        let mut roots = c.intersections.t[0];
        let used = c.intersect_ray(&mut roots);
        self.t[0] = roots;
        self.used = used as u8;
        for index in 0..self.used as usize {
            self.pt[index] = cubic.pt_at_t(self.t[0][index]);
        }
        self.used as i32
    }
}

// SkDCubic accessors to Intersection utilities

impl SkDCubic {
    /// Return the number of valid roots (0 < root < 1) for this cubic intersecting the
    /// specified horizontal line.
    pub(crate) fn horizontal_intersect(&self, y_intercept: f64, roots: &mut [f64]) -> i32 {
        LineCubicIntersections::horizontal_intersect_roots(self, y_intercept, roots)
    }

    /// Return the number of valid roots (0 < root < 1) for this cubic intersecting the
    /// specified vertical line.
    pub(crate) fn vertical_intersect(&self, x_intercept: f64, roots: &mut [f64]) -> i32 {
        LineCubicIntersections::vertical_intersect_roots(self, x_intercept, roots)
    }
}
