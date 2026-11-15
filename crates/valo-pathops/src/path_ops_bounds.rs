//! Port of Skia's src/pathops/SkPathOpsBounds.h: bounds that, unlike `SkRect`, do not consider a
//! line to be empty.
//!
//! Skia's `SkPathOpsBounds` derives from `SkRect`; here it wraps one and dereferences to it, so
//! the rect's fields and methods read as they do in Skia.

use std::ops::{Deref, DerefMut};

use crate::core::rect::SkRect;
use crate::path_ops_types::almost_less_or_equal_ulps_f32;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkPathOpsBounds(pub(crate) SkRect);

impl Deref for SkPathOpsBounds {
    type Target = SkRect;

    fn deref(&self) -> &SkRect {
        &self.0
    }
}

impl DerefMut for SkPathOpsBounds {
    fn deref_mut(&mut self) -> &mut SkRect {
        &mut self.0
    }
}

impl SkPathOpsBounds {
    pub(crate) fn intersects(a: &SkPathOpsBounds, b: &SkPathOpsBounds) -> bool {
        almost_less_or_equal_ulps_f32(a.left, b.right)
            && almost_less_or_equal_ulps_f32(b.left, a.right)
            && almost_less_or_equal_ulps_f32(a.top, b.bottom)
            && almost_less_or_equal_ulps_f32(b.top, a.bottom)
    }

    // Note that add(), unlike SkRect::join() or SkRect::growToInclude()
    // does not treat the bounds of horizontal and vertical lines as
    // empty rectangles.
    /// `add(SkScalar left, SkScalar top, SkScalar right, SkScalar bottom)`.
    pub(crate) fn add_ltrb(&mut self, left: f32, top: f32, right: f32, bottom: f32) {
        if left < self.left {
            self.left = left;
        }
        if top < self.top {
            self.top = top;
        }
        if right > self.right {
            self.right = right;
        }
        if bottom > self.bottom {
            self.bottom = bottom;
        }
    }

    /// `add(const SkPathOpsBounds& toAdd)`.
    pub(crate) fn add_bounds(&mut self, to_add: &SkPathOpsBounds) {
        self.add_ltrb(to_add.left, to_add.top, to_add.right, to_add.bottom);
    }
}
