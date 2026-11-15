//! Port of Skia's src/pathops/SkPathOpsConic.{h,cpp}: a conic (rational quadratic) in doubles,
//! and `SkTConic`, its `SkTCurve` for the TSect intersection.

use std::ops::Index;

use crate::core::float_bits::sk_ieee_double_divide;
use crate::core::point::SkPoint;
use crate::intersections::SkIntersections;
use crate::path_ops_cubic::SkDCubic;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::{SkDPoint, SkDVector};
use crate::path_ops_quad::SkDQuad;
use crate::path_ops_rect::SkDRect;
use crate::path_ops_t_curve::SkTCurve;
use crate::path_ops_types::zero_or_one;

// cribbed from the float version in SkGeometry.cpp
// Skia reads `src` at a stride of two (`&fPts[0].fX` or `.fY`), as these do.
fn conic_deriv_coeff(src: &[f64], w: f32, coeff: &mut [f64; 3]) {
    let p20 = src[4] - src[0];
    let p10 = src[2] - src[0];
    let w_p10 = w as f64 * p10;
    coeff[0] = w as f64 * p20 - p20;
    coeff[1] = p20 - 2.0 * w_p10;
    coeff[2] = w_p10;
}

fn conic_eval_tan(coord: &[f64], w: f32, t: f64) -> f64 {
    let mut coeff = [0.0; 3];
    conic_deriv_coeff(coord, w, &mut coeff);
    t * (t * coeff[0] + coeff[1]) + coeff[2]
}

fn conic_eval_numerator(src: &[f64], w: f32, t: f64) -> f64 {
    debug_assert!((0.0..=1.0).contains(&t));
    let src2w = src[2] * w as f64;
    let c = src[0];
    let a = src[4] - 2.0 * src2w + c;
    let b = 2.0 * (src2w - c);
    (a * t + b) * t + c
}

/// `2 * (w - 1)` is float arithmetic in Skia, widened when stored.
fn conic_eval_denominator(w: f32, t: f64) -> f64 {
    let b = (2.0 * (w - 1.0)) as f64;
    let c = 1.0;
    let a = -b;
    (a * t + b) * t + c
}

/* see quad subdivide for point rationale */
/* w rationale : the mid point between t1 and t2 could be determined from the computed a/b/c
  values if the computed w was known. Since we know the mid point at (t1+t2)/2, we'll assume
  that it is the same as the point on the new curve t==(0+1)/2.

   d / dz == conic_poly(dst, unknownW, .5) / conic_weight(unknownW, .5);

   conic_poly(dst, unknownW, .5)
                 =   a / 4 + (b * unknownW) / 2 + c / 4
                 =  (a + c) / 4 + (bx * unknownW) / 2

   conic_weight(unknownW, .5)
                 =   unknownW / 2 + 1 / 2

   d / dz                  == ((a + c) / 2 + b * unknownW) / (unknownW + 1)
   d / dz * (unknownW + 1) ==  (a + c) / 2 + b * unknownW
             unknownW       = ((a + c) / 2 - d / dz) / (d / dz - b)

   Thus, w is the ratio of the distance from the mid of end points to the on-curve point, and the
   distance of the on-curve point to the control point.
*/

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkDConic {
    pub(crate) pts: SkDQuad,
    pub(crate) weight: f32,
}

impl Index<usize> for SkDConic {
    type Output = SkDPoint;

    fn index(&self, n: usize) -> &SkDPoint {
        &self.pts[n]
    }
}

impl SkDConic {
    pub(crate) const K_POINT_COUNT: i32 = 3;
    pub(crate) const K_POINT_LAST: i32 = SkDConic::K_POINT_COUNT - 1;
    pub(crate) const K_MAX_INTERSECTIONS: i32 = 4;

    pub(crate) fn collapsed(&self) -> bool {
        self.pts.collapsed()
    }

    pub(crate) fn controls_inside(&self) -> bool {
        self.pts.controls_inside()
    }

    pub(crate) fn set(&mut self, pts: &[SkPoint], weight: f32) -> &SkDConic {
        self.pts.set(pts);
        self.weight = weight;
        self
    }

    pub(crate) fn dxdy_at_t(&self, t: f64) -> SkDVector {
        let coords = self.pts.coords();
        let mut result = SkDVector {
            x: conic_eval_tan(&coords[0..], self.weight, t),
            y: conic_eval_tan(&coords[1..], self.weight, t),
        };
        if result.x == 0.0 && result.y == 0.0 {
            if zero_or_one(t) {
                result = self.pts[2] - self.pts[0];
            } else {
                // incomplete
                // Skia prints "!k" with SkDebugf here.
            }
        }
        result
    }

    pub(crate) fn find_extrema(src: &[f64], weight: f32, t_value: &mut [f64]) -> i32 {
        let t = t_value;
        let mut coeff = [0.0; 3];
        conic_deriv_coeff(src, weight, &mut coeff);

        let mut t_values = [0.0; 2];
        let roots = SkDQuad::roots_valid_t(coeff[0], coeff[1], coeff[2], &mut t_values);
        // In extreme cases, the number of roots returned can be 2. Pathops
        // will fail later on, so there's no advantage to plumbing in an error
        // return here.

        if 1 == roots {
            t[0] = t_values[0];
            return 1;
        }
        0
    }

    pub(crate) fn hull_intersects_quad(&self, quad: &SkDQuad, is_linear: &mut bool) -> bool {
        self.pts.hull_intersects_quad(quad, is_linear)
    }

    pub(crate) fn hull_intersects_conic(&self, conic: &SkDConic, is_linear: &mut bool) -> bool {
        self.pts.hull_intersects_quad(&conic.pts, is_linear)
    }

    pub(crate) fn hull_intersects_cubic(&self, cubic: &SkDCubic, is_linear: &mut bool) -> bool {
        cubic.hull_intersects_conic(self, is_linear)
    }

    pub(crate) fn monotonic_in_x(&self) -> bool {
        self.pts.monotonic_in_x()
    }

    pub(crate) fn monotonic_in_y(&self) -> bool {
        self.pts.monotonic_in_y()
    }

    pub(crate) fn other_pts(&self, odd_man: i32, end_pt: &mut [SkDPoint]) {
        self.pts.other_pts(odd_man, end_pt);
    }

    pub(crate) fn pt_at_t(&self, t: f64) -> SkDPoint {
        if t == 0.0 {
            return self.pts[0];
        }
        if t == 1.0 {
            return self.pts[2];
        }
        let coords = self.pts.coords();
        let denominator = conic_eval_denominator(self.weight, t);
        SkDPoint {
            x: sk_ieee_double_divide(
                conic_eval_numerator(&coords[0..], self.weight, t),
                denominator,
            ),
            y: sk_ieee_double_divide(
                conic_eval_numerator(&coords[1..], self.weight, t),
                denominator,
            ),
        }
    }

    pub(crate) fn sub_divide(&self, t1: f64, t2: f64) -> SkDConic {
        let coords = self.pts.coords();
        let (px, py) = (&coords[0..], &coords[1..]);
        let ax;
        let ay;
        let az;
        if t1 == 0.0 {
            ax = self.pts[0].x;
            ay = self.pts[0].y;
            az = 1.0;
        } else if t1 != 1.0 {
            ax = conic_eval_numerator(px, self.weight, t1);
            ay = conic_eval_numerator(py, self.weight, t1);
            az = conic_eval_denominator(self.weight, t1);
        } else {
            ax = self.pts[2].x;
            ay = self.pts[2].y;
            az = 1.0;
        }
        let mid_t = (t1 + t2) / 2.0;
        let dx = conic_eval_numerator(px, self.weight, mid_t);
        let dy = conic_eval_numerator(py, self.weight, mid_t);
        let dz = conic_eval_denominator(self.weight, mid_t);
        let cx;
        let cy;
        let cz;
        if t2 == 1.0 {
            cx = self.pts[2].x;
            cy = self.pts[2].y;
            cz = 1.0;
        } else if t2 != 0.0 {
            cx = conic_eval_numerator(px, self.weight, t2);
            cy = conic_eval_numerator(py, self.weight, t2);
            cz = conic_eval_denominator(self.weight, t2);
        } else {
            cx = self.pts[0].x;
            cy = self.pts[0].y;
            cz = 1.0;
        }
        let bx = 2.0 * dx - (ax + cx) / 2.0;
        let by = 2.0 * dy - (ay + cy) / 2.0;
        let mut bz = 2.0 * dz - (az + cz) / 2.0;
        if bz == 0.0 {
            bz = 1.0; // if bz is 0, weight is 0, control point has no effect: any value will do
        }
        SkDConic {
            pts: SkDQuad {
                pts: [
                    SkDPoint {
                        x: ax / az,
                        y: ay / az,
                    },
                    SkDPoint {
                        x: bx / bz,
                        y: by / bz,
                    },
                    SkDPoint {
                        x: cx / cz,
                        y: cy / cz,
                    },
                ],
            },
            weight: (bz / (az * cz).sqrt()) as f32,
        }
    }

    /// `subDivide(const SkDPoint& a, const SkDPoint& c, double t1, double t2, SkScalar* weight)`.
    pub(crate) fn sub_divide_control(
        &self,
        a: &SkDPoint,
        c: &SkDPoint,
        t1: f64,
        t2: f64,
        weight: &mut f32,
    ) -> SkDPoint {
        let _ = (a, c); // Skia ignores them too
        let chopped = self.sub_divide(t1, t2);
        *weight = chopped.weight;
        chopped[1]
    }

    /// `static SubDivide(const SkPoint pts[kPointCount], SkScalar weight, const SkDPoint& a,
    /// const SkDPoint& c, double t1, double t2, SkScalar* newWeight)`.
    pub(crate) fn sub_divide_pts_control(
        pts: &[SkPoint],
        weight: f32,
        a: &SkDPoint,
        c: &SkDPoint,
        t1: f64,
        t2: f64,
        new_weight: &mut f32,
    ) -> SkDPoint {
        let mut conic = SkDConic::default();
        conic.set(pts, weight);
        conic.sub_divide_control(a, c, t1, t2, new_weight)
    }
}

/// `SkTConic`: an `SkDConic` as an `SkTCurve`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkTConic {
    pub(crate) conic: SkDConic,
}

impl SkTConic {
    pub(crate) fn new(c: &SkDConic) -> SkTConic {
        SkTConic { conic: *c }
    }

    pub(crate) fn collapsed(&self) -> bool {
        self.conic.collapsed()
    }

    pub(crate) fn controls_inside(&self) -> bool {
        self.conic.controls_inside()
    }

    pub(crate) fn dxdy_at_t(&self, t: f64) -> SkDVector {
        self.conic.dxdy_at_t(t)
    }

    pub(crate) fn hull_intersects_quad(&self, quad: &SkDQuad, is_linear: &mut bool) -> bool {
        quad.hull_intersects_conic(&self.conic, is_linear)
    }

    pub(crate) fn hull_intersects_conic(&self, conic: &SkDConic, is_linear: &mut bool) -> bool {
        conic.hull_intersects_conic(&self.conic, is_linear)
    }

    pub(crate) fn hull_intersects_cubic(&self, cubic: &SkDCubic, is_linear: &mut bool) -> bool {
        cubic.hull_intersects_conic(&self.conic, is_linear)
    }

    pub(crate) fn hull_intersects_t_curve(&self, curve: &SkTCurve, is_linear: &mut bool) -> bool {
        curve.hull_intersects_conic(&self.conic, is_linear)
    }

    pub(crate) fn intersect_ray(&self, i: &mut SkIntersections, line: &SkDLine) -> i32 {
        i.intersect_ray_conic(&self.conic, line)
    }

    pub(crate) fn max_intersections(&self) -> i32 {
        SkDConic::K_MAX_INTERSECTIONS
    }

    pub(crate) fn other_pts(&self, odd_man: i32, end_pt: &mut [SkDPoint]) {
        self.conic.other_pts(odd_man, end_pt);
    }

    pub(crate) fn point_count(&self) -> i32 {
        SkDConic::K_POINT_COUNT
    }

    pub(crate) fn point_last(&self) -> i32 {
        SkDConic::K_POINT_LAST
    }

    pub(crate) fn pt_at_t(&self, t: f64) -> SkDPoint {
        self.conic.pt_at_t(t)
    }

    pub(crate) fn set_bounds(&self, rect: &mut SkDRect) {
        rect.set_bounds_conic(&self.conic);
    }
}
