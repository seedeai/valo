//! Port of Skia's src/pathops/SkIntersections.{h,cpp}: the intersections found between two
//! curves, as t values on each and the points they meet at.
//!
//! The header declares the intersections of each pair of curve kinds; their bodies live in
//! the files that compute them (`d_line_intersection.rs`, `d_quad_line_intersection.rs`,
//! `d_conic_line_intersection.rs`, `d_cubic_line_intersection.rs`, and `path_ops_t_sect.rs`
//! for curve-curve), each in an `impl SkIntersections` of its own. Declarations the header
//! makes that no source file defines are left out.

use std::ops::Index;

use crate::core::float_bits::SK_SCALAR_MAX;
use crate::core::point::SkPoint;
use crate::path_ops_conic::SkDConic;
use crate::path_ops_cubic::SkDCubic;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::SkDPoint;
use crate::path_ops_quad::SkDQuad;
use crate::path_ops_t_curve::SkTCurve;
use crate::path_ops_types::{between, more_roughly_equal, precisely_equal, precisely_zero};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SkIntersections {
    /// FIXME: since scans store points as SkPoint, this should also
    pub(crate) pt: [SkDPoint; 13],
    /// used by nearly same to store alternate intersection point
    pub(crate) pt2: [SkDPoint; 2],
    pub(crate) t: [[f64; 13]; 2],
    /// bit set for each curve's coincident T
    pub(crate) is_coincident: [u16; 2],
    /// true if end points nearly match
    pub(crate) nearly_same: [bool; 2],
    pub(crate) used: u8,
    pub(crate) max: u8,
    pub(crate) allow_near: bool,
    pub(crate) swap: bool,
}

/// `operator[](int n)` gives Skia's `TArray` over `fT[n]`, so `i[0][k]` is `fT[0][k]`.
impl Index<usize> for SkIntersections {
    type Output = [f64; 13];

    fn index(&self, n: usize) -> &[f64; 13] {
        &self.t[n]
    }
}

impl SkIntersections {
    pub(crate) fn new() -> SkIntersections {
        let mut result = SkIntersections {
            pt: [SkDPoint::default(); 13],
            pt2: [SkDPoint::default(); 2],
            t: [[0.0; 13]; 2],
            is_coincident: [0; 2],
            nearly_same: [false; 2],
            used: 0,
            max: 0,
            allow_near: false,
            swap: false,
        };
        result.reset();
        result.max = 0; // require that the caller set the max
        result
    }

    pub(crate) fn clear_coincidence(&mut self, index: i32) {
        debug_assert!(index >= 0);
        let bit = 1 << index;
        self.is_coincident[0] &= !bit;
        self.is_coincident[1] &= !bit;
    }

    pub(crate) fn conic_horizontal(
        &mut self,
        a: &[SkPoint],
        weight: f32,
        left: f32,
        right: f32,
        y: f32,
        flipped: bool,
    ) -> i32 {
        let mut conic = SkDConic::default();
        conic.set(a, weight);
        self.max = 2;
        self.horizontal_conic(&conic, left as f64, right as f64, y as f64, flipped)
    }

    pub(crate) fn conic_vertical(
        &mut self,
        a: &[SkPoint],
        weight: f32,
        top: f32,
        bottom: f32,
        x: f32,
        flipped: bool,
    ) -> i32 {
        let mut conic = SkDConic::default();
        conic.set(a, weight);
        self.max = 2;
        self.vertical_conic(&conic, top as f64, bottom as f64, x as f64, flipped)
    }

    pub(crate) fn conic_line(&mut self, a: &[SkPoint], weight: f32, b: &[SkPoint]) -> i32 {
        let mut conic = SkDConic::default();
        conic.set(a, weight);
        let mut line = SkDLine::default();
        line.set(b);
        self.max = 3; // 2;  permit small coincident segment + non-coincident intersection
        self.intersect_conic_line(&conic, &line)
    }

    pub(crate) fn cubic_horizontal(
        &mut self,
        a: &[SkPoint],
        left: f32,
        right: f32,
        y: f32,
        flipped: bool,
    ) -> i32 {
        let mut cubic = SkDCubic::default();
        cubic.set(a);
        self.max = 3;
        self.horizontal_cubic(&cubic, left as f64, right as f64, y as f64, flipped)
    }

    pub(crate) fn cubic_vertical(
        &mut self,
        a: &[SkPoint],
        top: f32,
        bottom: f32,
        x: f32,
        flipped: bool,
    ) -> i32 {
        let mut cubic = SkDCubic::default();
        cubic.set(a);
        self.max = 3;
        self.vertical_cubic(&cubic, top as f64, bottom as f64, x as f64, flipped)
    }

    pub(crate) fn cubic_line(&mut self, a: &[SkPoint], b: &[SkPoint]) -> i32 {
        let mut cubic = SkDCubic::default();
        cubic.set(a);
        let mut line = SkDLine::default();
        line.set(b);
        self.max = 3;
        self.intersect_cubic_line(&cubic, &line)
    }

    pub(crate) fn has_t(&self, t: f64) -> bool {
        debug_assert!(t == 0.0 || t == 1.0);
        self.used > 0
            && (if t == 0.0 {
                self.t[0][0] == 0.0
            } else {
                self.t[0][self.used as usize - 1] == 1.0
            })
    }

    pub(crate) fn has_opp_t(&self, t: f64) -> bool {
        debug_assert!(t == 0.0 || t == 1.0);
        self.used > 0 && (self.t[1][0] == t || self.t[1][self.used as usize - 1] == t)
    }

    pub(crate) fn insert_swap(&mut self, one: f64, two: f64, pt: &SkDPoint) -> i32 {
        if self.swap {
            self.insert(two, one, pt)
        } else {
            self.insert(one, two, pt)
        }
    }

    pub(crate) fn is_coincident(&self, index: i32) -> bool {
        (self.is_coincident[0] & (1 << index)) != 0
    }

    pub(crate) fn line_horizontal(
        &mut self,
        a: &[SkPoint],
        left: f32,
        right: f32,
        y: f32,
        flipped: bool,
    ) -> i32 {
        let mut line = SkDLine::default();
        line.set(a);
        self.max = 2;
        self.horizontal_line(&line, left as f64, right as f64, y as f64, flipped)
    }

    pub(crate) fn line_vertical(
        &mut self,
        a: &[SkPoint],
        top: f32,
        bottom: f32,
        x: f32,
        flipped: bool,
    ) -> i32 {
        let mut line = SkDLine::default();
        line.set(a);
        self.max = 2;
        self.vertical_line(&line, top as f64, bottom as f64, x as f64, flipped)
    }

    pub(crate) fn line_line(&mut self, a: &[SkPoint], b: &[SkPoint]) -> i32 {
        let mut a_line = SkDLine::default();
        let mut b_line = SkDLine::default();
        a_line.set(a);
        b_line.set(b);
        self.max = 2;
        self.intersect_line_line(&a_line, &b_line)
    }

    pub(crate) fn pt(&self, index: usize) -> &SkDPoint {
        &self.pt[index]
    }

    pub(crate) fn quad_horizontal(
        &mut self,
        a: &[SkPoint],
        left: f32,
        right: f32,
        y: f32,
        flipped: bool,
    ) -> i32 {
        let mut quad = SkDQuad::default();
        quad.set(a);
        self.max = 2;
        self.horizontal_quad(&quad, left as f64, right as f64, y as f64, flipped)
    }

    pub(crate) fn quad_vertical(
        &mut self,
        a: &[SkPoint],
        top: f32,
        bottom: f32,
        x: f32,
        flipped: bool,
    ) -> i32 {
        let mut quad = SkDQuad::default();
        quad.set(a);
        self.max = 2;
        self.vertical_quad(&quad, top as f64, bottom as f64, x as f64, flipped)
    }

    pub(crate) fn quad_line(&mut self, a: &[SkPoint], b: &[SkPoint]) -> i32 {
        let mut quad = SkDQuad::default();
        quad.set(a);
        let mut line = SkDLine::default();
        line.set(b);
        self.intersect_quad_line(&quad, &line)
    }

    // leaves swap, max alone
    pub(crate) fn reset(&mut self) {
        self.allow_near = true;
        self.used = 0;
        self.is_coincident = [0; 2];
    }

    pub(crate) fn set_max(&mut self, max: i32) {
        debug_assert!(max <= self.pt.len() as i32);
        self.max = max as u8;
    }

    pub(crate) fn used(&self) -> i32 {
        self.used as i32
    }

    /// `intersectRay(const SkTCurve& tCurve, const SkDLine& line)`.
    pub(crate) fn intersect_ray_t_curve(&mut self, t_curve: &SkTCurve, line: &SkDLine) -> i32 {
        t_curve.intersect_ray(self, line)
    }

    pub(crate) fn closest_to(
        &self,
        range_start: f64,
        range_end: f64,
        test_pt: &SkDPoint,
        dist: &mut f64,
    ) -> i32 {
        let closest_dist = dist;
        let mut closest = -1;
        *closest_dist = SK_SCALAR_MAX as f64;
        for index in 0..self.used as usize {
            if !between(range_start, self.t[0][index], range_end) {
                continue;
            }
            let i_pt = &self.pt[index];
            let dist = test_pt.distance_squared(i_pt);
            if *closest_dist > dist {
                *closest_dist = dist;
                closest = index as i32;
            }
        }
        closest
    }

    pub(crate) fn flip(&mut self) {
        for index in 0..self.used as usize {
            self.t[1][index] = 1.0 - self.t[1][index];
        }
    }

    // FIXME : does not respect swap
    // `one < 0 || one > 1` is false for NaN, unlike `!(0..=1).contains`.
    #[allow(clippy::manual_range_contains)]
    pub(crate) fn insert(&mut self, one: f64, two: f64, pt: &SkDPoint) -> i32 {
        if self.is_coincident[0] == 3 && between(self.t[0][0], one, self.t[0][1]) {
            // For now, don't allow a mix of coincident and non-coincident intersections
            return -1;
        }
        debug_assert!(self.used <= 1 || self.t[0][0] <= self.t[0][1]);
        let mut index: usize = 0;
        while index < self.used as usize {
            let old_one = self.t[0][index];
            let old_two = self.t[1][index];
            if one == old_one && two == old_two {
                return -1;
            }
            if more_roughly_equal(old_one, one) && more_roughly_equal(old_two, two) {
                if (!precisely_zero(one) || precisely_zero(old_one))
                    && (!precisely_equal(one, 1.0) || precisely_equal(old_one, 1.0))
                    && (!precisely_zero(two) || precisely_zero(old_two))
                    && (!precisely_equal(two, 1.0) || precisely_equal(old_two, 1.0))
                {
                    return -1;
                }
                debug_assert!((0.0..=1.0).contains(&one));
                debug_assert!((0.0..=1.0).contains(&two));
                // remove this and reinsert below in case replacing would make list unsorted
                let remaining = self.used as usize - index - 1;
                self.pt.copy_within(index + 1..index + 1 + remaining, index);
                self.t[0].copy_within(index + 1..index + 1 + remaining, index);
                self.t[1].copy_within(index + 1..index + 1 + remaining, index);
                let clear_mask: i32 = !((1 << index) - 1);
                self.is_coincident[0] = (self.is_coincident[0] as i32
                    - ((self.is_coincident[0] as i32 >> 1) & clear_mask))
                    as u16;
                self.is_coincident[1] = (self.is_coincident[1] as i32
                    - ((self.is_coincident[1] as i32 >> 1) & clear_mask))
                    as u16;
                self.used -= 1;
                break;
            }
            index += 1;
        }
        index = 0;
        while index < self.used as usize {
            if self.t[0][index] > one {
                break;
            }
            index += 1;
        }
        if self.used >= self.max {
            // FIXME : this error, if it is to be handled at runtime in release, must
            // be propagated all the way back down to the caller, and return failure.
            self.used = 0;
            return 0;
        }
        let remaining = self.used as i32 - index as i32;
        if remaining > 0 {
            let remaining = remaining as usize;
            self.pt.copy_within(index..index + remaining, index + 1);
            self.t[0].copy_within(index..index + remaining, index + 1);
            self.t[1].copy_within(index..index + remaining, index + 1);
            let clear_mask: i32 = !((1 << index) - 1);
            self.is_coincident[0] =
                (self.is_coincident[0] as i32 + (self.is_coincident[0] as i32 & clear_mask)) as u16;
            self.is_coincident[1] =
                (self.is_coincident[1] as i32 + (self.is_coincident[1] as i32 & clear_mask)) as u16;
        }
        self.pt[index] = *pt;
        if one < 0.0 || one > 1.0 {
            return -1;
        }
        if two < 0.0 || two > 1.0 {
            return -1;
        }
        self.t[0][index] = one;
        self.t[1][index] = two;
        self.used += 1;
        debug_assert!(self.used as usize <= self.pt.len());
        index as i32
    }

    pub(crate) fn insert_near(&mut self, one: f64, two: f64, pt1: &SkDPoint, pt2: &SkDPoint) {
        debug_assert!(one == 0.0 || one == 1.0);
        debug_assert!(two == 0.0 || two == 1.0);
        debug_assert!(pt1 != pt2);
        self.nearly_same[if one != 0.0 { 1 } else { 0 }] = true;
        let _ = self.insert(one, two, pt1);
        self.pt2[if one != 0.0 { 1 } else { 0 }] = *pt2;
    }

    // start if index == 0 : end if index == 1
    pub(crate) fn insert_coincident(&mut self, one: f64, two: f64, pt: &SkDPoint) -> i32 {
        let index = self.insert_swap(one, two, pt);
        if index >= 0 {
            self.set_coincident(index);
        }
        index
    }

    pub(crate) fn merge(
        &mut self,
        a: &SkIntersections,
        a_index: i32,
        b: &SkIntersections,
        b_index: i32,
    ) {
        let (a_index, b_index) = (a_index as usize, b_index as usize);
        self.reset();
        self.t[0][0] = a.t[0][a_index];
        self.t[1][0] = b.t[0][b_index];
        self.pt[0] = a.pt[a_index];
        self.pt2[0] = b.pt[b_index];
        self.used = 1;
    }

    pub(crate) fn most_outside(&self, range_start: f64, range_end: f64, origin: &SkDPoint) -> i32 {
        let mut result: i32 = -1;
        for index in 0..self.used as usize {
            if !between(range_start, self.t[0][index], range_end) {
                continue;
            }
            if result < 0 {
                result = index as i32;
                continue;
            }
            let best = self.pt[result as usize] - *origin;
            let test = self.pt[index] - *origin;
            if test.cross_check(&best) < 0.0 {
                result = index as i32;
            }
        }
        result
    }

    pub(crate) fn remove_one(&mut self, index: i32) {
        self.used = self.used.wrapping_sub(1);
        let remaining = self.used as i32 - index;
        if remaining <= 0 {
            return;
        }
        let (index, remaining) = (index as usize, remaining as usize);
        self.pt.copy_within(index + 1..index + 1 + remaining, index);
        self.t[0].copy_within(index + 1..index + 1 + remaining, index);
        self.t[1].copy_within(index + 1..index + 1 + remaining, index);
        let co_bit = self.is_coincident[0] as i32 & (1 << index);
        self.is_coincident[0] = (self.is_coincident[0] as i32
            - (((self.is_coincident[0] as i32 >> 1) & !((1 << index) - 1)) + co_bit))
            as u16;
        debug_assert!((co_bit ^ (self.is_coincident[1] as i32 & (1 << index))) == 0);
        self.is_coincident[1] = (self.is_coincident[1] as i32
            - (((self.is_coincident[1] as i32 >> 1) & !((1 << index) - 1)) + co_bit))
            as u16;
    }

    pub(crate) fn set_coincident(&mut self, index: i32) {
        debug_assert!(index >= 0);
        let bit = 1 << index;
        self.is_coincident[0] = (self.is_coincident[0] as i32 | bit) as u16;
        self.is_coincident[1] = (self.is_coincident[1] as i32 | bit) as u16;
    }
}
