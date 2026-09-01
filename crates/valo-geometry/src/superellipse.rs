//! Rounded superellipse (the iOS "squircle") construction and hit testing.
//!
//! Ported from Flutter Impeller's `round_superellipse_param.cc`. Each corner
//! quadrant splits into two octants; an octant is a superellipse segment
//! `(x/a)ⁿ + (y/a)ⁿ = 1` joined tangent-continuously to a circular arc that
//! carries the curve across the 45° diagonal. The exponent `n` and the join
//! point come from lookup tables fitted against the iOS corner shape, keyed
//! by `ratio = size / radius`. Asymmetric radii are drawn by normalizing the
//! quadrant to a uniform radius and scaling back.

use crate::path::{constrain_radii_elliptical, PathBuilder};
use crate::point::Point;
use crate::rect::Rect;

/// Matches Impeller's `kEhCloseEnough`: radii at or below it are sharp.
const CLOSE_ENOUGH: f32 = 1e-3;

/// 1 - cos(π/4): distance from the corner's 45° midpoint to the bounding box,
/// per unit of corner radius (measured to be linear in the radius).
const GAP_FACTOR: f32 = 0.292_893_22;

// ── small vector helpers (Point deliberately has no scalar algebra) ────────

fn mul(p: Point, s: Point) -> Point {
    Point::new(p.x * s.x, p.y * s.y)
}

fn scaled(p: Point, s: f32) -> Point {
    Point::new(p.x * s, p.y * s)
}

fn length(p: Point) -> f32 {
    p.x.hypot(p.y)
}

fn flip(p: Point) -> Point {
    Point::new(p.y, p.x)
}

/// One eighth of a square-like rounded superellipse: the span from the top
/// axis (0, a) clockwise to the 45° diagonal.
///
/// `se_n == 0.0` marks a sharp corner; only `offset` and `se_a` apply then.
#[derive(Clone, Copy, Debug)]
struct Octant {
    offset: Point,
    se_a: f32,
    se_n: f32,
    /// `J`, where the superellipse hands over to the circular arc.
    circle_start: Point,
    circle_center: Point,
    circle_max_angle: f32,
}

/// One corner quadrant: two octants of a normalized (uniform-radius) shape
/// plus the scale that restores the asymmetric radii and the mirror signs.
#[derive(Clone, Copy, Debug)]
struct Quadrant {
    offset: Point,
    signed_scale: Point,
    top: Octant,
    right: Octant,
}

/// `RoundSuperellipse` is a rounded superellipse expanded to drawing form.
///
/// Radii are ordered clockwise from the top-left corner, `[x, y]` each, the
/// same convention as [`PathBuilder::rrect_radii_elliptical`].
#[derive(Clone, Copy, Debug)]
pub struct RoundSuperellipse {
    top_right: Quadrant,
    bottom_right: Quadrant,
    bottom_left: Quadrant,
    top_left: Quadrant,
    all_corners_same: bool,
}

// ── parameter computation ──────────────────────────────────────────────────

/// The point splitting `left..right` in the ratio `ratio_left : ratio_right`.
fn split(left: f32, right: f32, ratio_left: f32, ratio_right: f32) -> f32 {
    if ratio_left == 0.0 && ratio_right == 0.0 {
        return (left + right) / 2.0;
    }
    (left * ratio_right + right * ratio_left) / (ratio_left + ratio_right)
}

/// Fitted `{n, k}` with `xJ/a = 1 - 1/k`, indexed by `ratio = 2a / radius`.
/// Dense rows step 0.10 up to ratio 2.5, sparse rows step 0.50 to 5.0.
const N_AND_XJ: [[f32; 2]; 11] = [
    /*ratio=2.00*/ [2.000_000_00, 1.132_766_7],
    /*ratio=2.10*/ [2.183_498_1, 1.203_119_2],
    /*ratio=2.20*/ [2.338_886_7, 1.286_987_9],
    /*ratio=2.30*/ [2.486_605_6, 1.363_519_4],
    /*ratio=2.40*/ [2.622_266, 1.447_179_8],
    /*ratio=2.50*/ [2.751_489_9, 1.533_858_2],
    /*ratio=3.00*/ [3.362_982_7, 1.982_882_9],
    /*ratio=3.50*/ [4.086_499, 2.238_118_4],
    /*ratio=4.00*/ [4.854_811, 2.475_634_6],
    /*ratio=4.50*/ [5.629_455_6, 2.729_486],
    /*ratio=5.00*/ [6.430_238, 2.980_204],
];
const MIN_RATIO: f32 = 2.0;
const FIRST_STEP_INVERSE: f32 = 10.0;
const FIRST_MAX_RATIO: f32 = 2.5;
const FIRST_NUM_RECORDS: usize = 6;
const SECOND_STEP_INVERSE: f32 = 2.0;
const SECOND_MAX_RATIO: f32 = 5.0;
const THIRD_N_SLOPE: f32 = 1.559_599_4;
const THIRD_KXJ_SLOPE: f32 = 0.522_807_2;

/// Returns `(n, xJ / a)` for the given size-to-radius ratio.
fn compute_n_and_xj(ratio: f32) -> (f32, f32) {
    let last = N_AND_XJ.len() - 1;
    if ratio > SECOND_MAX_RATIO {
        let n = THIRD_N_SLOPE * (ratio - SECOND_MAX_RATIO) + N_AND_XJ[last][0];
        let k = THIRD_KXJ_SLOPE * (ratio - SECOND_MAX_RATIO) + N_AND_XJ[last][1];
        return (n, 1.0 - 1.0 / k);
    }
    let ratio = ratio.clamp(MIN_RATIO, SECOND_MAX_RATIO);
    let steps = if ratio < FIRST_MAX_RATIO {
        (ratio - MIN_RATIO) * FIRST_STEP_INVERSE
    } else {
        (ratio - FIRST_MAX_RATIO) * SECOND_STEP_INVERSE + (FIRST_NUM_RECORDS - 1) as f32
    };
    let left = (steps.floor() as usize).min(last - 1);
    let frac = steps - left as f32;
    let n = (1.0 - frac) * N_AND_XJ[left][0] + frac * N_AND_XJ[left + 1][0];
    let k = (1.0 - frac) * N_AND_XJ[left][1] + frac * N_AND_XJ[left + 1][1];
    (n, 1.0 - 1.0 / k)
}

/// The center of the circle of radius `r` through `a` and `b`, on the side
/// that keeps the arc convex toward the corner.
fn find_circle_center(a: Point, b: Point, r: f32) -> Point {
    let a_to_b = b - a;
    let m = scaled(a + b, 0.5);
    let c_to_m = Point::new(-a_to_b.y, a_to_b.x);
    let distance_am = length(a_to_b) / 2.0;
    let distance_cm = (r * r - distance_am * distance_am).max(0.0).sqrt();
    let len = length(c_to_m);
    m - scaled(c_to_m, distance_cm / len)
}

/// Expands one octant of a square-like rounded superellipse with half-size
/// `a` and corner radius `radius`, centered at `center`.
fn compute_octant(center: Point, a: f32, radius: f32) -> Octant {
    if radius <= CLOSE_ENOUGH {
        // Treated as sharp: a large `ratio` would overflow the fit.
        return Octant {
            offset: center,
            se_a: a,
            se_n: 0.0,
            circle_start: Point::new(a, a),
            circle_center: Point::ZERO,
            circle_max_angle: 0.0,
        };
    }

    let ratio = a * 2.0 / radius;
    let g = GAP_FACTOR * radius;

    let (n, xj_over_a) = compute_n_and_xj(ratio);
    let xj = xj_over_a * a;
    let yj = (1.0 - xj_over_a.powf(n)).powf(1.0 / n) * a;

    let tan_phi_j = (xj / yj).powf(n - 1.0);
    let d = (xj - tan_phi_j * yj) / (1.0 - tan_phi_j);
    let r = (a - d - g) * std::f32::consts::SQRT_2;

    let point_m = Point::new(a - g, a - g);
    let point_j = Point::new(xj, yj);
    let circle_center = find_circle_center(point_j, point_m, r);
    let vm = point_m - circle_center;
    let vj = point_j - circle_center;
    let circle_max_angle = (vm.x * vj.y - vm.y * vj.x).atan2(vm.x * vj.x + vm.y * vj.y);

    Octant {
        offset: center,
        se_a: a,
        se_n: n,
        circle_start: point_j,
        circle_center,
        circle_max_angle,
    }
}

/// Expands one corner quadrant. `corner` is the bounding-box corner and
/// `center` the quadrant's center; `sign` supplies the mirror signs when
/// `corner - center` has a zero component.
fn compute_quadrant(center: Point, corner: Point, in_radii: [f32; 2], sign: [f32; 2]) -> Quadrant {
    let corner_vector = corner - center;
    let radii = [
        in_radii[0].abs().min(corner_vector.x.abs()),
        in_radii[1].abs().min(corner_vector.y.abs()),
    ];

    // Normalize to a uniform radius: shrink the longer radius's axis so both
    // radii match the shorter one, and record the scale that restores it.
    let norm_radius = radii[0].min(radii[1]);
    let forward_scale = if norm_radius == 0.0 {
        [1.0, 1.0]
    } else {
        [radii[0] / norm_radius, radii[1] / norm_radius]
    };
    let norm_half_size = Point::new(
        corner_vector.x.abs() / forward_scale[0],
        corner_vector.y.abs() / forward_scale[1],
    );
    let raw_scale = Point::new(
        corner_vector.x / norm_half_size.x,
        corner_vector.y / norm_half_size.y,
    );
    let signed_scale = Point::new(
        if raw_scale.x.is_nan() {
            sign[0]
        } else {
            raw_scale.x
        },
        if raw_scale.y.is_nan() {
            sign[1]
        } else {
            raw_scale.y
        },
    );

    // The two octants belong to different square-like shapes whose centers
    // are offset by `c` in opposite directions so they share the circular arc.
    let c = norm_half_size.x - norm_half_size.y;

    Quadrant {
        offset: center,
        signed_scale,
        top: compute_octant(Point::new(0.0, -c), norm_half_size.x, norm_radius),
        right: compute_octant(Point::new(c, 0.0), norm_half_size.y, norm_radius),
    }
}

// ── containment ────────────────────────────────────────────────────────────

/// Whether `p` (already octant-local) is inside the first octant's curve.
/// Points outside the octant's angular span are vacuously contained.
fn octant_contains(param: &Octant, p: Point) -> bool {
    if p.x < 0.0 || p.y < 0.0 || p.y < p.x {
        return true;
    }
    if p.x <= param.circle_start.x {
        let px = p.x / param.se_a;
        let py = p.y / param.se_a;
        return px.powf(param.se_n) + py.powf(param.se_n) <= 1.0;
    }
    let d = param.circle_start - param.circle_center;
    let circle_radius_sq = d.x * d.x + d.y * d.y;
    let pc = p - param.circle_center;
    pc.x * pc.x + pc.y * pc.y < circle_radius_sq
}

fn corner_contains(param: &Quadrant, p: Point, check_quadrant: bool) -> bool {
    let rel = p - param.offset;
    let mut norm_point = Point::new(rel.x / param.signed_scale.x, rel.y / param.signed_scale.y);
    if check_quadrant {
        if norm_point.x < 0.0 || norm_point.y < 0.0 {
            return true;
        }
    } else {
        norm_point = Point::new(norm_point.x.abs(), norm_point.y.abs());
    }
    if param.top.se_n < 2.0 || param.right.se_n < 2.0 {
        // A rectangular corner. Top/left borders count as inside, bottom and
        // right do not, matching half-open rectangle containment.
        let x_delta = param.right.offset.x + param.right.se_a - norm_point.x;
        let y_delta = param.top.offset.y + param.top.se_a - norm_point.y;
        let x_within = x_delta > 0.0 || (x_delta == 0.0 && param.signed_scale.x < 0.0);
        let y_within = y_delta > 0.0 || (y_delta == 0.0 && param.signed_scale.y < 0.0);
        return x_within && y_within;
    }
    octant_contains(&param.top, norm_point - param.top.offset)
        && octant_contains(&param.right, flip(norm_point - param.right.offset))
}

// ── path emission ──────────────────────────────────────────────────────────

/// Conic weights `(factor1, factor2)` for the two superellipse conics,
/// indexed by `n` at unit steps from 2. Found by brute-force minimizing the
/// distance to the true curve. `weight1 = factor1·√n`, `weight2 =
/// factor2·(xJ/a)` — though see the note in `superellipse_bezier_factors`.
const BEZIER_FACTORS: [[f32; 2]; 13] = [
    /*n= 2*/ [0.7078, 8.3194],
    /*n= 3*/ [0.7895, 2.4523],
    /*n= 4*/ [0.8379, 1.8528],
    /*n= 5*/ [0.8701, 1.6891],
    /*n= 6*/ [0.8932, 1.5806],
    /*n= 7*/ [0.9107, 1.5043],
    /*n= 8*/ [0.9244, 1.4470],
    /*n= 9*/ [0.9355, 1.4037],
    /*n=10*/ [0.9448, 1.3701],
    /*n=11*/ [0.9526, 1.3431],
    /*n=12*/ [0.9594, 1.3212],
    /*n=13*/ [0.9653, 1.3032],
    /*n=14*/ [0.9705, 1.2880],
];

/// Returns `(weight1, weight2, yH/a)` for the conic pair approximating the
/// superellipse segment.
///
/// Ported bit-for-bit from Impeller, including its interpolation quirk: only
/// the `frac`-weighted right-hand table entry is multiplied by `√n` /
/// `xJ/a`. That is what Flutter ships and goldens against, so parity wins
/// over the comment's algebra.
fn superellipse_bezier_factors(n: f32, xj_over_a: f32, yj_over_a: f32) -> (f32, f32, f32) {
    const MIN_N: f32 = 2.0;
    let max_n = MIN_N + (BEZIER_FACTORS.len() - 1) as f32;
    let n_clamped = n.min(max_n);

    let steps = ((n_clamped - MIN_N).max(0.0)).min((BEZIER_FACTORS.len() - 1) as f32);
    let left = (steps.floor() as usize).min(BEZIER_FACTORS.len() - 2);
    let frac = steps - left as f32;

    let weight1 = (1.0 - frac) * BEZIER_FACTORS[left][0]
        + frac * BEZIER_FACTORS[left + 1][0] * n_clamped.sqrt();
    let weight2 =
        (1.0 - frac) * BEZIER_FACTORS[left][1] + frac * BEZIER_FACTORS[left + 1][1] * xj_over_a;

    // H splits the two conics; it slides toward A as n grows because the
    // flat span of the curve is the harder one to approximate.
    let yh_proportion = n_clamped.sqrt();
    let yh_over_a = (yh_proportion + yj_over_a) / (yh_proportion + 1.0);

    (weight1, weight2, yh_over_a)
}

/// The intersection of the lines through `p1` with slope `k1` and `p2` with
/// slope `k2`; their midpoint when (near) parallel.
fn intersection(p1: Point, k1: f32, p2: Point, k2: f32) -> Point {
    if (k1 - k2).abs() < CLOSE_ENOUGH {
        return scaled(p1 + p2, 0.5);
    }
    let x = (k1 * p1.x - k2 * p2.x + p2.y - p1.y) / (k1 - k2);
    let y = k1 * (x - p1.x) + p1.y;
    Point::new(x, y)
}

struct Transform {
    scale: Point,
    offset: Point,
    octant_offset: Point,
    flip: bool,
}

impl Transform {
    fn apply(&self, p: Point) -> Point {
        let p = if self.flip { flip(p) } else { p };
        self.offset + mul(p + self.octant_offset, self.scale)
    }
}

/// Emits one octant (superellipse conic pair + circular-arc cubic) into the
/// builder, already positioned at the octant's first point.
fn add_octant(b: &mut PathBuilder, param: &Octant, reverse: bool, transform: &Transform) {
    // Superellipse segment endpoints and tangent slopes.
    let a = Point::new(0.0, param.se_a);
    let j = param.circle_start;
    let (weight1, weight2, yh_over_a) =
        superellipse_bezier_factors(param.se_n, j.x / param.se_a, j.y / param.se_a);
    let h = Point::new(
        (1.0 - yh_over_a.powf(param.se_n)).powf(1.0 / param.se_n) * param.se_a,
        yh_over_a * param.se_a,
    );
    let k_a = 0.0;
    let k_j = -(j.x / j.y).powf(param.se_n - 1.0);
    let k_h = -(h.x / h.y).powf(param.se_n - 1.0);
    let c_ah = intersection(a, k_a, h, k_h);
    let c_hj = intersection(h, k_h, j, k_j);

    // Circular arc as a single cubic.
    let start_vector = j - param.circle_center;
    let ang = -param.circle_max_angle;
    let (sin_a, cos_a) = ang.sin_cos();
    let end_vector = Point::new(
        start_vector.x * cos_a - start_vector.y * sin_a,
        start_vector.x * sin_a + start_vector.y * cos_a,
    );
    let circle_end = param.circle_center + end_vector;
    let radius = length(start_vector);
    let start_tangent = scaled(Point::new(start_vector.y, -start_vector.x), 1.0 / radius);
    let end_tangent = scaled(Point::new(-end_vector.y, end_vector.x), 1.0 / radius);
    let bezier_factor = (param.circle_max_angle / 4.0).tan() * 4.0 / 3.0;
    let arc = [
        j,
        j + scaled(start_tangent, bezier_factor * radius),
        circle_end + scaled(end_tangent, bezier_factor * radius),
        circle_end,
    ];

    let t = |p: Point| transform.apply(p);
    if !reverse {
        b.conic_to(t(c_ah), t(h), weight1);
        b.conic_to(t(c_hj), t(j), weight2);
        b.cubic_to(t(arc[1]), t(arc[2]), t(arc[3]));
    } else {
        b.cubic_to(t(arc[2]), t(arc[1]), t(arc[0]));
        b.conic_to(t(c_hj), t(h), weight2);
        b.conic_to(t(c_ah), t(a), weight1);
    }
}

/// Emits one quadrant: the two octants, or two straight edges for a sharp
/// corner. `scale_sign` mirrors a shared quadrant into the other three.
fn add_quadrant(b: &mut PathBuilder, param: &Quadrant, reverse: bool, scale_sign: [f32; 2]) {
    let scale = Point::new(
        param.signed_scale.x * scale_sign[0],
        param.signed_scale.y * scale_sign[1],
    );
    let outer = |p: Point| param.offset + mul(p, scale);
    if param.top.se_n < 2.0 || param.right.se_n < 2.0 {
        b.line_to(outer(
            param.top.offset + Point::new(param.top.se_a, param.top.se_a),
        ));
        if !reverse {
            b.line_to(outer(
                param.right.offset + Point::new(param.right.se_a, 0.0),
            ));
        } else {
            b.line_to(outer(param.top.offset + Point::new(0.0, param.top.se_a)));
        }
        return;
    }
    let top_transform = Transform {
        scale,
        offset: param.offset,
        octant_offset: param.top.offset,
        flip: false,
    };
    let right_transform = Transform {
        scale,
        offset: param.offset,
        octant_offset: param.right.offset,
        flip: true,
    };
    if !reverse {
        add_octant(b, &param.top, false, &top_transform);
        add_octant(b, &param.right, true, &right_transform);
    } else {
        add_octant(b, &param.right, false, &right_transform);
        add_octant(b, &param.top, true, &top_transform);
    }
}

impl RoundSuperellipse {
    /// `new` expands a rect and per-corner radii into drawing parameters.
    ///
    /// `radii` is `[top-left, top-right, bottom-right, bottom-left]`, each
    /// `[x_radius, y_radius]`. Overlapping radii are proportionally reduced,
    /// as for rounded rectangles.
    pub fn new(r: Rect, radii: [[f32; 2]; 4]) -> Self {
        // A corner flat on either axis is FULLY sharp. Impeller zeroes it
        // before the fit scale (`RoundingRadii::Scaled`), so the non-flat
        // axis neither rounds its own corner, consumes scale budget, nor —
        // through the `split` calls below — shifts a neighbouring corner's
        // quadrant boundary.
        let radii = radii.map(|[x, y]| {
            if x <= 0.0 || y <= 0.0 {
                [0.0; 2]
            } else {
                [x, y]
            }
        });
        let radii = constrain_radii_elliptical(&r, radii);
        let [tl, tr, br, bl] = radii;
        let center = Point::new(r.x + r.width / 2.0, r.y + r.height / 2.0);
        let (l, t, rt, bm) = (r.x, r.y, r.right(), r.bottom());

        let all_same = tl == tr && tr == br && br == bl;
        let non_empty = tl[0] > 0.0 && tl[1] > 0.0;
        if all_same && non_empty {
            let quadrant = compute_quadrant(center, Point::new(rt, t), tr, [1.0, -1.0]);
            return Self {
                top_right: quadrant,
                bottom_right: quadrant,
                bottom_left: quadrant,
                top_left: quadrant,
                all_corners_same: true,
            };
        }

        let top_split = split(l, rt, tl[0], tr[0]);
        let right_split = split(t, bm, tr[1], br[1]);
        let bottom_split = split(l, rt, bl[0], br[0]);
        let left_split = split(t, bm, tl[1], bl[1]);

        Self {
            top_right: compute_quadrant(
                Point::new(top_split, right_split),
                Point::new(rt, t),
                tr,
                [1.0, -1.0],
            ),
            bottom_right: compute_quadrant(
                Point::new(bottom_split, right_split),
                Point::new(rt, bm),
                br,
                [1.0, 1.0],
            ),
            bottom_left: compute_quadrant(
                Point::new(bottom_split, left_split),
                Point::new(l, bm),
                bl,
                [-1.0, 1.0],
            ),
            top_left: compute_quadrant(
                Point::new(top_split, left_split),
                Point::new(l, t),
                tl,
                [-1.0, -1.0],
            ),
            all_corners_same: false,
        }
    }

    /// `contains` reports whether the point lies inside the shape.
    pub fn contains(&self, p: Point) -> bool {
        if self.all_corners_same {
            return corner_contains(&self.top_right, p, false);
        }
        corner_contains(&self.top_right, p, true)
            && corner_contains(&self.bottom_right, p, true)
            && corner_contains(&self.bottom_left, p, true)
            && corner_contains(&self.top_left, p, true)
    }

    /// `emit` appends the closed clockwise outline to the builder.
    pub fn emit(&self, b: &mut PathBuilder) {
        let q = &self.top_right;
        let start = q.offset + mul(q.top.offset + Point::new(0.0, q.top.se_a), q.signed_scale);
        b.move_to(start);
        if self.all_corners_same {
            add_quadrant(b, &self.top_right, false, [1.0, 1.0]);
            add_quadrant(b, &self.top_right, true, [1.0, -1.0]);
            add_quadrant(b, &self.top_right, false, [-1.0, -1.0]);
            add_quadrant(b, &self.top_right, true, [-1.0, 1.0]);
        } else {
            add_quadrant(b, &self.top_right, false, [1.0, 1.0]);
            add_quadrant(b, &self.bottom_right, true, [1.0, 1.0]);
            add_quadrant(b, &self.bottom_left, false, [1.0, 1.0]);
            add_quadrant(b, &self.top_left, true, [1.0, 1.0]);
        }
        b.line_to(start);
        b.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::FillRule;

    fn boundary_points(r: Rect, radii: [[f32; 2]; 4]) -> Vec<Point> {
        let mut b = PathBuilder::new();
        b.rsuperellipse_radii(r, radii);
        let path = b.build();
        path.flatten(0.05)
            .iter()
            .flat_map(|c| c.points.clone())
            .collect()
    }

    fn scale_about(p: Point, center: Point, f: f32) -> Point {
        let d = p - center;
        center + Point::new(d.x * f, d.y * f)
    }

    /// At ratio 2 (radius = half size) the fitted exponent is exactly 2, so
    /// the whole outline degenerates to a circle. This is a mathematical
    /// truth independent of the ported tables: if the lookup values or the
    /// emission transforms are wrong, the distances drift.
    #[test]
    fn fully_rounded_square_degenerates_to_a_circle() {
        let r = Rect::from_ltrb(-100.0, -100.0, 100.0, 100.0);
        let pts = boundary_points(r, [[100.0; 2]; 4]);
        assert!(pts.len() > 20);
        let mut worst = 0.0f32;
        for p in &pts {
            worst = worst.max((length(*p) - 100.0).abs());
        }
        assert!(worst < 0.6, "max deviation from circle: {worst}");

        let shape = RoundSuperellipse::new(r, [[100.0; 2]; 4]);
        for i in 0..64 {
            let ang = i as f32 / 64.0 * std::f32::consts::TAU;
            let (s, c) = ang.sin_cos();
            assert!(shape.contains(Point::new(c * 99.0, s * 99.0)));
            assert!(!shape.contains(Point::new(c * 101.0, s * 101.0)));
        }
    }

    /// Zero radii collapse to the plain rectangle outline.
    #[test]
    fn zero_radius_is_the_rectangle() {
        let r = Rect::from_ltrb(10.0, 20.0, 110.0, 80.0);
        let pts = boundary_points(r, [[0.0; 2]; 4]);
        for p in &pts {
            let on_x = (p.x - r.x).abs() < 1e-3 || (p.x - r.right()).abs() < 1e-3;
            let on_y = (p.y - r.y).abs() < 1e-3 || (p.y - r.bottom()).abs() < 1e-3;
            assert!(on_x || on_y, "{p:?} is off the rectangle outline");
        }
        let shape = RoundSuperellipse::new(r, [[0.0; 2]; 4]);
        assert!(shape.contains(Point::new(60.0, 50.0)));
        assert!(!shape.contains(Point::new(9.0, 50.0)));
        assert!(!shape.contains(Point::new(60.0, 81.0)));
    }

    /// The emitted outline and the analytic `contains` describe the same
    /// shape: every path point sits on the implicit boundary.
    #[test]
    fn path_and_contains_agree_on_the_boundary() {
        let cases: [(Rect, [[f32; 2]; 4]); 4] = [
            // Uniform.
            (Rect::from_ltrb(0.0, 0.0, 300.0, 300.0), [[60.0; 2]; 4]),
            // Elliptical radii on a non-square rect.
            (
                Rect::from_ltrb(-50.0, 10.0, 250.0, 130.0),
                [[40.0, 20.0]; 4],
            ),
            // Different radius per corner, one sharp.
            (
                Rect::from_ltrb(0.0, 0.0, 200.0, 160.0),
                [[30.0, 30.0], [10.0, 24.0], [0.0, 0.0], [52.0, 18.0]],
            ),
            // Oversized radii forced through the overlap constraint.
            (Rect::from_ltrb(0.0, 0.0, 100.0, 40.0), [[80.0; 2]; 4]),
        ];
        for (r, radii) in cases {
            let center = Point::new(r.x + r.width / 2.0, r.y + r.height / 2.0);
            let shape = RoundSuperellipse::new(r, radii);
            let pts = boundary_points(r, radii);
            assert!(pts.len() > 20);
            for p in &pts {
                // 1% inward/outward of the local half-extent clears the
                // conic-lowering and flattening tolerances.
                assert!(
                    shape.contains(scale_about(*p, center, 0.99)),
                    "{p:?} deflated should be inside for {r:?} {radii:?}"
                );
                assert!(
                    !shape.contains(scale_about(*p, center, 1.01)),
                    "{p:?} inflated should be outside for {r:?} {radii:?}"
                );
            }
        }
    }

    /// The filled path agrees with analytic containment everywhere except a
    /// thin band around the boundary.
    #[test]
    fn fill_matches_analytic_containment() {
        let r = Rect::from_ltrb(0.0, 0.0, 240.0, 160.0);
        let radii = [[50.0, 30.0], [24.0, 24.0], [70.0, 20.0], [8.0, 8.0]];
        let shape = RoundSuperellipse::new(r, radii);
        let center = Point::new(120.0, 80.0);
        let mut b = PathBuilder::new();
        b.rsuperellipse_radii(r, radii);
        let path = b.build();
        for iy in 0..48 {
            for ix in 0..48 {
                let p = Point::new(ix as f32 * 5.0 + 0.5, iy as f32 * (10.0 / 3.0) + 0.5);
                let robustly_in = shape.contains(scale_about(p, center, 1.02));
                let robustly_out = !shape.contains(scale_about(p, center, 0.98));
                if robustly_in {
                    assert!(path.contains(p, FillRule::NonZero), "{p:?} should fill");
                } else if robustly_out {
                    assert!(
                        !path.contains(p, FillRule::NonZero),
                        "{p:?} should not fill"
                    );
                }
            }
        }
    }

    /// The outline reaches all four edge midpoints and stays inside bounds.
    #[test]
    fn outline_fills_its_bounds() {
        let r = Rect::from_ltrb(0.0, 0.0, 200.0, 120.0);
        let pts = boundary_points(r, [[36.0; 2]; 4]);
        let eps = 0.1;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &pts {
            assert!(
                p.x >= -eps && p.x <= 200.0 + eps && p.y >= -eps && p.y <= 120.0 + eps,
                "{p:?} escapes the bounds"
            );
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
        assert!(min_x < eps && min_y < eps && max_x > 200.0 - eps && max_y > 120.0 - eps);
    }

    /// A corner flat on either axis is FULLY sharp: Impeller zeroes the
    /// whole corner before the fit scale and the split points
    /// (`RoundingRadii::Scaled`), so `[40, 0]` behaves exactly like
    /// `[0, 0]` — its non-flat axis must not shift the neighbouring
    /// corner's split point.
    #[test]
    fn a_flat_corner_is_fully_sharp() {
        let r = Rect::from_ltrb(0.0, 0.0, 200.0, 100.0);
        let rest = [[20.0, 20.0]; 3];
        let flat = [[40.0, 0.0], rest[0], rest[1], rest[2]];
        let zero = [[0.0, 0.0], rest[0], rest[1], rest[2]];
        assert_eq!(
            boundary_points(r, flat),
            boundary_points(r, zero),
            "a flat corner must build the identical outline to a zero corner"
        );
        let shape = RoundSuperellipse::new(r, flat);
        for (p, want) in [
            (Point::new(0.5, 0.5), true),
            (Point::new(0.5, 99.5), false),
            (Point::new(199.5, 0.5), false),
        ] {
            assert_eq!(shape.contains(p), want, "{p:?}");
        }
    }

    /// The corner's shape signature against the circular rounded rect with
    /// the same radius: measured from the rrect corner's circle center, the
    /// squircle boundary touches the circular arc exactly on the 45°
    /// diagonal and tucks about 1 unit inside it near the tangent points —
    /// the smooth hand-off into the straight edges. A plain rrect would sit
    /// at distance `radius` for every angle.
    #[test]
    fn corner_profile_differs_from_the_circular_rounded_rect() {
        let r = Rect::from_ltrb(0.0, 0.0, 300.0, 300.0);
        let radius = 60.0f32;
        let shape = RoundSuperellipse::new(r, [[radius; 2]; 4]);
        let corner_center = Point::new(300.0 - radius, radius);
        let boundary_distance = |angle_up_from_x: f32| {
            let (s, c) = angle_up_from_x.sin_cos();
            let dir = Point::new(c, -s);
            let (mut lo, mut hi) = (0.0f32, 2.0 * radius);
            for _ in 0..40 {
                let mid = (lo + hi) / 2.0;
                let p = corner_center + Point::new(dir.x * mid, dir.y * mid);
                if shape.contains(p) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        let quarter = std::f32::consts::FRAC_PI_2;
        let at_diagonal = boundary_distance(quarter / 2.0);
        assert!(
            (at_diagonal - radius).abs() < 0.05,
            "diagonal: {at_diagonal}"
        );
        let near_tangent = boundary_distance(quarter * 5.0 / 90.0);
        assert!(
            radius - near_tangent > 0.5 && radius - near_tangent < 2.0,
            "near tangent: {near_tangent}"
        );
    }
}
