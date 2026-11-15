//! Port of Skia's src/pathops/SkPathOpsTSect.{h,cpp}: the intersection of two curves by
//! repeatedly splitting each into spans of t and discarding the spans whose hulls cannot meet
//! the other curve's.
//!
//! Skia's `SkTSpan`s point at each other (the span list of a sect, and each span's `fBounded`
//! list of the opposite sect's spans it may meet), and each sect allocates spans and bounded
//! nodes from its own arena, sometimes for the other sect's spans. Here both sects of one
//! intersection share one `SkTSectHeap`, `SkTSpan` and `SkTSpanBounded` are handles into it,
//! and every method takes the heap explicitly; which sect's arena Skia used changes nothing.
//! A span's curve part (`fPart`, made by `SkTCurve::make` on the heap) is held by value.
//! `binarySearchCoin`'s local `SkTSpan work` is made in the heap too, unlinked, as Skia's
//! local only ever is.
//!
//! An `SkTSect` holds a copy of the curve Skia holds by const reference.
// Skia indexes its arrays in its loops; the port keeps the indices.
#![allow(clippy::needless_range_loop)]

use std::ops::{Index, IndexMut};

use crate::core::float_bits::{sk_is_nan, std_max, std_min, DBL_MAX, FLT_MAX, SK_SCALAR_NAN};
use crate::core::t_sort::sk_t_q_sort_by;
use crate::intersections::SkIntersections;
use crate::path_ops_conic::{SkDConic, SkTConic};
use crate::path_ops_cubic::{SkDCubic, SkTCubic};
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::{SkDPoint, SkDVector};
use crate::path_ops_quad::{SkDQuad, SkTQuad};
use crate::path_ops_rect::SkDRect;
use crate::path_ops_t_curve::SkTCurve;
use crate::path_ops_types::{
    approximately_greater_than_one, approximately_less_than_zero,
    approximately_zero_when_compared_to, arena_handle, between, precisely_zero,
    precisely_zero_when_compared_to, roughly_between,
};

/// `SkTCoincident`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SkTCoincident {
    pub(crate) perp_pt: SkDPoint,
    /// perpendicular intersection on opposite curve
    pub(crate) perp_t: f64,
    pub(crate) r#match: bool,
}

/// `SkTCoincident()` calls `init()`.
impl Default for SkTCoincident {
    fn default() -> SkTCoincident {
        let mut result = SkTCoincident {
            perp_pt: SkDPoint::default(),
            perp_t: 0.0,
            r#match: false,
        };
        result.init();
        result
    }
}

impl SkTCoincident {
    pub(crate) fn is_match(&self) -> bool {
        self.r#match
    }

    pub(crate) fn init(&mut self) {
        self.perp_t = -1.0;
        self.r#match = false;
        self.perp_pt.x = SK_SCALAR_NAN as f64;
        self.perp_pt.y = SK_SCALAR_NAN as f64;
    }

    pub(crate) fn mark_coincident(&mut self) {
        if !self.r#match {
            self.perp_t = -1.0;
        }
        self.r#match = true;
    }

    pub(crate) fn perp_pt(&self) -> &SkDPoint {
        &self.perp_pt
    }

    pub(crate) fn perp_t(&self) -> f64 {
        self.perp_t
    }

    pub(crate) fn set_perp(&mut self, c1: &SkTCurve, t: f64, c_pt: &SkDPoint, c2: &SkTCurve) {
        let dxdy = c1.dxdy_at_t(t);
        let perp = SkDLine {
            pts: [
                *c_pt,
                SkDPoint {
                    x: c_pt.x + dxdy.y,
                    y: c_pt.y - dxdy.x,
                },
            ],
        };
        let mut i = SkIntersections::new();
        let used = i.intersect_ray_t_curve(c2, &perp);
        // only keep closest
        if used == 0 || used == 3 {
            self.init();
            return;
        }
        self.perp_t = i[0][0];
        self.perp_pt = *i.pt(0);
        debug_assert!(used <= 2);
        if used == 2 {
            let dist_sq = (self.perp_pt - *c_pt).length_squared();
            let dist2_sq = (*i.pt(1) - *c_pt).length_squared();
            if dist2_sq < dist_sq {
                self.perp_t = i[0][1];
                self.perp_pt = *i.pt(1);
            }
        }
        self.r#match = c_pt.approximately_equal(&self.perp_pt);
    }
}

arena_handle! {
    /// Stands for `SkTSpanBounded*`.
    SkTSpanBounded
}

/// `SkTSpanBounded`'s fields.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkTSpanBoundedData {
    pub(crate) bounded: SkTSpan,
    pub(crate) next: Option<SkTSpanBounded>,
}

arena_handle! {
    /// Stands for `SkTSpan*`.
    SkTSpan
}

/// `SkTSpan`'s fields. Skia leaves all but `fPart` and the two `SkTCoincident`s uninitialized
/// until `init`/`splitAt`/`initBounds`; here they start at their defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SkTSpanData {
    pub(crate) part: SkTCurve,
    pub(crate) coin_start: SkTCoincident,
    pub(crate) coin_end: SkTCoincident,
    pub(crate) bounded: Option<SkTSpanBounded>,
    pub(crate) prev: Option<SkTSpan>,
    pub(crate) next: Option<SkTSpan>,
    pub(crate) bounds: SkDRect,
    pub(crate) start_t: f64,
    pub(crate) end_t: f64,
    pub(crate) bounds_max: f64,
    pub(crate) collapsed: bool,
    pub(crate) has_perp: bool,
    pub(crate) is_linear: bool,
    pub(crate) is_line: bool,
    pub(crate) deleted: bool,
}

/// The arena both sects of one intersection allocate their spans and bounded nodes from.
#[derive(Clone, Debug, Default)]
pub(crate) struct SkTSectHeap {
    pub(crate) spans: Vec<SkTSpanData>,
    pub(crate) bounded: Vec<SkTSpanBoundedData>,
}

impl Index<SkTSpan> for SkTSectHeap {
    type Output = SkTSpanData;

    fn index(&self, span: SkTSpan) -> &SkTSpanData {
        &self.spans[span.0 as usize]
    }
}

impl IndexMut<SkTSpan> for SkTSectHeap {
    fn index_mut(&mut self, span: SkTSpan) -> &mut SkTSpanData {
        &mut self.spans[span.0 as usize]
    }
}

impl Index<SkTSpanBounded> for SkTSectHeap {
    type Output = SkTSpanBoundedData;

    fn index(&self, bounded: SkTSpanBounded) -> &SkTSpanBoundedData {
        &self.bounded[bounded.0 as usize]
    }
}

impl IndexMut<SkTSpanBounded> for SkTSectHeap {
    fn index_mut(&mut self, bounded: SkTSpanBounded) -> &mut SkTSpanBoundedData {
        &mut self.bounded[bounded.0 as usize]
    }
}

impl SkTSectHeap {
    /// `heap.make<SkTSpan>(curve, heap)`: `SkTSpan(const SkTCurve&, SkArenaAlloc&)` sets only
    /// `fPart = curve.make(heap)`.
    pub(crate) fn make_span(&mut self, curve: &SkTCurve) -> SkTSpan {
        let span = SkTSpan(self.spans.len() as u32);
        self.spans.push(SkTSpanData {
            part: curve.make(),
            coin_start: SkTCoincident::default(),
            coin_end: SkTCoincident::default(),
            bounded: None,
            prev: None,
            next: None,
            bounds: SkDRect::default(),
            start_t: 0.0,
            end_t: 0.0,
            bounds_max: 0.0,
            collapsed: false,
            has_perp: false,
            is_linear: false,
            is_line: false,
            deleted: false,
        });
        span
    }

    /// `heap->make<SkTSpanBounded>()`.
    pub(crate) fn make_bounded(&mut self) -> SkTSpanBounded {
        let bounded = SkTSpanBounded(self.bounded.len() as u32);
        self.bounded.push(SkTSpanBoundedData::default());
        bounded
    }
}

impl SkTSpan {
    pub(crate) fn add_bounded(self, heap: &mut SkTSectHeap, span: SkTSpan) {
        let bounded = heap.make_bounded();
        heap[bounded].bounded = span;
        heap[bounded].next = heap[self].bounded;
        heap[self].bounded = Some(bounded);
    }

    pub(crate) fn closest_bounded_t(self, heap: &SkTSectHeap, pt: &SkDPoint) -> f64 {
        let mut result = -1.0;
        let mut closest = DBL_MAX;
        let mut test_bounded = heap[self].bounded;
        while let Some(tb) = test_bounded {
            let test = heap[tb].bounded;
            let start_dist = test.point_first(heap).distance_squared(pt);
            if closest > start_dist {
                closest = start_dist;
                result = heap[test].start_t;
            }
            let end_dist = test.point_last(heap).distance_squared(pt);
            if closest > end_dist {
                closest = end_dist;
                result = heap[test].end_t;
            }
            test_bounded = heap[tb].next;
        }
        debug_assert!(between(0.0, result, 1.0));
        result
    }

    pub(crate) fn contains(self, heap: &SkTSectHeap, t: f64) -> bool {
        let mut work = Some(self);
        while let Some(w) = work {
            if between(heap[w].start_t, t, heap[w].end_t) {
                return true;
            }
            work = heap[w].next;
        }
        false
    }

    pub(crate) fn end_t(self, heap: &SkTSectHeap) -> f64 {
        heap[self].end_t
    }

    pub(crate) fn find_opp_span(self, heap: &SkTSectHeap, opp: SkTSpan) -> Option<SkTSpan> {
        let mut bounded = heap[self].bounded;
        while let Some(b) = bounded {
            let test = heap[b].bounded;
            if opp == test {
                return Some(test);
            }
            bounded = heap[b].next;
        }
        None
    }

    pub(crate) fn find_opp_t(self, heap: &SkTSectHeap, t: f64) -> Option<SkTSpan> {
        self.opp_t(heap, t)
    }

    pub(crate) fn has_opp_t(self, heap: &SkTSectHeap, t: f64) -> bool {
        self.opp_t(heap, t).is_some()
    }

    // OPTIMIZE ? If at_most_end_pts_in_common detects that one quad is near linear,
    // use line intersection to guess a better split than 0.5
    // OPTIMIZE Once at_most_end_pts_in_common detects linear, mark span so all future splits are linear
    pub(crate) fn hulls_intersect(
        self,
        heap: &mut SkTSectHeap,
        span: SkTSpan,
        start: &mut bool,
        opp_start: &mut bool,
    ) -> i32 {
        let opp = span;
        if !heap[self].bounds.intersects(&heap[opp].bounds) {
            return 0;
        }
        let mut hull_sect = self.hull_check(heap, opp, start, opp_start);
        if hull_sect >= 0 {
            return hull_sect;
        }
        hull_sect = opp.hull_check(heap, self, opp_start, start);
        if hull_sect >= 0 {
            return hull_sect;
        }
        -1
    }

    pub(crate) fn init(self, heap: &mut SkTSectHeap, c: &SkTCurve) {
        heap[self].next = None;
        heap[self].prev = None;
        heap[self].start_t = 0.0;
        heap[self].end_t = 1.0;
        heap[self].bounded = None;
        self.reset_bounds(heap, c);
    }

    pub(crate) fn init_bounds(self, heap: &mut SkTSectHeap, c: &SkTCurve) -> bool {
        if sk_is_nan(heap[self].start_t) || sk_is_nan(heap[self].end_t) {
            return false;
        }
        let data = &mut heap[self];
        c.sub_divide(data.start_t, data.end_t, &mut data.part);
        data.bounds.set_bounds_t_curve(&data.part);
        data.coin_start.init();
        data.coin_end.init();
        data.bounds_max = std_max(data.bounds.width(), data.bounds.height());
        data.collapsed = data.part.collapsed();
        data.has_perp = false;
        data.deleted = false;
        data.bounds.valid()
    }

    pub(crate) fn is_bounded(self, heap: &SkTSectHeap) -> bool {
        heap[self].bounded.is_some()
    }

    pub(crate) fn linears_intersect(self, heap: &mut SkTSectHeap, span: SkTSpan) -> bool {
        let heap: &SkTSectHeap = heap;
        let result = self.linear_intersects(heap, &heap[span].part);
        if result <= 1 {
            return result != 0;
        }
        debug_assert!(heap[span].is_linear);
        let result = span.linear_intersects(heap, &heap[self].part);
        result != 0
    }

    pub(crate) fn mark_coincident(self, heap: &mut SkTSectHeap) {
        heap[self].coin_start.mark_coincident();
        heap[self].coin_end.mark_coincident();
    }

    pub(crate) fn next(self, heap: &SkTSectHeap) -> Option<SkTSpan> {
        heap[self].next
    }

    pub(crate) fn only_end_points_in_common(
        self,
        heap: &mut SkTSectHeap,
        opp: SkTSpan,
        start: &mut bool,
        opp_start: &mut bool,
        pts_in_common: &mut bool,
    ) -> bool {
        if opp.point_first(heap) == self.point_first(heap) {
            *start = true;
            *opp_start = true;
        } else if opp.point_first(heap) == self.point_last(heap) {
            *start = false;
            *opp_start = true;
        } else if opp.point_last(heap) == self.point_first(heap) {
            *start = true;
            *opp_start = false;
        } else if opp.point_last(heap) == self.point_last(heap) {
            *start = false;
            *opp_start = false;
        } else {
            *pts_in_common = false;
            return false;
        }
        *pts_in_common = true;
        let mut other_pts = [SkDPoint::default(); 4];
        let mut opp_other_pts = [SkDPoint::default(); 4];
        let base_index = if *start {
            0
        } else {
            heap[self].part.point_last()
        };
        heap[self].part.other_pts(base_index, &mut other_pts);
        let opp_base_index = if *opp_start {
            0
        } else {
            heap[opp].part.point_last()
        };
        heap[opp].part.other_pts(opp_base_index, &mut opp_other_pts);
        let base = heap[self].part[base_index as usize];
        for o1 in 0..(self.point_count(heap) - 1) as usize {
            let v1 = other_pts[o1] - base;
            for o2 in 0..(opp.point_count(heap) - 1) as usize {
                let v2 = opp_other_pts[o2] - base;
                if v2.dot(&v1) >= 0.0 {
                    return false;
                }
            }
        }
        true
    }

    pub(crate) fn part(self, heap: &SkTSectHeap) -> &SkTCurve {
        &heap[self].part
    }

    pub(crate) fn point_count(self, heap: &SkTSectHeap) -> i32 {
        heap[self].part.point_count()
    }

    pub(crate) fn point_first(self, heap: &SkTSectHeap) -> SkDPoint {
        heap[self].part[0]
    }

    pub(crate) fn point_last(self, heap: &SkTSectHeap) -> SkDPoint {
        let part = &heap[self].part;
        part[part.point_last() as usize]
    }

    pub(crate) fn remove_all_bounded(self, heap: &mut SkTSectHeap) -> bool {
        let mut delete_span = false;
        let mut bounded = heap[self].bounded;
        while let Some(b) = bounded {
            let opp = heap[b].bounded;
            delete_span |= opp.remove_bounded(heap, self);
            bounded = heap[b].next;
        }
        delete_span
    }

    pub(crate) fn remove_bounded(self, heap: &mut SkTSectHeap, opp: SkTSpan) -> bool {
        if heap[self].has_perp {
            let mut found_start = false;
            let mut found_end = false;
            let mut bounded = heap[self].bounded;
            while let Some(b) = bounded {
                let test = heap[b].bounded;
                if opp != test {
                    found_start |= between(
                        heap[test].start_t,
                        heap[self].coin_start.perp_t(),
                        heap[test].end_t,
                    );
                    found_end |= between(
                        heap[test].start_t,
                        heap[self].coin_end.perp_t(),
                        heap[test].end_t,
                    );
                }
                bounded = heap[b].next;
            }
            if !found_start || !found_end {
                heap[self].has_perp = false;
                heap[self].coin_start.init();
                heap[self].coin_end.init();
            }
        }
        let mut bounded = heap[self].bounded;
        let mut prev: Option<SkTSpanBounded> = None;
        while let Some(b) = bounded {
            let bounded_next = heap[b].next;
            if opp == heap[b].bounded {
                if let Some(p) = prev {
                    heap[p].next = bounded_next;
                    return false;
                } else {
                    heap[self].bounded = bounded_next;
                    return heap[self].bounded.is_none();
                }
            }
            prev = Some(b);
            bounded = bounded_next;
        }
        false
    }

    pub(crate) fn reset(self, heap: &mut SkTSectHeap) {
        heap[self].bounded = None;
    }

    pub(crate) fn reset_bounds(self, heap: &mut SkTSectHeap, curve: &SkTCurve) {
        heap[self].is_linear = false;
        heap[self].is_line = false;
        self.init_bounds(heap, curve);
    }

    pub(crate) fn split(self, heap: &mut SkTSectHeap, work: SkTSpan) -> bool {
        let t = (heap[work].start_t + heap[work].end_t) * 0.5;
        self.split_at(heap, work, t)
    }

    pub(crate) fn split_at(self, heap: &mut SkTSectHeap, work: SkTSpan, t: f64) -> bool {
        heap[self].start_t = t;
        heap[self].end_t = heap[work].end_t;
        if heap[self].start_t == heap[self].end_t {
            heap[self].collapsed = true;
            return false;
        }
        heap[work].end_t = t;
        if heap[work].start_t == heap[work].end_t {
            heap[work].collapsed = true;
            return false;
        }
        heap[self].prev = Some(work);
        heap[self].next = heap[work].next;
        heap[self].is_linear = heap[work].is_linear;
        heap[self].is_line = heap[work].is_line;

        heap[work].next = Some(self);
        if let Some(next) = heap[self].next {
            heap[next].prev = Some(self);
        }
        let mut bounded = heap[work].bounded;
        heap[self].bounded = None;
        while let Some(b) = bounded {
            let span = heap[b].bounded;
            self.add_bounded(heap, span);
            bounded = heap[b].next;
        }
        bounded = heap[self].bounded;
        while let Some(b) = bounded {
            let span = heap[b].bounded;
            span.add_bounded(heap, self);
            bounded = heap[b].next;
        }
        true
    }

    pub(crate) fn start_t(self, heap: &SkTSectHeap) -> f64 {
        heap[self].start_t
    }

    // returns 0 if no hull intersection
    //         1 if hulls intersect
    //         2 if hulls only share a common endpoint
    //        -1 if linear and further checking is required
    pub(crate) fn hull_check(
        self,
        heap: &mut SkTSectHeap,
        opp: SkTSpan,
        start: &mut bool,
        opp_start: &mut bool,
    ) -> i32 {
        if heap[self].is_linear {
            return -1;
        }
        let mut pts_in_common = false;
        if self.only_end_points_in_common(heap, opp, start, opp_start, &mut pts_in_common) {
            debug_assert!(pts_in_common);
            return 2;
        }
        let mut linear = false;
        let hulls_meet = heap[self]
            .part
            .hull_intersects_t_curve(&heap[opp].part, &mut linear);
        if hulls_meet {
            if !linear {
                // check set true if linear
                return 1;
            }
            heap[self].is_linear = true;
            heap[self].is_line = heap[self].part.controls_inside();
            return if pts_in_common { 1 } else { -1 };
        }
        // hull is not linear; check set true if intersected at the end points
        i32::from(pts_in_common) << 1 // 0 or 2
    }

    pub(crate) fn linear_intersects(self, heap: &SkTSectHeap, q2: &SkTCurve) -> i32 {
        let part = &heap[self].part;
        // looks like q1 is near-linear
        let mut start = 0usize;
        let mut end = part.point_last() as usize; // the outside points are usually the extremes
        if !part.controls_inside() {
            let mut dist = 0.0; // if there's any question, compute distance to find best outsiders
            for outer in 0..(self.point_count(heap) - 1) as usize {
                for inner in outer + 1..self.point_count(heap) as usize {
                    let test = (part[outer] - part[inner]).length_squared();
                    if dist > test {
                        continue;
                    }
                    dist = test;
                    start = outer;
                    end = inner;
                }
            }
        }
        // see if q2 is on one side of the line formed by the extreme points
        let orig_x = part[start].x;
        let orig_y = part[start].y;
        let adj = part[end].x - orig_x;
        let opp = part[end].y - orig_y;
        let max_part = std_max(adj.abs(), opp.abs());
        let mut sign = 0.0; // initialization to shut up warning in release build
        for n in 0..q2.point_count() as usize {
            let dx = q2[n].y - orig_y;
            let dy = q2[n].x - orig_x;
            let max_val = std_max(max_part, std_max(dx.abs(), dy.abs()));
            let test = (q2[n].y - orig_y) * adj - (q2[n].x - orig_x) * opp;
            if precisely_zero_when_compared_to(test, max_val) {
                return 1;
            }
            if approximately_zero_when_compared_to(test, max_val) {
                return 3;
            }
            if n == 0 {
                sign = test;
                continue;
            }
            if test * sign < 0.0 {
                return 1;
            }
        }
        0
    }

    pub(crate) fn opp_t(self, heap: &SkTSectHeap, t: f64) -> Option<SkTSpan> {
        let mut bounded = heap[self].bounded;
        while let Some(b) = bounded {
            let test = heap[b].bounded;
            if between(heap[test].start_t, t, heap[test].end_t) {
                return Some(test);
            }
            bounded = heap[b].next;
        }
        None
    }
}

/// `SkTSect`: one curve of an intersection and its list of live spans.
#[derive(Clone, Debug)]
pub(crate) struct SkTSect {
    pub(crate) curve: SkTCurve,
    pub(crate) head: Option<SkTSpan>,
    pub(crate) coincident: Option<SkTSpan>,
    pub(crate) deleted: Option<SkTSpan>,
    pub(crate) active_count: i32,
    pub(crate) removed_start_t: bool,
    pub(crate) removed_end_t: bool,
    pub(crate) hung: bool,
}

const COINCIDENT_SPAN_COUNT: i32 = 9;

impl SkTSect {
    pub(crate) const K_ZERO_S1_SET: i32 = 1;
    pub(crate) const K_ONE_S1_SET: i32 = 2;
    pub(crate) const K_ZERO_S2_SET: i32 = 4;
    pub(crate) const K_ONE_S2_SET: i32 = 8;

    /// `SkTSect(const SkTCurve& c)`.
    pub(crate) fn new(heap: &mut SkTSectHeap, c: &SkTCurve) -> SkTSect {
        let mut sect = SkTSect {
            curve: *c,
            head: None,
            coincident: None,
            deleted: None,
            active_count: 0,
            removed_start_t: false,
            removed_end_t: false,
            hung: false,
        };
        sect.reset_removed_ends();
        let head = sect.add_one(heap);
        sect.head = Some(head);
        head.init(heap, c);
        sect
    }

    pub(crate) fn binary_search(
        heap: &mut SkTSectHeap,
        sect1: &mut SkTSect,
        sect2: &mut SkTSect,
        intersections: &mut SkIntersections,
    ) {
        intersections.reset();
        intersections.set_max(sect1.curve.max_intersections() + 4); // give extra for slop
        let span1 = sect1.head.unwrap();
        let span2 = sect2.head.unwrap();
        let mut opp_sect = 0;
        let sect = sect1.intersects(heap, span1, sect2, span2, &mut opp_sect);
        if sect == 0 {
            return;
        }
        if sect == 2 && opp_sect == 2 {
            let _ = SkTSect::ends_equal(heap, sect1, sect2, intersections);
            return;
        }
        span1.add_bounded(heap, span2);
        span2.add_bounded(heap, span1);
        const K_MAX_COIN_LOOP_COUNT: i32 = 8;
        let mut coin_loop_count = K_MAX_COIN_LOOP_COUNT;
        let mut start1s = 0.0;
        let mut start1e = 0.0;
        loop {
            // find the largest bounds
            let Some(largest1) = sect1.bounds_max(heap) else {
                if sect1.hung {
                    return;
                }
                break;
            };
            let largest2 = sect2.bounds_max(heap);
            // split it
            let split1 = match largest2 {
                None => true,
                Some(largest2) => {
                    heap[largest1].bounds_max > heap[largest2].bounds_max
                        || (!heap[largest1].collapsed && heap[largest2].collapsed)
                }
            };
            if split1 {
                if sect2.hung {
                    return;
                }
                if heap[largest1].collapsed {
                    break;
                }
                sect1.reset_removed_ends();
                sect2.reset_removed_ends();
                // trim parts that don't intersect the opposite
                let half1 = sect1.add_one(heap);
                if !half1.split(heap, largest1) {
                    break;
                }
                if !sect1.trim(heap, largest1, sect2) {
                    return;
                }
                if !sect1.trim(heap, half1, sect2) {
                    return;
                }
            } else {
                let largest2 = largest2.unwrap();
                if heap[largest2].collapsed {
                    break;
                }
                sect1.reset_removed_ends();
                sect2.reset_removed_ends();
                // trim parts that don't intersect the opposite
                let half2 = sect2.add_one(heap);
                if !half2.split(heap, largest2) {
                    break;
                }
                if !sect2.trim(heap, largest2, sect1) {
                    return;
                }
                if !sect2.trim(heap, half2, sect1) {
                    return;
                }
            }
            // if there are 9 or more continuous spans on both sects, suspect coincidence
            if sect1.active_count >= COINCIDENT_SPAN_COUNT
                && sect2.active_count >= COINCIDENT_SPAN_COUNT
            {
                if coin_loop_count == K_MAX_COIN_LOOP_COUNT {
                    start1s = heap[sect1.head.unwrap()].start_t;
                    start1e = heap[sect1.tail(heap).unwrap()].end_t;
                }
                if !sect1.coincident_check(heap, sect2) {
                    return;
                }
                coin_loop_count -= 1;
                if coin_loop_count == 0 && sect1.head.is_some() && sect2.head.is_some() {
                    /* All known working cases resolve in two tries. Sadly, cubicConicTests[0]
                    gets stuck in a loop. It adds an extension to allow a coincident end
                    perpendicular to track its intersection in the opposite curve. However,
                    the bounding box of the extension does not intersect the original curve,
                    so the extension is discarded, only to be added again the next time around. */
                    sect1.coincident_force(heap, sect2, start1s, start1e);
                }
            }
            if sect1.active_count >= COINCIDENT_SPAN_COUNT
                && sect2.active_count >= COINCIDENT_SPAN_COUNT
            {
                let Some(head1) = sect1.head else {
                    return;
                };
                let tail1 = sect1.tail(heap);
                sect1.compute_perpendiculars(heap, sect2, head1, tail1);
                let Some(head2) = sect2.head else {
                    return;
                };
                let tail2 = sect2.tail(heap);
                sect2.compute_perpendiculars(heap, sect1, head2, tail2);
                if !sect1.remove_by_perpendicular(heap, sect2) {
                    return;
                }
                if sect1.collapsed(heap) > sect1.curve.max_intersections() {
                    break;
                }
            }
            if sect1.head.is_none() || sect2.head.is_none() {
                break;
            }
        }
        let mut coincident = sect1.coincident;
        if let Some(first_coincident) = coincident {
            // if there is more than one coincident span, check loosely to see if they should be joined
            if heap[first_coincident].next.is_some() {
                sect1.merge_coincidence(heap, sect2);
                coincident = sect1.coincident;
            }
            debug_assert!(sect2.coincident.is_some()); // courtesy check : coincidence only looks at sect 1
            loop {
                let Some(c) = coincident else {
                    return;
                };
                'body: {
                    if !heap[c].coin_start.is_match() {
                        break 'body;
                    }
                    if !heap[c].coin_end.is_match() {
                        break 'body;
                    }
                    let perp_t = heap[c].coin_start.perp_t();
                    if perp_t < 0.0 {
                        return;
                    }
                    let index = intersections.insert_coincident(
                        heap[c].start_t,
                        perp_t,
                        &c.point_first(heap),
                    );
                    if (intersections.insert_coincident(
                        heap[c].end_t,
                        heap[c].coin_end.perp_t(),
                        &c.point_last(heap),
                    ) < 0)
                        && index >= 0
                    {
                        intersections.clear_coincidence(index);
                    }
                }
                coincident = heap[c].next;
                if coincident.is_none() {
                    break;
                }
            }
        }
        let zero_one_set = SkTSect::ends_equal(heap, sect1, sect2, intersections);
        // if the final iteration contains an end (0 or 1),
        if sect1.removed_start_t && (zero_one_set & SkTSect::K_ZERO_S1_SET) == 0 {
            let mut perp = SkTCoincident::default(); // intersect perpendicular with opposite curve
            perp.set_perp(&sect1.curve, 0.0, &sect1.curve[0], &sect2.curve);
            if perp.is_match() {
                intersections.insert(0.0, perp.perp_t(), perp.perp_pt());
            }
        }
        if sect1.removed_end_t && (zero_one_set & SkTSect::K_ONE_S1_SET) == 0 {
            let mut perp = SkTCoincident::default();
            perp.set_perp(&sect1.curve, 1.0, &sect1.point_last(), &sect2.curve);
            if perp.is_match() {
                intersections.insert(1.0, perp.perp_t(), perp.perp_pt());
            }
        }
        if sect2.removed_start_t && (zero_one_set & SkTSect::K_ZERO_S2_SET) == 0 {
            let mut perp = SkTCoincident::default();
            perp.set_perp(&sect2.curve, 0.0, &sect2.curve[0], &sect1.curve);
            if perp.is_match() {
                intersections.insert(perp.perp_t(), 0.0, perp.perp_pt());
            }
        }
        if sect2.removed_end_t && (zero_one_set & SkTSect::K_ONE_S2_SET) == 0 {
            let mut perp = SkTCoincident::default();
            perp.set_perp(&sect2.curve, 1.0, &sect2.point_last(), &sect1.curve);
            if perp.is_match() {
                intersections.insert(perp.perp_t(), 1.0, perp.perp_pt());
            }
        }
        if sect1.head.is_none() || sect2.head.is_none() {
            return;
        }
        sect1.recover_collapsed(heap);
        sect2.recover_collapsed(heap);
        let mut result1 = sect1.head;
        // check heads and tails for zero and ones and insert them if we haven't already done so
        let head1 = result1.unwrap();
        if (zero_one_set & SkTSect::K_ZERO_S1_SET) == 0
            && approximately_less_than_zero(heap[head1].start_t)
        {
            let start1 = sect1.curve[0];
            if head1.is_bounded(heap) {
                let t = head1.closest_bounded_t(heap, &start1);
                if sect2.curve.pt_at_t(t).approximately_equal(&start1) {
                    intersections.insert(0.0, t, &start1);
                }
            }
        }
        let head2 = sect2.head.unwrap();
        if (zero_one_set & SkTSect::K_ZERO_S2_SET) == 0
            && approximately_less_than_zero(heap[head2].start_t)
        {
            let start2 = sect2.curve[0];
            if head2.is_bounded(heap) {
                let t = head2.closest_bounded_t(heap, &start2);
                if sect1.curve.pt_at_t(t).approximately_equal(&start2) {
                    intersections.insert(t, 0.0, &start2);
                }
            }
        }
        if (zero_one_set & SkTSect::K_ONE_S1_SET) == 0 {
            let Some(tail1) = sect1.tail(heap) else {
                return;
            };
            if approximately_greater_than_one(heap[tail1].end_t) {
                let end1 = sect1.point_last();
                if tail1.is_bounded(heap) {
                    let t = tail1.closest_bounded_t(heap, &end1);
                    if sect2.curve.pt_at_t(t).approximately_equal(&end1) {
                        intersections.insert(1.0, t, &end1);
                    }
                }
            }
        }
        if (zero_one_set & SkTSect::K_ONE_S2_SET) == 0 {
            let Some(tail2) = sect2.tail(heap) else {
                return;
            };
            if approximately_greater_than_one(heap[tail2].end_t) {
                let end2 = sect2.point_last();
                if tail2.is_bounded(heap) {
                    let t = tail2.closest_bounded_t(heap, &end2);
                    if sect1.curve.pt_at_t(t).approximately_equal(&end2) {
                        intersections.insert(t, 1.0, &end2);
                    }
                }
            }
        }
        let mut closest = SkClosestSect::new();
        loop {
            while let Some(r1) = result1 {
                if heap[r1].coin_start.is_match() && heap[r1].coin_end.is_match() {
                    result1 = heap[r1].next;
                } else {
                    break;
                }
            }
            let Some(r1) = result1 else {
                break;
            };
            let mut result2 = sect2.head;
            while let Some(r2) = result2 {
                closest.find(heap, r1, r2);
                result2 = heap[r2].next;
            }
            result1 = heap[r1].next;
            if result1.is_none() {
                break;
            }
        }
        closest.finish(heap, intersections);
        // if there is more than one intersection and it isn't already coincident, check
        let mut last = intersections.used() - 1;
        let mut index = 0;
        while index < last {
            if intersections.is_coincident(index) && intersections.is_coincident(index + 1) {
                index += 1;
                continue;
            }
            let mid_t =
                (intersections[0][index as usize] + intersections[0][index as usize + 1]) / 2.0;
            let mid_pt = sect1.curve.pt_at_t(mid_t);
            // intersect perpendicular with opposite curve
            let mut perp = SkTCoincident::default();
            perp.set_perp(&sect1.curve, mid_t, &mid_pt, &sect2.curve);
            if !perp.is_match() {
                index += 1;
                continue;
            }
            if intersections.is_coincident(index) {
                intersections.remove_one(index);
                last -= 1;
            } else if intersections.is_coincident(index + 1) {
                intersections.remove_one(index + 1);
                last -= 1;
            } else {
                intersections.set_coincident(index);
                index += 1;
            }
            intersections.set_coincident(index);
        }
    }

    pub(crate) fn has_bounded(&self, heap: &SkTSectHeap, span: SkTSpan) -> bool {
        let Some(mut test) = self.head else {
            return false;
        };
        loop {
            if test.find_opp_span(heap, span).is_some() {
                return true;
            }
            match test.next(heap) {
                Some(next) => test = next,
                None => break,
            }
        }
        false
    }

    pub(crate) fn add_following(
        &mut self,
        heap: &mut SkTSectHeap,
        prior: Option<SkTSpan>,
    ) -> SkTSpan {
        let result = self.add_one(heap);
        heap[result].start_t = match prior {
            Some(prior) => heap[prior].end_t,
            None => 0.0,
        };
        let next = match prior {
            Some(prior) => heap[prior].next,
            None => self.head,
        };
        heap[result].end_t = match next {
            Some(next) => heap[next].start_t,
            None => 1.0,
        };
        heap[result].prev = prior;
        heap[result].next = next;
        if let Some(prior) = prior {
            heap[prior].next = Some(result);
        } else {
            self.head = Some(result);
        }
        if let Some(next) = next {
            heap[next].prev = Some(result);
        }
        result.reset_bounds(heap, &self.curve);
        // world may not be consistent to call validate here
        result
    }

    pub(crate) fn add_for_perp(&mut self, heap: &mut SkTSectHeap, span: SkTSpan, t: f64) {
        if !span.has_opp_t(heap, t) {
            let mut prior_span = None;
            let opp = match self.span_at_t(heap, t, &mut prior_span) {
                Some(opp) => opp,
                None => self.add_following(heap, prior_span),
            };
            opp.add_bounded(heap, span);
            span.add_bounded(heap, opp);
        }
    }

    pub(crate) fn add_one(&mut self, heap: &mut SkTSectHeap) -> SkTSpan {
        let result = if let Some(deleted) = self.deleted {
            self.deleted = heap[deleted].next;
            deleted
        } else {
            heap.make_span(&self.curve)
        };
        result.reset(heap);
        heap[result].has_perp = false;
        heap[result].deleted = false;
        self.active_count += 1;
        result
    }

    pub(crate) fn add_split_at(
        &mut self,
        heap: &mut SkTSectHeap,
        span: SkTSpan,
        t: f64,
    ) -> SkTSpan {
        let result = self.add_one(heap);
        result.split_at(heap, span, t);
        result.init_bounds(heap, &self.curve);
        span.init_bounds(heap, &self.curve);
        result
    }

    pub(crate) fn binary_search_coin(
        &mut self,
        heap: &mut SkTSectHeap,
        sect2: &mut SkTSect,
        t_start: f64,
        t_step: f64,
        result_t: &mut f64,
        opp_t: &mut f64,
        opp_first: &mut Option<SkTSpan>,
    ) -> bool {
        let mut t_step = t_step;
        let work = heap.make_span(&self.curve);
        heap[work].start_t = t_start;
        heap[work].end_t = t_start;
        let mut result = t_start;
        let mut last = self.curve.pt_at_t(t_start);
        let mut opp_pt = SkDPoint::default();
        let mut flip = false;
        let mut contained = false;
        let down = t_step < 0.0;
        let opp = &sect2.curve;
        loop {
            t_step *= 0.5;
            heap[work].start_t += t_step;
            if flip {
                t_step = -t_step;
                flip = false;
            }
            work.init_bounds(heap, &self.curve);
            if heap[work].collapsed {
                return false;
            }
            if last.approximately_equal(&work.point_first(heap)) {
                break;
            }
            last = work.point_first(heap);
            let mut coin_start = heap[work].coin_start;
            coin_start.set_perp(&self.curve, heap[work].start_t, &last, opp);
            heap[work].coin_start = coin_start;
            if heap[work].coin_start.is_match() {
                let opp_t_test = heap[work].coin_start.perp_t();
                if sect2.head.unwrap().contains(heap, opp_t_test) {
                    *opp_t = opp_t_test;
                    opp_pt = *heap[work].coin_start.perp_pt();
                    contained = true;
                    if if down {
                        result <= heap[work].start_t
                    } else {
                        result >= heap[work].start_t
                    } {
                        *opp_first = None; // signal caller to fail
                        return false;
                    }
                    result = heap[work].start_t;
                    continue;
                }
            }
            t_step = -t_step;
            flip = true;
        }
        if !contained {
            return false;
        }
        if last.approximately_equal(&self.curve[0]) {
            result = 0.0;
        } else if last.approximately_equal(&self.point_last()) {
            result = 1.0;
        }
        if opp_pt.approximately_equal(&opp[0]) {
            *opp_t = 0.0;
        } else if opp_pt.approximately_equal(&sect2.point_last()) {
            *opp_t = 1.0;
        }
        *result_t = result;
        true
    }

    // OPTIMIZE ? keep a sorted list of sizes in the form of a doubly-linked list in quad span
    //            so that each quad sect has a pointer to the largest, and can update it as spans
    //            are split
    pub(crate) fn bounds_max(&mut self, heap: &SkTSectHeap) -> Option<SkTSpan> {
        let mut test = self.head.unwrap();
        let mut largest = test;
        let mut l_collapsed = heap[largest].collapsed;
        let mut safety_net = 10000;
        while let Some(next) = heap[test].next {
            test = next;
            safety_net -= 1;
            if safety_net == 0 {
                self.hung = true;
                return None;
            }
            let t_collapsed = heap[test].collapsed;
            if (l_collapsed && !t_collapsed)
                || (l_collapsed == t_collapsed && heap[largest].bounds_max < heap[test].bounds_max)
            {
                largest = test;
                l_collapsed = heap[test].collapsed;
            }
        }
        Some(largest)
    }

    pub(crate) fn coincident_check(&mut self, heap: &mut SkTSectHeap, sect2: &mut SkTSect) -> bool {
        let Some(mut first) = self.head else {
            return false;
        };
        let mut last: Option<SkTSpan> = None;
        let mut next: Option<SkTSpan>;
        'outer: loop {
            'body: {
                let consecutive = self.count_consecutive_spans(heap, first, &mut last);
                next = heap[last.unwrap()].next;
                if consecutive < COINCIDENT_SPAN_COUNT {
                    break 'body;
                }
                self.compute_perpendiculars(heap, sect2, first, last);
                // check to see if a range of points are on the curve
                let mut coin_start = Some(first);
                loop {
                    let success = self.extract_coincident(
                        heap,
                        sect2,
                        coin_start.unwrap(),
                        last,
                        &mut coin_start,
                    );
                    if !success {
                        return false;
                    }
                    if coin_start.is_none() || heap[last.unwrap()].deleted {
                        break;
                    }
                }
                if self.head.is_none() || sect2.head.is_none() {
                    break 'outer;
                }
                match next {
                    Some(n) if !heap[n].deleted => {}
                    _ => break 'outer,
                }
            }
            match next {
                Some(n) => first = n,
                None => break,
            }
        }
        true
    }

    pub(crate) fn coincident_force(
        &mut self,
        heap: &mut SkTSectHeap,
        sect2: &mut SkTSect,
        start1s: f64,
        start1e: f64,
    ) {
        let first = self.head.unwrap();
        let last = self.tail(heap);
        let opp_first = sect2.head.unwrap();
        let opp_last = sect2.tail(heap);
        let (Some(last), Some(opp_last)) = (last, opp_last) else {
            return;
        };
        let mut delete_empty_spans = self.update_bounded(heap, first, last, opp_first);
        delete_empty_spans |= sect2.update_bounded(heap, opp_first, opp_last, first);
        self.remove_span_range(heap, first, last);
        sect2.remove_span_range(heap, opp_first, opp_last);
        heap[first].start_t = start1s;
        heap[first].end_t = start1e;
        first.reset_bounds(heap, &self.curve);
        let mut coin_start = heap[first].coin_start;
        coin_start.set_perp(&self.curve, start1s, &self.curve[0], &sect2.curve);
        heap[first].coin_start = coin_start;
        let mut coin_end = heap[first].coin_end;
        coin_end.set_perp(&self.curve, start1e, &self.point_last(), &sect2.curve);
        heap[first].coin_end = coin_end;
        let opp_matched = heap[first].coin_start.perp_t() < heap[first].coin_end.perp_t();
        let mut opp_start_t = if heap[first].coin_start.perp_t() == -1.0 {
            0.0
        } else {
            std_max(0.0, heap[first].coin_start.perp_t())
        };
        let mut opp_end_t = if heap[first].coin_end.perp_t() == -1.0 {
            1.0
        } else {
            std_min(1.0, heap[first].coin_end.perp_t())
        };
        if !opp_matched {
            std::mem::swap(&mut opp_start_t, &mut opp_end_t);
        }
        heap[opp_first].start_t = opp_start_t;
        heap[opp_first].end_t = opp_end_t;
        opp_first.reset_bounds(heap, &sect2.curve);
        self.remove_coincident(heap, first, false);
        sect2.remove_coincident(heap, opp_first, true);
        if delete_empty_spans {
            self.delete_empty_spans(heap);
            sect2.delete_empty_spans(heap);
        }
    }

    pub(crate) fn coincident_has_t(&mut self, heap: &SkTSectHeap, t: f64) -> bool {
        let mut test = self.coincident;
        while let Some(span) = test {
            if between(heap[span].start_t, t, heap[span].end_t) {
                return true;
            }
            test = heap[span].next;
        }
        false
    }

    pub(crate) fn collapsed(&self, heap: &SkTSectHeap) -> i32 {
        let mut result = 0;
        let mut test = self.head;
        while let Some(span) = test {
            if heap[span].collapsed {
                result += 1;
            }
            test = span.next(heap);
        }
        result
    }

    pub(crate) fn compute_perpendiculars(
        &mut self,
        heap: &mut SkTSectHeap,
        sect2: &mut SkTSect,
        first: SkTSpan,
        last: Option<SkTSpan>,
    ) {
        let Some(last) = last else {
            return;
        };
        let mut work = first;
        let mut prior: Option<SkTSpan> = None;
        loop {
            if !heap[work].has_perp && !heap[work].collapsed {
                if let Some(prior) = prior {
                    heap[work].coin_start = heap[prior].coin_end;
                } else {
                    let mut coin_start = heap[work].coin_start;
                    coin_start.set_perp(
                        &self.curve,
                        heap[work].start_t,
                        &work.point_first(heap),
                        &sect2.curve,
                    );
                    heap[work].coin_start = coin_start;
                }
                if heap[work].coin_start.is_match() {
                    let perp_t = heap[work].coin_start.perp_t();
                    if sect2.coincident_has_t(heap, perp_t) {
                        heap[work].coin_start.init();
                    } else {
                        sect2.add_for_perp(heap, work, perp_t);
                    }
                }
                let mut coin_end = heap[work].coin_end;
                coin_end.set_perp(
                    &self.curve,
                    heap[work].end_t,
                    &work.point_last(heap),
                    &sect2.curve,
                );
                heap[work].coin_end = coin_end;
                if heap[work].coin_end.is_match() {
                    let perp_t = heap[work].coin_end.perp_t();
                    if sect2.coincident_has_t(heap, perp_t) {
                        heap[work].coin_end.init();
                    } else {
                        sect2.add_for_perp(heap, work, perp_t);
                    }
                }
                heap[work].has_perp = true;
            }
            if work == last {
                break;
            }
            prior = Some(work);
            work = heap[work].next.unwrap();
        }
    }

    pub(crate) fn count_consecutive_spans(
        &self,
        heap: &SkTSectHeap,
        first: SkTSpan,
        last: &mut Option<SkTSpan>,
    ) -> i32 {
        let mut consecutive = 1;
        let mut last_span = first;
        while let Some(next) = heap[last_span].next {
            if heap[next].start_t > heap[last_span].end_t {
                break;
            }
            consecutive += 1;
            last_span = next;
        }
        *last = Some(last_span);
        consecutive
    }

    pub(crate) fn delete_empty_spans(&mut self, heap: &mut SkTSectHeap) -> bool {
        let mut next = self.head;
        let mut safety_hatch = 1000;
        while let Some(test) = next {
            next = heap[test].next;
            if heap[test].bounded.is_none() && !self.remove_span(heap, test) {
                return false;
            }
            safety_hatch -= 1;
            if safety_hatch < 0 {
                return false;
            }
        }
        true
    }

    pub(crate) fn ends_equal(
        heap: &SkTSectHeap,
        sect1: &SkTSect,
        sect2: &SkTSect,
        intersections: &mut SkIntersections,
    ) -> i32 {
        let _ = heap;
        let mut zero_one_set = 0;
        if sect1.curve[0] == sect2.curve[0] {
            zero_one_set |= SkTSect::K_ZERO_S1_SET | SkTSect::K_ZERO_S2_SET;
            intersections.insert(0.0, 0.0, &sect1.curve[0]);
        }
        if sect1.curve[0] == sect2.point_last() {
            zero_one_set |= SkTSect::K_ZERO_S1_SET | SkTSect::K_ONE_S2_SET;
            intersections.insert(0.0, 1.0, &sect1.curve[0]);
        }
        if sect1.point_last() == sect2.curve[0] {
            zero_one_set |= SkTSect::K_ONE_S1_SET | SkTSect::K_ZERO_S2_SET;
            intersections.insert(1.0, 0.0, &sect1.point_last());
        }
        if sect1.point_last() == sect2.point_last() {
            zero_one_set |= SkTSect::K_ONE_S1_SET | SkTSect::K_ONE_S2_SET;
            intersections.insert(1.0, 1.0, &sect1.point_last());
        }
        // check for zero
        if (zero_one_set & (SkTSect::K_ZERO_S1_SET | SkTSect::K_ZERO_S2_SET)) == 0
            && sect1.curve[0].approximately_equal(&sect2.curve[0])
        {
            zero_one_set |= SkTSect::K_ZERO_S1_SET | SkTSect::K_ZERO_S2_SET;
            intersections.insert_near(0.0, 0.0, &sect1.curve[0], &sect2.curve[0]);
        }
        if (zero_one_set & (SkTSect::K_ZERO_S1_SET | SkTSect::K_ONE_S2_SET)) == 0
            && sect1.curve[0].approximately_equal(&sect2.point_last())
        {
            zero_one_set |= SkTSect::K_ZERO_S1_SET | SkTSect::K_ONE_S2_SET;
            intersections.insert_near(0.0, 1.0, &sect1.curve[0], &sect2.point_last());
        }
        // check for one
        if (zero_one_set & (SkTSect::K_ONE_S1_SET | SkTSect::K_ZERO_S2_SET)) == 0
            && sect1.point_last().approximately_equal(&sect2.curve[0])
        {
            zero_one_set |= SkTSect::K_ONE_S1_SET | SkTSect::K_ZERO_S2_SET;
            intersections.insert_near(1.0, 0.0, &sect1.point_last(), &sect2.curve[0]);
        }
        if (zero_one_set & (SkTSect::K_ONE_S1_SET | SkTSect::K_ONE_S2_SET)) == 0
            && sect1.point_last().approximately_equal(&sect2.point_last())
        {
            zero_one_set |= SkTSect::K_ONE_S1_SET | SkTSect::K_ONE_S2_SET;
            intersections.insert_near(1.0, 1.0, &sect1.point_last(), &sect2.point_last());
        }
        zero_one_set
    }

    pub(crate) fn extract_coincident(
        &mut self,
        heap: &mut SkTSectHeap,
        sect2: &mut SkTSect,
        first: SkTSpan,
        last: Option<SkTSpan>,
        result: &mut Option<SkTSpan>,
    ) -> bool {
        let mut last = last;
        let first = self.find_coincident_run(heap, first, &mut last);
        let (Some(mut first), Some(last)) = (first, last) else {
            *result = None;
            return true;
        };
        // march outwards to find limit of coincidence from here to previous and next spans
        let start_t = heap[first].start_t;
        let mut opp_start_t = 0.0;
        let mut opp_end_t = 0.0;
        let prev = heap[first].prev;
        debug_assert!(heap[first].coin_start.is_match());
        let mut opp_first = first.find_opp_t(heap, heap[first].coin_start.perp_t());
        let opp_matched = heap[first].coin_start.perp_t() < heap[first].coin_end.perp_t();
        let mut coin_start = 0.0;
        // `prev && prev->fEndT == startT && binarySearchCoin(...) && ... && (cutFirst = ...)`
        let mut split = None;
        if let Some(prev) = prev {
            if heap[prev].end_t == start_t {
                let t_step = heap[prev].start_t - start_t;
                if self.binary_search_coin(
                    heap,
                    sect2,
                    start_t,
                    t_step,
                    &mut coin_start,
                    &mut opp_start_t,
                    &mut opp_first,
                ) && heap[prev].start_t < coin_start
                    && coin_start < start_t
                {
                    split = prev
                        .opp_t(heap, opp_start_t)
                        .map(|cut_first| (prev, cut_first));
                }
            }
        }
        if let Some((prev, cut_first)) = split {
            let mut opp_first_span = cut_first;
            first = self.add_split_at(heap, prev, coin_start);
            first.mark_coincident(heap);
            heap[prev].coin_end.mark_coincident();
            if heap[opp_first_span].start_t < opp_start_t
                && opp_start_t < heap[opp_first_span].end_t
            {
                let opp_half = sect2.add_split_at(heap, opp_first_span, opp_start_t);
                if opp_matched {
                    heap[opp_first_span].coin_end.mark_coincident();
                    opp_half.mark_coincident(heap);
                    opp_first_span = opp_half;
                } else {
                    opp_first_span.mark_coincident(heap);
                    heap[opp_half].coin_start.mark_coincident();
                }
            }
            opp_first = Some(opp_first_span);
        } else if opp_first.is_none() {
            return false;
        }
        // FIXME: incomplete : if we're not at the end, find end of coin
        let mut opp_last = last.find_opp_t(heap, heap[last].coin_end.perp_t());
        if !opp_matched {
            std::mem::swap(&mut opp_first, &mut opp_last);
            std::mem::swap(&mut opp_start_t, &mut opp_end_t);
        }
        let Some(opp_first) = opp_first else {
            *result = None;
            return true;
        };
        let Some(opp_last) = opp_last else {
            *result = None;
            return true;
        };
        // reduce coincident runs to single entries
        let mut delete_empty_spans = self.update_bounded(heap, first, last, opp_first);
        delete_empty_spans |= sect2.update_bounded(heap, opp_first, opp_last, first);
        self.remove_span_range(heap, first, last);
        sect2.remove_span_range(heap, opp_first, opp_last);
        heap[first].end_t = heap[last].end_t;
        first.reset_bounds(heap, &self.curve);
        let mut first_coin_start = heap[first].coin_start;
        first_coin_start.set_perp(
            &self.curve,
            heap[first].start_t,
            &first.point_first(heap),
            &sect2.curve,
        );
        heap[first].coin_start = first_coin_start;
        let mut first_coin_end = heap[first].coin_end;
        first_coin_end.set_perp(
            &self.curve,
            heap[first].end_t,
            &first.point_last(heap),
            &sect2.curve,
        );
        heap[first].coin_end = first_coin_end;
        opp_start_t = heap[first].coin_start.perp_t();
        opp_end_t = heap[first].coin_end.perp_t();
        if between(0.0, opp_start_t, 1.0) && between(0.0, opp_end_t, 1.0) {
            if !opp_matched {
                std::mem::swap(&mut opp_start_t, &mut opp_end_t);
            }
            heap[opp_first].start_t = opp_start_t;
            heap[opp_first].end_t = opp_end_t;
            opp_first.reset_bounds(heap, &sect2.curve);
        }
        let last = heap[first].next;
        if !self.remove_coincident(heap, first, false) {
            return false;
        }
        if !sect2.remove_coincident(heap, opp_first, true) {
            return false;
        }
        if delete_empty_spans && (!self.delete_empty_spans(heap) || !sect2.delete_empty_spans(heap))
        {
            *result = None;
            return false;
        }
        *result = match last {
            Some(last) if !heap[last].deleted && self.head.is_some() && sect2.head.is_some() => {
                Some(last)
            }
            _ => None,
        };
        true
    }

    pub(crate) fn find_coincident_run(
        &mut self,
        heap: &mut SkTSectHeap,
        first: SkTSpan,
        last_ptr: &mut Option<SkTSpan>,
    ) -> Option<SkTSpan> {
        let mut work = first;
        let mut last_candidate: Option<SkTSpan> = None;
        let mut first: Option<SkTSpan> = None;
        // find the first fully coincident span
        loop {
            if heap[work].coin_start.is_match() {
                if !heap[work].coin_end.is_match() {
                    break;
                }
                last_candidate = Some(work);
                if first.is_none() {
                    first = Some(work);
                }
            } else if first.is_some() && heap[work].collapsed {
                *last_ptr = last_candidate;
                return first;
            } else {
                last_candidate = None;
            }
            if Some(work) == *last_ptr {
                return first;
            }
            match heap[work].next {
                Some(next) => work = next,
                None => return None,
            }
        }
        if last_candidate.is_some() {
            *last_ptr = last_candidate;
        }
        first
    }

    pub(crate) fn intersects(
        &mut self,
        heap: &mut SkTSectHeap,
        span: SkTSpan,
        opp: &mut SkTSect,
        opp_span: SkTSpan,
        opp_result: &mut i32,
    ) -> i32 {
        let mut span_start = false;
        let mut opp_start = false;
        let mut hull_result = span.hulls_intersect(heap, opp_span, &mut span_start, &mut opp_start);
        if hull_result >= 0 {
            if hull_result == 2 {
                // hulls have one point in common
                let span_bounded = heap[span].bounded;
                if span_bounded.is_none() || heap[span_bounded.unwrap()].next.is_none() {
                    debug_assert!(
                        span_bounded.is_none() || heap[span_bounded.unwrap()].bounded == opp_span
                    );
                    if span_start {
                        heap[span].end_t = heap[span].start_t;
                    } else {
                        heap[span].start_t = heap[span].end_t;
                    }
                } else {
                    hull_result = 1;
                }
                let opp_bounded = heap[opp_span].bounded;
                if opp_bounded.is_none() || heap[opp_bounded.unwrap()].next.is_none() {
                    if let Some(opp_bounded) = opp_bounded {
                        if heap[opp_bounded].bounded != span {
                            return 0;
                        }
                    }
                    if opp_start {
                        heap[opp_span].end_t = heap[opp_span].start_t;
                    } else {
                        heap[opp_span].start_t = heap[opp_span].end_t;
                    }
                    *opp_result = 2;
                } else {
                    *opp_result = 1;
                }
            } else {
                *opp_result = 1;
            }
            return hull_result;
        }
        if heap[span].is_line && heap[opp_span].is_line {
            let mut i = SkIntersections::new();
            let sects = self.lines_intersect(heap, span, opp, opp_span, &mut i);
            if sects == 2 {
                *opp_result = 1;
                return 1;
            }
            if sects == 0 {
                return -1;
            }
            self.removed_end_check(heap, span);
            heap[span].end_t = i[0][0];
            heap[span].start_t = i[0][0];
            opp.removed_end_check(heap, opp_span);
            heap[opp_span].end_t = i[1][0];
            heap[opp_span].start_t = i[1][0];
            *opp_result = 2;
            return 2;
        }
        if heap[span].is_linear || heap[opp_span].is_linear {
            *opp_result = i32::from(span.linears_intersect(heap, opp_span));
            return *opp_result;
        }
        *opp_result = 1;
        1
    }

    // while the intersection points are sufficiently far apart:
    // construct the tangent lines from the intersections
    // find the point where the tangent line intersects the opposite curve
    pub(crate) fn lines_intersect(
        &mut self,
        heap: &mut SkTSectHeap,
        span: SkTSpan,
        opp: &mut SkTSect,
        opp_span: SkTSpan,
        i: &mut SkIntersections,
    ) -> i32 {
        let mut this_ray_i = SkIntersections::new();
        let mut opp_ray_i = SkIntersections::new();
        let mut this_line = SkDLine {
            pts: [span.point_first(heap), span.point_last(heap)],
        };
        let mut opp_line = SkDLine {
            pts: [opp_span.point_first(heap), opp_span.point_last(heap)],
        };
        let mut loop_count = 0;
        let mut best_dist_sq = DBL_MAX;
        if this_ray_i.intersect_ray_t_curve(&opp.curve, &this_line) == 0 {
            return 0;
        }
        if opp_ray_i.intersect_ray_t_curve(&self.curve, &opp_line) == 0 {
            return 0;
        }
        // if the ends of each line intersect the opposite curve, the lines are coincident
        if this_ray_i.used() > 1 {
            let mut pt_matches = 0;
            for t_index in 0..this_ray_i.used() as usize {
                for l_index in 0..this_line.pts.len() {
                    pt_matches += i32::from(
                        this_ray_i
                            .pt(t_index)
                            .approximately_equal(&this_line.pts[l_index]),
                    );
                }
            }
            if pt_matches == 2 || is_parallel(&this_line, &opp.curve) {
                return 2;
            }
        }
        if opp_ray_i.used() > 1 {
            let mut pt_matches = 0;
            for o_index in 0..opp_ray_i.used() as usize {
                for l_index in 0..opp_line.pts.len() {
                    pt_matches += i32::from(
                        opp_ray_i
                            .pt(o_index)
                            .approximately_equal(&opp_line.pts[l_index]),
                    );
                }
            }
            if pt_matches == 2 || is_parallel(&opp_line, &self.curve) {
                return 2;
            }
        }
        loop {
            // pick the closest pair of points
            let mut closest = DBL_MAX;
            let mut close_index = 0usize;
            let mut opp_close_index = 0usize;
            for index in 0..opp_ray_i.used() as usize {
                if !roughly_between(heap[span].start_t, opp_ray_i[0][index], heap[span].end_t) {
                    continue;
                }
                for o_index in 0..this_ray_i.used() as usize {
                    if !roughly_between(
                        heap[opp_span].start_t,
                        this_ray_i[0][o_index],
                        heap[opp_span].end_t,
                    ) {
                        continue;
                    }
                    let dist_sq = this_ray_i.pt(index).distance_squared(opp_ray_i.pt(o_index));
                    if closest > dist_sq {
                        closest = dist_sq;
                        close_index = index;
                        opp_close_index = o_index;
                    }
                }
            }
            if closest == DBL_MAX {
                break;
            }
            let opp_i_pt = *this_ray_i.pt(opp_close_index);
            let i_pt = *opp_ray_i.pt(close_index);
            if between(
                heap[span].start_t,
                opp_ray_i[0][close_index],
                heap[span].end_t,
            ) && between(
                heap[opp_span].start_t,
                this_ray_i[0][opp_close_index],
                heap[opp_span].end_t,
            ) && opp_i_pt.approximately_equal(&i_pt)
            {
                i.merge(
                    &opp_ray_i,
                    close_index as i32,
                    &this_ray_i,
                    opp_close_index as i32,
                );
                return i.used();
            }
            let dist_sq = opp_i_pt.distance_squared(&i_pt);
            if best_dist_sq < dist_sq {
                return 0;
            }
            loop_count += 1;
            if loop_count > 5 {
                return 0;
            }
            best_dist_sq = dist_sq;
            let opp_start = opp_ray_i[0][close_index];
            this_line[0] = self.curve.pt_at_t(opp_start);
            this_line[1] = this_line[0] + self.curve.dxdy_at_t(opp_start);
            if this_ray_i.intersect_ray_t_curve(&opp.curve, &this_line) == 0 {
                break;
            }
            let start = this_ray_i[0][opp_close_index];
            opp_line[0] = opp.curve.pt_at_t(start);
            opp_line[1] = opp_line[0] + opp.curve.dxdy_at_t(start);
            if opp_ray_i.intersect_ray_t_curve(&self.curve, &opp_line) == 0 {
                break;
            }
        }
        // convergence may fail if the curves are nearly coincident
        let mut o_coin_s = SkTCoincident::default();
        let mut o_coin_e = SkTCoincident::default();
        o_coin_s.set_perp(
            &opp.curve,
            heap[opp_span].start_t,
            &opp_span.point_first(heap),
            &self.curve,
        );
        o_coin_e.set_perp(
            &opp.curve,
            heap[opp_span].end_t,
            &opp_span.point_last(heap),
            &self.curve,
        );
        let mut t_start = o_coin_s.perp_t();
        let mut t_end = o_coin_e.perp_t();
        let swap = t_start > t_end;
        if swap {
            std::mem::swap(&mut t_start, &mut t_end);
        }
        t_start = std_max(t_start, heap[span].start_t);
        t_end = std_min(t_end, heap[span].end_t);
        if t_start > t_end {
            return 0;
        }
        let perp_s: SkDVector = if t_start == heap[span].start_t {
            let mut coin_s = SkTCoincident::default();
            coin_s.set_perp(
                &self.curve,
                heap[span].start_t,
                &span.point_first(heap),
                &opp.curve,
            );
            span.point_first(heap) - *coin_s.perp_pt()
        } else if swap {
            *o_coin_e.perp_pt() - opp_span.point_last(heap)
        } else {
            *o_coin_s.perp_pt() - opp_span.point_first(heap)
        };
        let perp_e: SkDVector = if t_end == heap[span].end_t {
            let mut coin_e = SkTCoincident::default();
            coin_e.set_perp(
                &self.curve,
                heap[span].end_t,
                &span.point_last(heap),
                &opp.curve,
            );
            span.point_last(heap) - *coin_e.perp_pt()
        } else if swap {
            *o_coin_s.perp_pt() - opp_span.point_first(heap)
        } else {
            *o_coin_e.perp_pt() - opp_span.point_last(heap)
        };
        if perp_s.dot(&perp_e) >= 0.0 {
            return 0;
        }
        let mut coin_w = SkTCoincident::default();
        let mut work_t = t_start;
        let mut t_step = t_end - t_start;
        let mut work_pt;
        loop {
            t_step *= 0.5;
            if precisely_zero(t_step) {
                return 0;
            }
            work_t += t_step;
            work_pt = self.curve.pt_at_t(work_t);
            coin_w.set_perp(&self.curve, work_t, &work_pt, &opp.curve);
            let perp_t = coin_w.perp_t();
            if if coin_w.is_match() {
                !between(heap[opp_span].start_t, perp_t, heap[opp_span].end_t)
            } else {
                perp_t < 0.0
            } {
                continue;
            }
            let perp_w = work_pt - *coin_w.perp_pt();
            if (perp_s.dot(&perp_w) >= 0.0) == (t_step < 0.0) {
                t_step = -t_step;
            }
            if work_pt.approximately_equal(coin_w.perp_pt()) {
                break;
            }
        }
        let opp_t_test = coin_w.perp_t();
        if !opp.head.unwrap().contains(heap, opp_t_test) {
            return 0;
        }
        i.set_max(1);
        i.insert(work_t, opp_t_test, &work_pt);
        1
    }

    pub(crate) fn mark_span_gone(&mut self, heap: &mut SkTSectHeap, span: SkTSpan) -> bool {
        self.active_count -= 1;
        if self.active_count < 0 {
            return false;
        }
        heap[span].next = self.deleted;
        self.deleted = Some(span);
        heap[span].deleted = true;
        true
    }

    pub(crate) fn merge_coincidence(&mut self, heap: &mut SkTSectHeap, sect2: &mut SkTSect) {
        let mut small_limit = 0.0;
        loop {
            // find the smallest unprocessed span
            let mut smaller: Option<SkTSpan> = None;
            let mut test = self.coincident;
            loop {
                let Some(t) = test else {
                    return;
                };
                'body: {
                    if heap[t].start_t < small_limit {
                        break 'body;
                    }
                    if let Some(s) = smaller {
                        if heap[s].end_t < heap[t].start_t {
                            break 'body;
                        }
                    }
                    smaller = Some(t);
                }
                test = heap[t].next;
                if test.is_none() {
                    break;
                }
            }
            let Some(smaller) = smaller else {
                return;
            };
            small_limit = heap[smaller].end_t;
            // find next larger span
            let mut prior: Option<SkTSpan> = None;
            let mut larger: Option<SkTSpan> = None;
            let mut larger_prior: Option<SkTSpan> = None;
            let mut test = self.coincident.unwrap();
            loop {
                'body: {
                    if heap[test].start_t < heap[smaller].end_t {
                        break 'body;
                    }
                    if let Some(l) = larger {
                        if heap[l].start_t < heap[test].start_t {
                            break 'body;
                        }
                    }
                    larger_prior = prior;
                    larger = Some(test);
                }
                prior = Some(test);
                match heap[test].next {
                    Some(next) => test = next,
                    None => break,
                }
            }
            let Some(larger) = larger else {
                continue;
            };
            // check middle t value to see if it is coincident as well
            let mid_t = (heap[smaller].end_t + heap[larger].start_t) / 2.0;
            let mid_pt = self.curve.pt_at_t(mid_t);
            let mut coin = SkTCoincident::default();
            coin.set_perp(&self.curve, mid_t, &mid_pt, &sect2.curve);
            if coin.is_match() {
                heap[smaller].end_t = heap[larger].end_t;
                heap[smaller].coin_end = heap[larger].coin_end;
                if let Some(larger_prior) = larger_prior {
                    heap[larger_prior].next = heap[larger].next;
                } else {
                    self.coincident = heap[larger].next;
                }
            }
        }
    }

    pub(crate) fn point_last(&self) -> SkDPoint {
        self.curve[self.curve.point_last() as usize]
    }

    pub(crate) fn remove_by_perpendicular(
        &mut self,
        heap: &mut SkTSectHeap,
        opp: &mut SkTSect,
    ) -> bool {
        let mut test = self.head.unwrap();
        loop {
            let next = heap[test].next;
            'body: {
                if heap[test].coin_start.perp_t() < 0.0 || heap[test].coin_end.perp_t() < 0.0 {
                    break 'body;
                }
                let start_v = *heap[test].coin_start.perp_pt() - test.point_first(heap);
                let end_v = *heap[test].coin_end.perp_pt() - test.point_last(heap);
                if start_v.dot(&end_v) <= 0.0 {
                    break 'body;
                }
                if !self.remove_spans(heap, test, opp) {
                    return false;
                }
            }
            match next {
                Some(next) => test = next,
                None => break,
            }
        }
        true
    }

    pub(crate) fn recover_collapsed(&mut self, heap: &mut SkTSectHeap) {
        let mut deleted = self.deleted;
        while let Some(d) = deleted {
            let del_next = heap[d].next;
            if heap[d].collapsed {
                // `spanPtr` is `&fHead` while `span_ptr` is None, else `&span_ptr->fNext`.
                let mut span_ptr: Option<SkTSpan> = None;
                let mut span = self.head;
                while let Some(s) = span {
                    if heap[s].end_t <= heap[d].start_t {
                        span_ptr = Some(s);
                        span = heap[s].next;
                    } else {
                        break;
                    }
                }
                heap[d].next = span;
                match span_ptr {
                    None => self.head = Some(d),
                    Some(s) => heap[s].next = Some(d),
                }
            }
            deleted = del_next;
        }
    }

    pub(crate) fn remove_coincident(
        &mut self,
        heap: &mut SkTSectHeap,
        span: SkTSpan,
        is_between: bool,
    ) -> bool {
        if !self.unlink_span(heap, span) {
            return false;
        }
        if is_between || between(0.0, heap[span].coin_start.perp_t(), 1.0) {
            self.active_count -= 1;
            heap[span].next = self.coincident;
            self.coincident = Some(span);
        } else {
            self.mark_span_gone(heap, span);
        }
        true
    }

    pub(crate) fn remove_all_but(
        &mut self,
        heap: &mut SkTSectHeap,
        keep: SkTSpan,
        span: SkTSpan,
        opp: &mut SkTSect,
    ) {
        let mut test_bounded = heap[span].bounded;
        while let Some(tb) = test_bounded {
            let bounded = heap[tb].bounded;
            let next = heap[tb].next;
            // may have been deleted when opp did 'remove all but'
            if bounded != keep && !heap[bounded].deleted {
                let span_emptied = span.remove_bounded(heap, bounded);
                debug_assert!(!span_emptied);
                if bounded.remove_bounded(heap, span) {
                    opp.remove_span(heap, bounded);
                }
            }
            test_bounded = next;
        }
        debug_assert!(!heap[span].deleted);
        debug_assert!(span.find_opp_span(heap, keep).is_some());
        debug_assert!(keep.find_opp_span(heap, span).is_some());
    }

    pub(crate) fn remove_span(&mut self, heap: &mut SkTSectHeap, span: SkTSpan) -> bool {
        self.removed_end_check(heap, span);
        if !self.unlink_span(heap, span) {
            return false;
        }
        self.mark_span_gone(heap, span)
    }

    pub(crate) fn remove_span_range(
        &mut self,
        heap: &mut SkTSectHeap,
        first: SkTSpan,
        last: SkTSpan,
    ) {
        if first == last {
            return;
        }
        let span = first;
        let final_span = heap[last].next;
        let mut next = heap[span].next;
        while let Some(span) = next {
            if Some(span) == final_span {
                break;
            }
            next = heap[span].next;
            self.mark_span_gone(heap, span);
        }
        if let Some(final_span) = final_span {
            heap[final_span].prev = Some(first);
        }
        heap[first].next = final_span;
        // world may not be ready for validation here
    }

    pub(crate) fn remove_spans(
        &mut self,
        heap: &mut SkTSectHeap,
        span: SkTSpan,
        opp: &mut SkTSect,
    ) -> bool {
        let mut bounded = heap[span].bounded;
        while let Some(b) = bounded {
            let span_bounded = heap[b].bounded;
            let next = heap[b].next;
            if span.remove_bounded(heap, span_bounded) {
                // shuffles last into position 0
                self.remove_span(heap, span);
            }
            if span_bounded.remove_bounded(heap, span) {
                opp.remove_span(heap, span_bounded);
            }
            if heap[span].deleted && opp.has_bounded(heap, span) {
                return false;
            }
            bounded = next;
        }
        true
    }

    pub(crate) fn removed_end_check(&mut self, heap: &SkTSectHeap, span: SkTSpan) {
        if heap[span].start_t == 0.0 {
            self.removed_start_t = true;
        }
        if 1.0 == heap[span].end_t {
            self.removed_end_t = true;
        }
    }

    pub(crate) fn reset_removed_ends(&mut self) {
        self.removed_start_t = false;
        self.removed_end_t = false;
    }

    pub(crate) fn span_at_t(
        &mut self,
        heap: &SkTSectHeap,
        t: f64,
        prior_span: &mut Option<SkTSpan>,
    ) -> Option<SkTSpan> {
        let mut test = self.head;
        let mut prev = None;
        while let Some(span) = test {
            if heap[span].end_t < t {
                prev = test;
                test = heap[span].next;
            } else {
                break;
            }
        }
        *prior_span = prev;
        match test {
            Some(span) if heap[span].start_t <= t => Some(span),
            _ => None,
        }
    }

    pub(crate) fn tail(&mut self, heap: &SkTSectHeap) -> Option<SkTSpan> {
        let head = self.head.unwrap();
        let mut result = head;
        let mut next = head;
        let mut safety_net = 100000;
        while let Some(n) = heap[next].next {
            next = n;
            safety_net -= 1;
            if safety_net == 0 {
                return None;
            }
            if heap[next].end_t > heap[result].end_t {
                result = next;
            }
        }
        Some(result)
    }

    /* Each span has a range of opposite spans it intersects. After the span is split in two,
    adjust the range to its new size */
    pub(crate) fn trim(
        &mut self,
        heap: &mut SkTSectHeap,
        span: SkTSpan,
        opp: &mut SkTSect,
    ) -> bool {
        // FAIL_IF
        if !span.init_bounds(heap, &self.curve) {
            return false;
        }
        let mut test_bounded = heap[span].bounded;
        while let Some(tb) = test_bounded {
            let test = heap[tb].bounded;
            let next = heap[tb].next;
            let mut opp_sects = 0;
            let sects = self.intersects(heap, span, opp, test, &mut opp_sects);
            if sects >= 1 {
                if opp_sects == 2 {
                    test.init_bounds(heap, &opp.curve);
                    opp.remove_all_but(heap, span, test, self);
                }
                if sects == 2 {
                    span.init_bounds(heap, &self.curve);
                    self.remove_all_but(heap, test, span, opp);
                    return true;
                }
            } else {
                if span.remove_bounded(heap, test) {
                    self.remove_span(heap, span);
                }
                if test.remove_bounded(heap, span) {
                    opp.remove_span(heap, test);
                }
            }
            test_bounded = next;
        }
        true
    }

    pub(crate) fn unlink_span(&mut self, heap: &mut SkTSectHeap, span: SkTSpan) -> bool {
        let prev = heap[span].prev;
        let next = heap[span].next;
        if let Some(prev) = prev {
            heap[prev].next = next;
            if let Some(next) = next {
                heap[next].prev = Some(prev);
                if heap[next].start_t > heap[next].end_t {
                    return false;
                }
                // world may not be ready for validate here
            }
        } else {
            self.head = next;
            if let Some(next) = next {
                heap[next].prev = None;
            }
        }
        true
    }

    pub(crate) fn update_bounded(
        &mut self,
        heap: &mut SkTSectHeap,
        first: SkTSpan,
        last: SkTSpan,
        opp_first: SkTSpan,
    ) -> bool {
        let mut test = first;
        let final_span = last.next(heap);
        let mut delete_span = false;
        loop {
            delete_span |= test.remove_all_bounded(heap);
            match heap[test].next {
                Some(next) if Some(next) != final_span => test = next,
                _ => break,
            }
        }
        heap[first].bounded = None;
        first.add_bounded(heap, opp_first);
        // cannot call validate until remove span range is called
        delete_span
    }
}

/// `template<typename SkTCurve> static bool is_parallel(const SkDLine&, const SkTCurve&)`.
fn is_parallel(this_line: &SkDLine, opp: &SkTCurve) -> bool {
    if !opp.is_conic() {
        return false; // FIXME : breaks a lot of stuff now
    }
    let mut finds = 0;
    let mut this_perp = SkDLine::default();
    this_perp.pts[0].x = this_line.pts[1].x + (this_line.pts[1].y - this_line.pts[0].y);
    this_perp.pts[0].y = this_line.pts[1].y + (this_line.pts[0].x - this_line.pts[1].x);
    this_perp.pts[1] = this_line.pts[1];
    let mut perp_ray_i = SkIntersections::new();
    perp_ray_i.intersect_ray_t_curve(opp, &this_perp);
    for p_index in 0..perp_ray_i.used() as usize {
        finds += i32::from(
            perp_ray_i
                .pt(p_index)
                .approximately_equal(&this_perp.pts[1]),
        );
    }
    this_perp.pts[1].x = this_line.pts[0].x + (this_line.pts[1].y - this_line.pts[0].y);
    this_perp.pts[1].y = this_line.pts[0].y + (this_line.pts[0].x - this_line.pts[1].x);
    this_perp.pts[0] = this_line.pts[0];
    perp_ray_i.intersect_ray_t_curve(opp, &this_perp);
    for p_index in 0..perp_ray_i.used() as usize {
        finds += i32::from(
            perp_ray_i
                .pt(p_index)
                .approximately_equal(&this_perp.pts[0]),
        );
    }
    finds >= 2
}

/// `SkClosestRecord`. Skia leaves its fields uninitialized until `findEnd` sets them, apart from
/// `fClosest`, which `reset` sets.
#[derive(Clone, Copy, Debug, Default)]
struct SkClosestRecord {
    c1_span: SkTSpan,
    c2_span: SkTSpan,
    c1_start_t: f64,
    c1_end_t: f64,
    c2_start_t: f64,
    c2_end_t: f64,
    closest: f64,
    c1_index: i32,
    c2_index: i32,
}

impl SkClosestRecord {
    fn add_intersection(&self, heap: &SkTSectHeap, intersections: &mut SkIntersections) {
        let r1t = if self.c1_index != 0 {
            self.c1_span.end_t(heap)
        } else {
            self.c1_span.start_t(heap)
        };
        let r2t = if self.c2_index != 0 {
            self.c2_span.end_t(heap)
        } else {
            self.c2_span.start_t(heap)
        };
        intersections.insert(r1t, r2t, &self.c1_span.part(heap)[self.c1_index as usize]);
    }

    fn find_end(
        &mut self,
        heap: &SkTSectHeap,
        span1: SkTSpan,
        span2: SkTSpan,
        c1_index: i32,
        c2_index: i32,
    ) {
        let c1 = span1.part(heap);
        let c2 = span2.part(heap);
        if !c1[c1_index as usize].approximately_equal(&c2[c2_index as usize]) {
            return;
        }
        let dist = c1[c1_index as usize].distance_squared(&c2[c2_index as usize]);
        if self.closest < dist {
            return;
        }
        self.c1_span = span1;
        self.c2_span = span2;
        self.c1_start_t = span1.start_t(heap);
        self.c1_end_t = span1.end_t(heap);
        self.c2_start_t = span2.start_t(heap);
        self.c2_end_t = span2.end_t(heap);
        self.c1_index = c1_index;
        self.c2_index = c2_index;
        self.closest = dist;
    }

    fn mates_with(&self, heap: &SkTSectHeap, mate: &SkClosestRecord) -> bool {
        self.c1_span == mate.c1_span
            || self.c1_span.end_t(heap) == mate.c1_span.start_t(heap)
            || self.c1_span.start_t(heap) == mate.c1_span.end_t(heap)
            || self.c2_span == mate.c2_span
            || self.c2_span.end_t(heap) == mate.c2_span.start_t(heap)
            || self.c2_span.start_t(heap) == mate.c2_span.end_t(heap)
    }

    fn merge(&mut self, mate: &SkClosestRecord) {
        self.c1_span = mate.c1_span;
        self.c2_span = mate.c2_span;
        self.closest = mate.closest;
        self.c1_index = mate.c1_index;
        self.c2_index = mate.c2_index;
    }

    fn reset(&mut self) {
        self.closest = FLT_MAX as f64;
    }

    fn update(&mut self, mate: &SkClosestRecord) {
        self.c1_start_t = std_min(self.c1_start_t, mate.c1_start_t);
        self.c1_end_t = std_max(self.c1_end_t, mate.c1_end_t);
        self.c2_start_t = std_min(self.c2_start_t, mate.c2_start_t);
        self.c2_end_t = std_max(self.c2_end_t, mate.c2_end_t);
    }
}

/// `SkClosestSect`.
struct SkClosestSect {
    // this is oversized so that an extra records can merge into final one
    closest: Vec<SkClosestRecord>,
    used: i32,
}

impl SkClosestSect {
    fn new() -> SkClosestSect {
        let mut record = SkClosestRecord::default();
        record.reset();
        SkClosestSect {
            closest: vec![record],
            used: 0,
        }
    }

    fn find(&mut self, heap: &SkTSectHeap, span1: SkTSpan, span2: SkTSpan) -> bool {
        let used = self.used as usize;
        let record = &mut self.closest[used];
        record.find_end(heap, span1, span2, 0, 0);
        record.find_end(heap, span1, span2, 0, span2.part(heap).point_last());
        record.find_end(heap, span1, span2, span1.part(heap).point_last(), 0);
        record.find_end(
            heap,
            span1,
            span2,
            span1.part(heap).point_last(),
            span2.part(heap).point_last(),
        );
        if record.closest == FLT_MAX as f64 {
            return false;
        }
        let record = self.closest[used];
        for index in 0..used {
            let test = &mut self.closest[index];
            if test.mates_with(heap, &record) {
                if test.closest > record.closest {
                    test.merge(&record);
                }
                test.update(&record);
                self.closest[used].reset();
                return false;
            }
        }
        self.used += 1;
        let mut record = SkClosestRecord::default();
        record.reset();
        self.closest.push(record);
        true
    }

    fn finish(&self, heap: &SkTSectHeap, intersections: &mut SkIntersections) {
        // Skia sorts pointers to the records; here, their indices.
        let mut closest_ptrs: Vec<usize> = (0..self.used as usize).collect();
        sk_t_q_sort_by(&mut closest_ptrs, |a: &usize, b: &usize| {
            self.closest[*a].closest < self.closest[*b].closest
        });
        for index in 0..self.used as usize {
            let test = &self.closest[closest_ptrs[index]];
            test.add_intersection(heap, intersections);
        }
    }
}

/// The curve-curve intersections, defined at the end of SkPathOpsTSect.cpp: each wraps its
/// curves as `SkTCurve`s, makes the two sects (and here their shared heap) and runs
/// `BinarySearch`.
impl SkIntersections {
    /// The body every curve-curve `intersect` shares.
    fn intersect_t_curves(&mut self, curve1: &SkTCurve, curve2: &SkTCurve) -> i32 {
        let mut heap = SkTSectHeap::default();
        let mut sect1 = SkTSect::new(&mut heap, curve1);
        let mut sect2 = SkTSect::new(&mut heap, curve2);
        SkTSect::binary_search(&mut heap, &mut sect1, &mut sect2, self);
        self.used()
    }

    /// `intersect(const SkDQuad& q1, const SkDQuad& q2)`.
    pub(crate) fn intersect_quad_quad(&mut self, q1: &SkDQuad, q2: &SkDQuad) -> i32 {
        let quad1 = SkTCurve::Quad(SkTQuad::new(q1));
        let quad2 = SkTCurve::Quad(SkTQuad::new(q2));
        self.intersect_t_curves(&quad1, &quad2)
    }

    /// `intersect(const SkDConic& c, const SkDQuad& q)`.
    pub(crate) fn intersect_conic_quad(&mut self, c: &SkDConic, q: &SkDQuad) -> i32 {
        let conic = SkTCurve::Conic(SkTConic::new(c));
        let quad = SkTCurve::Quad(SkTQuad::new(q));
        self.intersect_t_curves(&conic, &quad)
    }

    /// `intersect(const SkDConic& c1, const SkDConic& c2)`.
    pub(crate) fn intersect_conic_conic(&mut self, c1: &SkDConic, c2: &SkDConic) -> i32 {
        let conic1 = SkTCurve::Conic(SkTConic::new(c1));
        let conic2 = SkTCurve::Conic(SkTConic::new(c2));
        self.intersect_t_curves(&conic1, &conic2)
    }

    /// `intersect(const SkDCubic& c, const SkDQuad& q)`.
    pub(crate) fn intersect_cubic_quad(&mut self, c: &SkDCubic, q: &SkDQuad) -> i32 {
        let cubic = SkTCurve::Cubic(SkTCubic::new(c));
        let quad = SkTCurve::Quad(SkTQuad::new(q));
        self.intersect_t_curves(&cubic, &quad)
    }

    /// `intersect(const SkDCubic& cu, const SkDConic& co)`.
    pub(crate) fn intersect_cubic_conic(&mut self, cu: &SkDCubic, co: &SkDConic) -> i32 {
        let cubic = SkTCurve::Cubic(SkTCubic::new(cu));
        let conic = SkTCurve::Conic(SkTConic::new(co));
        self.intersect_t_curves(&cubic, &conic)
    }

    /// `intersect(const SkDCubic& c1, const SkDCubic& c2)`.
    pub(crate) fn intersect_cubic_cubic(&mut self, c1: &SkDCubic, c2: &SkDCubic) -> i32 {
        let cubic1 = SkTCurve::Cubic(SkTCubic::new(c1));
        let cubic2 = SkTCurve::Cubic(SkTCubic::new(c2));
        self.intersect_t_curves(&cubic1, &cubic2)
    }
}
