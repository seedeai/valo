//! Where recorded content may show: nowhere, inside a rectangle, or
//! anywhere a clip lets it. A draw whose image filter colours transparent
//! pixels has no bounds of its own: only a clip bounds it.
//!
//! `Bounded` always holds a rectangle with area; one without area is
//! `Empty`. A reader matches the three cases and never asks a rectangle
//! whether it stands for nothing or for everything.

use valo_geometry::{Matrix, Rect};

/// `Bounds` is where recorded content may show.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Bounds {
    /// `Empty` content shows nowhere.
    Empty,
    /// `Bounded` content shows only inside the rectangle, which has area.
    Bounded(Rect),
    /// `Unbounded` content may show anywhere a clip lets it.
    Unbounded,
}

impl Bounds {
    /// `of` bounds content inside `rect`: empty when `rect` has no area.
    pub fn of(rect: Rect) -> Self {
        if rect.is_empty() {
            Self::Empty
        } else {
            Self::Bounded(rect)
        }
    }

    /// `rect` is the rectangle of bounded content.
    pub fn rect(&self) -> Option<Rect> {
        match self {
            Self::Bounded(rect) => Some(*rect),
            Self::Empty | Self::Unbounded => None,
        }
    }

    /// `is_empty` reports whether the content shows nowhere.
    pub fn is_empty(&self) -> bool {
        *self == Self::Empty
    }

    /// `map` is where the content shows once `transform` places it.
    pub fn map(&self, transform: &Matrix) -> Self {
        match self {
            Self::Bounded(rect) => Self::of(transform.map_rect(rect)),
            Self::Empty | Self::Unbounded => *self,
        }
    }

    /// `union` is where either content shows.
    pub fn union(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Unbounded, _) | (_, Self::Unbounded) => Self::Unbounded,
            (Self::Empty, bounds) | (bounds, Self::Empty) => *bounds,
            (Self::Bounded(a), Self::Bounded(b)) => Self::Bounded(a.union(b)),
        }
    }

    /// `intersect` is where both show: content cut by a clip.
    pub fn intersect(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Empty, _) | (_, Self::Empty) => Self::Empty,
            (Self::Unbounded, bounds) | (bounds, Self::Unbounded) => *bounds,
            (Self::Bounded(a), Self::Bounded(b)) => {
                a.intersect(b).map_or(Self::Empty, Self::Bounded)
            }
        }
    }

    /// `intersects` reports whether the two overlap anywhere.
    pub(crate) fn intersects(&self, other: &Self) -> bool {
        !self.intersect(other).is_empty()
    }

    /// `expand` moves bounded content's edges outward by `distance`.
    pub(crate) fn expand(&self, distance: f32) -> Self {
        match self {
            Self::Bounded(rect) => Self::of(rect.expand(distance)),
            Self::Empty | Self::Unbounded => *self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounded(x: f32, y: f32, width: f32, height: f32) -> Bounds {
        Bounds::Bounded(Rect::new(x, y, width, height))
    }

    #[test]
    fn a_rectangle_without_area_bounds_nothing() {
        assert_eq!(Bounds::of(Rect::new(4.0, 4.0, 0.0, 10.0)), Bounds::Empty);
        assert_eq!(
            Bounds::of(Rect::new(4.0, 4.0, 1.0, 10.0)),
            bounded(4.0, 4.0, 1.0, 10.0)
        );
    }

    #[test]
    fn a_union_takes_the_larger_case() {
        let a = bounded(0.0, 0.0, 10.0, 10.0);
        let b = bounded(20.0, 0.0, 10.0, 10.0);
        assert_eq!(a.union(&b), bounded(0.0, 0.0, 30.0, 10.0));
        assert_eq!(a.union(&Bounds::Empty), a);
        assert_eq!(Bounds::Empty.union(&Bounds::Empty), Bounds::Empty);
        assert_eq!(a.union(&Bounds::Unbounded), Bounds::Unbounded);
    }

    /// A clip bounds what has no bounds of its own, and a clip that misses
    /// the content leaves nothing.
    #[test]
    fn an_intersection_takes_the_smaller_case() {
        let clip = bounded(0.0, 0.0, 10.0, 10.0);
        assert_eq!(Bounds::Unbounded.intersect(&clip), clip);
        assert_eq!(
            bounded(5.0, 5.0, 10.0, 10.0).intersect(&clip),
            bounded(5.0, 5.0, 5.0, 5.0)
        );
        assert_eq!(bounded(20.0, 0.0, 5.0, 5.0).intersect(&clip), Bounds::Empty);
        assert_eq!(Bounds::Unbounded.intersect(&Bounds::Empty), Bounds::Empty);
        assert_eq!(
            Bounds::Unbounded.intersect(&Bounds::Unbounded),
            Bounds::Unbounded
        );
    }

    /// Unbounded content stays unbounded wherever a transform takes it; a
    /// transform that flattens a rectangle leaves nothing.
    #[test]
    fn a_transform_moves_only_bounded_content() {
        let moved = Matrix::translation(1.0e9, 0.0);
        assert_eq!(Bounds::Unbounded.map(&moved), Bounds::Unbounded);
        assert_eq!(
            bounded(0.0, 0.0, 10.0, 10.0).map(&Matrix::translation(5.0, 0.0)),
            bounded(5.0, 0.0, 10.0, 10.0)
        );
        assert_eq!(
            bounded(0.0, 0.0, 10.0, 10.0).map(&Matrix::scale(0.0, 1.0)),
            Bounds::Empty
        );
    }
}
