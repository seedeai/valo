//! Port of Skia's src/pathops/SkDConicLineIntersection.cpp: where a conic meets a line, or a
//! horizontal or vertical line. The file-local `LineConicIntersections` that does the work is
//! not declared in a header, so its port comes with these bodies.
// Skia indexes its arrays in its loops; the port keeps the indices.
#![allow(clippy::needless_range_loop)]

use crate::core::path::SkPathVerb;
use crate::intersections::SkIntersections;
use crate::path_ops_conic::SkDConic;
use crate::path_ops_curve::SkDCurve;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::SkDPoint;
use crate::path_ops_quad::SkDQuad;
use crate::path_ops_types::{
    approximately_equal, approximately_one_or_less_double, approximately_zero_or_more_double,
    sk_pin_t,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum PinTPoint {
    PointUninitialized,
    PointInitialized,
}

/// Skia's second constructor, `LineConicIntersections(const SkDConic& c)`, leaves the line and
/// the intersections unset and serves only `horizontalIntersect(double, double[2])` and
/// `verticalIntersect(double, double[2])`, which read only the conic (through `validT`); here
/// those three are associated functions taking the conic, and the struct is always made with a
/// line.
struct LineConicIntersections<'a> {
    conic: &'a SkDConic,
    line: &'a SkDLine,
    intersections: &'a mut SkIntersections,
    allow_near: bool,
}

impl<'a> LineConicIntersections<'a> {
    fn new(
        c: &'a SkDConic,
        l: &'a SkDLine,
        i: &'a mut SkIntersections,
    ) -> LineConicIntersections<'a> {
        i.set_max(4); // allow short partial coincidence plus discrete intersection
        LineConicIntersections {
            conic: c,
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
            let conic_mid_t = (self.intersections[0][index as usize]
                + self.intersections[0][index as usize + 1])
                / 2.0;
            let conic_mid_pt = self.conic.pt_at_t(conic_mid_t);
            let t = self.line.near_point(&conic_mid_pt, None);
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

    /// `horizontalIntersect(double axisIntercept, double roots[2])`.
    fn horizontal_intersect_roots(conic: &SkDConic, axis_intercept: f64, roots: &mut [f64]) -> i32 {
        let conic_vals = [conic[0].y, conic[1].y, conic[2].y];
        Self::valid_t(conic, &conic_vals, axis_intercept, roots)
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
        let mut roots = [0.0f64; 2];
        let count = Self::horizontal_intersect_roots(self.conic, axis_intercept, &mut roots);
        for index in 0..count as usize {
            let mut conic_t = roots[index];
            let mut pt = self.conic.pt_at_t(conic_t);
            let mut line_t = (pt.x - left) / (right - left);
            if self.pin_ts(
                &mut conic_t,
                &mut line_t,
                &mut pt,
                PinTPoint::PointInitialized,
            ) && self.unique_answer(conic_t, &pt)
            {
                self.intersections.insert(conic_t, line_t, &pt);
            }
        }
        if flipped {
            self.intersections.flip();
        }
        self.check_coincident();
        self.intersections.used()
    }

    fn intersect(&mut self) -> i32 {
        self.add_exact_end_points();
        if self.allow_near {
            self.add_near_end_points();
        }
        let mut root_vals = [0.0f64; 2];
        let roots = self.intersect_ray(&mut root_vals);
        for index in 0..roots as usize {
            let mut conic_t = root_vals[index];
            let mut line_t = self.find_line_t(conic_t);
            let mut pt = SkDPoint::default();
            if self.pin_ts(
                &mut conic_t,
                &mut line_t,
                &mut pt,
                PinTPoint::PointUninitialized,
            ) && self.unique_answer(conic_t, &pt)
            {
                self.intersections.insert(conic_t, line_t, &pt);
            }
        }
        self.check_coincident();
        self.intersections.used()
    }

    fn intersect_ray(&self, roots: &mut [f64]) -> i32 {
        let adj = self.line[1].x - self.line[0].x;
        let opp = self.line[1].y - self.line[0].y;
        let mut r = [0.0f64; 3];
        for n in 0..3 {
            r[n] =
                (self.conic[n].y - self.line[0].y) * adj - (self.conic[n].x - self.line[0].x) * opp;
        }
        Self::valid_t(self.conic, &r, 0.0, roots)
    }

    /// `validT(double r[3], double axisIntercept, double roots[2])`.
    fn valid_t(conic: &SkDConic, r: &[f64; 3], axis_intercept: f64, roots: &mut [f64]) -> i32 {
        let weight = conic.weight as f64;
        let mut a = r[2];
        let mut b = r[1] * weight - axis_intercept * weight + axis_intercept;
        let mut c = r[0];
        a += c - 2.0 * b; // A = a + c - 2*(b*w - xCept*w + xCept)
        b -= c; // B = b*w - w * xCept + xCept - a
        c -= axis_intercept;
        SkDQuad::roots_valid_t(a, 2.0 * b, c, roots)
    }

    /// `verticalIntersect(double axisIntercept, double roots[2])`.
    fn vertical_intersect_roots(conic: &SkDConic, axis_intercept: f64, roots: &mut [f64]) -> i32 {
        let conic_vals = [conic[0].x, conic[1].x, conic[2].x];
        Self::valid_t(conic, &conic_vals, axis_intercept, roots)
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
        let mut roots = [0.0f64; 2];
        let count = Self::vertical_intersect_roots(self.conic, axis_intercept, &mut roots);
        for index in 0..count as usize {
            let mut conic_t = roots[index];
            let mut pt = self.conic.pt_at_t(conic_t);
            let mut line_t = (pt.y - top) / (bottom - top);
            if self.pin_ts(
                &mut conic_t,
                &mut line_t,
                &mut pt,
                PinTPoint::PointInitialized,
            ) && self.unique_answer(conic_t, &pt)
            {
                self.intersections.insert(conic_t, line_t, &pt);
            }
        }
        if flipped {
            self.intersections.flip();
        }
        self.check_coincident();
        self.intersections.used()
    }

    // OPTIMIZE: Functions of the form add .. points are indentical to the conic routines.
    // add endpoints first to get zero and one t values exactly
    fn add_exact_end_points(&mut self) {
        for c_index in (0..SkDConic::K_POINT_COUNT).step_by(SkDConic::K_POINT_LAST as usize) {
            let c_index = c_index as usize;
            let line_t = self.line.exact_point(&self.conic[c_index]);
            if line_t < 0.0 {
                continue;
            }
            let conic_t = (c_index >> 1) as f64;
            self.intersections
                .insert(conic_t, line_t, &self.conic[c_index]);
        }
    }

    fn add_near_end_points(&mut self) {
        for c_index in (0..SkDConic::K_POINT_COUNT).step_by(SkDConic::K_POINT_LAST as usize) {
            let c_index = c_index as usize;
            let conic_t = (c_index >> 1) as f64;
            if self.intersections.has_t(conic_t) {
                continue;
            }
            let line_t = self.line.near_point(&self.conic[c_index], None);
            if line_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(conic_t, line_t, &self.conic[c_index]);
        }
        self.add_line_near_end_points();
    }

    fn add_line_near_end_points(&mut self) {
        // Skia casts the conic to an `SkDCurve`, whose conic member shares its layout.
        let mut curve = SkDCurve::default();
        curve.set_conic(self.conic);
        for l_index in 0..2usize {
            let line_t = l_index as f64;
            if self.intersections.has_opp_t(line_t) {
                continue;
            }
            let conic_t = curve.near_point(
                SkPathVerb::Conic,
                &self.line[l_index],
                &self.line[usize::from(l_index == 0)],
            );
            if conic_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(conic_t, line_t, &self.line[l_index]);
        }
    }

    fn add_exact_horizontal_end_points(&mut self, left: f64, right: f64, y: f64) {
        for c_index in (0..SkDConic::K_POINT_COUNT).step_by(SkDConic::K_POINT_LAST as usize) {
            let c_index = c_index as usize;
            let line_t = SkDLine::exact_point_h(&self.conic[c_index], left, right, y);
            if line_t < 0.0 {
                continue;
            }
            let conic_t = (c_index >> 1) as f64;
            self.intersections
                .insert(conic_t, line_t, &self.conic[c_index]);
        }
    }

    fn add_near_horizontal_end_points(&mut self, left: f64, right: f64, y: f64) {
        for c_index in (0..SkDConic::K_POINT_COUNT).step_by(SkDConic::K_POINT_LAST as usize) {
            let c_index = c_index as usize;
            let conic_t = (c_index >> 1) as f64;
            if self.intersections.has_t(conic_t) {
                continue;
            }
            let line_t = SkDLine::near_point_h(&self.conic[c_index], left, right, y);
            if line_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(conic_t, line_t, &self.conic[c_index]);
        }
        self.add_line_near_end_points();
    }

    fn add_exact_vertical_end_points(&mut self, top: f64, bottom: f64, x: f64) {
        for c_index in (0..SkDConic::K_POINT_COUNT).step_by(SkDConic::K_POINT_LAST as usize) {
            let c_index = c_index as usize;
            let line_t = SkDLine::exact_point_v(&self.conic[c_index], top, bottom, x);
            if line_t < 0.0 {
                continue;
            }
            let conic_t = (c_index >> 1) as f64;
            self.intersections
                .insert(conic_t, line_t, &self.conic[c_index]);
        }
    }

    fn add_near_vertical_end_points(&mut self, top: f64, bottom: f64, x: f64) {
        for c_index in (0..SkDConic::K_POINT_COUNT).step_by(SkDConic::K_POINT_LAST as usize) {
            let c_index = c_index as usize;
            let conic_t = (c_index >> 1) as f64;
            if self.intersections.has_t(conic_t) {
                continue;
            }
            let line_t = SkDLine::near_point_v(&self.conic[c_index], top, bottom, x);
            if line_t < 0.0 {
                continue;
            }
            self.intersections
                .insert(conic_t, line_t, &self.conic[c_index]);
        }
        self.add_line_near_end_points();
    }

    fn find_line_t(&self, t: f64) -> f64 {
        let xy = self.conic.pt_at_t(t);
        let dx = self.line[1].x - self.line[0].x;
        let dy = self.line[1].y - self.line[0].y;
        if dx.abs() > dy.abs() {
            return (xy.x - self.line[0].x) / dx;
        }
        (xy.y - self.line[0].y) / dy
    }

    fn pin_ts(
        &self,
        conic_t: &mut f64,
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
        *conic_t = sk_pin_t(*conic_t);
        let q_t = *conic_t;
        *line_t = sk_pin_t(*line_t);
        let l_t = *line_t;
        if l_t == 0.0
            || l_t == 1.0
            || (pt_set == PinTPoint::PointUninitialized && q_t != 0.0 && q_t != 1.0)
        {
            *pt = self.line.pt_at_t(l_t);
        } else if pt_set == PinTPoint::PointUninitialized {
            *pt = self.conic.pt_at_t(q_t);
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
        if grid_pt == self.conic[0].as_sk_point() {
            *pt = self.conic[0];
            *conic_t = 0.0;
        } else if grid_pt == self.conic[2].as_sk_point() {
            *pt = self.conic[2];
            *conic_t = 1.0;
        }
        true
    }

    fn unique_answer(&self, conic_t: f64, pt: &SkDPoint) -> bool {
        for inner in 0..self.intersections.used() as usize {
            if self.intersections.pt(inner) != pt {
                continue;
            }
            let existing_conic_t = self.intersections[0][inner];
            if conic_t == existing_conic_t {
                return false;
            }
            // check if midway on conic is also same point. If so, discard this
            let conic_mid_t = (existing_conic_t + conic_t) / 2.0;
            let conic_mid_pt = self.conic.pt_at_t(conic_mid_t);
            if conic_mid_pt.approximately_equal(pt) {
                return false;
            }
        }
        true
    }
}

impl SkIntersections {
    /// `horizontal(const SkDConic&, double left, double right, double y, bool flipped)`.
    pub(crate) fn horizontal_conic(
        &mut self,
        conic: &SkDConic,
        left: f64,
        right: f64,
        y: f64,
        flipped: bool,
    ) -> i32 {
        let line = SkDLine {
            pts: [SkDPoint { x: left, y }, SkDPoint { x: right, y }],
        };
        let mut c = LineConicIntersections::new(conic, &line, self);
        c.horizontal_intersect(y, left, right, flipped)
    }

    /// `vertical(const SkDConic&, double top, double bottom, double x, bool flipped)`.
    pub(crate) fn vertical_conic(
        &mut self,
        conic: &SkDConic,
        top: f64,
        bottom: f64,
        x: f64,
        flipped: bool,
    ) -> i32 {
        let line = SkDLine {
            pts: [SkDPoint { x, y: top }, SkDPoint { x, y: bottom }],
        };
        let mut c = LineConicIntersections::new(conic, &line, self);
        c.vertical_intersect(x, top, bottom, flipped)
    }

    /// `intersect(const SkDConic& conic, const SkDLine& line)`.
    pub(crate) fn intersect_conic_line(&mut self, conic: &SkDConic, line: &SkDLine) -> i32 {
        let allow_near = self.allow_near;
        let mut c = LineConicIntersections::new(conic, line, self);
        c.allow_near(allow_near);
        c.intersect()
    }

    /// `intersectRay(const SkDConic& conic, const SkDLine& line)`.
    pub(crate) fn intersect_ray_conic(&mut self, conic: &SkDConic, line: &SkDLine) -> i32 {
        let c = LineConicIntersections::new(conic, line, self);
        // Skia passes `fT[0]` as the roots; `intersectRay` writes them and reads nothing else of
        // the intersections, so they are written to a copy that replaces `fT[0]` after.
        let mut roots = c.intersections.t[0];
        let used = c.intersect_ray(&mut roots);
        self.t[0] = roots;
        self.used = used as u8;
        for index in 0..self.used as usize {
            self.pt[index] = conic.pt_at_t(self.t[0][index]);
        }
        self.used as i32
    }

    /// `static HorizontalIntercept(const SkDConic& conic, SkScalar y, double* roots)`.
    pub(crate) fn horizontal_intercept_conic(conic: &SkDConic, y: f32, roots: &mut [f64]) -> i32 {
        LineConicIntersections::horizontal_intersect_roots(conic, y as f64, roots)
    }

    /// `static VerticalIntercept(const SkDConic& conic, SkScalar x, double* roots)`.
    pub(crate) fn vertical_intercept_conic(conic: &SkDConic, x: f32, roots: &mut [f64]) -> i32 {
        LineConicIntersections::vertical_intersect_roots(conic, x as f64, roots)
    }
}
