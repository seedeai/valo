use std::sync::Arc;

use crate::{Matrix, Point, Rect};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

// Serialize ONLY (the serde feature is a debug dump): a deserializer would
// let malformed verb/point counts reach flatten() and index out of bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
enum Verb {
    Move,
    Line,
    Quad,
    Cubic,
    Close,
}

/// One flattened contour. `closed` is METADATA from the path's Close verb
/// (Impeller's `EndContour(origin, with_close)`) — never inferred from point
/// coincidence, so an open contour that happens to end at its start keeps
/// its caps. Closed contours end with the start point repeated: the closing
/// edge is part of the polyline (dashing and length walks see it); the
/// stroker drops the duplicate and joins at the seam instead of capping.
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    pub points: Vec<Point>,
    pub closed: bool,
}

/// An immutable path: verb + point arrays (SoA), built once via [`PathBuilder`],
/// shared by `Arc` inside display-list ops (cloning a recorded list never copies
/// point data). Flattening is the CALLER's move because tolerance depends on the
/// device scale at draw time — a path has no scale of its own.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Path {
    verbs: Vec<Verb>,
    points: Vec<Point>,
    /// Control-point bounds: conservative (curves stay inside their hull),
    /// which is exactly what the record-time oracle wants.
    bounds: Rect,
}

impl Path {
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    /// Heap footprint estimate (points dominate) — memory reports only.
    pub fn heap_bytes(&self) -> usize {
        self.points.len() * std::mem::size_of::<Point>() + self.verbs.len()
    }

    /// Flatten to polygonal contours at `tolerance` (max deviation, in the
    /// path's own units). Fills treat last→first as an implicit edge for
    /// every contour; strokes branch on [`Contour::closed`].
    pub fn flatten(&self, tolerance: f32) -> Vec<Contour> {
        let mut out = Flattener::new(tolerance.max(1e-4));
        let mut i = 0usize;
        for verb in &self.verbs {
            match verb {
                Verb::Move => {
                    out.move_to(self.points[i]);
                    i += 1;
                }
                Verb::Line => {
                    out.line_to(self.points[i]);
                    i += 1;
                }
                Verb::Quad => {
                    out.quad_to(self.points[i], self.points[i + 1]);
                    i += 2;
                }
                Verb::Cubic => {
                    out.cubic_to(self.points[i], self.points[i + 1], self.points[i + 2]);
                    i += 3;
                }
                Verb::Close => out.close(),
            }
        }
        out.finish()
    }
}

/// Records verbs/points and tracks bounds; `build` freezes into an `Arc<Path>`.
#[derive(Clone, Default)]
pub struct PathBuilder {
    verbs: Vec<Verb>,
    points: Vec<Point>,
    bounds: Option<Rect>,
    /// Where the current contour started (for `close`'s implicit edge).
    contour_open: bool,
}

impl PathBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn move_to(&mut self, p: impl Into<Point>) -> &mut Self {
        let p = p.into();
        self.verbs.push(Verb::Move);
        self.push_point(p);
        self.contour_open = true;
        self
    }

    pub fn line_to(&mut self, p: impl Into<Point>) -> &mut Self {
        let p = p.into();
        self.ensure_contour(p);
        self.verbs.push(Verb::Line);
        self.push_point(p);
        self
    }

    pub fn quad_to(&mut self, c: impl Into<Point>, p: impl Into<Point>) -> &mut Self {
        let (c, p) = (c.into(), p.into());
        self.ensure_contour(c);
        self.verbs.push(Verb::Quad);
        self.push_point(c);
        self.push_point(p);
        self
    }

    pub fn cubic_to(
        &mut self,
        c1: impl Into<Point>,
        c2: impl Into<Point>,
        p: impl Into<Point>,
    ) -> &mut Self {
        let (c1, c2, p) = (c1.into(), c2.into(), p.into());
        self.ensure_contour(c1);
        self.verbs.push(Verb::Cubic);
        self.push_point(c1);
        self.push_point(c2);
        self.push_point(p);
        self
    }

    pub fn close(&mut self) -> &mut Self {
        if self.contour_open {
            self.verbs.push(Verb::Close);
            self.contour_open = false;
        }
        self
    }

    // ── shape helpers (the common vocabulary) ──────────────────────────────

    pub fn rect(&mut self, r: Rect) -> &mut Self {
        self.move_to((r.x, r.y))
            .line_to((r.right(), r.y))
            .line_to((r.right(), r.bottom()))
            .line_to((r.x, r.bottom()))
            .close()
    }

    /// Rounded rect with one radius for all corners (clamped to half-extent).
    pub fn rrect(&mut self, r: Rect, radius: f32) -> &mut Self {
        self.rrect_radii(r, [radius; 4])
    }

    /// Per-corner CIRCULAR radii, clockwise from top-left: `[tl, tr, br,
    /// bl]` — the `rx == ry` case of [`Self::rrect_radii_elliptical`].
    pub fn rrect_radii(&mut self, r: Rect, radii: [f32; 4]) -> &mut Self {
        self.rrect_radii_elliptical(r, radii.map(|radius| [radius; 2]))
    }

    /// Per-corner ELLIPTICAL radii, clockwise from top-left: `[[rx, ry];
    /// 4]` for `[tl, tr, br, bl]` — the full CSS/Flutter rounded-rect
    /// (8 scalars). Radii are constrained together per axis (see
    /// [`constrain_radii_elliptical`]).
    pub fn rrect_radii_elliptical(&mut self, r: impl Into<Rect>, radii: [[f32; 2]; 4]) -> &mut Self {
        let r = r.into();
        let [tl, tr, br, bl] = constrain_radii_elliptical(&r, radii);
        if [tl, tr, br, bl].iter().all(|[x, y]| *x == 0.0 && *y == 0.0) {
            return self.rect(r);
        }
        // Cubic arc approximation of a quarter ELLIPSE per corner: the
        // quarter-circle control offsets, scaled per axis.
        let k = |rad: f32| rad * (1.0 - KAPPA);
        let (l, t, rr, b) = (r.x, r.y, r.right(), r.bottom());
        self.move_to((l + tl[0], t))
            .line_to((rr - tr[0], t))
            .cubic_to((rr - k(tr[0]), t), (rr, t + k(tr[1])), (rr, t + tr[1]))
            .line_to((rr, b - br[1]))
            .cubic_to((rr, b - k(br[1])), (rr - k(br[0]), b), (rr - br[0], b))
            .line_to((l + bl[0], b))
            .cubic_to((l + k(bl[0]), b), (l, b - k(bl[1])), (l, b - bl[1]))
            .line_to((l, t + tl[1]))
            .cubic_to((l, t + k(tl[1])), (l + k(tl[0]), t), (l + tl[0], t))
            .close()
    }

    pub fn circle(&mut self, center: impl Into<Point>, radius: f32) -> &mut Self {
        let c = center.into();
        let (r, k) = (radius, radius * KAPPA);
        self.move_to((c.x + r, c.y))
            .cubic_to((c.x + r, c.y + k), (c.x + k, c.y + r), (c.x, c.y + r))
            .cubic_to((c.x - k, c.y + r), (c.x - r, c.y + k), (c.x - r, c.y))
            .cubic_to((c.x - r, c.y - k), (c.x - k, c.y - r), (c.x, c.y - r))
            .cubic_to((c.x + k, c.y - r), (c.x + r, c.y - k), (c.x + r, c.y))
            .close()
    }

    pub fn build(self) -> Arc<Path> {
        Arc::new(Path {
            verbs: self.verbs,
            points: self.points,
            bounds: self.bounds.unwrap_or_default(),
        })
    }

    // ── internals ──────────────────────────────────────────────────────────

    /// A curve/line without a preceding move starts a contour at that point
    /// (Skia's implicit moveTo(0,0) is a footgun; starting at the target isn't).
    fn ensure_contour(&mut self, p: Point) {
        if !self.contour_open {
            self.move_to(p);
        }
    }

    fn push_point(&mut self, p: Point) {
        self.points.push(p);
        // Plain min/max accumulation — a zero-size seed rect is a valid
        // bound, wherever it sits (a rect-union "empty = identity" rule
        // would drop a first point at the origin).
        self.bounds = Some(match self.bounds {
            Some(b) => Rect::from_ltrb(
                b.x.min(p.x),
                b.y.min(p.y),
                b.right().max(p.x),
                b.bottom().max(p.y),
            ),
            None => Rect::new(p.x, p.y, 0.0, 0.0),
        });
    }
}

/// Circle-from-cubics constant (4/3·tan(π/8)).
const KAPPA: f32 = 0.552_284_8;

/// Skia's radii rule: shrink ALL four corners by ONE factor until every
/// adjacent pair fits its side — corners never overlap and the shape keeps
/// its proportions. Order is clockwise from top-left: `[tl, tr, br, bl]`.
pub fn constrain_radii(r: &Rect, radii: [f32; 4]) -> [f32; 4] {
    constrain_radii_elliptical(r, radii.map(|v| [v; 2])).map(|[x, _]| x)
}

/// The CSS/Skia overlap rule, per axis: each EDGE compares the two
/// adjacent radii's component ALONG it (top edge: tl.x + tr.x vs width;
/// right edge: tr.y + br.y vs height; …) and every radius scales by the
/// smallest fit so neighbouring arcs never cross.
pub fn constrain_radii_elliptical(r: &Rect, radii: [[f32; 2]; 4]) -> [[f32; 2]; 4] {
    let [tl, tr, br, bl] = radii.map(|[x, y]| [x.max(0.0), y.max(0.0)]);
    let fit = |side: f32, a: f32, b: f32| if a + b <= side { 1.0 } else { side / (a + b) };
    let f = fit(r.width, tl[0], tr[0])
        .min(fit(r.width, bl[0], br[0]))
        .min(fit(r.height, tl[1], bl[1]))
        .min(fit(r.height, tr[1], br[1]));
    [tl, tr, br, bl].map(|[x, y]| [x * f, y * f])
}

/// Curve → segments via Wang's formula (segment count from the second
/// difference of control points — deviation shrinks quadratically), then
/// uniform parameter steps. Deliberately approximate and cheap — the renderer's
/// contour cache is what keeps repeat draws from re-flattening.
struct Flattener {
    tolerance: f32,
    contours: Vec<Contour>,
    current: Vec<Point>,
}

impl Flattener {
    fn new(tolerance: f32) -> Self {
        Self {
            tolerance,
            contours: Vec::new(),
            current: Vec::new(),
        }
    }

    fn move_to(&mut self, p: Point) {
        self.flush(false);
        self.current.push(p);
    }

    fn line_to(&mut self, p: Point) {
        self.current.push(p);
    }

    fn quad_to(&mut self, c: Point, p: Point) {
        let Some(&start) = self.current.last() else {
            return;
        };
        let dev = second_difference(start, c, p);
        let n = segment_count((dev / (8.0 * self.tolerance)).sqrt());
        for i in 1..=n {
            let t = i as f32 / n as f32;
            self.current.push(eval_quad(start, c, p, t));
        }
    }

    fn cubic_to(&mut self, c1: Point, c2: Point, p: Point) {
        let Some(&start) = self.current.last() else {
            return;
        };
        let dev = second_difference(start, c1, c2).max(second_difference(c1, c2, p));
        let n = segment_count((3.0 * dev / (4.0 * self.tolerance)).sqrt());
        for i in 1..=n {
            let t = i as f32 / n as f32;
            self.current.push(eval_cubic(start, c1, c2, p, t));
        }
    }

    fn close(&mut self) {
        // Emit the closing edge back to the contour's start (unless the
        // last curve already landed there exactly).
        if let (Some(&first), Some(&last)) = (self.current.first(), self.current.last()) {
            if self.current.len() >= 2 && (first.x, first.y) != (last.x, last.y) {
                self.current.push(first);
            }
        }
        self.flush(true);
    }

    fn finish(mut self) -> Vec<Contour> {
        self.flush(false);
        self.contours
    }

    /// Keep EVERYTHING, even lone points — fills fan nothing from <3 points,
    /// but the stroker draws 2-point lines and caps lone points.
    fn flush(&mut self, closed: bool) {
        if !self.current.is_empty() {
            self.contours.push(Contour {
                points: std::mem::take(&mut self.current),
                closed,
            });
        }
    }
}

fn second_difference(a: Point, b: Point, c: Point) -> f32 {
    let dx = a.x - 2.0 * b.x + c.x;
    let dy = a.y - 2.0 * b.y + c.y;
    (dx * dx + dy * dy).sqrt()
}

fn segment_count(estimate: f32) -> u32 {
    (estimate.ceil() as u32).clamp(1, 64)
}

fn eval_quad(p0: Point, c: Point, p1: Point, t: f32) -> Point {
    let u = 1.0 - t;
    Point::new(
        u * u * p0.x + 2.0 * u * t * c.x + t * t * p1.x,
        u * u * p0.y + 2.0 * u * t * c.y + t * t * p1.y,
    )
}

fn eval_cubic(p0: Point, c1: Point, c2: Point, p1: Point, t: f32) -> Point {
    let u = 1.0 - t;
    let (uu, tt) = (u * u, t * t);
    Point::new(
        u * uu * p0.x + 3.0 * uu * t * c1.x + 3.0 * u * tt * c2.x + t * tt * p1.x,
        u * uu * p0.y + 3.0 * uu * t * c1.y + 3.0 * u * tt * c2.y + t * tt * p1.y,
    )
}

/// Device-space flattening tolerance for a draw under `transform`: keep curve
/// deviation under a quarter pixel wherever the content lands on screen.
pub fn local_tolerance(transform: &Matrix) -> f32 {
    0.25 / transform.max_scale().max(1e-3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radii_constrain_together() {
        let r = Rect::new(0.0, 0.0, 100.0, 40.0);
        // tl+bl = 80 > height 40 → everything scales by 0.5.
        let out = constrain_radii(&r, [40.0, 10.0, 10.0, 40.0]);
        assert_eq!(out, [20.0, 5.0, 5.0, 20.0]);
        // Already fitting radii pass through untouched.
        assert_eq!(constrain_radii(&r, [8.0, 8.0, 8.0, 8.0]), [8.0; 4]);
    }

    #[test]
    fn per_corner_rrect_stays_in_rect() {
        let r = Rect::new(10.0, 10.0, 100.0, 60.0);
        let mut b = PathBuilder::new();
        b.rrect_radii(r, [30.0, 0.0, 16.0, 8.0]);
        assert_eq!(b.build().bounds(), r);
    }

    #[test]
    fn bounds_cover_control_points() {
        let mut b = PathBuilder::new();
        b.move_to((10.0, 10.0)).quad_to((50.0, -20.0), (90.0, 10.0));
        let p = b.build();
        assert_eq!(p.bounds(), Rect::from_ltrb(10.0, -20.0, 90.0, 10.0));
    }

    #[test]
    fn circle_flattens_to_radius() {
        let mut b = PathBuilder::new();
        b.circle((0.0, 0.0), 100.0);
        let contours = b.build().flatten(0.1);
        assert_eq!(contours.len(), 1);
        assert!(contours[0].closed, "circle closes its contour");
        for p in &contours[0].points {
            let r = (p.x * p.x + p.y * p.y).sqrt();
            assert!((r - 100.0).abs() < 0.5, "point off circle: r={r}");
        }
    }

    #[test]
    fn finer_tolerance_means_more_segments() {
        let path = {
            let mut b = PathBuilder::new();
            b.circle((0.0, 0.0), 100.0);
            b.build()
        };
        let coarse = path.flatten(2.0)[0].points.len();
        let fine = path.flatten(0.05)[0].points.len();
        assert!(fine > coarse, "fine {fine} vs coarse {coarse}");
    }

    #[test]
    fn small_contours_survive_for_the_stroker() {
        let mut b = PathBuilder::new();
        b.move_to((0.0, 0.0)).line_to((10.0, 0.0)); // a stroked line segment
        b.move_to((50.0, 50.0)); // a lone point (caps render it)
        let contours = b.build().flatten(0.1);
        assert_eq!(contours.len(), 2);
        assert_eq!(contours[0].points.len(), 2);
        assert!(!contours[0].closed);
        assert_eq!(contours[1].points.len(), 1);
    }

    #[test]
    fn close_emits_the_closing_edge_and_marks_the_contour() {
        let mut b = PathBuilder::new();
        b.move_to((0.0, 0.0))
            .line_to((10.0, 0.0))
            .line_to((10.0, 10.0))
            .close();
        let contours = b.build().flatten(0.1);
        assert!(contours[0].closed);
        assert_eq!(contours[0].points.len(), 4, "closing edge in the polyline");
        assert_eq!(contours[0].points[3], Point::new(0.0, 0.0));
    }

    #[test]
    fn bounds_keep_a_first_point_at_the_origin() {
        let mut b = PathBuilder::new();
        b.move_to((0.0, 0.0)).line_to((50.0, 80.0));
        assert_eq!(b.build().bounds(), Rect::from_ltrb(0.0, 0.0, 50.0, 80.0));

        let mut b = PathBuilder::new();
        b.move_to((0.0, 0.0)).line_to((100.0, 0.0)); // zero-height line
        assert_eq!(b.build().bounds(), Rect::from_ltrb(0.0, 0.0, 100.0, 0.0));
    }

    #[test]
    fn curve_without_move_starts_contour() {
        let mut b = PathBuilder::new();
        b.line_to((10.0, 0.0))
            .line_to((10.0, 10.0))
            .line_to((0.0, 10.0));
        let contours = b.build().flatten(0.1);
        assert_eq!(contours.len(), 1);
        assert_eq!(contours[0].points.len(), 4);
    }

    /// The circular constructor must be EXACTLY the rx == ry case of the
    /// elliptical one — same constraint order, same cubics — so every
    /// existing rrect golden also pins the elliptical code path.
    #[test]
    fn circular_rrect_is_the_equal_axes_elliptical_case() {
        let r = Rect::new(10.0, 20.0, 120.0, 80.0);
        let radii = [24.0, 8.0, 30.0, 0.0];
        let mut circular = PathBuilder::new();
        circular.rrect_radii(r, radii);
        let mut elliptical = PathBuilder::new();
        elliptical.rrect_radii_elliptical(r, radii.map(|v| [v; 2]));
        assert_eq!(
            circular.build().flatten(0.1)[0].points,
            elliptical.build().flatten(0.1)[0].points,
        );
    }

    #[test]
    fn elliptical_radii_constrain_per_axis() {
        // A 100×40 rect with tall corner ellipses: the HEIGHT edges force
        // the scale (20 + 30 > 40 → f = 0.8); x components ride along.
        let r = Rect::new(0.0, 0.0, 100.0, 40.0);
        let out = constrain_radii_elliptical(
            &r,
            [[10.0, 20.0], [10.0, 20.0], [10.0, 30.0], [10.0, 30.0]],
        );
        assert_eq!(out[0], [8.0, 16.0]);
        assert_eq!(out[2], [8.0, 24.0]);
        // Negative radii clamp to zero before constraining.
        let out = constrain_radii_elliptical(&r, [[-5.0, 10.0], [0.0; 2], [0.0; 2], [0.0; 2]]);
        assert_eq!(out[0], [0.0, 10.0]);
    }

    #[test]
    fn elliptical_corner_lands_on_axis_extremes() {
        // One elliptical corner (rx 40, ry 10): the arc must start 40 in
        // from the corner on x and end 10 down on y.
        let r = Rect::new(0.0, 0.0, 200.0, 100.0);
        let mut b = PathBuilder::new();
        b.rrect_radii_elliptical(r, [[0.0; 2], [40.0, 10.0], [0.0; 2], [0.0; 2]]);
        let points = &b.build().flatten(0.05)[0].points;
        // The top edge stops at x = 160 (200 - rx) and the right edge
        // starts at y = 10 (ry) — both points must be on the outline.
        assert!(points
            .iter()
            .any(|p| (p.x - 160.0).abs() < 0.5 && p.y.abs() < 0.5));
        assert!(points
            .iter()
            .any(|p| (p.x - 200.0).abs() < 0.5 && (p.y - 10.0).abs() < 0.5));
    }
}
