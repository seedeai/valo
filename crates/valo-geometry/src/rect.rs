use crate::{Point, Size};

/// `Rect` is an axis-aligned rectangle in Valo's y-down coordinate system.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rect {
    /// `x` is the left edge.
    pub x: f32,
    /// `y` is the top edge.
    pub y: f32,
    /// `width` is the horizontal extent.
    pub width: f32,
    /// `height` is the vertical extent.
    pub height: f32,
}

impl Rect {
    /// `new` creates a rectangle from its top-left origin and size.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// `from_ltrb` creates a rectangle from its left, top, right, and bottom edges.
    pub fn from_ltrb(l: f32, t: f32, r: f32, b: f32) -> Self {
        Self {
            x: l,
            y: t,
            width: r - l,
            height: b - t,
        }
    }

    /// `from_origin_size` creates a rectangle from an origin and size.
    pub fn from_origin_size(origin: Point, size: Size) -> Self {
        Self {
            x: origin.x,
            y: origin.y,
            width: size.width,
            height: size.height,
        }
    }

    /// `right` returns the x-coordinate of the right edge.
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// `bottom` returns the y-coordinate of the bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// `origin` returns the top-left point.
    pub fn origin(&self) -> Point {
        Point::new(self.x, self.y)
    }

    /// `size` returns the rectangle's width and height.
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    /// `is_empty` reports whether either extent is nonpositive.
    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    /// `union` returns the smallest rectangle containing both rectangles.
    ///
    /// Empty rectangles act as the identity.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        Rect::from_ltrb(
            self.x.min(other.x),
            self.y.min(other.y),
            self.right().max(other.right()),
            self.bottom().max(other.bottom()),
        )
    }

    /// `intersect` returns the overlapping area or `None` when there is none.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let r = Rect::from_ltrb(
            self.x.max(other.x),
            self.y.max(other.y),
            self.right().min(other.right()),
            self.bottom().min(other.bottom()),
        );
        (!r.is_empty()).then_some(r)
    }

    /// `intersects` reports whether the rectangles overlap with positive area.
    pub fn intersects(&self, other: &Rect) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// `contains` reports whether a point lies within the half-open rectangle.
    ///
    /// Left and top edges are included; right and bottom edges are excluded.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    /// `contains_inclusive` reports whether a point lies within or on every edge.
    pub fn contains_inclusive(&self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    /// `expand` moves every edge outward by `d`.
    ///
    /// Negative values contract the rectangle.
    pub fn expand(&self, d: f32) -> Rect {
        Rect::new(
            self.x - d,
            self.y - d,
            self.width + 2.0 * d,
            self.height + 2.0 * d,
        )
    }

    /// `EVERYTHING` is the conservative bound used when content cannot be bounded.
    pub const EVERYTHING: Rect = Rect {
        x: -1.0e9,
        y: -1.0e9,
        width: 2.0e9,
        height: 2.0e9,
    };

    /// `corners` returns corners clockwise from the top-left.
    pub fn corners(&self) -> [Point; 4] {
        [
            Point::new(self.x, self.y),
            Point::new(self.right(), self.y),
            Point::new(self.right(), self.bottom()),
            Point::new(self.x, self.bottom()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_ignores_empty() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(Rect::default().union(&a), a);
        assert_eq!(a.union(&Rect::default()), a);
    }

    #[test]
    fn intersect_disjoint_is_none() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(20.0, 0.0, 10.0, 10.0);
        assert_eq!(a.intersect(&b), None);
        assert!(!a.intersects(&b));
    }

    #[test]
    fn intersect_overlap() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersect(&b), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
    }
}
