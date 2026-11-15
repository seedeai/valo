//! Port of Skia's src/pathops/SkPathOpsCurve.{h,cpp}: a curve of any verb, in doubles
//! (`SkDCurve`), and the tables of per-verb functions PathOps calls through a segment's verb.
//!
//! `SkDCurve` is a C++ union of a line, quad, conic and cubic sharing one point array. Here it
//! is the point array (sized for a cubic) and the conic's weight; `as_line`/`as_quad`/
//! `as_conic`/`as_cubic` read it as one member, `set_quad`/... write it as one. Skia's union
//! places the conic weight over the cubic's last point; no PathOps code writes one member and
//! reads the other there, so separate fields keep the same results.
//!
//! The tables are indexed by verb (`CurveIntercept` by `verb * 2 + vertical`); the move (and
//! line, for `CurveIntercept`) entries are null in Skia and `None` here.

use std::ops::{Index, IndexMut};

use crate::core::float_bits::{sk_t_abs, std_max, std_min, FLT_MAX};
use crate::core::path::SkPathVerb;
use crate::core::point::SkPoint;
use crate::intersections::SkIntersections;
use crate::path_ops_bounds::SkPathOpsBounds;
use crate::path_ops_conic::SkDConic;
use crate::path_ops_cubic::SkDCubic;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::{SkDPoint, SkDVector};
use crate::path_ops_quad::SkDQuad;
use crate::path_ops_rect::SkDRect;
use crate::path_ops_types::{
    almost_between_ulps, almost_equal_ulps_pin, between, roughly_zero_when_compared_to,
    sk_path_ops_verb_to_points, sk_pin_t,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkDCurve {
    /// The union's shared points: `fLine.fPts`, `fQuad.fPts`, `fConic.fPts.fPts`, `fCubic.fPts`.
    pub(crate) pts: [SkDPoint; 4],
    /// `fConic.fWeight`.
    pub(crate) weight: f32,
}

/// `operator[]` reads through `fCubic`, the union's largest member.
impl Index<usize> for SkDCurve {
    type Output = SkDPoint;

    fn index(&self, n: usize) -> &SkDPoint {
        &self.pts[n]
    }
}

impl IndexMut<usize> for SkDCurve {
    fn index_mut(&mut self, n: usize) -> &mut SkDPoint {
        &mut self.pts[n]
    }
}

impl SkDCurve {
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

    /// `fConic`.
    pub(crate) fn as_conic(&self) -> SkDConic {
        SkDConic {
            pts: self.as_quad(),
            weight: self.weight,
        }
    }

    /// `fCubic`.
    pub(crate) fn as_cubic(&self) -> SkDCubic {
        SkDCubic { pts: self.pts }
    }

    /// Writes `fQuad`.
    pub(crate) fn set_quad(&mut self, quad: &SkDQuad) {
        self.pts[..3].copy_from_slice(&quad.pts);
    }

    /// Writes `fConic`.
    pub(crate) fn set_conic(&mut self, conic: &SkDConic) {
        self.pts[..3].copy_from_slice(&conic.pts.pts);
        self.weight = conic.weight;
    }

    /// Writes `fCubic`.
    pub(crate) fn set_cubic(&mut self, cubic: &SkDCubic) {
        self.pts = cubic.pts;
    }

    // this cheats and assumes that the perpendicular to the point is the closest ray to the curve
    // this case (where the line and the curve are nearly coincident) may be the only case that counts
    pub(crate) fn near_point(&self, verb: SkPathVerb, xy: &SkDPoint, opp: &SkDPoint) -> f64 {
        let count = sk_path_ops_verb_to_points(verb) as usize;
        let mut min_x = self.pts[0].x;
        let mut max_x = min_x;
        for pt in &self.pts[1..=count] {
            min_x = std_min(min_x, pt.x);
            max_x = std_max(max_x, pt.x);
        }
        if !almost_between_ulps(min_x, xy.x, max_x) {
            return -1.0;
        }
        let mut min_y = self.pts[0].y;
        let mut max_y = min_y;
        for pt in &self.pts[1..=count] {
            min_y = std_min(min_y, pt.y);
            max_y = std_max(max_y, pt.y);
        }
        if !almost_between_ulps(min_y, xy.y, max_y) {
            return -1.0;
        }
        let mut i = SkIntersections::new();
        let perp = SkDLine {
            pts: [
                *xy,
                SkDPoint {
                    x: xy.x + opp.y - xy.y,
                    y: xy.y + xy.x - opp.x,
                },
            ],
        };
        (CURVE_D_INTERSECT_RAY[verb as usize].expect("a curve verb"))(self, &perp, &mut i);
        let mut min_index: i32 = -1;
        let mut min_dist = FLT_MAX as f64;
        for index in 0..i.used() {
            let dist = xy.distance(i.pt(index as usize));
            if min_dist > dist {
                min_dist = dist;
                min_index = index;
            }
        }
        if min_index < 0 {
            return -1.0;
        }
        let largest = std_max(std_max(max_x, max_y), -std_min(min_x, min_y));
        if !almost_equal_ulps_pin(largest, largest + min_dist) {
            // is distance within ULPS tolerance?
            return -1.0;
        }
        sk_pin_t(i[0][min_index as usize])
    }

    pub(crate) fn set_conic_bounds(
        &self,
        curve: &[SkPoint],
        curve_weight: f32,
        t_start: f64,
        t_end: f64,
        bounds: &mut SkPathOpsBounds,
    ) {
        let mut d_curve = SkDConic::default();
        d_curve.set(curve, curve_weight);
        let mut d_rect = SkDRect::default();
        d_rect.set_bounds_conic_part(&d_curve, &self.as_conic(), t_start, t_end);
        bounds.set_ltrb(
            d_rect.left as f32,
            d_rect.top as f32,
            d_rect.right as f32,
            d_rect.bottom as f32,
        );
    }

    pub(crate) fn set_cubic_bounds(
        &self,
        curve: &[SkPoint],
        curve_weight: f32,
        t_start: f64,
        t_end: f64,
        bounds: &mut SkPathOpsBounds,
    ) {
        let _ = curve_weight;
        let mut d_curve = SkDCubic::default();
        d_curve.set(curve);
        let mut d_rect = SkDRect::default();
        d_rect.set_bounds_cubic_part(&d_curve, &self.as_cubic(), t_start, t_end);
        bounds.set_ltrb(
            d_rect.left as f32,
            d_rect.top as f32,
            d_rect.right as f32,
            d_rect.bottom as f32,
        );
    }

    pub(crate) fn set_quad_bounds(
        &self,
        curve: &[SkPoint],
        curve_weight: f32,
        t_start: f64,
        t_end: f64,
        bounds: &mut SkPathOpsBounds,
    ) {
        let _ = curve_weight;
        let mut d_curve = SkDQuad::default();
        d_curve.set(curve);
        let mut d_rect = SkDRect::default();
        d_rect.set_bounds_quad_part(&d_curve, &self.as_quad(), t_start, t_end);
        bounds.set_ltrb(
            d_rect.left as f32,
            d_rect.top as f32,
            d_rect.right as f32,
            d_rect.bottom as f32,
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkDCurveSweep {
    pub(crate) curve: SkDCurve,
    pub(crate) sweep: [SkDVector; 2],
    is_curve: bool,
    /// cleared when a cubic's control point isn't between the sweep vectors
    ordered: bool,
}

impl SkDCurveSweep {
    pub(crate) fn is_curve(&self) -> bool {
        self.is_curve
    }

    pub(crate) fn is_ordered(&self) -> bool {
        self.ordered
    }

    pub(crate) fn set_curve_hull_sweep(&mut self, verb: SkPathVerb) {
        self.ordered = true;
        self.sweep[0] = self.curve[1] - self.curve[0];
        if SkPathVerb::Line == verb {
            self.sweep[1] = self.sweep[0];
            self.is_curve = false;
            return;
        }
        self.sweep[1] = self.curve[2] - self.curve[0];
        // OPTIMIZE: I do the following float check a lot -- probably need a
        // central place for this val-is-small-compared-to-curve check
        let mut max_val = 0.0;
        for index in 0..=sk_path_ops_verb_to_points(verb) as usize {
            max_val = std_max(
                max_val,
                std_max(sk_t_abs(self.curve[index].x), sk_t_abs(self.curve[index].y)),
            );
        }
        // Skia's `goto setIsCurve` leaves this block.
        'set_is_curve: {
            if SkPathVerb::Cubic != verb {
                if roughly_zero_when_compared_to(self.sweep[0].x, max_val)
                    && roughly_zero_when_compared_to(self.sweep[0].y, max_val)
                {
                    self.sweep[0] = self.sweep[1];
                }
                break 'set_is_curve;
            }
            let third_sweep = self.curve[3] - self.curve[0];
            if self.sweep[0].x == 0.0 && self.sweep[0].y == 0.0 {
                self.sweep[0] = self.sweep[1];
                self.sweep[1] = third_sweep;
                if roughly_zero_when_compared_to(self.sweep[0].x, max_val)
                    && roughly_zero_when_compared_to(self.sweep[0].y, max_val)
                {
                    self.sweep[0] = self.sweep[1];
                    self.curve[1] = self.curve[3];
                }
                break 'set_is_curve;
            }
            let s1x3 = self.sweep[0].cross_check(&third_sweep);
            let s3x2 = third_sweep.cross_check(&self.sweep[1]);
            if s1x3 * s3x2 >= 0.0 {
                // if third vector is on or between first two vectors
                break 'set_is_curve;
            }
            let s2x1 = self.sweep[1].cross_check(&self.sweep[0]);
            // FIXME: If the sweep of the cubic is greater than 180 degrees, we're in trouble
            // probably such wide sweeps should be artificially subdivided earlier so that never happens
            debug_assert!(s1x3 * s2x1 < 0.0 || s1x3 * s3x2 < 0.0);
            if s3x2 * s2x1 < 0.0 {
                debug_assert!(s2x1 * s1x3 > 0.0);
                self.sweep[0] = self.sweep[1];
                self.ordered = false;
            }
            self.sweep[1] = third_sweep;
        }
        self.is_curve = self.sweep[0].cross_check(&self.sweep[1]) != 0.0;
    }
}

fn dline_xy_at_t(a: &[SkPoint], _weight: f32, t: f64) -> SkDPoint {
    let mut line = SkDLine::default();
    line.set(a);
    line.pt_at_t(t)
}

fn dquad_xy_at_t(a: &[SkPoint], _weight: f32, t: f64) -> SkDPoint {
    let mut quad = SkDQuad::default();
    quad.set(a);
    quad.pt_at_t(t)
}

fn dconic_xy_at_t(a: &[SkPoint], weight: f32, t: f64) -> SkDPoint {
    let mut conic = SkDConic::default();
    conic.set(a, weight);
    conic.pt_at_t(t)
}

fn dcubic_xy_at_t(a: &[SkPoint], _weight: f32, t: f64) -> SkDPoint {
    let mut cubic = SkDCubic::default();
    cubic.set(a);
    cubic.pt_at_t(t)
}

pub(crate) type CurveDPointAtTFn = fn(&[SkPoint], f32, f64) -> SkDPoint;

pub(crate) const CURVE_D_POINT_AT_T: [Option<CurveDPointAtTFn>; 5] = [
    None,
    Some(dline_xy_at_t),
    Some(dquad_xy_at_t),
    Some(dconic_xy_at_t),
    Some(dcubic_xy_at_t),
];

fn ddline_xy_at_t(c: &SkDCurve, t: f64) -> SkDPoint {
    c.as_line().pt_at_t(t)
}

fn ddquad_xy_at_t(c: &SkDCurve, t: f64) -> SkDPoint {
    c.as_quad().pt_at_t(t)
}

fn ddconic_xy_at_t(c: &SkDCurve, t: f64) -> SkDPoint {
    c.as_conic().pt_at_t(t)
}

fn ddcubic_xy_at_t(c: &SkDCurve, t: f64) -> SkDPoint {
    c.as_cubic().pt_at_t(t)
}

pub(crate) type CurveDDPointAtTFn = fn(&SkDCurve, f64) -> SkDPoint;

pub(crate) const CURVE_DD_POINT_AT_T: [Option<CurveDDPointAtTFn>; 5] = [
    None,
    Some(ddline_xy_at_t),
    Some(ddquad_xy_at_t),
    Some(ddconic_xy_at_t),
    Some(ddcubic_xy_at_t),
];

fn fline_xy_at_t(a: &[SkPoint], weight: f32, t: f64) -> SkPoint {
    dline_xy_at_t(a, weight, t).as_sk_point()
}

fn fquad_xy_at_t(a: &[SkPoint], weight: f32, t: f64) -> SkPoint {
    dquad_xy_at_t(a, weight, t).as_sk_point()
}

fn fconic_xy_at_t(a: &[SkPoint], weight: f32, t: f64) -> SkPoint {
    dconic_xy_at_t(a, weight, t).as_sk_point()
}

fn fcubic_xy_at_t(a: &[SkPoint], weight: f32, t: f64) -> SkPoint {
    dcubic_xy_at_t(a, weight, t).as_sk_point()
}

pub(crate) type CurvePointAtTFn = fn(&[SkPoint], f32, f64) -> SkPoint;

pub(crate) const CURVE_POINT_AT_T: [Option<CurvePointAtTFn>; 5] = [
    None,
    Some(fline_xy_at_t),
    Some(fquad_xy_at_t),
    Some(fconic_xy_at_t),
    Some(fcubic_xy_at_t),
];

fn dline_dxdy_at_t(a: &[SkPoint], _weight: f32, _t: f64) -> SkDVector {
    let mut line = SkDLine::default();
    line.set(a);
    line[1] - line[0]
}

fn dquad_dxdy_at_t(a: &[SkPoint], _weight: f32, t: f64) -> SkDVector {
    let mut quad = SkDQuad::default();
    quad.set(a);
    quad.dxdy_at_t(t)
}

fn dconic_dxdy_at_t(a: &[SkPoint], weight: f32, t: f64) -> SkDVector {
    let mut conic = SkDConic::default();
    conic.set(a, weight);
    conic.dxdy_at_t(t)
}

fn dcubic_dxdy_at_t(a: &[SkPoint], _weight: f32, t: f64) -> SkDVector {
    let mut cubic = SkDCubic::default();
    cubic.set(a);
    cubic.dxdy_at_t(t)
}

pub(crate) type CurveDSlopeAtTFn = fn(&[SkPoint], f32, f64) -> SkDVector;

pub(crate) const CURVE_D_SLOPE_AT_T: [Option<CurveDSlopeAtTFn>; 5] = [
    None,
    Some(dline_dxdy_at_t),
    Some(dquad_dxdy_at_t),
    Some(dconic_dxdy_at_t),
    Some(dcubic_dxdy_at_t),
];

fn ddline_dxdy_at_t(c: &SkDCurve, _t: f64) -> SkDVector {
    c.pts[1] - c.pts[0]
}

fn ddquad_dxdy_at_t(c: &SkDCurve, t: f64) -> SkDVector {
    c.as_quad().dxdy_at_t(t)
}

fn ddconic_dxdy_at_t(c: &SkDCurve, t: f64) -> SkDVector {
    c.as_conic().dxdy_at_t(t)
}

fn ddcubic_dxdy_at_t(c: &SkDCurve, t: f64) -> SkDVector {
    c.as_cubic().dxdy_at_t(t)
}

pub(crate) type CurveDDSlopeAtTFn = fn(&SkDCurve, f64) -> SkDVector;

pub(crate) const CURVE_DD_SLOPE_AT_T: [Option<CurveDDSlopeAtTFn>; 5] = [
    None,
    Some(ddline_dxdy_at_t),
    Some(ddquad_dxdy_at_t),
    Some(ddconic_dxdy_at_t),
    Some(ddcubic_dxdy_at_t),
];

fn line_intersect_ray(a: &[SkPoint], _weight: f32, ray: &SkDLine, i: &mut SkIntersections) {
    let mut line = SkDLine::default();
    line.set(a);
    i.intersect_ray_line(&line, ray);
}

fn quad_intersect_ray(a: &[SkPoint], _weight: f32, ray: &SkDLine, i: &mut SkIntersections) {
    let mut quad = SkDQuad::default();
    quad.set(a);
    i.intersect_ray_quad(&quad, ray);
}

fn conic_intersect_ray(a: &[SkPoint], weight: f32, ray: &SkDLine, i: &mut SkIntersections) {
    let mut conic = SkDConic::default();
    conic.set(a, weight);
    i.intersect_ray_conic(&conic, ray);
}

fn cubic_intersect_ray(a: &[SkPoint], _weight: f32, ray: &SkDLine, i: &mut SkIntersections) {
    let mut cubic = SkDCubic::default();
    cubic.set(a);
    i.intersect_ray_cubic(&cubic, ray);
}

pub(crate) type CurveIntersectRayFn = fn(&[SkPoint], f32, &SkDLine, &mut SkIntersections);

pub(crate) const CURVE_INTERSECT_RAY: [Option<CurveIntersectRayFn>; 5] = [
    None,
    Some(line_intersect_ray),
    Some(quad_intersect_ray),
    Some(conic_intersect_ray),
    Some(cubic_intersect_ray),
];

fn dline_intersect_ray(c: &SkDCurve, ray: &SkDLine, i: &mut SkIntersections) {
    i.intersect_ray_line(&c.as_line(), ray);
}

fn dquad_intersect_ray(c: &SkDCurve, ray: &SkDLine, i: &mut SkIntersections) {
    i.intersect_ray_quad(&c.as_quad(), ray);
}

fn dconic_intersect_ray(c: &SkDCurve, ray: &SkDLine, i: &mut SkIntersections) {
    i.intersect_ray_conic(&c.as_conic(), ray);
}

fn dcubic_intersect_ray(c: &SkDCurve, ray: &SkDLine, i: &mut SkIntersections) {
    i.intersect_ray_cubic(&c.as_cubic(), ray);
}

pub(crate) type CurveDIntersectRayFn = fn(&SkDCurve, &SkDLine, &mut SkIntersections);

pub(crate) const CURVE_D_INTERSECT_RAY: [Option<CurveDIntersectRayFn>; 5] = [
    None,
    Some(dline_intersect_ray),
    Some(dquad_intersect_ray),
    Some(dconic_intersect_ray),
    Some(dcubic_intersect_ray),
];

fn line_intercept_h(a: &[SkPoint], _weight: f32, y: f32, roots: &mut [f64]) -> i32 {
    if a[0].y == a[1].y {
        return 0;
    }
    let mut line = SkDLine::default();
    roots[0] = SkIntersections::horizontal_intercept_line(line.set(a), y as f64);
    i32::from(between(0.0, roots[0], 1.0))
}

fn line_intercept_v(a: &[SkPoint], _weight: f32, x: f32, roots: &mut [f64]) -> i32 {
    if a[0].x == a[1].x {
        return 0;
    }
    let mut line = SkDLine::default();
    roots[0] = SkIntersections::vertical_intercept_line(line.set(a), x as f64);
    i32::from(between(0.0, roots[0], 1.0))
}

fn quad_intercept_h(a: &[SkPoint], _weight: f32, y: f32, roots: &mut [f64]) -> i32 {
    let mut quad = SkDQuad::default();
    SkIntersections::horizontal_intercept_quad(quad.set(a), y, roots)
}

fn quad_intercept_v(a: &[SkPoint], _weight: f32, x: f32, roots: &mut [f64]) -> i32 {
    let mut quad = SkDQuad::default();
    SkIntersections::vertical_intercept_quad(quad.set(a), x, roots)
}

fn conic_intercept_h(a: &[SkPoint], w: f32, y: f32, roots: &mut [f64]) -> i32 {
    let mut conic = SkDConic::default();
    SkIntersections::horizontal_intercept_conic(conic.set(a, w), y, roots)
}

fn conic_intercept_v(a: &[SkPoint], w: f32, x: f32, roots: &mut [f64]) -> i32 {
    let mut conic = SkDConic::default();
    SkIntersections::vertical_intercept_conic(conic.set(a, w), x, roots)
}

fn cubic_intercept_h(a: &[SkPoint], _weight: f32, y: f32, roots: &mut [f64]) -> i32 {
    let mut cubic = SkDCubic::default();
    cubic.set(a).horizontal_intersect(y as f64, roots)
}

fn cubic_intercept_v(a: &[SkPoint], _weight: f32, x: f32, roots: &mut [f64]) -> i32 {
    let mut cubic = SkDCubic::default();
    cubic.set(a).vertical_intersect(x as f64, roots)
}

pub(crate) type CurveInterceptFn = fn(&[SkPoint], f32, f32, &mut [f64]) -> i32;

/// Indexed by `verb * 2 + xy_index(dir)`.
pub(crate) const CURVE_INTERCEPT: [Option<CurveInterceptFn>; 10] = [
    None,
    None,
    Some(line_intercept_h),
    Some(line_intercept_v),
    Some(quad_intercept_h),
    Some(quad_intercept_v),
    Some(conic_intercept_h),
    Some(conic_intercept_v),
    Some(cubic_intercept_h),
    Some(cubic_intercept_v),
];
