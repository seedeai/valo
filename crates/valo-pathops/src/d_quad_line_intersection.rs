//! Port of Skia's src/pathops/SkDQuadLineIntersection.cpp: where a quad meets a line, or a
//! horizontal or vertical line. The file-local `LineQuadraticIntersections` that does the work
//! is not declared in a header, so its port comes with these bodies.
// Skia indexes its arrays in its loops; the port keeps the indices.
#![allow(clippy::needless_range_loop)]

use crate::core::path::SkPathVerb;
use crate::intersections::SkIntersections;
use crate::path_ops_curve::SkDCurve;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::SkDPoint;
use crate::path_ops_quad::SkDQuad;
use crate::path_ops_types::{
    approximately_equal, approximately_one_or_less_double, approximately_zero_or_more_double,
    sk_pin_t,
};

/*
Find the intersection of a line and quadratic by solving for valid t values.

From http://stackoverflow.com/questions/1853637/how-to-find-the-mathematical-function-defining-a-bezier-curve

"A Bezier curve is a parametric function. A quadratic Bezier curve (i.e. three
control points) can be expressed as: F(t) = A(1 - t)^2 + B(1 - t)t + Ct^2 where
A, B and C are points and t goes from zero to one.

This will give you two equations:

  x = a(1 - t)^2 + b(1 - t)t + ct^2
  y = d(1 - t)^2 + e(1 - t)t + ft^2

If you add for instance the line equation (y = kx + m) to that, you'll end up
with three equations and three unknowns (x, y and t)."

Similar to above, the quadratic is represented as
  x = a(1-t)^2 + 2b(1-t)t + ct^2
  y = d(1-t)^2 + 2e(1-t)t + ft^2
and the line as
  y = g*x + h

Using Mathematica, solve for the values of t where the quadratic intersects the
line:

  (in)  t1 = Resultant[a*(1 - t)^2 + 2*b*(1 - t)*t + c*t^2 - x,
                       d*(1 - t)^2 + 2*e*(1 - t)*t  + f*t^2 - g*x - h, x]
  (out) -d + h + 2 d t - 2 e t - d t^2 + 2 e t^2 - f t^2 +
         g  (a - 2 a t + 2 b t + a t^2 - 2 b t^2 + c t^2)
  (in)  Solve[t1 == 0, t]
  (out) {
    {t -> (-2 d + 2 e +   2 a g - 2 b g    -
      Sqrt[(2 d - 2 e -   2 a g + 2 b g)^2 -
          4 (-d + 2 e - f + a g - 2 b g    + c g) (-d + a g + h)]) /
         (2 (-d + 2 e - f + a g - 2 b g    + c g))
         },
    {t -> (-2 d + 2 e +   2 a g - 2 b g    +
      Sqrt[(2 d - 2 e -   2 a g + 2 b g)^2 -
          4 (-d + 2 e - f + a g - 2 b g    + c g) (-d + a g + h)]) /
         (2 (-d + 2 e - f + a g - 2 b g    + c g))
         }
        }

Using the results above (when the line tends towards horizontal)
       A =   (-(d - 2*e + f) + g*(a - 2*b + c)     )
       B = 2*( (d -   e    ) - g*(a -   b    )     )
       C =   (-(d          ) + g*(a          ) + h )

If g goes to infinity, we can rewrite the line in terms of x.
  x = g'*y + h'

And solve accordingly in Mathematica:

  (in)  t2 = Resultant[a*(1 - t)^2 + 2*b*(1 - t)*t + c*t^2 - g'*y - h',
                       d*(1 - t)^2 + 2*e*(1 - t)*t  + f*t^2 - y, y]
  (out)  a - h' - 2 a t + 2 b t + a t^2 - 2 b t^2 + c t^2 -
         g'  (d - 2 d t + 2 e t + d t^2 - 2 e t^2 + f t^2)
  (in)  Solve[t2 == 0, t]
  (out) {
    {t -> (2 a - 2 b -   2 d g' + 2 e g'    -
    Sqrt[(-2 a + 2 b +   2 d g' - 2 e g')^2 -
          4 (a - 2 b + c - d g' + 2 e g' - f g') (a - d g' - h')]) /
         (2 (a - 2 b + c - d g' + 2 e g' - f g'))
         },
    {t -> (2 a - 2 b -   2 d g' + 2 e g'    +
    Sqrt[(-2 a + 2 b +   2 d g' - 2 e g')^2 -
          4 (a - 2 b + c - d g' + 2 e g' - f g') (a - d g' - h')])/
         (2 (a - 2 b + c - d g' + 2 e g' - f g'))
         }
        }

Thus, if the slope of the line tends towards vertical, we use:
       A =   ( (a - 2*b + c) - g'*(d  - 2*e + f)      )
       B = 2*(-(a -   b    ) + g'*(d  -   e    )      )
       C =   ( (a          ) - g'*(d           ) - h' )
 */

#[derive(Clone, Copy, PartialEq, Eq)]
enum PinTPoint {
    PointUninitialized,
    PointInitialized,
}

/// Skia's second constructor, `LineQuadraticIntersections(const SkDQuad& q)`, leaves the line
/// and the intersections unset and serves only `horizontalIntersect(double, double[2])` and
/// `verticalIntersect(double, double[2])`, which read only the quad; here those two are
/// associated functions taking the quad, and the struct is always made with a line.
struct LineQuadraticIntersections<'a> {
    quad: &'a SkDQuad,
    line: &'a SkDLine,
    intersections: &'a mut SkIntersections,
    allow_near: bool,
}

impl<'a> LineQuadraticIntersections<'a> {
    fn new(
        q: &'a SkDQuad,
        l: &'a SkDLine,
        i: &'a mut SkIntersections,
    ) -> LineQuadraticIntersections<'a> {
        i.set_max(5); // allow short partial coincidence plus discrete intersections
        LineQuadraticIntersections {
            quad: q,
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
            let quad_mid_t = (self.intersections[0][index as usize]
                + self.intersections[0][index as usize + 1])
                / 2.0;
            let quad_mid_pt = self.quad.pt_at_t(quad_mid_t);
            let t = self.line.near_point(&quad_mid_pt, None);
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

    fn intersect_ray(&self, roots: &mut [f64]) -> i32 {
        /*
            solve by rotating line+quad so line is horizontal, then finding the roots
            set up matrix to rotate quad to x-axis
            |cos(a) -sin(a)|
            |sin(a)  cos(a)|
            note that cos(a) = A(djacent) / Hypoteneuse
                      sin(a) = O(pposite) / Hypoteneuse
            since we are computing Ts, we can ignore hypoteneuse, the scale factor:
            |  A     -O    |
            |  O      A    |
            A = line[1].fX - line[0].fX (adjacent side of the right triangle)
            O = line[1].fY - line[0].fY (opposite side of the right triangle)
            for each of the three points (e.g. n = 0 to 2)
            quad[n].fY' = (quad[n].fY - line[0].fY) * A - (quad[n].fX - line[0].fX) * O
        */
        let adj = self.line[1].x - self.line[0].x;
        let opp = self.line[1].y - self.line[0].y;
        let mut r = [0.0f64; 3];
        for n in 0..3 {
            r[n] =
                (self.quad[n].y - self.line[0].y) * adj - (self.quad[n].x - self.line[0].x) * opp;
        }
        let mut a = r[2];
        let mut b = r[1];
        let c = r[0];
        a += c - 2.0 * b; // A = a - 2*b + c
        b -= c; // B = -(b - c)
        SkDQuad::roots_valid_t(a, 2.0 * b, c, roots)
    }

    fn intersect(&mut self) -> i32 {
        self.add_exact_end_points();
        if self.allow_near {
            self.add_near_end_points();
        }
        let mut root_vals = [0.0f64; 2];
        let roots = self.intersect_ray(&mut root_vals);
        for index in 0..roots as usize {
            let mut quad_t = root_vals[index];
            let mut line_t = self.find_line_t(quad_t);
            let mut pt = SkDPoint::default();
            if self.pin_ts(
                &mut quad_t,
                &mut line_t,
                &mut pt,
                PinTPoint::PointUninitialized,
            ) && self.unique_answer(quad_t, &pt)
            {
                self.intersections.insert(quad_t, line_t, &pt);
            }
        }
        self.check_coincident();
        self.intersections.used()
    }

    /// `horizontalIntersect(double axisIntercept, double roots[2])`.
    fn horizontal_intersect_roots(quad: &SkDQuad, axis_intercept: f64, roots: &mut [f64]) -> i32 {
        let mut d = quad[2].y; // f
        let mut e = quad[1].y; // e
        let mut f = quad[0].y; // d
        d += f - 2.0 * e; // D = d - 2*e + f
        e -= f; // E = -(d - e)
        f -= axis_intercept;
        SkDQuad::roots_valid_t(d, 2.0 * e, f, roots)
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
        let mut root_vals = [0.0f64; 2];
        let roots = Self::horizontal_intersect_roots(self.quad, axis_intercept, &mut root_vals);
        for index in 0..roots as usize {
            let mut quad_t = root_vals[index];
            let mut pt = self.quad.pt_at_t(quad_t);
            let mut line_t = (pt.x - left) / (right - left);
            if self.pin_ts(
                &mut quad_t,
                &mut line_t,
                &mut pt,
                PinTPoint::PointInitialized,
            ) && self.unique_answer(quad_t, &pt)
            {
                self.intersections.insert(quad_t, line_t, &pt);
            }
        }
        if flipped {
            self.intersections.flip();
        }
        self.check_coincident();
        self.intersections.used()
    }

    fn unique_answer(&self, quad_t: f64, pt: &SkDPoint) -> bool {
        for inner in 0..self.intersections.used() as usize {
            if self.intersections.pt(inner) != pt {
                continue;
            }
            let existing_quad_t = self.intersections[0][inner];
            if quad_t == existing_quad_t {
                return false;
            }
            // check if midway on quad is also same point. If so, discard this
            let quad_mid_t = (existing_quad_t + quad_t) / 2.0;
            let quad_mid_pt = self.quad.pt_at_t(quad_mid_t);
            if quad_mid_pt.approximately_equal(pt) {
                return false;
            }
        }
        true
    }

    /// `verticalIntersect(double axisIntercept, double roots[2])`.
    fn vertical_intersect_roots(quad: &SkDQuad, axis_intercept: f64, roots: &mut [f64]) -> i32 {
        let mut d = quad[2].x; // f
        let mut e = quad[1].x; // e
        let mut f = quad[0].x; // d
        d += f - 2.0 * e; // D = d - 2*e + f
        e -= f; // E = -(d - e)
        f -= axis_intercept;
        SkDQuad::roots_valid_t(d, 2.0 * e, f, roots)
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
        let mut root_vals = [0.0f64; 2];
        let roots = Self::vertical_intersect_roots(self.quad, axis_intercept, &mut root_vals);
        for index in 0..roots as usize {
            let mut quad_t = root_vals[index];
            let mut pt = self.quad.pt_at_t(quad_t);
            let mut line_t = (pt.y - top) / (bottom - top);
            if self.pin_ts(
                &mut quad_t,
                &mut line_t,
                &mut pt,
                PinTPoint::PointInitialized,
            ) && self.unique_answer(quad_t, &pt)
            {
                self.intersections.insert(quad_t, line_t, &pt);
            }
        }
        if flipped {
            self.intersections.flip();
        }
        self.check_coincident();
        self.intersections.used()
    }

    // add endpoints first to get zero and one t values exactly
    fn add_exact_end_points(&mut self) {
        for q_index in (0..3).step_by(2) {
            let line_t = self.line.exact_point(&self.quad[q_index]);
            if line_t < 0.0 {
                continue;
            }
            let quad_t = (q_index >> 1) as f64;
            self.intersections
                .insert(quad_t, line_t, &self.quad[q_index]);
        }
    }

    fn add_near_end_points(&mut self) {
        for q_index in (0..3).step_by(2) {
            let quad_t = (q_index >> 1) as f64;
            if self.intersections.has_t(quad_t) {
                continue;
            }
            let line_t = self.line.near_point(&self.quad[q_index], None);
            if line_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(quad_t, line_t, &self.quad[q_index]);
        }
        self.add_line_near_end_points();
    }

    fn add_line_near_end_points(&mut self) {
        // Skia casts the quad to an `SkDCurve`, whose quad member shares its points.
        let mut curve = SkDCurve::default();
        curve.set_quad(self.quad);
        for l_index in 0..2usize {
            let line_t = l_index as f64;
            if self.intersections.has_opp_t(line_t) {
                continue;
            }
            let quad_t = curve.near_point(
                SkPathVerb::Quad,
                &self.line[l_index],
                &self.line[usize::from(l_index == 0)],
            );
            if quad_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(quad_t, line_t, &self.line[l_index]);
        }
    }

    fn add_exact_horizontal_end_points(&mut self, left: f64, right: f64, y: f64) {
        for q_index in (0..3).step_by(2) {
            let line_t = SkDLine::exact_point_h(&self.quad[q_index], left, right, y);
            if line_t < 0.0 {
                continue;
            }
            let quad_t = (q_index >> 1) as f64;
            self.intersections
                .insert(quad_t, line_t, &self.quad[q_index]);
        }
    }

    fn add_near_horizontal_end_points(&mut self, left: f64, right: f64, y: f64) {
        for q_index in (0..3).step_by(2) {
            let quad_t = (q_index >> 1) as f64;
            if self.intersections.has_t(quad_t) {
                continue;
            }
            let line_t = SkDLine::near_point_h(&self.quad[q_index], left, right, y);
            if line_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(quad_t, line_t, &self.quad[q_index]);
        }
        self.add_line_near_end_points();
    }

    fn add_exact_vertical_end_points(&mut self, top: f64, bottom: f64, x: f64) {
        for q_index in (0..3).step_by(2) {
            let line_t = SkDLine::exact_point_v(&self.quad[q_index], top, bottom, x);
            if line_t < 0.0 {
                continue;
            }
            let quad_t = (q_index >> 1) as f64;
            self.intersections
                .insert(quad_t, line_t, &self.quad[q_index]);
        }
    }

    fn add_near_vertical_end_points(&mut self, top: f64, bottom: f64, x: f64) {
        for q_index in (0..3).step_by(2) {
            let quad_t = (q_index >> 1) as f64;
            if self.intersections.has_t(quad_t) {
                continue;
            }
            let line_t = SkDLine::near_point_v(&self.quad[q_index], top, bottom, x);
            if line_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(quad_t, line_t, &self.quad[q_index]);
        }
        self.add_line_near_end_points();
    }

    fn find_line_t(&self, t: f64) -> f64 {
        let xy = self.quad.pt_at_t(t);
        let dx = self.line[1].x - self.line[0].x;
        let dy = self.line[1].y - self.line[0].y;
        if dx.abs() > dy.abs() {
            return (xy.x - self.line[0].x) / dx;
        }
        (xy.y - self.line[0].y) / dy
    }

    fn pin_ts(
        &self,
        quad_t: &mut f64,
        line_t: &mut f64,
        pt: &mut SkDPoint,
        pt_set: PinTPoint,
    ) -> bool {
        if !approximately_one_or_less_double(*line_t) {
            return false;
        }
        if !approximately_zero_or_more_double(*line_t) {
            return false;
        }
        *quad_t = sk_pin_t(*quad_t);
        let q_t = *quad_t;
        *line_t = sk_pin_t(*line_t);
        let l_t = *line_t;
        if l_t == 0.0
            || l_t == 1.0
            || (pt_set == PinTPoint::PointUninitialized && q_t != 0.0 && q_t != 1.0)
        {
            *pt = self.line.pt_at_t(l_t);
        } else if pt_set == PinTPoint::PointUninitialized {
            *pt = self.quad.pt_at_t(q_t);
        }
        let grid_pt = pt.as_sk_point();
        if SkDPoint::approximately_equal_sk_points(&grid_pt, &self.line[0].as_sk_point()) {
            *pt = self.line[0];
            *line_t = 0.0;
        } else if SkDPoint::approximately_equal_sk_points(&grid_pt, &self.line[1].as_sk_point()) {
            *pt = self.line[1];
            *line_t = 1.0;
        }
        if self.intersections.used() > 0 && approximately_equal(self.intersections[1][0], *line_t) {
            return false;
        }
        if grid_pt == self.quad[0].as_sk_point() {
            *pt = self.quad[0];
            *quad_t = 0.0;
        } else if grid_pt == self.quad[2].as_sk_point() {
            *pt = self.quad[2];
            *quad_t = 1.0;
        }
        true
    }
}

impl SkIntersections {
    /// `horizontal(const SkDQuad&, double left, double right, double y, bool flipped)`.
    pub(crate) fn horizontal_quad(
        &mut self,
        quad: &SkDQuad,
        left: f64,
        right: f64,
        y: f64,
        flipped: bool,
    ) -> i32 {
        let line = SkDLine {
            pts: [SkDPoint { x: left, y }, SkDPoint { x: right, y }],
        };
        let mut q = LineQuadraticIntersections::new(quad, &line, self);
        q.horizontal_intersect(y, left, right, flipped)
    }

    /// `vertical(const SkDQuad&, double top, double bottom, double x, bool flipped)`.
    pub(crate) fn vertical_quad(
        &mut self,
        quad: &SkDQuad,
        top: f64,
        bottom: f64,
        x: f64,
        flipped: bool,
    ) -> i32 {
        let line = SkDLine {
            pts: [SkDPoint { x, y: top }, SkDPoint { x, y: bottom }],
        };
        let mut q = LineQuadraticIntersections::new(quad, &line, self);
        q.vertical_intersect(x, top, bottom, flipped)
    }

    /// `intersect(const SkDQuad& quad, const SkDLine& line)`.
    pub(crate) fn intersect_quad_line(&mut self, quad: &SkDQuad, line: &SkDLine) -> i32 {
        let allow_near = self.allow_near;
        let mut q = LineQuadraticIntersections::new(quad, line, self);
        q.allow_near(allow_near);
        q.intersect()
    }

    /// `intersectRay(const SkDQuad& quad, const SkDLine& line)`.
    pub(crate) fn intersect_ray_quad(&mut self, quad: &SkDQuad, line: &SkDLine) -> i32 {
        let q = LineQuadraticIntersections::new(quad, line, self);
        // Skia passes `fT[0]` as the roots; `intersectRay` writes them and reads nothing else of
        // the intersections, so they are written to a copy that replaces `fT[0]` after.
        let mut roots = q.intersections.t[0];
        let used = q.intersect_ray(&mut roots);
        self.t[0] = roots;
        self.used = used as u8;
        for index in 0..self.used as usize {
            self.pt[index] = quad.pt_at_t(self.t[0][index]);
        }
        self.used as i32
    }

    /// `static HorizontalIntercept(const SkDQuad& quad, SkScalar y, double* roots)`.
    pub(crate) fn horizontal_intercept_quad(quad: &SkDQuad, y: f32, roots: &mut [f64]) -> i32 {
        LineQuadraticIntersections::horizontal_intersect_roots(quad, y as f64, roots)
    }

    /// `static VerticalIntercept(const SkDQuad& quad, SkScalar x, double* roots)`.
    pub(crate) fn vertical_intercept_quad(quad: &SkDQuad, x: f32, roots: &mut [f64]) -> i32 {
        LineQuadraticIntersections::vertical_intersect_roots(quad, x as f64, roots)
    }
}
