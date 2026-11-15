//! Port of the parts of Skia's include/core/SkRect.h and src/core/SkRect.cpp that PathOps calls.

use crate::core::float_bits::{sk_is_finite_f32, std_max, std_min};
use crate::core::point::SkPoint;

/// `SkRect`: four float edges, sorted when `left <= right` and `top <= bottom`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkRect {
    pub(crate) left: f32,
    pub(crate) top: f32,
    pub(crate) right: f32,
    pub(crate) bottom: f32,
}

impl SkRect {
    pub(crate) const fn make_empty() -> SkRect {
        SkRect {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        }
    }

    pub(crate) const fn make_ltrb(left: f32, top: f32, right: f32, bottom: f32) -> SkRect {
        SkRect {
            left,
            top,
            right,
            bottom,
        }
    }

    pub(crate) fn set_ltrb(&mut self, left: f32, top: f32, right: f32, bottom: f32) {
        self.left = left;
        self.top = top;
        self.right = right;
        self.bottom = bottom;
    }

    pub(crate) fn is_finite(&self) -> bool {
        sk_is_finite_f32(&[self.left, self.top, self.right, self.bottom])
    }

    pub(crate) fn make_sorted(&self) -> SkRect {
        SkRect::make_ltrb(
            std_min(self.left, self.right),
            std_min(self.top, self.bottom),
            std_max(self.left, self.right),
            std_max(self.top, self.bottom),
        )
    }

    /// `SkRect::Bounds`: the bounds of `points`, empty for no points, `None` if any coordinate
    /// is infinite or NaN. Skia's 64-bit variant: `fminf`/`fmaxf`, which `f32::min`/`max` are.
    pub(crate) fn bounds(points: &[SkPoint]) -> Option<SkRect> {
        if points.is_empty() {
            return Some(SkRect::make_empty());
        }
        let mut l = points[0].x;
        let mut t = points[0].y;
        let mut r = points[0].x;
        let mut b = points[0].y;
        let mut nx = 0.0_f32;
        let mut ny = 0.0_f32;
        for p in points {
            l = p.x.min(l);
            t = p.y.min(t);
            r = p.x.max(r);
            b = p.y.max(b);
            // we do this to look for infinities or nans
            nx *= p.x;
            ny *= p.y;
        }
        // if this is true, all our values were finite
        if nx == 0.0 && ny == 0.0 {
            return Some(SkRect::make_ltrb(l, t, r, b));
        }
        None
    }

    pub(crate) fn set_bounds_check(&mut self, pts: &[SkPoint]) -> bool {
        if let Some(bounds) = SkRect::bounds(pts) {
            *self = bounds;
            true
        } else {
            *self = SkRect::make_empty();
            false
        }
    }

    pub(crate) fn set_bounds(&mut self, pts: &[SkPoint]) {
        let _ = self.set_bounds_check(pts);
    }

    /// Returns false, leaving `self` unchanged, when the intersection is empty or NaN.
    pub(crate) fn intersect(&mut self, r: &SkRect) -> bool {
        let l = std_max(r.left, self.left);
        let rr = std_min(r.right, self.right);
        let t = std_max(r.top, self.top);
        let b = std_min(r.bottom, self.bottom);
        // do the !(opposite) check so we return false if either arg is NaN
        if !(l < rr && t < b) {
            return false;
        }
        self.set_ltrb(l, t, rr, b);
        true
    }
}
