//! Port of the parts of Skia's src/core/SkGeometry.{h,cpp} that PathOps calls: chopping a quad
//! at its maximum curvature, the conic and its `chopAt`, and cubic classification.
//!
//! Skia computes some of these with `skvx::float2`; each lane is the same float arithmetic on
//! x and y, written out here per coordinate in the same order.

use crate::core::float_bits::{sk_is_finite_f32, sk_is_nan_f32, std_max, SK_SCALAR_1};
use crate::core::point::SkPoint;

/// `interp(v0, v1, t)` on one lane of a `float2`.
fn interp(v0: f32, v1: f32, t: f32) -> f32 {
    v0 + (v1 - v0) * t
}

/// `SkChopQuadAt`: splits the quad `src` at `t` into `dst[0..3]` and `dst[2..5]`.
pub(crate) fn sk_chop_quad_at(src: &[SkPoint], dst: &mut [SkPoint], t: f32) {
    debug_assert!(t > 0.0 && t < SK_SCALAR_1);
    let p0 = src[0];
    let p1 = src[1];
    let p2 = src[2];
    let p01 = SkPoint::new(interp(p0.x, p1.x, t), interp(p0.y, p1.y, t));
    let p12 = SkPoint::new(interp(p1.x, p2.x, t), interp(p1.y, p2.y, t));
    dst[0] = p0;
    dst[1] = p01;
    dst[2] = SkPoint::new(interp(p01.x, p12.x, t), interp(p01.y, p12.y, t));
    dst[3] = p12;
    dst[4] = p2;
}

//  F(t)    = a (1 - t) ^ 2 + 2 b t (1 - t) + c t ^ 2
//  F'(t)   = 2 (b - a) + 2 (a - 2b + c) t
//  F''(t)  = 2 (a - 2b + c)
//
//  A = 2 (b - a)
//  B = 2 (a - 2b + c)
//
//  Maximum curvature for a quadratic means solving
//  Fx' Fx'' + Fy' Fy'' = 0
//
//  t = - (Ax Bx + Ay By) / (Bx ^ 2 + By ^ 2)
//
/// `SkFindQuadMaxCurvature`.
pub(crate) fn sk_find_quad_max_curvature(src: &[SkPoint]) -> f32 {
    let ax = src[1].x - src[0].x;
    let ay = src[1].y - src[0].y;
    let bx = src[0].x - src[1].x - src[1].x + src[2].x;
    let by = src[0].y - src[1].y - src[1].y + src[2].y;

    let mut numer = -(ax * bx + ay * by);
    let mut denom = bx * bx + by * by;
    if denom < 0.0 {
        numer = -numer;
        denom = -denom;
    }
    if numer <= 0.0 {
        return 0.0;
    }
    if numer >= denom {
        // Also catches denom=0.
        return 1.0;
    }
    let t = numer / denom;
    debug_assert!((0.0..1.0).contains(&t) || sk_is_nan_f32(t));
    t
}

/// `SkChopQuadAtMaxCurvature`: 2 when it split `src` into `dst[0..5]`, else 1 with `src` copied.
pub(crate) fn sk_chop_quad_at_max_curvature(src: &[SkPoint], dst: &mut [SkPoint]) -> i32 {
    let t = sk_find_quad_max_curvature(src);
    if t > 0.0 && t < 1.0 {
        sk_chop_quad_at(src, dst, t);
        2
    } else {
        dst[..3].copy_from_slice(&src[..3]);
        1
    }
}

/// `SkCubicType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkCubicType {
    Serpentine,
    Loop,
    /// Cusp at a non-infinite parameter value with an inflection at t=infinity.
    LocalCusp,
    /// Cusp with a cusp at t=infinity and a local inflection.
    CuspAtInfinity,
    Quadratic,
    LineOrPoint,
}

// Assumes the third component of points is 1.
// Calcs p0 . (p1 x p2)
fn calc_dot_cross_cubic(p0: &SkPoint, p1: &SkPoint, p2: &SkPoint) -> f64 {
    let x_comp = p0.x as f64 * (p1.y as f64 - p2.y as f64);
    let y_comp = p0.y as f64 * (p2.x as f64 - p1.x as f64);
    let w_comp = p1.x as f64 * p2.y as f64 - p1.y as f64 * p2.x as f64;
    x_comp + y_comp + w_comp
}

// Returns a positive power of 2 that, when multiplied by n, and excepting the two edge cases listed
// below, shifts the exponent of n to yield a magnitude somewhere inside [1..2).
// Returns 2^1023 if abs(n) < 2^-1022 (including 0).
// Returns NaN if n is Inf or NaN.
fn previous_inverse_pow2(n: f64) -> f64 {
    let mut bits = n.to_bits();
    bits = (((1023u64 * 2) << 52) + ((1u64 << 52) - 1)).wrapping_sub(bits); // exp=-exp
    bits &= 0x7ffu64 << 52; // mantissa=1.0, sign=0
    f64::from_bits(bits)
}

fn write_cubic_inflection_roots(
    t0: f64,
    s0: f64,
    t1: f64,
    s1: f64,
    t: &mut [f64; 2],
    s: &mut [f64; 2],
) {
    t[0] = t0;
    s[0] = s0;

    // This copysign/abs business orients the implicit function so positive values are always on the
    // "left" side of the curve.
    t[1] = -t1.copysign(t1 * s1);
    s[1] = -s1.abs();

    // Ensure t[0]/s[0] <= t[1]/s[1] (s[1] is negative from above).
    if s[1].copysign(s[0]) * t[0] > -s[0].abs() * t[1] {
        t.swap(0, 1);
        s.swap(0, 1);
    }
}

/// `SkClassifyCubic(P, t, s, d)`: Skia's pointer parameters, which may be null, are `Option`s.
/// Ported with its file-local helpers (`calc_dot_cross_cubic`, `previous_inverse_pow2`,
/// `write_cubic_inflection_roots`).
pub(crate) fn sk_classify_cubic(
    p: &[SkPoint],
    t: Option<&mut [f64; 2]>,
    s: Option<&mut [f64; 2]>,
    d: Option<&mut [f64; 4]>,
) -> SkCubicType {
    // Find the cubic's inflection function, I = [T^3  -3T^2  3T  -1] dot D. (D0 will always be 0
    // for integral cubics.)
    //
    // See "Resolution Independent Curve Rendering using Programmable Graphics Hardware",
    // 4.2 Curve Categorization:
    //
    // https://www.microsoft.com/en-us/research/wp-content/uploads/2005/01/p1000-loop.pdf
    let a1 = calc_dot_cross_cubic(&p[0], &p[3], &p[2]);
    let a2 = calc_dot_cross_cubic(&p[1], &p[0], &p[3]);
    let a3 = calc_dot_cross_cubic(&p[2], &p[1], &p[0]);

    let mut d3 = 3.0 * a3;
    let mut d2 = d3 - a2;
    let mut d1 = d2 - a2 + a1;

    // Shift the exponents in D so the largest magnitude falls somewhere in 1..2. This protects us
    // from overflow down the road while solving for roots and KLM functionals.
    let d_max = std_max(std_max(d1.abs(), d2.abs()), d3.abs());
    let norm = previous_inverse_pow2(d_max);
    d1 *= norm;
    d2 *= norm;
    d3 *= norm;

    if let Some(d) = d {
        d[3] = d3;
        d[2] = d2;
        d[1] = d1;
        d[0] = 0.0;
    }

    // `t && s`: both out-arrays given.
    let ts = match (t, s) {
        (Some(t), Some(s)) => Some((t, s)),
        _ => None,
    };

    // Now use the inflection function to classify the cubic.
    //
    // See "Resolution Independent Curve Rendering using Programmable Graphics Hardware",
    // 4.4 Integral Cubics:
    //
    // https://www.microsoft.com/en-us/research/wp-content/uploads/2005/01/p1000-loop.pdf
    if 0.0 != d1 {
        let discr = 3.0 * d2 * d2 - 4.0 * d1 * d3;
        if discr > 0.0 {
            // Serpentine.
            if let Some((t, s)) = ts {
                let q = 3.0 * d2 + (3.0 * discr).sqrt().copysign(d2);
                write_cubic_inflection_roots(q, 6.0 * d1, 2.0 * d3, q, t, s);
            }
            SkCubicType::Serpentine
        } else if discr < 0.0 {
            // Loop.
            if let Some((t, s)) = ts {
                let q = d2 + (-discr).sqrt().copysign(d2);
                write_cubic_inflection_roots(q, 2.0 * d1, 2.0 * (d2 * d2 - d3 * d1), d1 * q, t, s);
            }
            SkCubicType::Loop
        } else {
            // Cusp.
            if let Some((t, s)) = ts {
                write_cubic_inflection_roots(d2, 2.0 * d1, d2, 2.0 * d1, t, s);
            }
            SkCubicType::LocalCusp
        }
    } else if 0.0 != d2 {
        // Cusp at T=infinity.
        if let Some((t, s)) = ts {
            write_cubic_inflection_roots(d3, 3.0 * d2, 1.0, 0.0, t, s); // T1=infinity.
        }
        SkCubicType::CuspAtInfinity
    } else {
        // Degenerate.
        if let Some((t, s)) = ts {
            write_cubic_inflection_roots(1.0, 0.0, 1.0, 0.0, t, s); // T0=T1=infinity.
        }
        if 0.0 != d3 {
            SkCubicType::Quadratic
        } else {
            SkCubicType::LineOrPoint
        }
    }
}

/// `SkConic`: a rational quadratic, its three points and the middle point's weight.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkConic {
    pub(crate) pts: [SkPoint; 3],
    pub(crate) w: f32,
}

impl SkConic {
    /// `SkConic(const SkPoint pts[3], SkScalar w)`.
    pub(crate) fn new(pts: &[SkPoint], w: f32) -> SkConic {
        let mut conic = SkConic::default();
        conic.set(pts, w);
        conic
    }

    pub(crate) fn set(&mut self, pts: &[SkPoint], w: f32) {
        self.pts.copy_from_slice(&pts[..3]);
        self.set_w(w);
    }

    pub(crate) fn set_w(&mut self, w: f32) {
        if sk_is_finite_f32(&[w]) {
            debug_assert!(w > 0.0);
        }
        // Guard against bad weights by forcing them to 1.
        self.w = if w > 0.0 && sk_is_finite_f32(&[w]) {
            w
        } else {
            1.0
        };
    }

    /// `SkConic::chopAt(SkScalar t, SkConic dst[2])`: false if infinity or NaN is generated.
    /// Ported with its file-local helpers (`ratquad_mapTo3D`, `p3d_interp`, `project_down`).
    #[must_use]
    pub(crate) fn chop_at(&self, t: f32, dst: &mut [SkConic; 2]) -> bool {
        let mut tmp = [SkPoint3::default(); 3];
        let mut tmp2 = [SkPoint3::default(); 3];

        ratquad_map_to_3d(&self.pts, self.w, &mut tmp);

        // Skia interpolates through float pointers at a stride of 3; here one lane at a time.
        let lane = |p: &[SkPoint3; 3], f: fn(&SkPoint3) -> f32| [f(&p[0]), f(&p[1]), f(&p[2])];
        let xs = p3d_interp(&lane(&tmp, |p| p.x), t);
        let ys = p3d_interp(&lane(&tmp, |p| p.y), t);
        let zs = p3d_interp(&lane(&tmp, |p| p.z), t);
        for i in 0..3 {
            tmp2[i] = SkPoint3 {
                x: xs[i],
                y: ys[i],
                z: zs[i],
            };
        }

        dst[0].pts[0] = self.pts[0];
        dst[0].pts[1] = project_down(&tmp2[0]);
        dst[0].pts[2] = project_down(&tmp2[1]);
        dst[1].pts[0] = dst[0].pts[2];
        dst[1].pts[1] = project_down(&tmp2[2]);
        dst[1].pts[2] = self.pts[2];

        // to put in "standard form", where w0 and w2 are both 1, we compute the
        // new w1 as sqrt(w1*w1/w0*w2)
        // or
        // w1 /= sqrt(w0*w2)
        //
        // However, in our case, we know that for dst[0]:
        //     w0 == 1, and for dst[1], w2 == 1
        //
        let root = tmp2[1].z.sqrt();
        dst[0].w = tmp2[0].z / root;
        dst[1].w = tmp2[2].z / root;
        // `SkIsFinite(&dst[0].fPts[0].fX, 7 * 2)`: both conics' points and weights.
        let mut values = [0.0_f32; 14];
        for (i, conic) in dst.iter().enumerate() {
            let base = i * 7;
            for (j, pt) in conic.pts.iter().enumerate() {
                values[base + 2 * j] = pt.x;
                values[base + 2 * j + 1] = pt.y;
            }
            values[base + 6] = conic.w;
        }
        sk_is_finite_f32(&values)
    }
}

/// `SkPoint3` (include/core/SkPoint3.h).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SkPoint3 {
    x: f32,
    y: f32,
    z: f32,
}

/// `SkScalarInterp`.
fn sk_scalar_interp(a: f32, b: f32, t: f32) -> f32 {
    debug_assert!((0.0..=SK_SCALAR_1).contains(&t));
    a + (b - a) * t
}

// We only interpolate one dimension at a time (the first, at +0, +3, +6).
fn p3d_interp(src: &[f32; 3], t: f32) -> [f32; 3] {
    let ab = sk_scalar_interp(src[0], src[1], t);
    let bc = sk_scalar_interp(src[1], src[2], t);
    [ab, sk_scalar_interp(ab, bc, t), bc]
}

fn ratquad_map_to_3d(src: &[SkPoint; 3], w: f32, dst: &mut [SkPoint3; 3]) {
    dst[0] = SkPoint3 {
        x: src[0].x * 1.0,
        y: src[0].y * 1.0,
        z: 1.0,
    };
    dst[1] = SkPoint3 {
        x: src[1].x * w,
        y: src[1].y * w,
        z: w,
    };
    dst[2] = SkPoint3 {
        x: src[2].x * 1.0,
        y: src[2].y * 1.0,
        z: 1.0,
    };
}

fn project_down(src: &SkPoint3) -> SkPoint {
    SkPoint::new(src.x / src.z, src.y / src.z)
}
