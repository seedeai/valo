//! Port of Skia's src/pathops/SkLineParameters.h: a line segment as the line `ax + by + c = 0`.

use crate::core::float_bits::DBL_EPSILON;
use crate::path_ops_cubic::SkDCubic;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::SkDPoint;
use crate::path_ops_quad::SkDQuad;
use crate::path_ops_types::{approximately_zero, not_almost_equal_ulps};

// Sources
// computer-aided design - volume 22 number 9 november 1990 pp 538 - 549
// online at http://cagd.cs.byu.edu/~tom/papers/bezclip.pdf

// This turns a line segment into a parameterized line, of the form
// ax + by + c = 0
// When a^2 + b^2 == 1, the line is normalized.
// The distance to the line for (x, y) is d(x,y) = ax + by + c
//
// Note that the distances below are not necessarily normalized. To get the true
// distance, it's necessary to either call normalize() after xxxEndPoints(), or
// divide the result of xxxDistance() by sqrt(normalSquared())

/// Skia leaves the coefficients uninitialized until an `xxxEndPoints` call; here they start
/// at zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkLineParameters {
    a: f64,
    b: f64,
    c: f64,
}

impl SkLineParameters {
    pub(crate) fn cubic_end_points(&mut self, pts: &SkDCubic) -> bool {
        let mut end_index = 1;
        self.cubic_end_points_range(pts, 0, end_index);
        if self.dy() != 0.0 {
            return true;
        }
        if self.dx() == 0.0 {
            end_index += 1;
            self.cubic_end_points_range(pts, 0, end_index);
            debug_assert!(end_index == 2);
            if self.dy() != 0.0 {
                return true;
            }
            if self.dx() == 0.0 {
                end_index += 1;
                self.cubic_end_points_range(pts, 0, end_index); // line
                debug_assert!(end_index == 3);
                return false;
            }
        }
        // FIXME: after switching to round sort, remove bumping fA
        if self.dx() < 0.0 {
            // only worry about y bias when breaking cw/ccw tie
            return true;
        }
        // if cubic tangent is on x axis, look at next control point to break tie
        // control point may be approximate, so it must move significantly to account for error
        end_index += 1;
        if not_almost_equal_ulps(pts[0].y, pts[end_index].y) {
            if pts[0].y > pts[end_index].y {
                self.a = DBL_EPSILON; // push it from 0 to slightly negative (y() returns -a)
            }
            return true;
        }
        if end_index == 3 {
            return true;
        }
        debug_assert!(end_index == 2);
        if pts[0].y > pts[3].y {
            self.a = DBL_EPSILON; // push it from 0 to slightly negative (y() returns -a)
        }
        true
    }

    /// `cubicEndPoints(const SkDCubic& pts, int s, int e)`.
    pub(crate) fn cubic_end_points_range(&mut self, pts: &SkDCubic, s: usize, e: usize) {
        self.a = pts[s].y - pts[e].y;
        self.b = pts[e].x - pts[s].x;
        self.c = pts[s].x * pts[e].y - pts[e].x * pts[s].y;
    }

    /// Skia casts `part[0]` to an `SkDLine` reference: the line through `part[0]` and `part[1]`.
    pub(crate) fn cubic_part(&mut self, part: &SkDCubic) -> f64 {
        self.cubic_end_points(part);
        if part[0] == part[1]
            || (SkDLine {
                pts: [part[0], part[1]],
            })
            .near_ray(&part[2])
        {
            return self.point_distance(&part[3]);
        }
        self.point_distance(&part[2])
    }

    pub(crate) fn line_end_points(&mut self, pts: &SkDLine) {
        self.a = pts[0].y - pts[1].y;
        self.b = pts[1].x - pts[0].x;
        self.c = pts[0].x * pts[1].y - pts[1].x * pts[0].y;
    }

    pub(crate) fn quad_end_points(&mut self, pts: &SkDQuad) -> bool {
        self.quad_end_points_range(pts, 0, 1);
        if self.dy() != 0.0 {
            return true;
        }
        if self.dx() == 0.0 {
            self.quad_end_points_range(pts, 0, 2);
            return false;
        }
        if self.dx() < 0.0 {
            // only worry about y bias when breaking cw/ccw tie
            return true;
        }
        // FIXME: after switching to round sort, remove this
        if pts[0].y > pts[2].y {
            self.a = DBL_EPSILON;
        }
        true
    }

    /// `quadEndPoints(const SkDQuad& pts, int s, int e)`.
    pub(crate) fn quad_end_points_range(&mut self, pts: &SkDQuad, s: usize, e: usize) {
        self.a = pts[s].y - pts[e].y;
        self.b = pts[e].x - pts[s].x;
        self.c = pts[s].x * pts[e].y - pts[e].x * pts[s].y;
    }

    pub(crate) fn normal_squared(&self) -> f64 {
        self.a * self.a + self.b * self.b
    }

    pub(crate) fn normalize(&mut self) -> bool {
        let normal = self.normal_squared().sqrt();
        if approximately_zero(normal) {
            self.a = 0.0;
            self.b = 0.0;
            self.c = 0.0;
            return false;
        }
        let reciprocal = 1.0 / normal;
        self.a *= reciprocal;
        self.b *= reciprocal;
        self.c *= reciprocal;
        true
    }

    /// `controlPtDistance(const SkDCubic& pts, int index)`.
    pub(crate) fn control_pt_distance_cubic(&self, pts: &SkDCubic, index: usize) -> f64 {
        debug_assert!(index == 1 || index == 2);
        self.a * pts[index].x + self.b * pts[index].y + self.c
    }

    /// `controlPtDistance(const SkDQuad& pts)`.
    pub(crate) fn control_pt_distance_quad(&self, pts: &SkDQuad) -> f64 {
        self.a * pts[1].x + self.b * pts[1].y + self.c
    }

    pub(crate) fn point_distance(&self, pt: &SkDPoint) -> f64 {
        self.a * pt.x + self.b * pt.y + self.c
    }

    pub(crate) fn dx(&self) -> f64 {
        self.b
    }

    pub(crate) fn dy(&self) -> f64 {
        -self.a
    }
}
