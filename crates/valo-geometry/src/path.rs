use std::sync::Arc;

use crate::{Matrix, Point, Rect};

/// `FillRule` determines which regions of overlapping contours are filled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FillRule {
    /// `NonZero` fills regions whose signed winding count is nonzero.
    #[default]
    NonZero,
    /// `EvenOdd` fills regions crossed an odd number of times.
    EvenOdd,
}

/// `Winding` selects the traversal direction of a closed contour.
///
/// Direction matters wherever traversal carries meaning: under the nonzero fill
/// rule two overlapping contours cancel when their windings oppose and add
/// when they agree, and dashing walks a contour in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Winding {
    /// `Clockwise` traverses in the clockwise direction on Valo's y-down plane.
    #[default]
    Clockwise,
    /// `CounterClockwise` traverses in the counterclockwise direction.
    CounterClockwise,
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

/// `Contour` is one path contour flattened into a polyline.
///
/// Closed contours repeat their first point at the end so measurement and
/// dashing include the closing edge. Closure remains explicit metadata rather
/// than being inferred from coincident endpoints.
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    /// `points` contains the flattened polyline in traversal order.
    pub points: Vec<Point>,
    /// `closed` indicates whether the source contour ended with `close`.
    pub closed: bool,
    /// `has_segments` distinguishes drawn zero-length contours from a lone move.
    ///
    /// A close or explicit zero-length segment counts; a bare `move_to` does not.
    pub has_segments: bool,
}

/// `Path` is an immutable collection of line and Bézier contours.
///
/// Build paths with [`PathBuilder`]. Display lists retain shared [`Arc`] handles,
/// so recording and nesting do not copy path data.
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
    /// `bounds` returns conservative control-point bounds.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// `tight_bounds` returns exact axis-aligned curve bounds.
    pub fn tight_bounds(&self) -> Rect {
        let mut bounds = TightBounds::default();
        let mut point_index = 0usize;
        let mut cursor = Point::ZERO;
        let mut contour_start = Point::ZERO;
        for verb in &self.verbs {
            match verb {
                Verb::Move => {
                    cursor = self.points[point_index];
                    contour_start = cursor;
                    point_index += 1;
                    bounds.include(cursor);
                }
                Verb::Line => {
                    cursor = self.points[point_index];
                    point_index += 1;
                    bounds.include(cursor);
                }
                Verb::Quad => {
                    let control = self.points[point_index];
                    let end = self.points[point_index + 1];
                    point_index += 2;
                    include_quadratic_extrema(&mut bounds, cursor, control, end);
                    cursor = end;
                }
                Verb::Cubic => {
                    let first = self.points[point_index];
                    let second = self.points[point_index + 1];
                    let end = self.points[point_index + 2];
                    point_index += 3;
                    include_cubic_extrema(&mut bounds, cursor, first, second, end);
                    cursor = end;
                }
                Verb::Close => {
                    cursor = contour_start;
                    bounds.include(cursor);
                }
            }
        }
        bounds.rect()
    }

    /// `is_empty` reports whether the path contains no commands.
    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    /// `heap_bytes` returns an estimate of owned path storage.
    pub fn heap_bytes(&self) -> usize {
        self.points.len() * std::mem::size_of::<Point>() + self.verbs.len()
    }

    /// `contains` reports whether a point lies inside the filled path.
    ///
    /// The query evaluates the original curves, implicitly closes open contours,
    /// and treats points on the outline as inside.
    pub fn contains(&self, point: Point, fill_rule: FillRule) -> bool {
        if !self.bounds.contains_inclusive(point) {
            return false;
        }
        let crossings = self.walk_crossings(point);
        match fill_rule {
            FillRule::NonZero => crossings.is_inside_non_zero(),
            FillRule::EvenOdd => crossings.is_inside_even_odd(),
        }
    }

    /// Ray-cast the whole path, one segment at a time.
    fn walk_crossings(&self, point: Point) -> crate::winding::Crossings {
        let mut crossings = crate::winding::Crossings::default();
        let mut index = 0usize;
        let mut cursor = Point::ZERO;
        let mut contour_start = Point::ZERO;
        let mut contour_open = false;
        for verb in &self.verbs {
            match verb {
                Verb::Move => {
                    // A new contour closes the previous one: fills always see
                    // that last→first edge, whether or not Close was recorded.
                    if contour_open {
                        crossings.line(cursor, contour_start, point);
                    }
                    contour_open = true;
                    contour_start = self.points[index];
                    cursor = contour_start;
                    index += 1;
                }
                Verb::Line => {
                    crossings.line(cursor, self.points[index], point);
                    cursor = self.points[index];
                    index += 1;
                }
                Verb::Quad => {
                    crossings.quad(cursor, self.points[index], self.points[index + 1], point);
                    cursor = self.points[index + 1];
                    index += 2;
                }
                Verb::Cubic => {
                    crossings.cubic(
                        cursor,
                        self.points[index],
                        self.points[index + 1],
                        self.points[index + 2],
                        point,
                    );
                    cursor = self.points[index + 2];
                    index += 3;
                }
                Verb::Close => {
                    crossings.line(cursor, contour_start, point);
                    cursor = contour_start;
                    contour_open = false;
                }
            }
        }
        if contour_open {
            crossings.line(cursor, contour_start, point);
        }
        crossings
    }

    /// `measure` returns an arc-length measurement for each nonempty contour.
    ///
    /// `tolerance` is the maximum flattening deviation in path coordinates.
    pub fn measure(&self, tolerance: f32) -> Vec<crate::ContourMeasure> {
        self.flatten(tolerance)
            .iter()
            .filter_map(crate::ContourMeasure::of)
            .collect()
    }

    /// `flatten` approximates curves with polygonal contours.
    ///
    /// `tolerance` is the maximum deviation in path coordinates. Fill
    /// operations implicitly close every contour; stroke operations use
    /// [`Contour::closed`].
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

#[derive(Default)]
struct TightBounds(Option<(f32, f32, f32, f32)>);

impl TightBounds {
    fn include(&mut self, point: Point) {
        self.0 = Some(match self.0 {
            Some((left, top, right, bottom)) => (
                left.min(point.x),
                top.min(point.y),
                right.max(point.x),
                bottom.max(point.y),
            ),
            None => (point.x, point.y, point.x, point.y),
        });
    }

    fn rect(self) -> Rect {
        self.0
            .map_or_else(Rect::default, |(left, top, right, bottom)| {
                Rect::from_ltrb(left, top, right, bottom)
            })
    }
}

fn include_quadratic_extrema(bounds: &mut TightBounds, start: Point, control: Point, end: Point) {
    bounds.include(start);
    bounds.include(end);
    for (start_axis, control_axis, end_axis) in
        [(start.x, control.x, end.x), (start.y, control.y, end.y)]
    {
        let denominator = start_axis as f64 - 2.0 * control_axis as f64 + end_axis as f64;
        if denominator == 0.0 {
            continue;
        }
        let parameter = ((start_axis as f64 - control_axis as f64) / denominator) as f32;
        if parameter > 0.0 && parameter < 1.0 {
            bounds.include(eval_quad(start, control, end, parameter));
        }
    }
}

fn include_cubic_extrema(
    bounds: &mut TightBounds,
    start: Point,
    first: Point,
    second: Point,
    end: Point,
) {
    bounds.include(start);
    bounds.include(end);
    for (start_axis, first_axis, second_axis, end_axis) in [
        (start.x, first.x, second.x, end.x),
        (start.y, first.y, second.y, end.y),
    ] {
        for parameter in cubic_extrema(start_axis, first_axis, second_axis, end_axis)
            .into_iter()
            .flatten()
        {
            if parameter > 0.0 && parameter < 1.0 {
                bounds.include(eval_cubic(start, first, second, end, parameter));
            }
        }
    }
}

fn cubic_extrema(start: f32, first: f32, second: f32, end: f32) -> [Option<f32>; 2] {
    let start = start as f64;
    let first = first as f64;
    let second = second as f64;
    let end = end as f64;
    let quadratic = -start + 3.0 * first - 3.0 * second + end;
    let linear = 2.0 * (start - 2.0 * first + second);
    let constant = first - start;
    if quadratic == 0.0 {
        return [unit_root(-constant, linear), None];
    }
    let discriminant = linear * linear - 4.0 * quadratic * constant;
    if discriminant < 0.0 || !discriminant.is_finite() {
        return [None, None];
    }

    // Numerical Recipes / Skia: Q/A and C/Q avoid the cancellation in the
    // ordinary (-B ± sqrt(D)) / 2A formula when one root is much smaller.
    let root = discriminant.sqrt();
    let q = -0.5 * (linear + root.copysign(linear));
    let first_root = unit_root(q, quadratic);
    let second_root = unit_root(constant, q).filter(|value| Some(*value) != first_root);
    [first_root, second_root]
}

fn unit_root(numerator: f64, denominator: f64) -> Option<f32> {
    if denominator == 0.0 {
        return None;
    }
    let value = numerator / denominator;
    (value.is_finite() && value > 0.0 && value < 1.0).then_some(value as f32)
}

/// `PathBuilder` records commands used to create an immutable [`Path`].
#[derive(Clone, Default)]
pub struct PathBuilder {
    verbs: Vec<Verb>,
    points: Vec<Point>,
    bounds: Option<Rect>,
    /// Where a segment recorded after a `close` resumes.
    ///
    /// It OUTLIVES the close — that is the whole point. Without it
    /// `M10,10 L30,10 Z L30,30` loses its diagonal, because the line would
    /// start at its own destination. Skia does the same in `ensureMove`
    /// (`moveTo(fPts[fLastMoveIndex])` when the last verb was a close), and
    /// Impeller inherits it by building on `SkPathBuilder`.
    ///
    /// NOT always the contour's origin, which is why it is not called that.
    /// For `close` it is. For `rect` and `roundRect` WHATWG names the point
    /// separately — "create a new subpath with the point (x, y)" — and for a
    /// rounded rectangle `(x, y)` is a bounding-box corner the outline never
    /// touches, since the walk begins at the top-left tangent. The two
    /// coincide only at radius zero.
    resume_point: Option<Point>,
    contour_open: bool,
}

impl PathBuilder {
    /// `new` creates an empty path builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// `move_to` starts a new contour at `p`.
    pub fn move_to(&mut self, p: impl Into<Point>) -> &mut Self {
        let p = p.into();
        self.verbs.push(Verb::Move);
        self.push_point(p);
        self.resume_point = Some(p);
        self.contour_open = true;
        self
    }

    /// `line_to` adds a straight segment to `p`.
    pub fn line_to(&mut self, p: impl Into<Point>) -> &mut Self {
        let p = p.into();
        self.ensure_contour(p);
        self.verbs.push(Verb::Line);
        self.push_point(p);
        self
    }

    /// `quad_to` adds a quadratic Bézier through control point `c` to `p`.
    pub fn quad_to(&mut self, c: impl Into<Point>, p: impl Into<Point>) -> &mut Self {
        let (c, p) = (c.into(), p.into());
        self.ensure_contour(c);
        self.verbs.push(Verb::Quad);
        self.push_point(c);
        self.push_point(p);
        self
    }

    /// `cubic_to` adds a cubic Bézier through two control points to `p`.
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

    /// `close` adds a segment back to the current contour's starting point.
    ///
    /// It has no effect when no contour is open.
    pub fn close(&mut self) -> &mut Self {
        if self.contour_open {
            self.verbs.push(Verb::Close);
            self.contour_open = false;
        }
        self
    }

    // ── shape helpers (the common vocabulary) ──────────────────────────────

    /// `rect` adds a closed rectangular contour.
    pub fn rect(&mut self, r: Rect) -> &mut Self {
        self.move_to((r.x, r.y))
            .line_to((r.right(), r.y))
            .line_to((r.right(), r.bottom()))
            .line_to((r.x, r.bottom()))
            .close();
        // WHATWG's separate closing step: "create a new subpath with the
        // point (x, y)". Stated here rather than inherited from the traversal
        // above, so reordering the walk cannot move it.
        self.resume_point = Some(Point::new(r.x, r.y));
        self
    }

    /// `rrect` adds a closed rounded rectangle with one corner radius.
    pub fn rrect(&mut self, r: Rect, radius: f32) -> &mut Self {
        self.rrect_radii(r, [radius; 4])
    }

    /// `rrect_radii` adds a rounded rectangle with circular corner radii.
    ///
    /// `radii` is ordered clockwise from the top-left.
    pub fn rrect_radii(&mut self, r: Rect, radii: [f32; 4]) -> &mut Self {
        self.rrect_radii_elliptical(r, radii.map(|radius| [radius; 2]))
    }

    /// `rrect_radii_elliptical` adds per-corner elliptical radii.
    ///
    /// Each corner is `[x_radius, y_radius]`, starting at the top-left. Radii
    /// are proportionally reduced when adjacent corners would overlap.
    pub fn rrect_radii_elliptical(
        &mut self,
        r: impl Into<Rect>,
        radii: [[f32; 2]; 4],
    ) -> &mut Self {
        self.rrect_radii_elliptical_wound(r, radii, Winding::Clockwise)
    }

    /// `rrect_radii_elliptical_wound` adds a rounded rectangle with explicit winding.
    ///
    /// Opposing contours cancel under [`FillRule::NonZero`], and dashing follows
    /// this traversal order.
    pub fn rrect_radii_elliptical_wound(
        &mut self,
        r: impl Into<Rect>,
        radii: [[f32; 2]; 4],
        winding: Winding,
    ) -> &mut Self {
        let r = r.into();
        let [tl, tr, br, bl] = constrain_radii_elliptical(&r, radii);
        let (l, t, rr, b) = (r.x, r.y, r.right(), r.bottom());
        if [tl, tr, br, bl].iter().all(|[x, y]| *x == 0.0 && *y == 0.0) {
            match winding {
                Winding::Clockwise => self.rect(r),
                Winding::CounterClockwise => self
                    .move_to((l, t))
                    .line_to((l, b))
                    .line_to((rr, b))
                    .line_to((rr, t))
                    .close(),
            };
            self.resume_point = Some(Point::new(l, t));
            return self;
        }
        // Cubic arc approximation of a quarter ELLIPSE per corner: the
        // quarter-circle control offsets, scaled per axis.
        let k = |rad: f32| rad * (1.0 - KAPPA);
        match winding {
            Winding::Clockwise => self
                .move_to((l + tl[0], t))
                .line_to((rr - tr[0], t))
                .cubic_to((rr - k(tr[0]), t), (rr, t + k(tr[1])), (rr, t + tr[1]))
                .line_to((rr, b - br[1]))
                .cubic_to((rr, b - k(br[1])), (rr - k(br[0]), b), (rr - br[0], b))
                .line_to((l + bl[0], b))
                .cubic_to((l + k(bl[0]), b), (l, b - k(bl[1])), (l, b - bl[1]))
                .line_to((l, t + tl[1]))
                .cubic_to((l, t + k(tl[1])), (l + k(tl[0]), t), (l + tl[0], t))
                .close(),
            // The same anchors in reverse, each corner's two control points
            // swapped with it — so the two directions are the identical
            // outline and differ only in traversal.
            Winding::CounterClockwise => self
                .move_to((l + tl[0], t))
                .cubic_to((l + k(tl[0]), t), (l, t + k(tl[1])), (l, t + tl[1]))
                .line_to((l, b - bl[1]))
                .cubic_to((l, b - k(bl[1])), (l + k(bl[0]), b), (l + bl[0], b))
                .line_to((rr - br[0], b))
                .cubic_to((rr - k(br[0]), b), (rr, b - k(br[1])), (rr, b - br[1]))
                .line_to((rr, t + tr[1]))
                .cubic_to((rr, t + k(tr[1])), (rr - k(tr[0]), t), (rr - tr[0], t))
                .line_to((l + tl[0], t))
                .close(),
        };
        // WHATWG step 14, SEPARATE from the outline that step 12 walks:
        // "create a new subpath with the point (x, y)". For a rounded
        // rectangle that corner is not on the outline at all — the walk
        // begins at the top-left tangent — so this cannot be inherited from
        // the traversal the way `close`'s resumption point is. Blink does the
        // same explicitly, chaining `.MoveTo(x, y)` after its rounded-rect
        // builder (`canvas_path.cc`).
        //
        // Verified against the spec text and current Blink source rather than
        // by probing a browser. This corner of Canvas2D has already produced
        // two places where the prose and every implementation disagree, so
        // that distinction is worth keeping in view.
        self.resume_point = Some(Point::new(l, t));
        self
    }

    /// `arc` adds a circular arc.
    ///
    /// Angles are radians clockwise from +x in Valo's y-down coordinates.
    /// Sweeps are limited to one full turn.
    pub fn arc(
        &mut self,
        center: impl Into<Point>,
        radius: f32,
        start_angle: f32,
        sweep_angle: f32,
    ) -> &mut Self {
        self.ellipse(center, [radius; 2], 0.0, start_angle, sweep_angle)
    }

    /// `ellipse` adds an elliptical arc.
    ///
    /// `radii` are the x and y half-extents, `x_axis_rotation` turns the
    /// ellipse, and angles are radians clockwise from +x. An active contour is
    /// connected to the arc's first point. Sweeps are limited to one full turn.
    ///
    /// Non-finite input is ignored. Negative radii trigger a debug assertion
    /// and are ignored in release builds.
    pub fn ellipse(
        &mut self,
        center: impl Into<Point>,
        radii: [f32; 2],
        x_axis_rotation: f32,
        start_angle: f32,
        sweep_angle: f32,
    ) -> &mut Self {
        let center = center.into();
        let [radius_x, radius_y] = radii;
        // Every input, not just the radii: a NaN centre flows into NaN points,
        // and `f32::min`/`max` drop those from the bounds accumulator without
        // complaint — an under-reported box silently breaks culling and
        // hit-testing later.
        let finite = center.x.is_finite()
            && center.y.is_finite()
            && radius_x.is_finite()
            && radius_y.is_finite()
            && x_axis_rotation.is_finite()
            && start_angle.is_finite()
            && sweep_angle.is_finite();
        debug_assert!(
            radius_x >= 0.0 && radius_y >= 0.0,
            "negative radii draw nothing; Canvas2D throws here"
        );
        if !finite || radius_x < 0.0 || radius_y < 0.0 {
            return self;
        }

        // Canvas2D stops at one full turn, and Skia routes full sweeps to an
        // oval. Without this, `sweep = 1e20` passes the finite check above and
        // asks for ~1e19 cubic pieces — an allocation the process does not
        // survive, reachable by any embedder forwarding user input.
        let full_turn = std::f32::consts::TAU;
        let sweep_angle = sweep_angle.clamp(-full_turn, full_turn);

        let unit_circle_to_ellipse = unit_circle_map(center, radii, x_axis_rotation);
        let first = unit_circle_to_ellipse.map_point(unit_circle_point(start_angle));
        // Canvas2D runs a straight line in to the arc's start when a contour
        // is live. A CLOSED contour still counts as live for this: it resumes
        // at its origin and then runs the line, so an arc after `closePath`
        // stays connected to the seam. Only a path with no contour at all
        // starts at the arc.
        if self.contour_open || self.resume_point.is_some() {
            self.ensure_contour(first);
            self.line_to(first);
        } else {
            self.move_to(first);
        }
        if sweep_angle != 0.0 {
            self.push_arc_cubics(&unit_circle_to_ellipse, start_angle, sweep_angle);
        }
        // A whole turn ends where it began: close it, so the stroker joins the
        // seam instead of capping it (Skia's full sweeps produce a closed oval).
        if sweep_angle.abs() >= full_turn {
            self.close();
        }
        self
    }

    /// `arc_to` rounds the corner between the current point, `corner`, and `next`.
    ///
    /// Zero or negative radius, coincident points, and straight-through corners
    /// fall back to a line ending at `corner`.
    pub fn arc_to(
        &mut self,
        corner: impl Into<Point>,
        next: impl Into<Point>,
        radius: f32,
    ) -> &mut Self {
        let (corner, next) = (corner.into(), next.into());
        self.ensure_contour(corner);
        let start = *self.points.last().expect("ensure_contour opened a contour");

        // Skia's construction, in f64: the tangent length follows from the
        // half-angle at the corner, and the centre sits one radius along the
        // inward normal of the incoming edge.
        let incoming = normalize(
            corner.x as f64 - start.x as f64,
            corner.y as f64 - start.y as f64,
        );
        let outgoing = normalize(
            next.x as f64 - corner.x as f64,
            next.y as f64 - corner.y as f64,
        );
        let (Some(incoming), Some(outgoing)) = (incoming, outgoing) else {
            return self.line_to(corner);
        };
        let cosine = incoming.0 * outgoing.0 + incoming.1 * outgoing.1;
        let sine = incoming.0 * outgoing.1 - incoming.1 * outgoing.0;
        if radius <= 0.0 || !radius.is_finite() || sine.abs() < 1.0 / (1 << 12) as f64 {
            return self.line_to(corner);
        }

        let tangent_length = (radius as f64 * (1.0 - cosine) / sine).abs();
        let entry = Point::new(
            corner.x - (tangent_length * incoming.0) as f32,
            corner.y - (tangent_length * incoming.1) as f32,
        );
        // The turn's sign puts the centre on the side the arc bends toward.
        let turn = sine.signum() as f32;
        let center = Point::new(
            entry.x + radius * turn * -(incoming.1 as f32),
            entry.y + radius * turn * incoming.0 as f32,
        );
        let exit = Point::new(
            corner.x + (tangent_length * outgoing.0) as f32,
            corner.y + (tangent_length * outgoing.1) as f32,
        );

        let start_angle = (entry.y - center.y).atan2(entry.x - center.x);
        let end_angle = (exit.y - center.y).atan2(exit.x - center.x);
        let sweep = shortest_sweep(start_angle, end_angle, turn);

        self.line_to(entry);
        let map = unit_circle_map(center, [radius; 2], 0.0);
        self.push_arc_cubics(&map, start_angle, sweep);
        self
    }

    /// `circle` adds a closed circular contour.
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

    /// `append` adds a transformed copy of another path.
    ///
    /// The appended path's final contour becomes the current contour for
    /// subsequent commands.
    pub fn append(&mut self, path: &Path, transform: &Matrix) -> &mut Self {
        if path.verbs.is_empty() {
            // Appending nothing must change nothing — in particular it must
            // not close this builder's open contour.
            return self;
        }
        let mut point = path.points.iter();
        let mut cursor = Point::ZERO;
        let mut contour_start = Point::ZERO;
        for verb in &path.verbs {
            let count = match verb {
                Verb::Move | Verb::Line => 1,
                Verb::Quad => 2,
                Verb::Cubic => 3,
                Verb::Close => 0,
            };
            self.verbs.push(*verb);
            for _ in 0..count {
                let Some(&p) = point.next() else {
                    return self;
                };
                cursor = transform.map_point(p);
                self.push_point(cursor);
            }
            match verb {
                Verb::Move => contour_start = cursor,
                Verb::Close => cursor = contour_start,
                _ => {}
            }
        }
        // WHATWG's `Path2D.addPath` ends by "creating a new subpath with the
        // last point in path", which is what lets a following `line_to`
        // continue from where the source stopped. A source ending mid-contour
        // already leaves this builder there; one ending in `close` does not,
        // and without the reopen the next segment would start at its own
        // endpoint and the connecting edge would vanish.
        //
        // The reopen leaves a lone-point contour when nothing follows. That
        // costs no pixels here: it sits exactly on the closed contour's seam,
        // which the fill and the stroke's join already cover.
        if matches!(path.verbs.last(), Some(Verb::Close)) {
            self.move_to(cursor);
        } else {
            // The appended contour is this builder's contour now, origin and
            // all — leaving the receiver's own origin in place would send a
            // later `close` + segment back to the wrong seam.
            self.resume_point = Some(contour_start);
            self.contour_open = true;
        }
        self
    }

    /// `build` consumes the builder and returns a shared immutable path.
    pub fn build(self) -> Arc<Path> {
        Arc::new(Path {
            verbs: self.verbs,
            points: self.points,
            bounds: self.bounds.unwrap_or_default(),
        })
    }

    // ── internals ──────────────────────────────────────────────────────────

    /// `push_arc_cubics` approximates an arc with cubic pieces of at most 90°.
    ///
    /// The current point must already be at the arc's start.
    fn push_arc_cubics(&mut self, map: &Matrix, start_angle: f32, sweep_angle: f32) {
        let piece_count = (sweep_angle.abs() / std::f32::consts::FRAC_PI_2)
            .ceil()
            .max(1.0);
        let step = sweep_angle / piece_count;
        // Control-point offset for a Bézier matching an arc of `step`: at a
        // quarter turn this is exactly KAPPA.
        let reach = 4.0 / 3.0 * (step / 4.0).tan();

        let mut angle = start_angle;
        for _ in 0..piece_count as u32 {
            let (from, to) = (unit_circle_point(angle), unit_circle_point(angle + step));
            let first = Point::new(from.x - reach * from.y, from.y + reach * from.x);
            let second = Point::new(to.x + reach * to.y, to.y - reach * to.x);
            self.cubic_to(
                map.map_point(first),
                map.map_point(second),
                map.map_point(to),
            );
            angle += step;
        }
    }

    /// `ensure_contour` guarantees an open contour before recording a segment.
    ///
    /// After a `close` the path resumes at the CLOSED contour's origin — the
    /// spec's "new subpath with the last point", and Skia's `ensureMove`.
    /// Resuming at the incoming point instead silently deletes the segment
    /// from the seam, which is the whole bug this exists to prevent.
    ///
    /// A path that never had a contour starts at the incoming point: Skia's
    /// implicit `moveTo(0, 0)` there is a footgun valo does not copy.
    fn ensure_contour(&mut self, p: Point) {
        if self.contour_open {
            return;
        }
        self.move_to(self.resume_point.unwrap_or(p));
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

/// `KAPPA` is the cubic control ratio `4/3 × tan(π/8)` for a quarter circle.
const KAPPA: f32 = 0.552_284_8;

/// `unit_circle_point` returns the point at `angle` on the unit circle.
fn unit_circle_point(angle: f32) -> Point {
    let (sine, cosine) = angle.sin_cos();
    Point::new(cosine, sine)
}

/// `unit_circle_map` creates the transform from a unit circle to an ellipse.
fn unit_circle_map(center: Point, radii: [f32; 2], rotation: f32) -> Matrix {
    let [radius_x, radius_y] = radii;
    let (sine, cosine) = rotation.sin_cos();
    Matrix::from_affine(
        radius_x * cosine,
        radius_x * sine,
        -radius_y * sine,
        radius_y * cosine,
        center.x,
        center.y,
    )
}

/// `shortest_sweep` returns the sub-turn sweep matching `direction`.
fn shortest_sweep(start: f32, end: f32, direction: f32) -> f32 {
    let mut sweep = end - start;
    let turn = std::f32::consts::TAU;
    while sweep > 0.0 && direction < 0.0 {
        sweep -= turn;
    }
    while sweep < 0.0 && direction > 0.0 {
        sweep += turn;
    }
    sweep
}

/// `normalize` returns a unit vector or `None` when no finite direction exists.
fn normalize(x: f64, y: f64) -> Option<(f64, f64)> {
    let length = (x * x + y * y).sqrt();
    (length.is_finite() && length > 0.0).then(|| (x / length, y / length))
}

/// `constrain_radii` proportionally reduces circular radii to fit a rectangle.
///
/// Radii are ordered clockwise from the top-left. Negative values become zero.
pub fn constrain_radii(r: &Rect, radii: [f32; 4]) -> [f32; 4] {
    constrain_radii_elliptical(r, radii.map(|v| [v; 2])).map(|[x, _]| x)
}

/// `constrain_radii_elliptical` proportionally reduces elliptical radii to fit.
///
/// Corners are ordered clockwise from the top-left as `[x_radius, y_radius]`.
/// Negative components become zero.
pub fn constrain_radii_elliptical(r: &Rect, radii: [[f32; 2]; 4]) -> [[f32; 2]; 4] {
    let [tl, tr, br, bl] = radii.map(|[x, y]| [x.max(0.0), y.max(0.0)]);
    let fit = |side: f32, a: f32, b: f32| if a + b <= side { 1.0 } else { side / (a + b) };
    let f = fit(r.width, tl[0], tr[0])
        .min(fit(r.width, bl[0], br[0]))
        .min(fit(r.height, tl[1], bl[1]))
        .min(fit(r.height, tr[1], br[1]));
    [tl, tr, br, bl].map(|[x, y]| [x * f, y * f])
}

/// `Flattener` approximates curves with uniformly parameterized line segments.
struct Flattener {
    tolerance: f32,
    contours: Vec<Contour>,
    current: Vec<Point>,
    /// Whether a segment verb has landed since the last `move_to`.
    has_segments: bool,
}

impl Flattener {
    fn new(tolerance: f32) -> Self {
        Self {
            tolerance,
            contours: Vec::new(),
            current: Vec::new(),
            has_segments: false,
        }
    }

    fn move_to(&mut self, p: Point) {
        self.flush(false);
        self.current.push(p);
        self.has_segments = false;
    }

    fn line_to(&mut self, p: Point) {
        self.current.push(p);
        self.has_segments = true;
    }

    fn quad_to(&mut self, c: Point, p: Point) {
        let Some(&start) = self.current.last() else {
            return;
        };
        self.has_segments = true;
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
        self.has_segments = true;
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
        // Closing is itself a drawing command: `move_to(p)` then `close()` is
        // an explicit zero-length SUBPATH, which strokes exactly like an
        // explicit zero-length segment. Impeller says so directly — its
        // `Close()` calls `SegmentEncountered()` — and Skia turns move+close
        // into a zero-length line for every non-butt cap.
        if !self.current.is_empty() {
            self.has_segments = true;
        }
        self.flush(true);
    }

    fn finish(mut self) -> Vec<Contour> {
        self.flush(false);
        self.contours
    }

    /// `flush` keeps every contour, even lone points.
    ///
    /// Fills fan nothing from <3 points,
    /// but the stroker draws 2-point lines and caps EXPLICIT zero-length
    /// subpaths. A move-only contour is kept too, carrying `has_segments:
    /// false` so the stroker can tell the two apart.
    fn flush(&mut self, closed: bool) {
        if !self.current.is_empty() {
            self.contours.push(Contour {
                points: std::mem::take(&mut self.current),
                closed,
                has_segments: self.has_segments,
            });
        }
        self.has_segments = false;
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

/// `local_tolerance` returns local curve tolerance for quarter-pixel device error.
pub fn local_tolerance(transform: &Matrix) -> f32 {
    0.25 / transform.max_scale().max(1e-3)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The observable meaning of winding: under the NON-ZERO rule two
    /// overlapping contours cancel when their directions oppose and reinforce
    /// when they agree. This is the property Chrome exhibits for a
    /// `roundRect` given with a negative width, and the reason normalizing
    /// the box without carrying the direction is wrong — the second rectangle
    /// would add instead of subtract.
    #[test]
    fn opposed_windings_cancel_under_the_non_zero_rule() {
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let radii = [[12.0, 12.0]; 4];
        let inside = Point::new(50.0, 50.0);

        let mut opposed = PathBuilder::new();
        opposed.rrect_radii_elliptical_wound(rect, radii, Winding::Clockwise);
        opposed.rrect_radii_elliptical_wound(rect, radii, Winding::CounterClockwise);
        assert!(
            !opposed.build().contains(inside, FillRule::NonZero),
            "opposed windings must cancel"
        );

        let mut agreeing = PathBuilder::new();
        agreeing.rrect_radii_elliptical_wound(rect, radii, Winding::Clockwise);
        agreeing.rrect_radii_elliptical_wound(rect, radii, Winding::Clockwise);
        assert!(
            agreeing.build().contains(inside, FillRule::NonZero),
            "agreeing windings must reinforce"
        );
    }

    /// Direction must not move the OUTLINE, only the traversal. A reversed
    /// corner whose control points were not swapped with it would bulge the
    /// wrong way and show up here.
    #[test]
    fn winding_reverses_the_walk_without_moving_the_outline() {
        let rect = Rect::new(10.0, 20.0, 80.0, 60.0);
        let radii = [[8.0, 14.0], [4.0, 4.0], [20.0, 6.0], [0.0, 0.0]];
        let wound = |winding| {
            let mut path = PathBuilder::new();
            path.rrect_radii_elliptical_wound(rect, radii, winding);
            path.build()
        };
        let clockwise = wound(Winding::Clockwise);
        let counter = wound(Winding::CounterClockwise);
        assert_eq!(clockwise.tight_bounds(), counter.tight_bounds());
        // Sample across the shape, including just inside and outside each
        // rounded corner.
        for point in [
            Point::new(50.0, 50.0),
            Point::new(14.0, 30.0),
            Point::new(86.0, 24.0),
            Point::new(74.0, 76.0),
            Point::new(12.0, 78.0),
            Point::new(5.0, 15.0),
            Point::new(95.0, 85.0),
        ] {
            assert_eq!(
                clockwise.contains(point, FillRule::NonZero),
                counter.contains(point, FillRule::NonZero),
                "the two directions disagree about {point:?}"
            );
        }
    }

    /// WHATWG leaves a one-point subpath at the closed contour's origin, so a
    /// segment recorded after `closePath` starts from the SEAM.
    ///
    /// The bug this pins is invisible to any test that paints immediately
    /// after the close — the closed shape looks right and the missing
    /// diagonal is a segment that was never recorded at all. That is exactly
    /// why the conformance fuzzer never caught it.
    #[test]
    fn a_segment_after_close_resumes_at_the_contour_origin() {
        let mut path = PathBuilder::new();
        path.move_to((10.0, 10.0));
        path.line_to((30.0, 10.0));
        path.close();
        path.line_to((30.0, 30.0));
        let path = path.build();

        // The diagonal runs (10,10) → (30,30); its midpoint is (20,20).
        let contours = path.flatten(0.05);
        let resumed = contours.last().expect("the path continues after the close");
        assert_eq!(
            resumed.points.first().copied(),
            Some(Point::new(10.0, 10.0)),
            "the segment after close must start at the contour origin, not its own end"
        );
        assert!(crate::stroke_contains(
            &contours,
            &crate::Stroke::new(6.0),
            0.05,
            Point::new(20.0, 20.0)
        ));
    }

    /// `rect` and `roundRect` resume at `(x, y)` — the bounding box's corner,
    /// which is a SEPARATE spec step from the outline they walk.
    ///
    /// For a rounded rectangle that corner is not on the outline at all: the
    /// walk starts at the top-left tangent, `(18, 10)` here. The two points
    /// coincide only at radius zero, which is exactly why a rect-only test
    /// would miss this.
    #[test]
    fn a_segment_after_a_shape_helper_resumes_at_the_box_corner() {
        let box_corner = Point::new(10.0, 10.0);
        for corner in [0.0f32, 8.0] {
            let mut path = PathBuilder::new();
            if corner == 0.0 {
                path.rect(Rect::new(10.0, 10.0, 40.0, 40.0));
            } else {
                path.rrect_radii_elliptical(Rect::new(10.0, 10.0, 40.0, 40.0), [[corner; 2]; 4]);
            }
            path.line_to((90.0, 90.0));

            let contours = path.build().flatten(0.05);
            let resumed = contours.last().expect("the path continues after the shape");
            assert_eq!(
                resumed.points.first().copied(),
                Some(box_corner),
                "corner radius {corner}: the trailing segment starts at (x, y)"
            );
        }

        // The same thing said in ink, which is how the divergence was found:
        // the diagonal from (10,10) is stroked and the one from the tangent
        // (18,10) is not.
        let mut path = PathBuilder::new();
        path.rrect_radii_elliptical(Rect::new(10.0, 10.0, 40.0, 40.0), [[8.0; 2]; 4]);
        path.line_to((90.0, 90.0));
        let contours = path.build().flatten(0.05);
        let stroke = crate::Stroke::new(4.0);
        assert!(
            crate::stroke_contains(&contours, &stroke, 0.05, Point::new(50.0, 50.0)),
            "the diagonal from (10,10) must be stroked"
        );
        assert!(
            !crate::stroke_contains(&contours, &stroke, 0.05, Point::new(54.0, 50.0)),
            "the diagonal from the tangent (18,10) must not be"
        );
    }

    /// `closePath` keeps the contour-origin rule — the shape helpers' `(x, y)`
    /// override must not have leaked into it.
    #[test]
    fn close_still_resumes_at_the_contour_origin() {
        let mut path = PathBuilder::new();
        path.move_to((10.0, 10.0));
        path.line_to((30.0, 10.0));
        path.line_to((30.0, 30.0));
        path.close();
        path.line_to((90.0, 90.0));
        let contours = path.build().flatten(0.05);
        assert_eq!(
            contours
                .last()
                .and_then(|contour| contour.points.first())
                .copied(),
            Some(Point::new(10.0, 10.0)),
            "close resumes where the contour began, not at any box corner"
        );
    }

    /// A path that never opened a contour still starts where it is told —
    /// Skia's implicit `moveTo(0, 0)` is deliberately not copied.
    #[test]
    fn a_first_segment_with_no_contour_starts_at_its_own_point() {
        let mut path = PathBuilder::new();
        path.line_to((30.0, 30.0));
        assert_eq!(
            path.build().bounds(),
            Rect::from_ltrb(30.0, 30.0, 30.0, 30.0)
        );
    }

    #[test]
    fn append_carries_verbs_through_the_transform() {
        let mut source = PathBuilder::new();
        source.rect(Rect::new(0.0, 0.0, 10.0, 10.0));
        let source = source.build();

        let mut target = PathBuilder::new();
        target.rect(Rect::new(0.0, 0.0, 4.0, 4.0));
        target.append(&source, &Matrix::translation(100.0, 50.0));
        let target = target.build();

        assert_eq!(target.bounds(), Rect::from_ltrb(0.0, 0.0, 110.0, 60.0));
        assert!(target.contains(Point::new(105.0, 55.0), FillRule::NonZero));
        assert!(!target.contains(Point::new(5.0, 5.0), FillRule::NonZero));
    }

    /// The reopen after a closed source is what keeps the next segment
    /// connected. Without it the `line_to` below starts a fresh contour at
    /// its own endpoint and the edge from the seam disappears.
    #[test]
    fn appending_a_closed_contour_reopens_at_its_seam() {
        let mut source = PathBuilder::new();
        source.move_to((10.0, 10.0));
        source.line_to((20.0, 10.0));
        source.close();
        let source = source.build();

        let mut target = PathBuilder::new();
        target.append(&source, &Matrix::IDENTITY);
        target.line_to((10.0, 40.0));
        let built = target.build();

        // The seam is (10, 10); the new edge runs from there to (10, 40).
        assert_eq!(built.bounds(), Rect::from_ltrb(10.0, 10.0, 20.0, 40.0));
        assert!(built.contains(Point::new(10.0, 25.0), FillRule::NonZero));
    }

    /// An appended OPEN contour becomes the receiver's contour, origin
    /// included. Keeping the receiver's own origin would send a later
    /// `close` + segment back to the wrong seam.
    #[test]
    fn appending_an_open_contour_hands_over_its_origin() {
        let mut source = PathBuilder::new();
        source.move_to((50.0, 50.0));
        source.line_to((60.0, 50.0));
        let source = source.build();

        let mut target = PathBuilder::new();
        target.move_to((0.0, 0.0));
        target.line_to((10.0, 0.0));
        target.append(&source, &Matrix::IDENTITY);
        target.close();
        target.line_to((90.0, 90.0));

        let contours = target.build().flatten(0.05);
        let resumed = contours.last().expect("the path continues after the close");
        assert_eq!(
            resumed.points.first().copied(),
            Some(Point::new(50.0, 50.0)),
            "the resumed segment must start at the APPENDED contour's origin"
        );
    }

    #[test]
    fn appending_nothing_leaves_an_open_contour_open() {
        let empty = PathBuilder::new().build();
        let mut target = PathBuilder::new();
        target.move_to((0.0, 0.0));
        target.line_to((10.0, 0.0));
        target.append(&empty, &Matrix::IDENTITY);
        target.line_to((10.0, 10.0));
        assert_eq!(
            target.build().bounds(),
            Rect::from_ltrb(0.0, 0.0, 10.0, 10.0)
        );
    }

    #[test]
    fn appending_an_open_contour_leaves_it_open() {
        let mut source = PathBuilder::new();
        source.move_to((0.0, 0.0));
        source.line_to((10.0, 0.0));
        let source = source.build();

        let mut target = PathBuilder::new();
        target.append(&source, &Matrix::IDENTITY);
        // Without the contour-open handoff this would restart at the origin
        // and the bounds would be unchanged by the new point.
        target.line_to((10.0, 10.0));
        assert_eq!(
            target.build().bounds(),
            Rect::from_ltrb(0.0, 0.0, 10.0, 10.0)
        );
    }

    #[test]
    fn tight_bounds_use_curve_extrema_not_control_points() {
        let mut path = PathBuilder::new();
        path.move_to((0.0, 0.0));
        path.quad_to((100.0, 100.0), (200.0, 0.0));
        let path = path.build();
        assert_eq!(path.bounds(), Rect::new(0.0, 0.0, 200.0, 100.0));
        assert_eq!(path.tight_bounds(), Rect::new(0.0, 0.0, 200.0, 50.0));
    }

    #[test]
    fn tight_bounds_keep_extrema_below_f32_epsilon() {
        let mut path = PathBuilder::new();
        path.move_to((0.0, 0.0));
        path.quad_to((0.0, 1.0e-8), (0.0, 0.0));
        let bounds = path.build().tight_bounds();
        assert!((bounds.height - 5.0e-9).abs() < 1.0e-12);
    }

    #[test]
    fn cubic_extrema_preserve_the_small_root() {
        let roots = cubic_extrema(0.0, 1.0e-8, -0.5, -0.5);
        assert!(roots
            .into_iter()
            .flatten()
            .any(|root| (root - 1.0e-8).abs() < 1.0e-10));
    }

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

    // ── arcs ────────────────────────────────────────────────────────────────

    /// Every point of a swept circle sits on the circle, to well under a
    /// tenth of a pixel — the cubic approximation's whole claim.
    #[test]
    fn swept_arc_stays_on_its_circle() {
        let (center, radius) = (Point::new(50.0, 60.0), 40.0);
        let mut b = PathBuilder::new();
        b.arc(center, radius, 0.0, std::f32::consts::TAU);
        for point in &b.build().flatten(0.01)[0].points {
            let offset = (point.x - center.x).hypot(point.y - center.y);
            assert!(
                (offset - radius).abs() < 0.05,
                "point {point:?} is {offset} from the centre, not {radius}"
            );
        }
    }

    /// A quarter turn ends exactly where trigonometry says it does.
    #[test]
    fn quarter_arc_ends_where_it_should() {
        let mut b = PathBuilder::new();
        b.arc((0.0, 0.0), 100.0, 0.0, std::f32::consts::FRAC_PI_2);
        let points = &b.build().flatten(0.01)[0].points;
        let (first, last) = (points[0], *points.last().unwrap());
        assert!(
            (first.x - 100.0).abs() < 0.01 && first.y.abs() < 0.01,
            "{first:?}"
        );
        assert!(
            last.x.abs() < 0.05 && (last.y - 100.0).abs() < 0.05,
            "{last:?}"
        );
    }

    /// An ellipse reaches its own half-extents on each axis.
    #[test]
    fn ellipse_reaches_both_radii() {
        let mut b = PathBuilder::new();
        b.ellipse((0.0, 0.0), [80.0, 20.0], 0.0, 0.0, std::f32::consts::TAU);
        let points = &b.build().flatten(0.01)[0].points;
        let widest = points.iter().fold(0.0f32, |m, p| m.max(p.x.abs()));
        let tallest = points.iter().fold(0.0f32, |m, p| m.max(p.y.abs()));
        assert!((widest - 80.0).abs() < 0.1, "widest {widest}");
        assert!((tallest - 20.0).abs() < 0.1, "tallest {tallest}");
    }

    /// The rotation turns the ellipse: a 90° turn swaps which axis is long.
    #[test]
    fn ellipse_rotation_swaps_the_axes() {
        let mut b = PathBuilder::new();
        b.ellipse(
            (0.0, 0.0),
            [80.0, 20.0],
            std::f32::consts::FRAC_PI_2,
            0.0,
            std::f32::consts::TAU,
        );
        let points = &b.build().flatten(0.01)[0].points;
        let widest = points.iter().fold(0.0f32, |m, p| m.max(p.x.abs()));
        let tallest = points.iter().fold(0.0f32, |m, p| m.max(p.y.abs()));
        assert!((widest - 20.0).abs() < 0.1, "widest {widest}");
        assert!((tallest - 80.0).abs() < 0.1, "tallest {tallest}");
    }

    /// A right-angle `arc_to` with radius r touches down r before the corner
    /// and leaves r after it, and every point between is r from the centre
    /// the two tangents share.
    #[test]
    fn arc_to_rounds_a_right_angle() {
        let radius = 20.0f32;
        let mut b = PathBuilder::new();
        b.move_to((0.0, 0.0))
            .arc_to((100.0, 0.0), (100.0, 100.0), radius);
        let points = &b.build().flatten(0.01)[0].points;

        let entry = Point::new(100.0 - radius, 0.0);
        let exit = Point::new(100.0, radius);
        assert!(points
            .iter()
            .any(|p| (p.x - entry.x).abs() < 0.1 && (p.y - entry.y).abs() < 0.1));
        assert!(points
            .iter()
            .any(|p| (p.x - exit.x).abs() < 0.1 && (p.y - exit.y).abs() < 0.1));

        let center = Point::new(100.0 - radius, radius);
        for point in points.iter().filter(|p| p.x > entry.x - 0.01) {
            let offset = (point.x - center.x).hypot(point.y - center.y);
            assert!(
                (offset - radius).abs() < 0.1,
                "{point:?} is {offset} from the centre"
            );
        }
    }

    /// Collinear points and a zero radius both degenerate to a plain line,
    /// which is what the Canvas2D algorithm prescribes.
    #[test]
    fn degenerate_arc_to_falls_back_to_a_line() {
        for (corner, next, radius) in [
            ((50.0, 0.0), (100.0, 0.0), 20.0), // straight through
            ((50.0, 0.0), (50.0, 50.0), 0.0),  // no radius
        ] {
            let mut b = PathBuilder::new();
            b.move_to((0.0, 0.0)).arc_to(corner, next, radius);
            let points = &b.build().flatten(0.01)[0].points;
            assert_eq!(points.len(), 2, "expected a bare line, got {points:?}");
            assert!((points[1].x - corner.0).abs() < 0.01 && (points[1].y - corner.1).abs() < 0.01);
        }
    }

    // ── containment ─────────────────────────────────────────────────────────

    #[test]
    fn rect_contains_what_it_covers() {
        let mut b = PathBuilder::new();
        b.rect(Rect::new(10.0, 10.0, 80.0, 60.0));
        let path = b.build();
        assert!(path.contains(Point::new(50.0, 40.0), FillRule::NonZero));
        assert!(!path.contains(Point::new(5.0, 40.0), FillRule::NonZero));
        assert!(!path.contains(Point::new(50.0, 80.0), FillRule::NonZero));
        // Exactly on the outline counts as inside — on EVERY edge. The far
        // two are the ones a half-open bounds check silently loses.
        for on_outline in [
            Point::new(10.0, 40.0), // left
            Point::new(50.0, 10.0), // top
            Point::new(90.0, 40.0), // right
            Point::new(50.0, 70.0), // bottom
            Point::new(90.0, 70.0), // the far corner
        ] {
            assert!(
                path.contains(on_outline, FillRule::NonZero),
                "{on_outline:?} is on the outline and must count as inside"
            );
        }
    }

    /// Canvas2D caps an arc at one turn. Without the clamp a huge sweep asks
    /// for billions of cubic pieces, which is an allocation the process does
    /// not survive — so this test is a crash guard, not a geometry check.
    #[test]
    fn an_enormous_sweep_stays_one_turn() {
        let mut b = PathBuilder::new();
        b.arc((0.0, 0.0), 50.0, 0.0, 1e20);
        let path = b.build();
        let contours = path.flatten(0.1);
        assert_eq!(contours.len(), 1);
        // One turn at this tolerance is a few hundred points, never millions.
        assert!(
            contours[0].points.len() < 1_000,
            "a clamped turn should stay small, got {}",
            contours[0].points.len()
        );
        assert!(contours[0].closed, "a full turn closes its contour");
    }

    #[test]
    fn a_negative_sweep_turns_the_other_way() {
        let quarter = std::f32::consts::FRAC_PI_2;
        let mut clockwise = PathBuilder::new();
        clockwise.arc((0.0, 0.0), 50.0, 0.0, quarter);
        let mut anticlockwise = PathBuilder::new();
        anticlockwise.arc((0.0, 0.0), 50.0, 0.0, -quarter);

        // y-down: a positive sweep from +x heads towards +y, a negative one
        // towards -y. Both start at the same point.
        let forward = clockwise.build().bounds();
        let backward = anticlockwise.build().bounds();
        assert!(forward.bottom() > 40.0, "positive sweep reaches +y");
        assert!(backward.y < -40.0, "negative sweep reaches -y");
    }

    /// Containment runs on the CURVE, so it agrees with the true circle at
    /// every angle — a flattened test would drift inside the chords.
    #[test]
    fn circle_containment_is_exact_all_the_way_round() {
        let (center, radius) = (Point::new(0.0, 0.0), 100.0f32);
        let mut b = PathBuilder::new();
        b.circle(center, radius);
        let path = b.build();
        for step in 0..64 {
            let angle = step as f32 / 64.0 * std::f32::consts::TAU;
            let (sine, cosine) = angle.sin_cos();
            let inside = Point::new(cosine * radius * 0.99, sine * radius * 0.99);
            let outside = Point::new(cosine * radius * 1.01, sine * radius * 1.01);
            assert!(
                path.contains(inside, FillRule::NonZero),
                "{inside:?} should be in"
            );
            assert!(
                !path.contains(outside, FillRule::NonZero),
                "{outside:?} should be out"
            );
        }
    }

    /// The two fill rules disagree exactly where they should: a hole wound
    /// the same way as its parent is solid under non-zero, empty under
    /// even-odd.
    #[test]
    fn fill_rules_disagree_about_a_same_wound_hole() {
        let mut b = PathBuilder::new();
        b.rect(Rect::new(0.0, 0.0, 100.0, 100.0));
        b.rect(Rect::new(25.0, 25.0, 50.0, 50.0));
        let path = b.build();
        let middle = Point::new(50.0, 50.0);
        assert!(path.contains(middle, FillRule::NonZero));
        assert!(!path.contains(middle, FillRule::EvenOdd));
        // Between the rings both rules agree it is filled.
        let ring = Point::new(10.0, 50.0);
        assert!(path.contains(ring, FillRule::NonZero));
        assert!(path.contains(ring, FillRule::EvenOdd));
    }

    /// An unclosed contour still fills, so it must still contain.
    #[test]
    fn open_contour_closes_implicitly() {
        let mut b = PathBuilder::new();
        b.move_to((0.0, 0.0))
            .line_to((100.0, 0.0))
            .line_to((100.0, 100.0));
        let path = b.build();
        assert!(path.contains(Point::new(80.0, 40.0), FillRule::NonZero));
        assert!(!path.contains(Point::new(20.0, 60.0), FillRule::NonZero));
    }

    /// Curved segments contribute their real crossings, not a chord's.
    #[test]
    fn containment_handles_curves_that_double_back() {
        let mut b = PathBuilder::new();
        b.move_to((0.0, 0.0))
            .cubic_to((120.0, 120.0), (-20.0, 120.0), (100.0, 0.0))
            .close();
        let path = b.build();
        assert!(path.contains(Point::new(50.0, 40.0), FillRule::NonZero));
        assert!(!path.contains(Point::new(50.0, -10.0), FillRule::NonZero));
        assert!(!path.contains(Point::new(-30.0, 40.0), FillRule::NonZero));
    }
}
