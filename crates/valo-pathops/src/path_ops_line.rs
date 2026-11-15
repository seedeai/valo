//! Port of Skia's src/pathops/SkPathOpsLine.{h,cpp}: a line segment in doubles.

use std::ops::{Index, IndexMut};

use crate::core::float_bits::{std_max, std_min};
use crate::core::point::SkPoint;
use crate::path_ops_point::{SkDPoint, SkDVector};
use crate::path_ops_types::{
    almost_bequal_ulps, almost_between_ulps, almost_equal_ulps, almost_equal_ulps_pin, between,
    roughly_equal_ulps, sk_pin_t,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkDLine {
    pub(crate) pts: [SkDPoint; 2],
}

impl Index<usize> for SkDLine {
    type Output = SkDPoint;

    fn index(&self, n: usize) -> &SkDPoint {
        debug_assert!(n < 2);
        &self.pts[n]
    }
}

impl IndexMut<usize> for SkDLine {
    fn index_mut(&mut self, n: usize) -> &mut SkDPoint {
        debug_assert!(n < 2);
        &mut self.pts[n]
    }
}

impl SkDLine {
    pub(crate) fn set(&mut self, pts: &[SkPoint]) -> &SkDLine {
        self.pts[0].set(&pts[0]);
        self.pts[1].set(&pts[1]);
        self
    }

    pub(crate) fn exact_point(&self, xy: &SkDPoint) -> f64 {
        if *xy == self.pts[0] {
            // do cheapest test first
            return 0.0;
        }
        if *xy == self.pts[1] {
            return 1.0;
        }
        -1.0
    }

    pub(crate) fn exact_point_h(xy: &SkDPoint, left: f64, right: f64, y: f64) -> f64 {
        if xy.y == y {
            if xy.x == left {
                return 0.0;
            }
            if xy.x == right {
                return 1.0;
            }
        }
        -1.0
    }

    pub(crate) fn exact_point_v(xy: &SkDPoint, top: f64, bottom: f64, x: f64) -> f64 {
        if xy.x == x {
            if xy.y == top {
                return 0.0;
            }
            if xy.y == bottom {
                return 1.0;
            }
        }
        -1.0
    }

    /// `unequal` may be null in Skia.
    pub(crate) fn near_point(&self, xy: &SkDPoint, unequal: Option<&mut bool>) -> f64 {
        if !almost_between_ulps(self.pts[0].x, xy.x, self.pts[1].x)
            || !almost_between_ulps(self.pts[0].y, xy.y, self.pts[1].y)
        {
            return -1.0;
        }
        // project a perpendicular ray from the point to the line; find the T on the line
        let len = self.pts[1] - self.pts[0]; // the x/y magnitudes of the line
        let denom = len.x * len.x + len.y * len.y; // see DLine intersectRay
        let ab0 = *xy - self.pts[0];
        let numer = len.x * ab0.x + ab0.y * len.y;
        if !between(0.0, numer, denom) {
            return -1.0;
        }
        if denom == 0.0 {
            return 0.0;
        }
        let mut t = numer / denom;
        let real_pt = self.pt_at_t(t);
        let dist = real_pt.distance(xy); // OPTIMIZATION: can we compare against distSq instead ?

        // find the ordinal in the original line with the largest unsigned exponent
        let tiniest = std_min(
            std_min(std_min(self.pts[0].x, self.pts[0].y), self.pts[1].x),
            self.pts[1].y,
        );
        let mut largest = std_max(
            std_max(std_max(self.pts[0].x, self.pts[0].y), self.pts[1].x),
            self.pts[1].y,
        );
        largest = std_max(largest, -tiniest);
        if !almost_equal_ulps_pin(largest, largest + dist) {
            // is the dist within ULPS tolerance?
            return -1.0;
        }
        if let Some(unequal) = unequal {
            *unequal = largest as f32 != (largest + dist) as f32;
        }
        t = sk_pin_t(t); // a looser pin breaks skpwww_lptemp_com_3
        debug_assert!(between(0.0, t, 1.0));
        t
    }

    pub(crate) fn near_ray(&self, xy: &SkDPoint) -> bool {
        // project a perpendicular ray from the point to the line; find the T on the line
        let len = self.pts[1] - self.pts[0]; // the x/y magnitudes of the line
        let denom = len.x * len.x + len.y * len.y; // see DLine intersectRay
        let ab0 = *xy - self.pts[0];
        let numer = len.x * ab0.x + ab0.y * len.y;
        let t = numer / denom;
        let real_pt = self.pt_at_t(t);
        let dist = real_pt.distance(xy); // OPTIMIZATION: can we compare against distSq instead ?

        // find the ordinal in the original line with the largest unsigned exponent
        let tiniest = std_min(
            std_min(std_min(self.pts[0].x, self.pts[0].y), self.pts[1].x),
            self.pts[1].y,
        );
        let mut largest = std_max(
            std_max(std_max(self.pts[0].x, self.pts[0].y), self.pts[1].x),
            self.pts[1].y,
        );
        largest = std_max(largest, -tiniest);
        roughly_equal_ulps(largest, largest + dist) // is the dist within ULPS tolerance?
    }

    pub(crate) fn near_point_h(xy: &SkDPoint, left: f64, right: f64, y: f64) -> f64 {
        if !almost_bequal_ulps(xy.y, y) {
            return -1.0;
        }
        if !almost_between_ulps(left, xy.x, right) {
            return -1.0;
        }
        let mut t = (xy.x - left) / (right - left);
        t = sk_pin_t(t);
        debug_assert!(between(0.0, t, 1.0));
        let real_pt_x = (1.0 - t) * left + t * right;
        let dist_u = SkDVector {
            x: xy.y - y,
            y: xy.x - real_pt_x,
        };
        let dist_sq = dist_u.x * dist_u.x + dist_u.y * dist_u.y;
        let dist = dist_sq.sqrt(); // OPTIMIZATION: can we compare against distSq instead ?
        let tiniest = std_min(std_min(y, left), right);
        let mut largest = std_max(std_max(y, left), right);
        largest = std_max(largest, -tiniest);
        if !almost_equal_ulps(largest, largest + dist) {
            // is the dist within ULPS tolerance?
            return -1.0;
        }
        t
    }

    pub(crate) fn near_point_v(xy: &SkDPoint, top: f64, bottom: f64, x: f64) -> f64 {
        if !almost_bequal_ulps(xy.x, x) {
            return -1.0;
        }
        if !almost_between_ulps(top, xy.y, bottom) {
            return -1.0;
        }
        let mut t = (xy.y - top) / (bottom - top);
        t = sk_pin_t(t);
        debug_assert!(between(0.0, t, 1.0));
        let real_pt_y = (1.0 - t) * top + t * bottom;
        let dist_u = SkDVector {
            x: xy.x - x,
            y: xy.y - real_pt_y,
        };
        let dist_sq = dist_u.x * dist_u.x + dist_u.y * dist_u.y;
        let dist = dist_sq.sqrt(); // OPTIMIZATION: can we compare against distSq instead ?
        let tiniest = std_min(std_min(x, top), bottom);
        let mut largest = std_max(std_max(x, top), bottom);
        largest = std_max(largest, -tiniest);
        if !almost_equal_ulps(largest, largest + dist) {
            // is the dist within ULPS tolerance?
            return -1.0;
        }
        t
    }

    pub(crate) fn pt_at_t(&self, t: f64) -> SkDPoint {
        if 0.0 == t {
            return self.pts[0];
        }
        if 1.0 == t {
            return self.pts[1];
        }
        let one_t = 1.0 - t;
        SkDPoint {
            x: one_t * self.pts[0].x + t * self.pts[1].x,
            y: one_t * self.pts[0].y + t * self.pts[1].y,
        }
    }
}
