//! Port of Skia's src/pathops/SkPathOpsTCurve.h: the curve a TSect intersects.
//!
//! Skia's `SkTCurve` is an abstract class with three subclasses (`SkTQuad`, `SkTConic`,
//! `SkTCubic`) that the TSect allocates on its heap and calls through virtual methods. Here it
//! is an enum of the three, held by value; each method dispatches to the subclass's method of
//! the same name, so a virtual call is a `match`.

use std::ops::Index;

use crate::intersections::SkIntersections;
use crate::path_ops_conic::{SkDConic, SkTConic};
use crate::path_ops_cubic::{SkDCubic, SkTCubic};
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::{SkDPoint, SkDVector};
use crate::path_ops_quad::{SkDQuad, SkTQuad};
use crate::path_ops_rect::SkDRect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SkTCurve {
    Quad(SkTQuad),
    Conic(SkTConic),
    Cubic(SkTCubic),
}

impl Index<usize> for SkTCurve {
    type Output = SkDPoint;

    fn index(&self, n: usize) -> &SkDPoint {
        match self {
            SkTCurve::Quad(c) => &c.quad[n],
            SkTCurve::Conic(c) => &c.conic[n],
            SkTCurve::Cubic(c) => &c.cubic[n],
        }
    }
}

impl SkTCurve {
    pub(crate) fn collapsed(&self) -> bool {
        match self {
            SkTCurve::Quad(c) => c.collapsed(),
            SkTCurve::Conic(c) => c.collapsed(),
            SkTCurve::Cubic(c) => c.collapsed(),
        }
    }

    pub(crate) fn controls_inside(&self) -> bool {
        match self {
            SkTCurve::Quad(c) => c.controls_inside(),
            SkTCurve::Conic(c) => c.controls_inside(),
            SkTCurve::Cubic(c) => c.controls_inside(),
        }
    }

    pub(crate) fn dxdy_at_t(&self, t: f64) -> SkDVector {
        match self {
            SkTCurve::Quad(c) => c.dxdy_at_t(t),
            SkTCurve::Conic(c) => c.dxdy_at_t(t),
            SkTCurve::Cubic(c) => c.dxdy_at_t(t),
        }
    }

    pub(crate) fn hull_intersects_quad(&self, quad: &SkDQuad, is_linear: &mut bool) -> bool {
        match self {
            SkTCurve::Quad(c) => c.hull_intersects_quad(quad, is_linear),
            SkTCurve::Conic(c) => c.hull_intersects_quad(quad, is_linear),
            SkTCurve::Cubic(c) => c.hull_intersects_quad(quad, is_linear),
        }
    }

    pub(crate) fn hull_intersects_conic(&self, conic: &SkDConic, is_linear: &mut bool) -> bool {
        match self {
            SkTCurve::Quad(c) => c.hull_intersects_conic(conic, is_linear),
            SkTCurve::Conic(c) => c.hull_intersects_conic(conic, is_linear),
            SkTCurve::Cubic(c) => c.hull_intersects_conic(conic, is_linear),
        }
    }

    pub(crate) fn hull_intersects_cubic(&self, cubic: &SkDCubic, is_linear: &mut bool) -> bool {
        match self {
            SkTCurve::Quad(c) => c.hull_intersects_cubic(cubic, is_linear),
            SkTCurve::Conic(c) => c.hull_intersects_cubic(cubic, is_linear),
            SkTCurve::Cubic(c) => c.hull_intersects_cubic(cubic, is_linear),
        }
    }

    pub(crate) fn hull_intersects_t_curve(&self, curve: &SkTCurve, is_linear: &mut bool) -> bool {
        match self {
            SkTCurve::Quad(c) => c.hull_intersects_t_curve(curve, is_linear),
            SkTCurve::Conic(c) => c.hull_intersects_t_curve(curve, is_linear),
            SkTCurve::Cubic(c) => c.hull_intersects_t_curve(curve, is_linear),
        }
    }

    pub(crate) fn intersect_ray(&self, i: &mut SkIntersections, line: &SkDLine) -> i32 {
        match self {
            SkTCurve::Quad(c) => c.intersect_ray(i, line),
            SkTCurve::Conic(c) => c.intersect_ray(i, line),
            SkTCurve::Cubic(c) => c.intersect_ray(i, line),
        }
    }

    pub(crate) fn is_conic(&self) -> bool {
        matches!(self, SkTCurve::Conic(_))
    }

    /// `make(SkArenaAlloc&)`: a new, default curve of the same kind.
    pub(crate) fn make(&self) -> SkTCurve {
        match self {
            SkTCurve::Quad(_) => SkTCurve::Quad(SkTQuad::default()),
            SkTCurve::Conic(_) => SkTCurve::Conic(SkTConic::default()),
            SkTCurve::Cubic(_) => SkTCurve::Cubic(SkTCubic::default()),
        }
    }

    pub(crate) fn max_intersections(&self) -> i32 {
        match self {
            SkTCurve::Quad(c) => c.max_intersections(),
            SkTCurve::Conic(c) => c.max_intersections(),
            SkTCurve::Cubic(c) => c.max_intersections(),
        }
    }

    pub(crate) fn other_pts(&self, odd_man: i32, end_pt: &mut [SkDPoint]) {
        match self {
            SkTCurve::Quad(c) => c.other_pts(odd_man, end_pt),
            SkTCurve::Conic(c) => c.other_pts(odd_man, end_pt),
            SkTCurve::Cubic(c) => c.other_pts(odd_man, end_pt),
        }
    }

    pub(crate) fn point_count(&self) -> i32 {
        match self {
            SkTCurve::Quad(c) => c.point_count(),
            SkTCurve::Conic(c) => c.point_count(),
            SkTCurve::Cubic(c) => c.point_count(),
        }
    }

    pub(crate) fn point_last(&self) -> i32 {
        match self {
            SkTCurve::Quad(c) => c.point_last(),
            SkTCurve::Conic(c) => c.point_last(),
            SkTCurve::Cubic(c) => c.point_last(),
        }
    }

    pub(crate) fn pt_at_t(&self, t: f64) -> SkDPoint {
        match self {
            SkTCurve::Quad(c) => c.pt_at_t(t),
            SkTCurve::Conic(c) => c.pt_at_t(t),
            SkTCurve::Cubic(c) => c.pt_at_t(t),
        }
    }

    pub(crate) fn set_bounds(&self, rect: &mut SkDRect) {
        match self {
            SkTCurve::Quad(c) => c.set_bounds(rect),
            SkTCurve::Conic(c) => c.set_bounds(rect),
            SkTCurve::Cubic(c) => c.set_bounds(rect),
        }
    }

    /// Skia casts `curve` to this curve's subclass and assigns its curve; `curve` was made by
    /// this curve's `make`, so it is of the same kind and is replaced whole here.
    pub(crate) fn sub_divide(&self, t1: f64, t2: f64, curve: &mut SkTCurve) {
        *curve = match self {
            SkTCurve::Quad(c) => SkTCurve::Quad(SkTQuad {
                quad: c.quad.sub_divide(t1, t2),
            }),
            SkTCurve::Conic(c) => SkTCurve::Conic(SkTConic {
                conic: c.conic.sub_divide(t1, t2),
            }),
            SkTCurve::Cubic(c) => SkTCurve::Cubic(SkTCubic {
                cubic: c.cubic.sub_divide(t1, t2),
            }),
        };
    }
}
