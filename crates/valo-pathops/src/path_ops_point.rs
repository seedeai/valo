//! Port of Skia's src/pathops/SkPathOpsPoint.h: points and vectors in doubles, and their
//! tolerant comparisons.

use std::ops::{Add, AddAssign, Sub};

use crate::core::float_bits::{sk_t_abs, std_max, std_min};
use crate::core::point::{SkPoint, SkVector};
use crate::path_ops_types::{
    almost_dequal_ulps, almost_equal_ulps, almost_equal_ulps_no_normal_check, almost_pequal_ulps,
    approximately_equal, roughly_equal, roughly_equal_ulps, roughly_equal_ulps_f32,
    roughly_zero_when_compared_to,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkDVector {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

impl SkDVector {
    pub(crate) fn set(&mut self, pt: &SkVector) -> &mut SkDVector {
        self.x = pt.x as f64;
        self.y = pt.y as f64;
        self
    }

    // similar to cross, this bastardization considers nearly coincident to be zero
    // uses ulps epsilon == 16
    pub(crate) fn cross_check(&self, a: &SkDVector) -> f64 {
        let xy = self.x * a.y;
        let yx = self.y * a.x;
        if almost_equal_ulps(xy, yx) {
            0.0
        } else {
            xy - yx
        }
    }

    // allow tinier numbers
    pub(crate) fn cross_no_normal_check(&self, a: &SkDVector) -> f64 {
        let xy = self.x * a.y;
        let yx = self.y * a.x;
        if almost_equal_ulps_no_normal_check(xy, yx) {
            0.0
        } else {
            xy - yx
        }
    }

    pub(crate) fn dot(&self, a: &SkDVector) -> f64 {
        self.x * a.x + self.y * a.y
    }

    pub(crate) fn length(&self) -> f64 {
        self.length_squared().sqrt()
    }

    pub(crate) fn length_squared(&self) -> f64 {
        self.x * self.x + self.y * self.y
    }
}

/// `operator==` compares both coordinates exactly, as the derived `PartialEq` does.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkDPoint {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

impl Sub for SkDPoint {
    type Output = SkDVector;

    fn sub(self, b: SkDPoint) -> SkDVector {
        SkDVector {
            x: self.x - b.x,
            y: self.y - b.y,
        }
    }
}

// only used by testing
impl AddAssign<SkDVector> for SkDPoint {
    fn add_assign(&mut self, v: SkDVector) {
        self.x += v.x;
        self.y += v.y;
    }
}

// only used by testing
impl Add<SkDVector> for SkDPoint {
    type Output = SkDPoint;

    fn add(self, v: SkDVector) -> SkDPoint {
        let mut result = self;
        result += v;
        result
    }
}

impl SkDPoint {
    /// `set(const SkPoint&)`, also standing for `operator=(const SkPoint&)`.
    pub(crate) fn set(&mut self, pt: &SkPoint) {
        self.x = pt.x as f64;
        self.y = pt.y as f64;
    }

    // note: this can not be implemented with
    // return approximately_equal(a.fY, fY) && approximately_equal(a.fX, fX);
    // because that will not take the magnitude of the values into account
    pub(crate) fn approximately_d_equal(&self, a: &SkDPoint) -> bool {
        if approximately_equal(self.x, a.x) && approximately_equal(self.y, a.y) {
            return true;
        }
        if !roughly_equal_ulps(self.x, a.x) || !roughly_equal_ulps(self.y, a.y) {
            return false;
        }
        let dist = self.distance(a); // OPTIMIZATION: can we compare against distSq instead ?
        let tiniest = std_min(std_min(std_min(self.x, a.x), self.y), a.y);
        let mut largest = std_max(std_max(std_max(self.x, a.x), self.y), a.y);
        largest = std_max(largest, -tiniest);
        almost_dequal_ulps(largest, largest + dist) // is the dist within ULPS tolerance?
    }

    pub(crate) fn approximately_d_equal_sk_point(&self, a: &SkPoint) -> bool {
        let mut d_a = SkDPoint::default();
        d_a.set(a);
        self.approximately_d_equal(&d_a)
    }

    pub(crate) fn approximately_equal(&self, a: &SkDPoint) -> bool {
        if approximately_equal(self.x, a.x) && approximately_equal(self.y, a.y) {
            return true;
        }
        if !roughly_equal_ulps(self.x, a.x) || !roughly_equal_ulps(self.y, a.y) {
            return false;
        }
        let dist = self.distance(a); // OPTIMIZATION: can we compare against distSq instead ?
        let tiniest = std_min(std_min(std_min(self.x, a.x), self.y), a.y);
        let mut largest = std_max(std_max(std_max(self.x, a.x), self.y), a.y);
        largest = std_max(largest, -tiniest);
        almost_pequal_ulps(largest, largest + dist) // is the dist within ULPS tolerance?
    }

    pub(crate) fn approximately_equal_sk_point(&self, a: &SkPoint) -> bool {
        let mut d_a = SkDPoint::default();
        d_a.set(a);
        self.approximately_equal(&d_a)
    }

    /// `static ApproximatelyEqual(const SkPoint&, const SkPoint&)`.
    pub(crate) fn approximately_equal_sk_points(a: &SkPoint, b: &SkPoint) -> bool {
        if approximately_equal(a.x as f64, b.x as f64)
            && approximately_equal(a.y as f64, b.y as f64)
        {
            return true;
        }
        if !roughly_equal_ulps_f32(a.x, b.x) || !roughly_equal_ulps_f32(a.y, b.y) {
            return false;
        }
        let mut d_a = SkDPoint::default();
        let mut d_b = SkDPoint::default();
        d_a.set(a);
        d_b.set(b);
        let dist = d_a.distance(&d_b); // OPTIMIZATION: can we compare against distSq instead ?
        let tiniest = std_min(std_min(std_min(a.x, b.x), a.y), b.y);
        let mut largest = std_max(std_max(std_max(a.x, b.x), a.y), b.y);
        largest = std_max(largest, -tiniest);
        almost_dequal_ulps(largest as f64, largest as f64 + dist) // is dist within ULPS tolerance?
    }

    pub(crate) fn as_sk_point(&self) -> SkPoint {
        SkPoint {
            x: self.x as f32,
            y: self.y as f32,
        }
    }

    pub(crate) fn distance(&self, a: &SkDPoint) -> f64 {
        let temp = *self - *a;
        temp.length()
    }

    pub(crate) fn distance_squared(&self, a: &SkDPoint) -> f64 {
        let temp = *self - *a;
        temp.length_squared()
    }

    pub(crate) fn mid(a: &SkDPoint, b: &SkDPoint) -> SkDPoint {
        SkDPoint {
            x: (a.x + b.x) / 2.0,
            y: (a.y + b.y) / 2.0,
        }
    }

    pub(crate) fn roughly_equal(&self, a: &SkDPoint) -> bool {
        if roughly_equal(self.x, a.x) && roughly_equal(self.y, a.y) {
            return true;
        }
        let dist = self.distance(a); // OPTIMIZATION: can we compare against distSq instead ?
        let tiniest = std_min(std_min(std_min(self.x, a.x), self.y), a.y);
        let mut largest = std_max(std_max(std_max(self.x, a.x), self.y), a.y);
        largest = std_max(largest, -tiniest);
        roughly_equal_ulps(largest, largest + dist) // is the dist within ULPS tolerance?
    }

    /// `static RoughlyEqual(const SkPoint&, const SkPoint&)`.
    pub(crate) fn roughly_equal_sk_points(a: &SkPoint, b: &SkPoint) -> bool {
        if !roughly_equal_ulps_f32(a.x, b.x) && !roughly_equal_ulps_f32(a.y, b.y) {
            return false;
        }
        let mut d_a = SkDPoint::default();
        let mut d_b = SkDPoint::default();
        d_a.set(a);
        d_b.set(b);
        let dist = d_a.distance(&d_b); // OPTIMIZATION: can we compare against distSq instead ?
        let tiniest = std_min(std_min(std_min(a.x, b.x), a.y), b.y);
        let mut largest = std_max(std_max(std_max(a.x, b.x), a.y), b.y);
        largest = std_max(largest, -tiniest);
        roughly_equal_ulps(largest as f64, largest as f64 + dist) // is dist within ULPS tolerance?
    }

    // very light weight check, should only be used for inequality check
    pub(crate) fn way_roughly_equal(a: &SkPoint, b: &SkPoint) -> bool {
        let largest_number = std_max(
            sk_t_abs(a.x),
            std_max(sk_t_abs(a.y), std_max(sk_t_abs(b.x), sk_t_abs(b.y))),
        );
        let diffs = *a - *b;
        let largest_diff = std_max(sk_t_abs(diffs.x), sk_t_abs(diffs.y));
        roughly_zero_when_compared_to(largest_diff as f64, largest_number as f64)
    }
}
