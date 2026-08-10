//! Exact point-in-path winding — the math behind [`crate::Path::contains`].
//!
//! Ported from Skia's `SkPathPriv::Contains`: every segment contributes a
//! signed crossing of the horizontal ray at `y`, solved on the CURVE itself
//! rather than on a flattened approximation, so the answer never depends on a
//! tolerance. Curves are first split where they turn around in y, which leaves
//! pieces that are monotonic and therefore cross the ray at most once.

use crate::Point;

/// One walk's running totals. `winding` is the signed crossing count; a
/// non-zero value settles the query for either fill rule. `on_boundary`
/// counts segments the point sits exactly on, which is what the tie-break
/// consults when the winding cancels out.
#[derive(Default)]
pub(crate) struct Crossings {
    winding: i32,
    on_boundary: u32,
}

impl Crossings {
    /// Inside under the non-zero rule.
    pub(crate) fn is_inside_non_zero(&self) -> bool {
        self.winding != 0 || self.touches_boundary()
    }

    /// Inside under the even-odd rule.
    pub(crate) fn is_inside_even_odd(&self) -> bool {
        (self.winding & 1) != 0 || self.touches_boundary()
    }

    /// A point exactly ON the outline counts as inside. Skia disambiguates
    /// the remaining case — an even number of boundary touches under the
    /// non-zero rule — by comparing the tangents of the coincident edges;
    /// reaching it needs exact float coincidence, so valo treats every
    /// boundary hit as inside instead of carrying that machinery.
    fn touches_boundary(&self) -> bool {
        self.on_boundary > 0
    }

    pub(crate) fn line(&mut self, from: Point, to: Point, at: Point) {
        let mut low = from.y;
        let mut high = to.y;
        let mut direction = 1;
        if low > high {
            std::mem::swap(&mut low, &mut high);
            direction = -1;
        }
        if at.y < low || at.y > high {
            return;
        }
        if self.note_if_on_segment(at, from, to) {
            return;
        }
        // The upper endpoint belongs to the NEXT segment, so it never counts
        // twice — the half-open rule that keeps shared vertices honest.
        if at.y == high {
            return;
        }

        let side = (to.x - from.x) * (at.y - from.y) - (to.y - from.y) * (at.x - from.x);
        if side == 0.0 {
            if at.x != to.x || at.y != to.y {
                self.on_boundary += 1;
            }
        } else if side.signum() as i32 != direction {
            self.winding += direction;
        }
    }

    pub(crate) fn quad(&mut self, from: Point, control: Point, to: Point, at: Point) {
        let mut pieces = [Point::ZERO; 5];
        let split = chop_quad_at_y_extremum(&[from, control, to], &mut pieces);
        self.monotonic_quad(&pieces[0..3], at);
        if split {
            self.monotonic_quad(&pieces[2..5], at);
        }
    }

    pub(crate) fn cubic(&mut self, from: Point, first: Point, second: Point, to: Point, at: Point) {
        let mut pieces = [Point::ZERO; 10];
        let count = chop_cubic_at_y_extrema(&[from, first, second, to], &mut pieces);
        for index in 0..=count {
            self.monotonic_cubic(&pieces[index * 3..index * 3 + 4], at);
        }
    }

    /// A quad that only moves one way in y: at most one crossing, found by
    /// solving y(t) = at.y and evaluating x there.
    fn monotonic_quad(&mut self, points: &[Point], at: Point) {
        let (start, control, end) = (points[0], points[1], points[2]);
        let Some(direction) = self.enter_monotonic(start, end, at) else {
            return;
        };

        let roots = unit_quadratic_roots(
            start.y - 2.0 * control.y + end.y,
            2.0 * (control.y - start.y),
            start.y - at.y,
        );
        let x_at_crossing = match roots {
            // No root means y(t) is constant at the query height; the
            // crossing sits at whichever end the ray enters from.
            None => points[(1 - direction) as usize].x,
            Some(t) => {
                let c = start.x;
                let a = end.x - 2.0 * control.x + c;
                let b = 2.0 * (control.x - c);
                (a * t + b) * t + c
            }
        };
        self.settle(x_at_crossing, at, end, direction);
    }

    /// The cubic twin of [`Self::monotonic_quad`]; the crossing parameter
    /// comes from a bisection because a cubic root has no cheap closed form.
    fn monotonic_cubic(&mut self, points: &[Point], at: Point) {
        let (start, end) = (points[0], points[3]);
        let Some(direction) = self.enter_monotonic(start, end, at) else {
            return;
        };

        let (min_x, max_x) = points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
            (lo.min(p.x), hi.max(p.x))
        });
        if at.x < min_x {
            return;
        }
        if at.x > max_x {
            self.winding += direction;
            return;
        }

        let t = monotonic_cubic_parameter_at_y(points, at.y);
        let x_at_crossing = evaluate_cubic(points[0].x, points[1].x, points[2].x, points[3].x, t);
        self.settle(x_at_crossing, at, end, direction);
    }

    /// The guard every monotonic piece shares: reject the ray entirely, or
    /// report the direction its crossing would count in.
    fn enter_monotonic(&mut self, start: Point, end: Point, at: Point) -> Option<i32> {
        let (mut low, mut high, mut direction) = (start.y, end.y, 1);
        if low > high {
            std::mem::swap(&mut low, &mut high);
            direction = -1;
        }
        if at.y < low || at.y > high {
            return None;
        }
        if self.note_if_on_segment(at, start, end) {
            return None;
        }
        if at.y == high {
            return None;
        }
        Some(direction)
    }

    /// A crossing strictly left of the query point counts; one that lands on
    /// it means the point is on the outline.
    fn settle(&mut self, x_at_crossing: f32, at: Point, end: Point, direction: i32) {
        if nearly_equal(x_at_crossing, at.x) {
            if at.x != end.x || at.y != end.y {
                self.on_boundary += 1;
            }
            return;
        }
        if x_at_crossing < at.x {
            self.winding += direction;
        }
    }

    /// Skia's `checkOnCurve`: a horizontal segment swallows any point along
    /// it, and otherwise only the segment's own start point counts.
    fn note_if_on_segment(&mut self, at: Point, start: Point, end: Point) -> bool {
        let on_segment = if start.y == end.y {
            is_between(start.x, at.x, end.x) && at.x != end.x
        } else {
            at.x == start.x && at.y == start.y
        };
        if on_segment {
            self.on_boundary += 1;
        }
        on_segment
    }
}

/// Roots of `a·t² + b·t + c` inside [0, 1]. Monotonic pieces have at most one,
/// which is the only one this query wants.
fn unit_quadratic_roots(a: f32, b: f32, c: f32) -> Option<f32> {
    if a == 0.0 {
        if b == 0.0 {
            return None;
        }
        return in_unit_interval(-c / b);
    }
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return None;
    }
    let root = discriminant.sqrt();
    // The numerically stable pairing: never subtract two near-equal numbers.
    let q = if b < 0.0 {
        -(b - root) / 2.0
    } else {
        -(b + root) / 2.0
    };
    in_unit_interval(q / a).or_else(|| in_unit_interval(c / q))
}

fn in_unit_interval(t: f32) -> Option<f32> {
    (t.is_finite() && (0.0..=1.0).contains(&t)).then_some(t)
}

/// Split a quad where it turns around in y. Returns whether it split; the
/// output always holds the pieces back to back, sharing the middle point.
fn chop_quad_at_y_extremum(points: &[Point; 3], out: &mut [Point; 5]) -> bool {
    let (y0, y1, y2) = (points[0].y, points[1].y, points[2].y);
    let denominator = y0 - 2.0 * y1 + y2;
    let t = in_unit_interval(if denominator == 0.0 {
        f32::NAN
    } else {
        (y0 - y1) / denominator
    })
    .filter(|t| *t > 0.0 && *t < 1.0);

    match t {
        None => {
            out[0..3].copy_from_slice(points);
            false
        }
        Some(t) => {
            let ab = lerp(points[0], points[1], t);
            let bc = lerp(points[1], points[2], t);
            let mid = lerp(ab, bc, t);
            out[0] = points[0];
            out[1] = ab;
            out[2] = mid;
            out[3] = bc;
            out[4] = points[2];
            true
        }
    }
}

/// Split a cubic at each y turnaround (up to two). Returns the number of
/// splits; the output holds `count + 1` cubics sharing their end points.
fn chop_cubic_at_y_extrema(points: &[Point; 4], out: &mut [Point; 10]) -> usize {
    // y'(t) as a quadratic in Bernstein form, differentiated.
    let (y0, y1, y2, y3) = (points[0].y, points[1].y, points[2].y, points[3].y);
    let a = -y0 + 3.0 * (y1 - y2) + y3;
    let b = 2.0 * (y0 - 2.0 * y1 + y2);
    let c = y1 - y0;

    let mut extrema = [0.0f32; 2];
    let mut count = 0;
    for root in quadratic_roots_in_open_unit(a, b, c) {
        extrema[count] = root;
        count += 1;
    }

    out[0..4].copy_from_slice(points);
    if count == 0 {
        return 0;
    }
    chop_cubic_at(points, extrema[0], &mut out[0..7]);
    if count == 2 {
        // The second root moves into the tail piece's own parameter space.
        let remapped = ((extrema[1] - extrema[0]) / (1.0 - extrema[0])).clamp(0.0, 1.0);
        let tail = [out[3], out[4], out[5], out[6]];
        chop_cubic_at(&tail, remapped, &mut out[3..10]);
    }
    count
}

/// de Casteljau at `t`, writing both halves back to back.
fn chop_cubic_at(points: &[Point; 4], t: f32, out: &mut [Point]) {
    let ab = lerp(points[0], points[1], t);
    let bc = lerp(points[1], points[2], t);
    let cd = lerp(points[2], points[3], t);
    let abc = lerp(ab, bc, t);
    let bcd = lerp(bc, cd, t);
    let mid = lerp(abc, bcd, t);
    out[0] = points[0];
    out[1] = ab;
    out[2] = abc;
    out[3] = mid;
    out[4] = bcd;
    out[5] = cd;
    out[6] = points[3];
}

fn quadratic_roots_in_open_unit(a: f32, b: f32, c: f32) -> impl Iterator<Item = f32> {
    let mut roots = [0.0f32; 2];
    let mut count = 0;
    if a == 0.0 {
        if b != 0.0 {
            roots[0] = -c / b;
            count = 1;
        }
    } else {
        let discriminant = b * b - 4.0 * a * c;
        if discriminant >= 0.0 {
            let root = discriminant.sqrt();
            roots[0] = (-b - root) / (2.0 * a);
            roots[1] = (-b + root) / (2.0 * a);
            count = 2;
        }
    }
    let mut sorted = roots;
    if count == 2 && sorted[0] > sorted[1] {
        sorted.swap(0, 1);
    }
    sorted
        .into_iter()
        .take(count)
        .filter(|t| t.is_finite() && *t > 0.0 && *t < 1.0)
}

/// The parameter where a y-monotonic cubic reaches `y`, by bisection —
/// Skia's `SkCubicClipper::ChopMonoAtY`. 24 halvings put the answer well
/// inside float precision over [0, 1].
fn monotonic_cubic_parameter_at_y(points: &[Point], y: f32) -> f32 {
    let (mut low, mut high) = (0.0f32, 1.0f32);
    let ascending = points[3].y >= points[0].y;
    for _ in 0..24 {
        let mid = (low + high) * 0.5;
        let value = evaluate_cubic(points[0].y, points[1].y, points[2].y, points[3].y, mid);
        if (value < y) == ascending {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low + high) * 0.5
}

/// Bernstein cubic at `t`, in Horner form.
fn evaluate_cubic(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a = p3 - p0 + 3.0 * (p1 - p2);
    let b = 3.0 * (p0 - 2.0 * p1 + p2);
    let c = 3.0 * (p1 - p0);
    ((a * t + b) * t + c) * t + p0
}

fn lerp(a: Point, b: Point, t: f32) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn is_between(a: f32, value: f32, b: f32) -> bool {
    (value - a) * (value - b) <= 0.0
}

fn nearly_equal(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.0 / (1 << 12) as f32
}
