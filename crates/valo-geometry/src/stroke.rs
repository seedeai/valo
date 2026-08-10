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
            1 => lone_point(&mut strip, pts[0], stroke, half, tolerance),
            _ => stroke_contour(&mut strip, &pts, contour.closed, stroke, half, tolerance),
        }
    }
    strip.out
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

/// A lone point strokes as its cap shape (Impeller promotes lone Butt to
/// Square — a dot should be visible).
fn lone_point(strip: &mut Strip, p: Point, stroke: &Stroke, half: f32, tolerance: f32) {
    match stroke.cap {
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
        Cap::Butt | Cap::Square => {
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

fn open_contour(points: Vec<Point>) -> Contour {
    Contour {
        points,
        closed: false,
    }
}

/// (interval index, remaining length in it) at `offset` into the cycle.
fn interval_at(intervals: &[f32], offset: f32) -> (usize, f32) {
    let mut left = offset;
    for (i, &len) in intervals.iter().enumerate() {
        if left < len {
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
        }]
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
}
