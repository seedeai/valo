//! Port of the parts of Skia's include/core/SkPoint.h (SkPoint_impl.h) and
//! src/core/SkPointPriv.h that PathOps calls.

use std::ops::{Add, Sub};

use crate::core::float_bits::sk_is_finite_f32;

/// `SkPoint` is a point or vector in the plane, two 32-bit floats.
///
/// Paths store their points as `SkPoint`s, in the coordinates the caller works in; y grows
/// downward as in Skia, though the boolean operations do not depend on the orientation.
/// Comparisons are exact float comparisons, so a point holding NaN is not equal to itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkPoint {
    /// `x` is the horizontal coordinate.
    pub x: f32,
    /// `y` is the vertical coordinate.
    pub y: f32,
}

/// `SkVector` is Skia's name for a point used as a direction.
pub(crate) type SkVector = SkPoint;

impl SkPoint {
    /// `new` makes the point (x, y).
    pub const fn new(x: f32, y: f32) -> SkPoint {
        SkPoint { x, y }
    }

    pub(crate) fn set(&mut self, x: f32, y: f32) {
        self.x = x;
        self.y = y;
    }

    pub(crate) fn is_finite(&self) -> bool {
        sk_is_finite_f32(&[self.x, self.y])
    }

    pub(crate) fn dot_product(a: &SkVector, b: &SkVector) -> f32 {
        a.x * b.x + a.y * b.y
    }

    pub(crate) fn cross_product(a: &SkVector, b: &SkVector) -> f32 {
        a.x * b.y - a.y * b.x
    }

    pub(crate) fn dot(&self, vec: &SkVector) -> f32 {
        SkPoint::dot_product(self, vec)
    }
}

impl Sub for SkPoint {
    type Output = SkVector;

    fn sub(self, b: SkPoint) -> SkVector {
        SkPoint {
            x: self.x - b.x,
            y: self.y - b.y,
        }
    }
}

impl Add for SkPoint {
    type Output = SkPoint;

    fn add(self, b: SkVector) -> SkPoint {
        SkPoint {
            x: self.x + b.x,
            y: self.y + b.y,
        }
    }
}

/// Stands for Skia's `SkPointPriv`, a namespace of point helpers.
pub(crate) struct SkPointPriv;

impl SkPointPriv {
    pub(crate) fn distance_to_sqd(pt: &SkPoint, a: &SkPoint) -> f32 {
        let dx = pt.x - a.x;
        let dy = pt.y - a.y;
        dx * dx + dy * dy
    }
}
