//! Arc-length measurement and sampling along flattened path contours.
//!
//! Accuracy follows the flattening tolerance supplied to [`crate::Path::measure`].

use crate::{Contour, Point};

/// `PathSample` contains a position and direction along a measured contour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathSample {
    /// `position` is the sampled point on the contour.
    pub position: Point,
    /// `tangent` is the unit direction toward increasing distance.
    pub tangent: Point,
}

/// `ContourMeasure` finds positions, tangents, and segments by distance along a contour.
///
/// Create measurements with [`crate::Path::measure`].
#[derive(Clone, Debug)]
pub struct ContourMeasure {
    points: Vec<Point>,
    /// Distance from the start to each point, so the last entry is the whole
    /// contour's length. Monotonic, which is what makes sampling a binary
    /// search rather than a walk.
    distances: Vec<f32>,
    closed: bool,
}

impl ContourMeasure {
    /// `of` measures a flattened contour with positive length.
    ///
    /// Zero-length segments are discarded.
    pub(crate) fn of(contour: &Contour) -> Option<Self> {
        let mut points = Vec::with_capacity(contour.points.len());
        let mut distances = Vec::with_capacity(contour.points.len());
        let mut total = 0.0;
        for &point in &contour.points {
            match points.last() {
                None => {
                    points.push(point);
                    distances.push(0.0);
                }
                Some(&previous) => {
                    let step = distance_between(previous, point);
                    if step > 0.0 {
                        total += step;
                        points.push(point);
                        distances.push(total);
                    }
                }
            }
        }
        (points.len() > 1).then_some(Self {
            points,
            distances,
            closed: contour.closed,
        })
    }

    /// `length` returns the contour's total arc length.
    pub fn length(&self) -> f32 {
        *self
            .distances
            .last()
            .expect("measured contours have length")
    }

    /// `is_closed` reports whether the source contour was explicitly closed.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// `sample` returns the position and tangent at a contour distance.
    ///
    /// Distances clamp to the contour's ends. `NaN` samples the start.
    pub fn sample(&self, distance: f32) -> PathSample {
        // NaN would reach the comparator and take the binary search down with
        // it; the start is the honest answer. Infinities need no special case
        // — they clamp to the ends like any over-long distance.
        let distance = if distance.is_nan() { 0.0 } else { distance };
        let distance = distance.clamp(0.0, self.length());
        let index = self.segment_containing(distance);
        let (start, end) = (self.points[index], self.points[index + 1]);
        let span = self.distances[index + 1] - self.distances[index];
        let fraction = (distance - self.distances[index]) / span;
        PathSample {
            position: lerp(start, end, fraction),
            tangent: unit_vector(start, end),
        }
    }

    /// `segment` extracts the open contour between two distances.
    ///
    /// Distances clamp to the measured contour. Empty, reversed, or `NaN`
    /// ranges return `None`.
    pub fn segment(&self, start: f32, end: f32) -> Option<Contour> {
        if start.is_nan() || end.is_nan() {
            return None;
        }
        let length = self.length();
        let (start, end) = (start.clamp(0.0, length), end.clamp(0.0, length));
        if end <= start {
            return None;
        }
        let mut points = vec![self.sample(start).position];
        let first = self.segment_containing(start);
        let last = self.segment_containing(end);
        for index in first + 1..=last {
            points.push(self.points[index]);
        }
        points.push(self.sample(end).position);
        Some(Contour {
            points: dedup_adjacent(points),
            closed: false,
            // A slice of a measured contour is real geometry by construction:
            // `end <= start` returned above, so there is length here.
            has_segments: true,
        })
    }

    /// `segment_containing` returns the line segment containing `distance`.
    fn segment_containing(&self, distance: f32) -> usize {
        match self
            .distances
            .binary_search_by(|entry| entry.partial_cmp(&distance).expect("finite distances"))
        {
            Ok(index) => index.min(self.points.len() - 2),
            Err(insert) => insert - 1,
        }
    }
}

fn distance_between(a: Point, b: Point) -> f32 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

fn lerp(a: Point, b: Point, t: f32) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn unit_vector(a: Point, b: Point) -> Point {
    let length = distance_between(a, b);
    Point::new((b.x - a.x) / length, (b.y - a.y) / length)
}

fn dedup_adjacent(points: Vec<Point>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for point in points {
        if out.last() != Some(&point) {
            out.push(point);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::{Path, PathBuilder, Point};

    fn measured(build: impl FnOnce(&mut PathBuilder)) -> Vec<crate::ContourMeasure> {
        let mut builder = PathBuilder::new();
        build(&mut builder);
        let path: std::sync::Arc<Path> = builder.build();
        path.measure(0.05)
    }

    #[test]
    fn a_straight_line_measures_its_own_length() {
        let measures = measured(|b| {
            b.move_to((10.0, 10.0)).line_to((10.0, 60.0));
        });
        assert_eq!(measures.len(), 1);
        assert!((measures[0].length() - 50.0).abs() < 1e-3);
        assert!(!measures[0].is_closed());
    }

    #[test]
    fn sampling_walks_the_line_and_clamps_past_its_ends() {
        let measures = measured(|b| {
            b.move_to((0.0, 0.0)).line_to((100.0, 0.0));
        });
        let middle = measures[0].sample(25.0);
        assert!((middle.position.x - 25.0).abs() < 1e-3);
        assert!((middle.tangent.x - 1.0).abs() < 1e-3);
        assert!(middle.tangent.y.abs() < 1e-3);

        // Past either end the sample sticks to the end point.
        assert!((measures[0].sample(-10.0).position.x - 0.0).abs() < 1e-3);
        assert!((measures[0].sample(500.0).position.x - 100.0).abs() < 1e-3);
    }

    #[test]
    fn a_closed_square_measures_its_whole_perimeter() {
        let measures = measured(|b| {
            b.rect(crate::Rect::new(0.0, 0.0, 30.0, 30.0));
        });
        assert_eq!(measures.len(), 1);
        assert!(measures[0].is_closed());
        // The closing edge counts: four sides, not three.
        assert!((measures[0].length() - 120.0).abs() < 1e-3);
    }

    #[test]
    fn a_circle_measures_near_two_pi_r() {
        let measures = measured(|b| {
            b.circle((0.0, 0.0), 50.0);
        });
        let circumference = std::f32::consts::TAU * 50.0;
        // Flattening cuts corners, so the polyline is a touch short.
        let error = (measures[0].length() - circumference).abs() / circumference;
        assert!(error < 0.001, "circumference off by {error}");
    }

    #[test]
    fn each_contour_is_measured_separately() {
        let measures = measured(|b| {
            b.move_to((0.0, 0.0)).line_to((10.0, 0.0));
            b.move_to((0.0, 20.0)).line_to((0.0, 60.0));
        });
        assert_eq!(measures.len(), 2);
        assert!((measures[0].length() - 10.0).abs() < 1e-3);
        assert!((measures[1].length() - 40.0).abs() < 1e-3);
    }

    #[test]
    fn a_segment_spans_exactly_the_requested_stretch() {
        let measures = measured(|b| {
            b.move_to((0.0, 0.0))
                .line_to((100.0, 0.0))
                .line_to((100.0, 100.0));
        });
        let segment = measures[0].segment(50.0, 150.0).expect("non-empty");
        assert_eq!(segment.points.first(), Some(&Point::new(50.0, 0.0)));
        assert_eq!(segment.points.last(), Some(&Point::new(100.0, 50.0)));
        // It keeps the corner it crosses.
        assert!(segment.points.contains(&Point::new(100.0, 0.0)));
        assert!(!segment.closed);
    }

    #[test]
    fn an_empty_or_reversed_range_measures_nothing() {
        let measures = measured(|b| {
            b.move_to((0.0, 0.0)).line_to((10.0, 0.0));
        });
        assert!(measures[0].segment(5.0, 5.0).is_none());
        assert!(measures[0].segment(8.0, 2.0).is_none());
    }

    #[test]
    fn non_finite_distances_answer_instead_of_panicking() {
        let measures = measured(|b| {
            b.move_to((0.0, 0.0)).line_to((10.0, 0.0));
        });
        assert_eq!(measures[0].sample(f32::NAN).position, Point::new(0.0, 0.0));
        assert!(measures[0].segment(f32::NAN, 5.0).is_none());
        assert!(measures[0].segment(0.0, f32::INFINITY).is_some());
    }

    #[test]
    fn degenerate_contours_are_dropped_rather_than_measured() {
        // A lone point has no length, so there is nothing to sample.
        let measures = measured(|b| {
            b.move_to((5.0, 5.0));
        });
        assert!(measures.is_empty());
    }
}
