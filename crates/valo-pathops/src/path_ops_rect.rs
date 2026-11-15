//! Port of Skia's src/pathops/SkPathOpsRect.{h,cpp}: a rectangle in doubles, and the bounds of
//! the curves.

// Skia indexes its arrays; the loops keep its indices.
#![allow(clippy::needless_range_loop)]

use crate::core::float_bits::{std_max, std_min};
use crate::path_ops_conic::SkDConic;
use crate::path_ops_cubic::SkDCubic;
use crate::path_ops_point::SkDPoint;
use crate::path_ops_quad::SkDQuad;
use crate::path_ops_t_curve::SkTCurve;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkDRect {
    pub(crate) left: f64,
    pub(crate) top: f64,
    pub(crate) right: f64,
    pub(crate) bottom: f64,
}

impl SkDRect {
    pub(crate) fn add(&mut self, pt: &SkDPoint) {
        self.left = std_min(self.left, pt.x);
        self.top = std_min(self.top, pt.y);
        self.right = std_max(self.right, pt.x);
        self.bottom = std_max(self.bottom, pt.y);
    }

    pub(crate) fn intersects(&self, r: &SkDRect) -> bool {
        debug_assert!(self.left <= self.right);
        debug_assert!(self.top <= self.bottom);
        debug_assert!(r.left <= r.right);
        debug_assert!(r.top <= r.bottom);
        r.left <= self.right && self.left <= r.right && r.top <= self.bottom && self.top <= r.bottom
    }

    pub(crate) fn set(&mut self, pt: &SkDPoint) {
        self.left = pt.x;
        self.right = pt.x;
        self.top = pt.y;
        self.bottom = pt.y;
    }

    pub(crate) fn width(&self) -> f64 {
        self.right - self.left
    }

    pub(crate) fn height(&self) -> f64 {
        self.bottom - self.top
    }

    /// `setBounds(const SkDConic&)`.
    pub(crate) fn set_bounds_conic(&mut self, curve: &SkDConic) {
        self.set_bounds_conic_part(curve, curve, 0.0, 1.0);
    }

    /// `setBounds(const SkDConic& curve, const SkDConic& sub, double tStart, double tEnd)`.
    pub(crate) fn set_bounds_conic_part(
        &mut self,
        curve: &SkDConic,
        sub: &SkDConic,
        t_start: f64,
        t_end: f64,
    ) {
        let (start_t, end_t) = (t_start, t_end);
        self.set(&sub[0]);
        self.add(&sub[2]);
        let mut t_values = [0.0; 2];
        let mut roots = 0;
        let coords = sub.pts.coords();
        if !sub.monotonic_in_x() {
            roots = SkDConic::find_extrema(&coords[0..], sub.weight, &mut t_values);
        }
        if !sub.monotonic_in_y() {
            roots +=
                SkDConic::find_extrema(&coords[1..], sub.weight, &mut t_values[roots as usize..]);
        }
        for index in 0..roots as usize {
            let t = start_t + (end_t - start_t) * t_values[index];
            self.add(&curve.pt_at_t(t));
        }
    }

    /// `setBounds(const SkDCubic&)`.
    pub(crate) fn set_bounds_cubic(&mut self, curve: &SkDCubic) {
        self.set_bounds_cubic_part(curve, curve, 0.0, 1.0);
    }

    /// `setBounds(const SkDCubic& curve, const SkDCubic& sub, double tStart, double tEnd)`.
    pub(crate) fn set_bounds_cubic_part(
        &mut self,
        curve: &SkDCubic,
        sub: &SkDCubic,
        t_start: f64,
        t_end: f64,
    ) {
        let (start_t, end_t) = (t_start, t_end);
        self.set(&sub[0]);
        self.add(&sub[3]);
        let mut t_values = [0.0; 4];
        let mut roots = 0;
        let coords = sub.coords();
        if !sub.monotonic_in_x() {
            roots = SkDCubic::find_extrema(&coords[0..], &mut t_values);
        }
        if !sub.monotonic_in_y() {
            roots += SkDCubic::find_extrema(&coords[1..], &mut t_values[roots as usize..]);
        }
        for index in 0..roots as usize {
            let t = start_t + (end_t - start_t) * t_values[index];
            self.add(&curve.pt_at_t(t));
        }
    }

    /// `setBounds(const SkDQuad&)`.
    pub(crate) fn set_bounds_quad(&mut self, curve: &SkDQuad) {
        self.set_bounds_quad_part(curve, curve, 0.0, 1.0);
    }

    /// `setBounds(const SkDQuad& curve, const SkDQuad& sub, double tStart, double tEnd)`.
    pub(crate) fn set_bounds_quad_part(
        &mut self,
        curve: &SkDQuad,
        sub: &SkDQuad,
        t_start: f64,
        t_end: f64,
    ) {
        let (start_t, end_t) = (t_start, t_end);
        self.set(&sub[0]);
        self.add(&sub[2]);
        let mut t_values = [0.0; 2];
        let mut roots = 0;
        let coords = sub.coords();
        if !sub.monotonic_in_x() {
            roots = SkDQuad::find_extrema(&coords[0..], &mut t_values);
        }
        if !sub.monotonic_in_y() {
            roots += SkDQuad::find_extrema(&coords[1..], &mut t_values[roots as usize..]);
        }
        for index in 0..roots as usize {
            let t = start_t + (end_t - start_t) * t_values[index];
            self.add(&curve.pt_at_t(t));
        }
    }

    /// `setBounds(const SkTCurve&)`.
    pub(crate) fn set_bounds_t_curve(&mut self, curve: &SkTCurve) {
        curve.set_bounds(self);
    }

    pub(crate) fn valid(&self) -> bool {
        self.left <= self.right && self.top <= self.bottom
    }
}
