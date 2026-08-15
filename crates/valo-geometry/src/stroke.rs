//! Stroke geometry: flattened polylines → one triangle strip (Impeller's
//! StrokePathGeometry shape — CPU strips, joins fanned around the pivot,
//! caps at open ends; drawn directly, no stencil). Translucent strokes
//! double-blend where join fans overlap the segment quads — the same
//! accepted artifact Impeller carries. Stencil-then-cover over the strip is
//! the escape hatch if that overlap ever has to go.

use crate::{Contour, Point};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Cap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Join {
    #[default]
    Miter,
    Round,
    Bevel,
}

/// On/off intervals cycled along each contour, `phase` px into the cycle.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Dash {
    pub intervals: Vec<f32>,
    pub phase: f32,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Stroke {
    pub width: f32,
    pub cap: Cap,
    pub join: Join,
    /// Miter length ÷ half-width beyond which a join bevels (SVG default 4).
    pub miter_limit: f32,
    pub dash: Option<Dash>,
}

impl Stroke {
    pub fn new(width: f32) -> Self {
        Self {
            width,
            cap: Cap::default(),
            join: Join::default(),
            miter_limit: 4.0,
            dash: None,
        }
    }
}

/// Strip vertices (x,y pairs) stroking `contours`; contours stitch with
/// degenerate triangles. `tolerance` sizes round join/cap arcs, like the
/// flattener sizes curves.
pub fn stroke_strip(contours: &[Contour], stroke: &Stroke, tolerance: f32) -> Vec<f32> {
    let half = stroke.width * 0.5;
    if half <= 0.0 {
        return Vec::new();
    }
    let mut strip = Strip::default();
    for contour in contours {
        let mut pts = dedup(&contour.points);
        // Closed polylines carry the duplicated start (the closing edge);
        // the wraparound below re-adds that edge, so drop the duplicate.
        if contour.closed && pts.len() >= 2 && distance(pts[0], *pts.last().unwrap()) < 1e-4 {
            pts.pop();
        }
        match pts.len() {
            0 => {}
            // One point after dedup means either a bare `move_to` or an
            // explicit segment that went nowhere. They look identical here,
            // which is exactly why the contour carries the answer: a
            // move-only subpath is never stroked at all (SVG 2 §13.4;
            // Skia's `fSegmentCount > 0` gate), while an explicit
            // zero-length one still gets its caps.
            1 if contour.has_segments => lone_point(&mut strip, pts[0], stroke, half, tolerance),
            1 => {}
            _ => stroke_contour(&mut strip, &pts, contour.closed, stroke, half, tolerance),
        }
    }
    strip.out
}

/// Whether `point` lands on the ink `stroke_strip` would produce — Canvas2D's
/// `isPointInStroke`.
///
/// This hit-tests the very triangles the renderer draws, so the answer can
/// never disagree with the pixels. The alternative, converting a stroke into
/// an outline PATH and filling it, is a genuinely hard problem (offset
/// curves, self-intersection removal) and buys nothing here.
pub fn stroke_contains(
    contours: &[Contour],
    stroke: &Stroke,
    tolerance: f32,
    point: Point,
) -> bool {
    let strip = stroke_strip(contours, stroke, tolerance);
    let vertex = |index: usize| Point::new(strip[index * 2], strip[index * 2 + 1]);
    let vertices = strip.len() / 2;
    (2..vertices).any(|i| in_triangle(point, vertex(i - 2), vertex(i - 1), vertex(i)))
}

/// Point-in-triangle for one strip triple.
///
/// Two rules, and both are load-bearing:
///
/// AREA FIRST. `stitch` joins sub-strips by repeating vertices, so a path with
/// a second contour — or any dashed path, which is all second contours —
/// produces triples with two or three coincident corners. Those cover no
/// pixels, but their cross products are zero, and a sign-only test reads a
/// zero as "not on the far side", so a degenerate triple would report EVERY
/// point inside. That is the difference between a hit test and a constant
/// `true`, so zero-area triples are rejected before the sign test rather than
/// by it.
///
/// SIGN-AGNOSTIC AFTER. A strip alternates winding by construction, so a rule
/// demanding one orientation would answer "outside" for half the real ink.
fn in_triangle(p: Point, a: Point, b: Point, c: Point) -> bool {
    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if area == 0.0 {
        return false;
    }
    let side = |from: Point, to: Point| {
        (to.x - from.x) * (p.y - from.y) - (to.y - from.y) * (p.x - from.x)
    };
    let (ab, bc, ca) = (side(a, b), side(b, c), side(c, a));
    let negative = ab < 0.0 || bc < 0.0 || ca < 0.0;
    let positive = ab > 0.0 || bc > 0.0 || ca > 0.0;
    !(negative && positive)
}

/// Split contours into the dash pattern's ON stretches (each stroked with
/// its own caps, always open — a closed contour starts dashing at its seam).
/// Invalid patterns disable dashing, like Skia's SkDashPathEffect.
pub fn dash_contours(contours: &[Contour], dash: &Dash) -> Vec<Contour> {
    let Some(dash) = normalize_dash(dash) else {
        return contours.to_vec();
    };
    let mut out = Vec::new();
    for contour in contours {
        dash_contour(&mut out, &contour.points, &dash);
    }
    out
}

/// SVG rules: an odd interval count repeats the list so on/off alternate
/// across the doubled cycle; negative or zero-total patterns mean no dash.
fn normalize_dash(dash: &Dash) -> Option<Dash> {
    let sum: f32 = dash.intervals.iter().sum();
    if dash.intervals.is_empty() || sum <= 0.0 || dash.intervals.iter().any(|&v| v < 0.0) {
        return None;
    }
    let mut intervals = dash.intervals.clone();
    if intervals.len() % 2 == 1 {
        intervals.extend(dash.intervals.iter().copied());
    }
    Some(Dash {
        intervals,
        phase: dash.phase,
    })
}

// ── strip assembly ──────────────────────────────────────────────────────────

#[derive(Default)]
struct Strip {
    out: Vec<f32>,
}

impl Strip {
    fn emit(&mut self, p: Point) {
        self.out.extend_from_slice(&[p.x, p.y]);
    }

    /// Degenerate stitch: repeat the last vertex, then the next one twice.
    fn stitch(&mut self, next: Point) {
        if self.out.is_empty() {
            self.emit(next);
            return;
        }
        let last = Point::new(self.out[self.out.len() - 2], self.out[self.out.len() - 1]);
        self.emit(last);
        self.emit(next);
        self.emit(next);
    }
}

fn stroke_contour(
    strip: &mut Strip,
    pts: &[Point],
    closed: bool,
    stroke: &Stroke,
    half: f32,
    tolerance: f32,
) {
    let first_normal = normal(pts[0], pts[1], half);
    if closed {
        strip.stitch(add(pts[0], first_normal));
    } else {
        start_cap(strip, pts[0], pts[1], stroke.cap, half, tolerance);
        strip.emit(add(pts[0], first_normal));
    }
    strip.emit(sub(pts[0], first_normal));

    let segments = if closed { pts.len() } else { pts.len() - 1 };
    for i in 0..segments {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        let n = normal(a, b, half);
        strip.emit(add(b, n));
        strip.emit(sub(b, n));
        let last = i + 1 == segments;
        if !last || closed {
            let c = pts[(i + 2) % pts.len()];
            join(strip, b, a, c, stroke, half, tolerance);
            let n_next = normal(b, c, half);
            strip.emit(add(b, n_next));
            strip.emit(sub(b, n_next));
        }
    }
    if !closed {
        end_cap(
            strip,
            pts[pts.len() - 2],
            pts[pts.len() - 1],
            stroke.cap,
            half,
            tolerance,
        );
    }
}

/// Join at pivot `p` between incoming (from `a`) and outgoing (to `c`)
/// segments: fan triangles on the OUTER side of the turn.
fn join(
    strip: &mut Strip,
    p: Point,
    a: Point,
    c: Point,
    stroke: &Stroke,
    half: f32,
    tolerance: f32,
) {
    let d0 = direction(a, p);
    let d1 = direction(p, c);
    let cross = d0.x * d1.y - d0.y * d1.x;
    if cross.abs() < 1e-6 {
        return; // collinear — segment quads already meet
    }
    // y-down: cross > 0 turns right; the outer side is then the LEFT
    // offset (−perp). `s` signs the outer normals.
    let s = if cross > 0.0 { -1.0 } else { 1.0 };
    let n0 = scale(perp(d0), half * s);
    let n1 = scale(perp(d1), half * s);
    let from = add(p, n0);
    let to = add(p, n1);
    match stroke.join {
        Join::Bevel => fan(strip, p, &[from, to]),
        Join::Miter => {
            let dot = d0.x * d1.x + d0.y * d1.y;
            // ratio = miter length / half-width = 1/cos(θ/2).
            let ratio = (2.0 / (1.0 + dot).max(1e-6)).sqrt();
            if ratio > stroke.miter_limit.max(1.0) {
                fan(strip, p, &[from, to]);
            } else {
                let m = Point::new(n0.x + n1.x, n0.y + n1.y);
                let tip = add(p, scale(m, 1.0 / (1.0 + dot).max(1e-6)));
                fan(strip, p, &[from, tip, to]);
            }
        }
        Join::Round => {
            let points = arc_points(p, n0, n1, half, tolerance);
            fan(strip, p, &points);
        }
    }
}

/// Fan around `pivot` through `rim` points, as strip triangles
/// (rim₀, pivot, rim₁), (pivot, rim₁, pivot), … — overlaps are fine.
fn fan(strip: &mut Strip, pivot: Point, rim: &[Point]) {
    for &q in rim {
        strip.emit(q);
        strip.emit(pivot);
    }
}

fn start_cap(strip: &mut Strip, p: Point, toward: Point, cap: Cap, half: f32, tolerance: f32) {
    let d = direction(p, toward);
    let n = scale(perp(d), half);
    match cap {
        Cap::Butt => strip.stitch(add(p, n)),
        Cap::Square => {
            let back = sub(p, scale(d, half));
            strip.stitch(add(back, n));
            strip.emit(sub(back, n));
        }
        Cap::Round => {
            // Semicircle BEHIND the start: −n → −d → +n, two quarter arcs
            // (a single π sweep is direction-ambiguous).
            let back = scale(d, -half);
            let mut rim = arc_points(p, scale(n, -1.0), back, half, tolerance);
            rim.extend(arc_points(p, back, n, half, tolerance));
            strip.stitch(p);
            fan(strip, p, &rim);
        }
    }
}

fn end_cap(strip: &mut Strip, from: Point, p: Point, cap: Cap, half: f32, tolerance: f32) {
    let d = direction(from, p);
    let n = scale(perp(d), half);
    match cap {
        Cap::Butt => {}
        Cap::Square => {
            let out = add(p, scale(d, half));
            strip.emit(add(out, n));
            strip.emit(sub(out, n));
        }
        Cap::Round => {
            // Semicircle PAST the end: +n → +d → −n.
            let fwd = scale(d, half);
            let mut rim = arc_points(p, n, fwd, half, tolerance);
            rim.extend(arc_points(p, fwd, scale(n, -1.0), half, tolerance));
            fan(strip, p, &rim);
        }
    }
}

/// A lone point strokes as its cap shape. A BUTT cap draws nothing, which is
/// what the cap definitions give with no special case: butt terminates
/// exactly at the endpoint, so two coincident endpoints enclose no area,
/// while round and square extend half a width past it and enclose area even
/// at zero length.
///
/// This follows SVG 2, Skia (`SkPathStroker::preJoinTo` bails for a butt cap
/// on a zero-length segment) and what browsers actually paint. It is worth
/// being precise about the last one: the WHATWG canvas algorithm prunes every
/// zero-length segment before stroking, so read literally it paints nothing
/// for ANY cap — but no browser implements that, and Chrome paints round and
/// square. Browser behaviour is the target here, not the prose.
///
/// Impeller substitutes Square instead so a dot stays visible, deliberately
/// and by its own convention rather than anything Flutter forces on it. valo
/// followed that until it turned out to be discontinuous: under that rule a
/// zero-length segment paints a full box while a 0.001-long one paints almost
/// nothing, so a line animating to zero flashes a square at the end.
fn lone_point(strip: &mut Strip, p: Point, stroke: &Stroke, half: f32, tolerance: f32) {
    match stroke.cap {
        Cap::Butt => {}
        Cap::Round => {
            // Full circle as four explicit quarters.
            let (r, l) = (Point::new(half, 0.0), Point::new(-half, 0.0));
            let (dn, up) = (Point::new(0.0, half), Point::new(0.0, -half));
            let mut rim = arc_points(p, r, dn, half, tolerance);
            rim.extend(arc_points(p, dn, l, half, tolerance));
            rim.extend(arc_points(p, l, up, half, tolerance));
            rim.extend(arc_points(p, up, r, half, tolerance));
            strip.stitch(p);
            fan(strip, p, &rim);
        }
        Cap::Square => {
            strip.stitch(Point::new(p.x - half, p.y - half));
            strip.emit(Point::new(p.x - half, p.y + half));
            strip.emit(Point::new(p.x + half, p.y - half));
            strip.emit(Point::new(p.x + half, p.y + half));
        }
    }
}

/// Points along the arc from offset `from` to offset `to` around `center`
/// (radius = |offset|), stepping by the flattener's angle-for-tolerance.
fn arc_points(center: Point, from: Point, to: Point, radius: f32, tolerance: f32) -> Vec<Point> {
    let a0 = from.y.atan2(from.x);
    let mut a1 = to.y.atan2(to.x);
    let mut sweep = a1 - a0;
    if sweep > std::f32::consts::PI {
        a1 -= std::f32::consts::TAU;
        sweep = a1 - a0;
    } else if sweep < -std::f32::consts::PI {
        a1 += std::f32::consts::TAU;
        sweep = a1 - a0;
    }
    let max_step = 2.0
        * (1.0 - (tolerance / radius.max(1e-3)).clamp(0.0, 0.5))
            .acos()
            .max(0.1);
    let steps = (sweep.abs() / max_step).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|i| {
            let t = a0 + sweep * (i as f32 / steps as f32);
            Point::new(center.x + radius * t.cos(), center.y + radius * t.sin())
        })
        .collect()
}

// ── dashing ─────────────────────────────────────────────────────────────────

fn dash_contour(out: &mut Vec<Contour>, contour: &[Point], dash: &Dash) {
    let cycle: f32 = dash.intervals.iter().sum();
    let (mut index, mut remaining) = interval_at(&dash.intervals, dash.phase.rem_euclid(cycle));
    let mut on = index % 2 == 0;
    let mut current: Vec<Point> = Vec::new();
    if on {
        current.push(contour[0]);
    }
    for pair in contour.windows(2) {
        let (mut a, b) = (pair[0], pair[1]);
        let mut len = distance(a, b);
        // Strict `>`: a zero-on interval landing EXACTLY at the end of the
        // subpath is not entered. WHATWG's trace-a-path would enter it — both
        // its exit tests are strict too, so it places a final direction-
        // bearing point at `position == subpath width` — but Chrome does not
        // paint that endpoint dot, and browser parity is what this shim is
        // for. Same call as the zero-length-pruning divergence noted on
        // `lone_point`.
        while len > remaining {
            let cut = lerp(a, b, remaining / len);
            if on {
                current.push(cut);
                out.push(open_contour(std::mem::take(&mut current)));
            } else {
                current.push(cut);
            }
            on = !on;
            a = cut;
            len -= remaining;
            index += 1;
            remaining = dash.intervals[index % dash.intervals.len()];
        }
        remaining -= len;
        if on {
            current.push(b);
        }
    }
    if on && current.len() > 1 {
        out.push(open_contour(current));
    }
}

/// One ON stretch of a dash pattern. Always `has_segments`: a dash is cut
/// from real geometry, and a ZERO-LENGTH on interval is the case that depends
/// on it — it reduces to a single point and still has to paint its caps.
fn open_contour(points: Vec<Point>) -> Contour {
    Contour {
        points,
        closed: false,
        has_segments: true,
    }
}

/// (interval index, remaining length in it) at `offset` into the cycle.
/// The interval `offset` falls in, and how much of it remains.
///
/// A ZERO-LENGTH interval can never satisfy `left < len`, but the dash
/// algorithm still has to enter it: `[0, 6]` at phase 0 opens in WHATWG's
/// "zero-on" state, which paints a dot at the path start and then repeats
/// every 6px. Walking past it drops that first dot only — the later ones
/// survive because the emit loop handles a zero `remaining` — which reads as
/// a phase error rather than a missing dash.
///
/// Widening the test to `left <= len` instead would enter EVERY interval one
/// step early: at offset 10 of `[10, 6]` it would return interval 0 with
/// nothing left, inventing a dot at an ordinary boundary. So the zero-length
/// case gets its own clause rather than a loosened comparison.
fn interval_at(intervals: &[f32], offset: f32) -> (usize, f32) {
    let mut left = offset;
    for (i, &len) in intervals.iter().enumerate() {
        if left < len || (len <= 0.0 && left <= 0.0) {
            return (i, len - left);
        }
        left -= len;
    }
    (0, intervals[0])
}

// ── small vector helpers ────────────────────────────────────────────────────

fn direction(a: Point, b: Point) -> Point {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    Point::new(dx / len, dy / len)
}

fn perp(d: Point) -> Point {
    Point::new(-d.y, d.x)
}

fn normal(a: Point, b: Point, half: f32) -> Point {
    scale(perp(direction(a, b)), half)
}

fn add(p: Point, v: Point) -> Point {
    Point::new(p.x + v.x, p.y + v.y)
}

fn sub(p: Point, v: Point) -> Point {
    Point::new(p.x - v.x, p.y - v.y)
}

fn scale(v: Point, k: f32) -> Point {
    Point::new(v.x * k, v.y * k)
}

fn lerp(a: Point, b: Point, t: f32) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn distance(a: Point, b: Point) -> f32 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

fn dedup(contour: &[Point]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(contour.len());
    for &p in contour {
        if out.last().is_none_or(|&last| distance(last, p) > 1e-5) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extents(strip: &[f32]) -> (f32, f32, f32, f32) {
        let xs: Vec<f32> = strip.iter().step_by(2).copied().collect();
        let ys: Vec<f32> = strip.iter().skip(1).step_by(2).copied().collect();
        (
            xs.iter().copied().fold(f32::MAX, f32::min),
            ys.iter().copied().fold(f32::MAX, f32::min),
            xs.iter().copied().fold(f32::MIN, f32::max),
            ys.iter().copied().fold(f32::MIN, f32::max),
        )
    }

    fn open(points: Vec<Point>) -> Vec<Contour> {
        vec![Contour {
            points,
            closed: false,
            has_segments: true,
        }]
    }

    /// A bare `move_to` paints NOTHING under every cap; an explicit
    /// zero-length segment paints for round and square. The two reduce to the
    /// same single point, so only the contour's `has_segments` metadata can
    /// tell them apart — which is the whole reason it exists.
    #[test]
    fn a_move_only_contour_never_strokes_but_a_zero_length_segment_does() {
        let at = Point::new(10.0, 10.0);
        let move_only = vec![Contour {
            points: vec![at],
            closed: false,
            has_segments: false,
        }];
        let zero_length = vec![Contour {
            points: vec![at, at],
            closed: false,
            has_segments: true,
        }];
        let move_and_close = vec![Contour {
            points: vec![at],
            closed: true,
            has_segments: true,
        }];
        for cap in [Cap::Butt, Cap::Round, Cap::Square] {
            let stroke = Stroke {
                cap,
                ..Stroke::new(8.0)
            };
            assert!(
                stroke_strip(&move_only, &stroke, 0.25).is_empty(),
                "a bare move_to must paint nothing under {cap:?}"
            );
        }
        // move + close paints wherever an explicit zero-length segment does.
        for cap in [Cap::Round, Cap::Square] {
            let stroke = Stroke {
                cap,
                ..Stroke::new(8.0)
            };
            assert_eq!(
                stroke_strip(&move_and_close, &stroke, 0.25),
                stroke_strip(&zero_length, &stroke, 0.25),
                "move+close must stroke like an explicit zero-length segment ({cap:?})"
            );
        }

        let butt = Stroke {
            cap: Cap::Butt,
            ..Stroke::new(8.0)
        };
        assert!(
            stroke_strip(&zero_length, &butt, 0.25).is_empty(),
            "a butt cap has no area to give a zero-length segment"
        );
        for cap in [Cap::Round, Cap::Square] {
            let stroke = Stroke {
                cap,
                ..Stroke::new(8.0)
            };
            let strip = stroke_strip(&zero_length, &stroke, 0.25);
            assert!(
                !strip.is_empty(),
                "{cap:?} must paint a zero-length segment"
            );
            let (x0, y0, x1, y1) = extents(&strip);
            assert!(
                (x0 - 6.0).abs() < 0.01
                    && (y0 - 6.0).abs() < 0.01
                    && (x1 - 14.0).abs() < 0.01
                    && (y1 - 14.0).abs() < 0.01,
                "{cap:?} should span the full stroke width, got {:?}",
                (x0, y0, x1, y1)
            );
        }
    }

    /// The flattener is what assigns `has_segments`, so the distinction has
    /// to survive a real path walk rather than only a hand-built contour.
    #[test]
    fn the_flattener_records_whether_a_contour_ever_moved() {
        use crate::PathBuilder;

        let mut move_only = PathBuilder::new();
        move_only.move_to((10.0, 10.0));
        let flattened = move_only.build().flatten(0.25);
        assert_eq!(flattened.len(), 1);
        assert!(!flattened[0].has_segments);

        let mut zero_length = PathBuilder::new();
        zero_length.move_to((10.0, 10.0));
        zero_length.line_to((10.0, 10.0));
        let flattened = zero_length.build().flatten(0.25);
        assert_eq!(flattened.len(), 1);
        assert!(flattened[0].has_segments);

        // `move_to` + `close` is an explicit zero-length SUBPATH, not a bare
        // move: closepath emits the closing edge, so it strokes like an
        // explicit zero-length segment. SVG names `M 30,30 Z` for exactly
        // this, and Skia and Impeller both convert it to a capped point.
        let mut move_and_close = PathBuilder::new();
        move_and_close.move_to((10.0, 10.0));
        move_and_close.close();
        let flattened = move_and_close.build().flatten(0.25);
        assert_eq!(flattened.len(), 1);
        assert!(
            flattened[0].has_segments,
            "close draws; a bare move does not"
        );

        // A move-only contour followed by a real one must not contaminate it,
        // and vice versa.
        let mut mixed = PathBuilder::new();
        mixed.move_to((0.0, 0.0));
        mixed.line_to((10.0, 0.0));
        mixed.move_to((50.0, 50.0));
        let flattened = mixed.build().flatten(0.25);
        assert_eq!(flattened.len(), 2);
        assert!(flattened[0].has_segments);
        assert!(!flattened[1].has_segments);
    }

    /// `[0, 6]` is WHATWG's "zero-on" pattern: a dot at the path start and
    /// every 6px after it. `interval_at` used to walk straight past a
    /// zero-length first interval, which dropped the START dot only — the
    /// rest survive, so a count-only assertion would still pass while every
    /// dot sat in the wrong place.
    #[test]
    fn a_zero_length_on_interval_puts_a_dot_at_the_path_start() {
        // 22px, not a multiple of the period, so the endpoint case stays out
        // of this test — see `the_endpoint_dot_follows_browsers_not_the_spec`
        // for why valo omits it.
        let line = open(vec![Point::new(0.0, 50.0), Point::new(22.0, 50.0)]);
        let dashes = dash_contours(
            &line,
            &Dash {
                intervals: vec![0.0, 6.0],
                phase: 0.0,
            },
        );
        let positions: Vec<f32> = dashes.iter().map(|contour| contour.points[0].x).collect();
        assert_eq!(positions, vec![0.0, 6.0, 12.0, 18.0]);
        assert!(
            dashes.iter().all(|contour| contour.has_segments),
            "a zero-length on dash is real geometry and must keep its caps"
        );
    }

    /// The endpoint dot is a deliberate spec divergence, pinned so it cannot
    /// drift silently. `[0, 6]` on a 24px line is an exact number of periods,
    /// so the literal WHATWG algorithm places a final dot at 24 — its exit
    /// tests are strict, so `position == subpath width` does not terminate.
    /// Chrome omits it, and this shim follows Chrome.
    #[test]
    fn the_endpoint_dot_follows_browsers_not_the_spec() {
        let line = open(vec![Point::new(0.0, 50.0), Point::new(24.0, 50.0)]);
        let dashes = dash_contours(
            &line,
            &Dash {
                intervals: vec![0.0, 6.0],
                phase: 0.0,
            },
        );
        let positions: Vec<f32> = dashes.iter().map(|c| c.points[0].x).collect();
        assert_eq!(
            positions,
            vec![0.0, 6.0, 12.0, 18.0],
            "the dot at 24 is the spec's, not the browser's"
        );
    }

    /// The zero-length clause must not fire at ordinary boundaries: at    /// The zero-length clause must not fire at ordinary boundaries: at
    /// offset 10 of `[10, 6]` the walk is exactly at the start of the OFF
    /// interval, not sitting on a zero-length one.
    #[test]
    fn an_ordinary_interval_boundary_gains_no_extra_dash() {
        assert_eq!(interval_at(&[10.0, 6.0], 10.0), (1, 6.0));
        assert_eq!(interval_at(&[10.0, 6.0], 0.0), (0, 10.0));
        assert_eq!(interval_at(&[10.0, 6.0], 4.0), (0, 6.0));
        assert_eq!(interval_at(&[0.0, 6.0], 0.0), (0, 0.0));
    }

    #[test]
    fn stroke_contains_answers_inside_the_ink_and_nowhere_else() {
        let line = open(vec![Point::new(10.0, 50.0), Point::new(90.0, 50.0)]);
        let stroke = Stroke::new(10.0);
        assert!(stroke_contains(
            &line,
            &stroke,
            0.25,
            Point::new(50.0, 50.0)
        ));
        assert!(stroke_contains(
            &line,
            &stroke,
            0.25,
            Point::new(50.0, 54.0)
        ));
        // The fill of an open line is empty, so only the stroke can hit —
        // 12px off the centre line is past the 5px half-width.
        assert!(!stroke_contains(
            &line,
            &stroke,
            0.25,
            Point::new(50.0, 62.0)
        ));
        // Butt caps end exactly at the endpoint.
        assert!(!stroke_contains(
            &line,
            &stroke,
            0.25,
            Point::new(95.0, 50.0)
        ));
    }

    #[test]
    fn a_wider_stroke_reaches_further() {
        let line = open(vec![Point::new(10.0, 50.0), Point::new(90.0, 50.0)]);
        let point = Point::new(50.0, 58.0);
        assert!(!stroke_contains(&line, &Stroke::new(10.0), 0.25, point));
        assert!(stroke_contains(&line, &Stroke::new(24.0), 0.25, point));
    }

    /// The strip stitches its sub-strips together with repeated vertices, so
    /// a SECOND contour is what first produces zero-area triples. Reading one
    /// of those as a hit makes the whole query answer `true` everywhere.
    #[test]
    fn a_second_contour_does_not_make_everything_hit() {
        let two = vec![
            Contour {
                points: vec![Point::new(10.0, 20.0), Point::new(90.0, 20.0)],
                closed: false,
                has_segments: true,
            },
            Contour {
                points: vec![Point::new(10.0, 80.0), Point::new(90.0, 80.0)],
                closed: false,
                has_segments: true,
            },
        ];
        let stroke = Stroke::new(10.0);
        assert!(stroke_contains(&two, &stroke, 0.25, Point::new(50.0, 20.0)));
        assert!(stroke_contains(&two, &stroke, 0.25, Point::new(50.0, 80.0)));
        // Between the two lines, and far outside every one of them.
        assert!(!stroke_contains(
            &two,
            &stroke,
            0.25,
            Point::new(50.0, 50.0)
        ));
        assert!(!stroke_contains(
            &two,
            &stroke,
            0.25,
            Point::new(5000.0, 5000.0)
        ));
    }

    /// Dashing turns one contour into many, so every dashed stroke hits the
    /// degenerate-triple case — and a gap has to answer `false`.
    #[test]
    fn dash_gaps_are_not_part_of_the_stroke() {
        let dashed = dash_contours(
            &open(vec![Point::new(0.0, 50.0), Point::new(100.0, 50.0)]),
            &Dash {
                intervals: vec![10.0, 10.0],
                phase: 0.0,
            },
        );
        assert!(
            dashed.len() > 2,
            "the pattern has to produce several dashes"
        );
        let stroke = Stroke::new(10.0);
        // 0..10 is on, 10..20 is off, 20..30 is on again.
        assert!(stroke_contains(
            &dashed,
            &stroke,
            0.25,
            Point::new(5.0, 50.0)
        ));
        assert!(!stroke_contains(
            &dashed,
            &stroke,
            0.25,
            Point::new(15.0, 50.0)
        ));
        assert!(stroke_contains(
            &dashed,
            &stroke,
            0.25,
            Point::new(25.0, 50.0)
        ));
        assert!(!stroke_contains(
            &dashed,
            &stroke,
            0.25,
            Point::new(50.0, 200.0)
        ));
    }

    fn hline() -> Vec<Contour> {
        open(vec![Point::new(10.0, 50.0), Point::new(110.0, 50.0)])
    }

    #[test]
    fn butt_caps_stop_at_the_endpoints() {
        let strip = stroke_strip(&hline(), &Stroke::new(10.0), 0.25);
        let (x0, y0, x1, y1) = extents(&strip);
        assert_eq!((x0, x1), (10.0, 110.0));
        assert_eq!((y0, y1), (45.0, 55.0));
    }

    #[test]
    fn square_and_round_caps_extend_half_width() {
        for cap in [Cap::Square, Cap::Round] {
            let stroke = Stroke {
                cap,
                ..Stroke::new(10.0)
            };
            let (x0, _, x1, _) = extents(&stroke_strip(&hline(), &stroke, 0.25));
            assert!((x0 - 5.0).abs() < 0.3, "{cap:?} start: {x0}");
            assert!((x1 - 115.0).abs() < 0.3, "{cap:?} end: {x1}");
        }
    }

    #[test]
    fn miter_spikes_until_the_limit_bevels() {
        // A right angle: miter ratio = √2 < 4 → spike reaches the corner.
        let angle = open(vec![
            Point::new(0.0, 100.0),
            Point::new(100.0, 100.0),
            Point::new(100.0, 0.0),
        ]);
        let diagonal = |strip: &[f32]| {
            strip
                .chunks_exact(2)
                .map(|v| v[0] + v[1])
                .fold(f32::MIN, f32::max)
        };
        let strip = stroke_strip(&angle, &Stroke::new(20.0), 0.25);
        assert!(
            (diagonal(&strip) - 220.0).abs() < 0.1,
            "miter tip reaches (110,110): {}",
            diagonal(&strip)
        );

        // Limit 1.0 → always bevels: corners stop at the offset points.
        let bevel = Stroke {
            miter_limit: 1.0,
            ..Stroke::new(20.0)
        };
        let strip = stroke_strip(&angle, &bevel, 0.25);
        assert!(
            diagonal(&strip) <= 210.0 + 0.1,
            "beveled corner: {}",
            diagonal(&strip)
        );
    }

    #[test]
    fn dash_splits_by_length() {
        let dashed = dash_contours(
            &hline(),
            &Dash {
                intervals: vec![30.0, 20.0],
                phase: 0.0,
            },
        );
        assert_eq!(dashed.len(), 2, "100px line, 30on/20off: {dashed:?}");
        assert_eq!(dashed[0].points[0].x, 10.0);
        assert!((dashed[0].points.last().unwrap().x - 40.0).abs() < 0.01);
        assert!((dashed[1].points[0].x - 60.0).abs() < 0.01);
        assert!((dashed[1].points.last().unwrap().x - 90.0).abs() < 0.01);
    }

    #[test]
    fn odd_interval_dash_alternates_across_the_doubled_cycle() {
        // SVG doubles [30] to [30,30]; phase 30 starts in the OFF half.
        let dashed = dash_contours(
            &hline(),
            &Dash {
                intervals: vec![30.0],
                phase: 30.0,
            },
        );
        assert_eq!(dashed.len(), 2, "{dashed:?}");
        assert!((dashed[0].points[0].x - 40.0).abs() < 0.01, "{dashed:?}");
        assert!((dashed[1].points[0].x - 100.0).abs() < 0.01, "{dashed:?}");
    }

    #[test]
    fn invalid_dash_patterns_disable_dashing() {
        for intervals in [vec![], vec![-5.0, 10.0], vec![0.0, 0.0]] {
            let dashed = dash_contours(
                &hline(),
                &Dash {
                    intervals,
                    phase: 0.0,
                },
            );
            assert_eq!(dashed.len(), 1, "pattern passes through as solid");
            assert_eq!(dashed[0].points.len(), 2);
        }
    }

    #[test]
    fn closed_contour_has_no_caps_and_wraps_joins() {
        let square = vec![Contour {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(100.0, 0.0),
                Point::new(100.0, 100.0),
                Point::new(0.0, 100.0),
                Point::new(0.0, 0.0),
            ],
            closed: true,
            has_segments: true,
        }];
        let strip = stroke_strip(&square, &Stroke::new(10.0), 0.25);
        let (x0, y0, x1, y1) = extents(&strip);
        // Miter corners reach the outer square exactly.
        assert_eq!((x0, y0, x1, y1), (-5.0, -5.0, 105.0, 105.0));
    }

    #[test]
    fn closure_is_metadata_not_point_coincidence() {
        // Impeller's with_close: the SAME points stroke differently by flag —
        // closed joins at the seam, open caps there (one fewer join).
        let points = vec![
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Point::new(100.0, 100.0),
            Point::new(0.0, 100.0),
            Point::new(0.0, 0.0),
        ];
        let by_flag = |closed: bool| {
            stroke_strip(
                &[Contour {
                    points: points.clone(),
                    closed,
                    has_segments: true,
                }],
                &Stroke::new(10.0),
                0.25,
            )
        };
        assert_ne!(
            by_flag(true).len(),
            by_flag(false).len(),
            "seam treatment must come from the flag"
        );
    }

    /// Skia's `SkPathStroker::preJoinTo` bails on a butt cap over a
    /// zero-length segment, and Canvas2D and SVG say the same. The rule is
    /// continuous: butt-capped ink shrinks to nothing as the segment does,
    /// where promoting to Square would flash a full box at exactly zero.
    #[test]
    fn a_zero_length_subpath_paints_only_for_extending_caps() {
        let dot = |cap| {
            let contour = Contour {
                points: vec![Point::new(8.0, 8.0), Point::new(8.0, 8.0)],
                closed: false,
                has_segments: true,
            };
            let mut stroke = Stroke::new(4.0);
            stroke.cap = cap;
            stroke_strip(&[contour], &stroke, 0.25)
        };
        assert!(dot(Cap::Butt).is_empty(), "butt caps enclose no area");
        assert!(
            !dot(Cap::Square).is_empty(),
            "square extends past the point"
        );
        assert!(!dot(Cap::Round).is_empty(), "round extends past the point");
    }

    /// The continuity that motivates the rule above: a butt-capped segment's
    /// ink must fall away smoothly as its length does, never jumping.
    #[test]
    fn butt_capped_ink_is_continuous_as_a_segment_vanishes() {
        let area_at = |length: f32| {
            let contour = Contour {
                points: vec![Point::new(8.0, 8.0), Point::new(8.0 + length, 8.0)],
                closed: false,
                has_segments: true,
            };
            let mut stroke = Stroke::new(4.0);
            stroke.cap = Cap::Butt;
            let strip = stroke_strip(&[contour], &stroke, 0.25);
            let (x0, y0, x1, y1) = extents(&strip);
            if strip.is_empty() {
                0.0
            } else {
                (x1 - x0) * (y1 - y0)
            }
        };
        assert!(
            area_at(0.001) < 0.05,
            "a hair-thin segment paints hardly anything"
        );
        assert_eq!(area_at(0.0), 0.0, "and zero paints nothing at all");
    }
}
