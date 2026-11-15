//! Port of Skia's src/pathops/SkOpSpan.{h,cpp}: the points where a segment meets others.
//!
//! An `SkOpSpanBase` is a t on a segment, with the ring of `SkOpPtT`s (point and t pairs, on
//! this and other segments) that meet there; an `SkOpSpan` is a span base that is not the
//! segment's last, so it also carries the winding of the stretch to the next span.
//!
//! All three are handles into `SkOpGlobalState`. A span and a span base share one storage
//! (`SkOpSpanData`, holding the span-only fields too), as Skia's `SkOpSpan` derives from
//! `SkOpSpanBase`; `SkOpSpan` wraps the span base's handle and dereferences to it, and
//! `up_cast` is the wrap. Skia embeds each span's `SkOpPtT` and never moves it, so the
//! embedded pt-t has the span's index.

use std::ops::{Deref, Index, IndexMut};

use crate::core::float_bits::{std_max, std_min, SK_MIN_S32};
use crate::core::point::SkPoint;
use crate::op_angle::SkOpAngle;
use crate::op_segment::SkOpSegment;
use crate::path_ops_types::{arena_handle, between, zero_or_one, SkOpGlobalState};

arena_handle! {
    /// Stands for `SkOpPtT*`: the pt-t embedded in the span of the same index.
    SkOpPtT
}

impl SkOpPtT {
    /// The pt-t embedded in `span` (`&span->fPtT`).
    pub(crate) fn embedded(span: SkOpSpanBase) -> SkOpPtT {
        SkOpPtT(span.0)
    }
}

/// `SkOpPtT`'s fields: subset of op span used by terminal span (when t is equal to one).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkOpPtTData {
    pub(crate) t: f64,
    /// cache of point value at this t
    pub(crate) pt: SkPoint,
    /// contains winding data
    pub(crate) span: SkOpSpanBase,
    /// intersection on opposite curve or alias on this curve
    pub(crate) next: SkOpPtT,
    /// set if removed from span list
    pub(crate) deleted: bool,
    /// set if at some point a coincident span pointed here (`mutable` in Skia)
    pub(crate) coincident: bool,
}

impl Index<SkOpPtT> for SkOpGlobalState {
    type Output = SkOpPtTData;

    fn index(&self, pt_t: SkOpPtT) -> &SkOpPtTData {
        &self.spans[pt_t.0 as usize].pt_t
    }
}

impl IndexMut<SkOpPtT> for SkOpGlobalState {
    fn index_mut(&mut self, pt_t: SkOpPtT) -> &mut SkOpPtTData {
        &mut self.spans[pt_t.0 as usize].pt_t
    }
}

impl SkOpPtT {
    pub(crate) fn active(self, state: &SkOpGlobalState) -> Option<SkOpPtT> {
        if !state[self].deleted {
            return Some(self);
        }
        let mut pt_t = self;
        let stop_pt_t = pt_t;
        loop {
            pt_t = pt_t.next(state);
            if pt_t == stop_pt_t {
                break;
            }
            if state[pt_t].span == state[self].span && !state[pt_t].deleted {
                return Some(pt_t);
            }
        }
        None // should never return deleted; caller must abort
    }

    // please keep in sync with debugAddOpp()
    pub(crate) fn add_opp(self, state: &mut SkOpGlobalState, opp: SkOpPtT, opp_prev: SkOpPtT) {
        let old_next = state[self].next;
        debug_assert!(self != opp);
        state[self].next = opp;
        debug_assert!(opp_prev != old_next);
        state[opp_prev].next = old_next;
    }

    pub(crate) fn coincident(self, state: &SkOpGlobalState) -> bool {
        state[self].coincident
    }

    /// `contains(const SkOpPtT*)`.
    pub(crate) fn contains_pt_t(self, state: &SkOpGlobalState, check: SkOpPtT) -> bool {
        let mut pt_t = self;
        let stop_pt_t = pt_t;
        loop {
            pt_t = pt_t.next(state);
            if pt_t == stop_pt_t {
                break;
            }
            if pt_t == check {
                return true;
            }
        }
        false
    }

    /// `contains(const SkOpSegment*, double t)`.
    pub(crate) fn contains_segment_t(
        self,
        state: &SkOpGlobalState,
        segment: SkOpSegment,
        t: f64,
    ) -> bool {
        let mut pt_t = self;
        let stop_pt_t = pt_t;
        loop {
            pt_t = pt_t.next(state);
            if pt_t == stop_pt_t {
                break;
            }
            if state[pt_t].t == t && pt_t.segment(state) == segment {
                return true;
            }
        }
        false
    }

    /// `contains(const SkOpSegment*)`.
    pub(crate) fn contains_segment(
        self,
        state: &SkOpGlobalState,
        check: SkOpSegment,
    ) -> Option<SkOpPtT> {
        debug_assert!(self.segment(state) != check);
        let mut pt_t = self;
        let stop_pt_t = pt_t;
        loop {
            pt_t = pt_t.next(state);
            if pt_t == stop_pt_t {
                break;
            }
            if pt_t.segment(state) == check && !pt_t.deleted(state) {
                return Some(pt_t);
            }
        }
        None
    }

    pub(crate) fn deleted(self, state: &SkOpGlobalState) -> bool {
        state[self].deleted
    }

    pub(crate) fn find(self, state: &SkOpGlobalState, segment: SkOpSegment) -> Option<SkOpPtT> {
        let mut pt_t = self;
        let stop_pt_t = pt_t;
        loop {
            if pt_t.segment(state) == segment && !pt_t.deleted(state) {
                return Some(pt_t);
            }
            pt_t = state[pt_t].next;
            if stop_pt_t == pt_t {
                break;
            }
        }
        None
    }

    pub(crate) fn init(
        self,
        state: &mut SkOpGlobalState,
        span: SkOpSpanBase,
        t: f64,
        pt: &SkPoint,
    ) {
        let data = &mut state[self];
        data.t = t;
        data.pt = *pt;
        data.span = span;
        data.next = self;
        data.deleted = false;
        data.coincident = false;
    }

    pub(crate) fn insert(self, state: &mut SkOpGlobalState, span: SkOpPtT) {
        debug_assert!(span != self);
        state[span].next = state[self].next;
        state[self].next = span;
    }

    pub(crate) fn next(self, state: &SkOpGlobalState) -> SkOpPtT {
        state[self].next
    }

    // returns nullptr if this is already in the opp ptT loop
    pub(crate) fn opp_prev(self, state: &SkOpGlobalState, opp: SkOpPtT) -> Option<SkOpPtT> {
        // find the fOpp ptr to opp
        let mut opp_prev = state[opp].next;
        if opp_prev == self {
            return None;
        }
        while state[opp_prev].next != opp {
            opp_prev = state[opp_prev].next;
            if opp_prev == self {
                return None;
            }
        }
        Some(opp_prev)
    }

    pub(crate) fn overlaps(
        state: &SkOpGlobalState,
        s1: SkOpPtT,
        e1: SkOpPtT,
        s2: SkOpPtT,
        e2: SkOpPtT,
        s_out: &mut Option<SkOpPtT>,
        e_out: &mut Option<SkOpPtT>,
    ) -> bool {
        let start1 = if state[s1].t < state[e1].t { s1 } else { e1 };
        let start2 = if state[s2].t < state[e2].t { s2 } else { e2 };
        *s_out = if between(state[s1].t, state[start2].t, state[e1].t) {
            Some(start2)
        } else if between(state[s2].t, state[start1].t, state[e2].t) {
            Some(start1)
        } else {
            None
        };
        let end1 = if state[s1].t < state[e1].t { e1 } else { s1 };
        let end2 = if state[s2].t < state[e2].t { e2 } else { s2 };
        *e_out = if between(state[s1].t, state[end2].t, state[e1].t) {
            Some(end2)
        } else if between(state[s2].t, state[end1].t, state[e2].t) {
            Some(end1)
        } else {
            None
        };
        if *s_out == *e_out {
            return false;
        }
        debug_assert!(s_out.is_none() || *s_out != *e_out);
        s_out.is_some() && e_out.is_some()
    }

    pub(crate) fn pt_already_seen(self, state: &SkOpGlobalState, head: SkOpPtT) -> bool {
        let mut check = head;
        while self != check {
            if state[self].pt == state[check].pt {
                return true;
            }
            check = state[check].next;
        }
        false
    }

    pub(crate) fn prev(self, state: &SkOpGlobalState) -> SkOpPtT {
        let mut result = self;
        let mut next = self;
        loop {
            next = state[next].next;
            if next == self {
                break;
            }
            result = next;
        }
        debug_assert!(state[result].next == self);
        result
    }

    pub(crate) fn segment(self, state: &SkOpGlobalState) -> SkOpSegment {
        self.span(state).segment(state)
    }

    /// `const` in Skia, writing its `mutable` flag.
    pub(crate) fn set_coincident(self, state: &mut SkOpGlobalState) {
        state[self].coincident = true;
    }

    pub(crate) fn set_deleted(self, state: &mut SkOpGlobalState) {
        state[self].deleted = true;
    }

    pub(crate) fn set_span(self, state: &mut SkOpGlobalState, span: SkOpSpanBase) {
        state[self].span = span;
    }

    pub(crate) fn span(self, state: &SkOpGlobalState) -> SkOpSpanBase {
        state[self].span
    }

    pub(crate) fn starter(self, state: &SkOpGlobalState, end: SkOpPtT) -> SkOpPtT {
        if state[self].t < state[end].t {
            self
        } else {
            end
        }
    }
}

arena_handle! {
    /// Stands for `SkOpSpanBase*`.
    SkOpSpanBase
}

/// Stands for `SkOpSpan*`: a span base known not to be its segment's last. It compares equal
/// to the `SkOpSpanBase` of the same index, as Skia's pointers do.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub(crate) struct SkOpSpan(pub(crate) SkOpSpanBase);

impl Deref for SkOpSpan {
    type Target = SkOpSpanBase;

    fn deref(&self) -> &SkOpSpanBase {
        &self.0
    }
}

impl PartialEq<SkOpSpanBase> for SkOpSpan {
    fn eq(&self, other: &SkOpSpanBase) -> bool {
        self.0 == *other
    }
}

impl PartialEq<SkOpSpan> for SkOpSpanBase {
    fn eq(&self, other: &SkOpSpan) -> bool {
        *self == other.0
    }
}

impl From<SkOpSpan> for SkOpSpanBase {
    fn from(span: SkOpSpan) -> SkOpSpanBase {
        span.0
    }
}

/// `SkOpSpanBase::Collapsed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Collapsed {
    No,
    Yes,
    Error,
}

/// The fields of `SkOpSpanBase` and of `SkOpSpan`, which a span base leaves unused.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkOpSpanData {
    /// list of points and t values associated with the start of this span
    pub(crate) pt_t: SkOpPtTData,
    /// segment that contains this span
    pub(crate) segment: SkOpSegment,
    /// linked list of coincident spans that end here (may point to itself)
    pub(crate) coin_end: SkOpSpanBase,
    /// points to next angle from span start to end
    pub(crate) from_angle: Option<SkOpAngle>,
    /// previous intersection point
    pub(crate) prev: Option<SkOpSpan>,
    /// number of times intersections have been added to span
    pub(crate) span_adds: i32,
    /// set after span has been added to chase array
    pub(crate) chased: bool,
    // SkOpSpan's fields:
    /// linked list of spans coincident with this one (may point to itself)
    pub(crate) coincident: SkOpSpan,
    /// points to next angle from span start to end
    pub(crate) to_angle: Option<SkOpAngle>,
    /// next intersection point
    pub(crate) next: Option<SkOpSpanBase>,
    /// accumulated from contours surrounding this one.
    pub(crate) wind_sum: i32,
    /// for binary operators: the opposite winding sum
    pub(crate) opp_sum: i32,
    /// 0 == canceled; 1 == normal; >1 == coincident
    pub(crate) wind_value: i32,
    /// normally 0 -- when binary coincident edges combine, opp value goes here
    pub(crate) opp_value: i32,
    /// specifies direction and t value to try next
    pub(crate) top_t_try: i32,
    /// if set, this span to next higher T has been processed
    pub(crate) done: bool,
    pub(crate) already_added: bool,
}

impl Index<SkOpSpanBase> for SkOpGlobalState {
    type Output = SkOpSpanData;

    fn index(&self, span: SkOpSpanBase) -> &SkOpSpanData {
        &self.spans[span.0 as usize]
    }
}

impl IndexMut<SkOpSpanBase> for SkOpGlobalState {
    fn index_mut(&mut self, span: SkOpSpanBase) -> &mut SkOpSpanData {
        &mut self.spans[span.0 as usize]
    }
}

impl Index<SkOpSpan> for SkOpGlobalState {
    type Output = SkOpSpanData;

    fn index(&self, span: SkOpSpan) -> &SkOpSpanData {
        &self.spans[span.0 .0 as usize]
    }
}

impl IndexMut<SkOpSpan> for SkOpGlobalState {
    fn index_mut(&mut self, span: SkOpSpan) -> &mut SkOpSpanData {
        &mut self.spans[span.0 .0 as usize]
    }
}

impl SkOpSpanBase {
    pub(crate) fn add_opp(self, state: &mut SkOpGlobalState, opp: SkOpSpanBase) -> bool {
        let Some(opp_prev) = self.pt_t().opp_prev(state, opp.pt_t()) else {
            return true;
        };
        if !self.merge_matches(state, opp) {
            return false;
        }
        self.pt_t().add_opp(state, opp.pt_t(), opp_prev);
        self.check_for_collapsed_coincidence(state);
        true
    }

    pub(crate) fn bump_span_adds(self, state: &mut SkOpGlobalState) {
        state[self].span_adds += 1;
    }

    pub(crate) fn chased(self, state: &SkOpGlobalState) -> bool {
        state[self].chased
    }

    // please keep in sync with debugCheckForCollapsedCoincidence()
    pub(crate) fn check_for_collapsed_coincidence(self, state: &mut SkOpGlobalState) {
        let coins = state
            .coincidence()
            .expect("the global state has a coincidence");
        if coins.is_empty(state) {
            return;
        }
        // the insert above may have put both ends of a coincident run in the same span
        // for each coincident ptT in loop; see if its opposite in is also in the loop
        // this implementation is the motivation for marking that a ptT is referenced by a
        // coincident span
        let head = self.pt_t();
        let mut test = head;
        loop {
            if test.coincident(state) {
                coins.mark_collapsed(state, test);
            }
            test = test.next(state);
            if test == head {
                break;
            }
        }
        coins.release_deleted(state);
    }

    pub(crate) fn collapsed(self, state: &SkOpGlobalState, s: f64, e: f64) -> Collapsed {
        let start = self.pt_t();
        let mut start_next: Option<SkOpPtT> = None;
        let mut walk = start;
        let mut min = state[walk].t;
        let mut max = min;
        let segment = self.segment(state);
        let mut safety_net = 100000;
        loop {
            walk = walk.next(state);
            if walk == start {
                break;
            }
            safety_net -= 1;
            if safety_net == 0 {
                return Collapsed::Error;
            }
            if Some(walk) == start_next {
                return Collapsed::Error;
            }
            if walk.segment(state) != segment {
                continue;
            }
            min = std_min(min, state[walk].t);
            max = std_max(max, state[walk].t);
            if between(min, s, max) && between(min, e, max) {
                return Collapsed::Yes;
            }
            start_next = Some(start.next(state));
        }
        Collapsed::No
    }

    /// `contains(const SkOpSpanBase*)`.
    pub(crate) fn contains_span(self, state: &SkOpGlobalState, span: SkOpSpanBase) -> bool {
        let start = self.pt_t();
        let check = span.pt_t();
        let mut walk = start;
        loop {
            walk = walk.next(state);
            if walk == start {
                break;
            }
            if walk == check {
                return true;
            }
        }
        false
    }

    /// `contains(const SkOpSegment*)`.
    pub(crate) fn contains_segment(
        self,
        state: &SkOpGlobalState,
        segment: SkOpSegment,
    ) -> Option<SkOpPtT> {
        let start = self.pt_t();
        let mut walk = start;
        loop {
            walk = walk.next(state);
            if walk == start {
                break;
            }
            if walk.deleted(state) {
                continue;
            }
            if walk.segment(state) == segment && walk.span(state).pt_t() == walk {
                return Some(walk);
            }
        }
        None
    }

    /// `containsCoinEnd(const SkOpSpanBase* coin)`.
    pub(crate) fn contains_coin_end(self, state: &SkOpGlobalState, coin: SkOpSpanBase) -> bool {
        debug_assert!(self != coin);
        let mut next = self;
        loop {
            next = state[next].coin_end;
            if next == self {
                break;
            }
            if next == coin {
                return true;
            }
        }
        false
    }

    /// `containsCoinEnd(const SkOpSegment*)`.
    pub(crate) fn contains_coin_end_segment(
        self,
        state: &SkOpGlobalState,
        segment: SkOpSegment,
    ) -> bool {
        debug_assert!(self.segment(state) != segment);
        let mut next = self;
        loop {
            next = state[next].coin_end;
            if next == self {
                break;
            }
            if next.segment(state) == segment {
                return true;
            }
        }
        false
    }

    pub(crate) fn deleted(self, state: &SkOpGlobalState) -> bool {
        state[self].pt_t.deleted
    }

    pub(crate) fn r#final(self, state: &SkOpGlobalState) -> bool {
        state[self].pt_t.t == 1.0
    }

    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn from_angle(self, state: &SkOpGlobalState) -> Option<SkOpAngle> {
        state[self].from_angle
    }

    pub(crate) fn init_base(
        self,
        state: &mut SkOpGlobalState,
        parent: SkOpSegment,
        prev: Option<SkOpSpan>,
        t: f64,
        pt: &SkPoint,
    ) {
        state[self].segment = parent;
        self.pt_t().init(state, self, t, pt);
        let data = &mut state[self];
        data.coin_end = self;
        data.from_angle = None;
        data.prev = prev;
        data.span_adds = 0;
        data.chased = false;
    }

    // Please keep this in sync with debugInsertCoinEnd()
    pub(crate) fn insert_coin_end(self, state: &mut SkOpGlobalState, coin: SkOpSpanBase) {
        if self.contains_coin_end(state, coin) {
            debug_assert!(coin.contains_coin_end(state, self));
            return;
        }
        debug_assert!(self != coin);
        let coin_next = state[coin].coin_end;
        state[coin].coin_end = state[self].coin_end;
        state[self].coin_end = coin_next;
    }

    // this pair of spans share a common t value or point; merge them and eliminate duplicates
    // this does not compute the best t or pt value; this merely moves all data into a single list
    pub(crate) fn merge(self, state: &mut SkOpGlobalState, span: SkOpSpan) {
        let span_pt_t = span.pt_t();
        debug_assert!(self.t(state) != state[span_pt_t].t);
        debug_assert!(!zero_or_one(state[span_pt_t].t));
        span.release(state, self.pt_t());
        if self.contains_span(state, span.0) {
            return; // merge is already in the ptT loop
        }
        let mut remainder = span_pt_t.next(state);
        self.pt_t().insert(state, span_pt_t);
        while remainder != span_pt_t {
            let next = remainder.next(state);
            let mut compare = span_pt_t.next(state);
            let mut already_in_loop = false;
            while compare != span_pt_t {
                let next_c = compare.next(state);
                if next_c.span(state) == remainder.span(state)
                    && state[next_c].t == state[remainder].t
                {
                    already_in_loop = true;
                    break;
                }
                compare = next_c;
            }
            if !already_in_loop {
                span_pt_t.insert(state, remainder);
            }
            remainder = next;
        }
        state[self].span_adds += state[span].span_adds;
    }

    // please keep in sync with debugMergeMatches()
    // Look to see if pt-t linked list contains same segment more than once
    // if so, and if each pt-t is directly pointed to by spans in that segment,
    // merge them
    // keep the points, but remove spans so that the segment doesn't have 2 or more
    // spans pointing to the same pt-t loop at different loop elements
    pub(crate) fn merge_matches(self, state: &mut SkOpGlobalState, opp: SkOpSpanBase) -> bool {
        let mut test = self.pt_t();
        let stop = test;
        let mut safety_hatch = 1000000;
        loop {
            safety_hatch -= 1;
            if safety_hatch == 0 {
                return false;
            }
            let test_next = test.next(state);
            'test_body: {
                if test.deleted(state) {
                    break 'test_body;
                }
                let test_base = test.span(state);
                debug_assert!(test_base.pt_t() == test);
                let segment = test.segment(state);
                if segment.done(state) {
                    break 'test_body;
                }
                let mut inner = opp.pt_t();
                let inner_stop = inner;
                'inner_loop: loop {
                    'inner_body: {
                        if inner.segment(state) != segment {
                            break 'inner_body;
                        }
                        if inner.deleted(state) {
                            break 'inner_body;
                        }
                        let inner_base = inner.span(state);
                        debug_assert!(inner_base.pt_t() == inner);
                        // when the intersection is first detected, the span base is marked if
                        // there are more than one point in the intersection.
                        if !zero_or_one(state[inner].t) {
                            inner_base.up_cast().release(state, test);
                        } else if !zero_or_one(state[test].t) {
                            test_base.up_cast().release(state, inner);
                        } else {
                            segment.mark_all_done(state); // mark segment as collapsed
                            test.set_deleted(state);
                            inner.set_deleted(state);
                        }
                        break 'inner_loop;
                    }
                    inner = inner.next(state);
                    if inner == inner_stop {
                        break;
                    }
                }
            }
            test = test_next;
            if test == stop {
                break;
            }
        }
        self.check_for_collapsed_coincidence(state);
        true
    }

    pub(crate) fn prev(self, state: &SkOpGlobalState) -> Option<SkOpSpan> {
        state[self].prev
    }

    pub(crate) fn pt(self, state: &SkOpGlobalState) -> SkPoint {
        state[self].pt_t.pt
    }

    /// `&fPtT`.
    pub(crate) fn pt_t(self) -> SkOpPtT {
        SkOpPtT::embedded(self)
    }

    pub(crate) fn segment(self, state: &SkOpGlobalState) -> SkOpSegment {
        state[self].segment
    }

    pub(crate) fn set_chased(self, state: &mut SkOpGlobalState, chased: bool) {
        state[self].chased = chased;
    }

    pub(crate) fn set_from_angle(self, state: &mut SkOpGlobalState, angle: Option<SkOpAngle>) {
        state[self].from_angle = angle;
    }

    pub(crate) fn set_prev(self, state: &mut SkOpGlobalState, prev: Option<SkOpSpan>) {
        state[self].prev = prev;
    }

    pub(crate) fn simple(self, state: &SkOpGlobalState) -> bool {
        let pt_t = self.pt_t();
        pt_t.next(state).next(state) == pt_t
    }

    pub(crate) fn span_adds_count(self, state: &SkOpGlobalState) -> i32 {
        state[self].span_adds
    }

    /// `starter(const SkOpSpanBase* end)`, both overloads.
    pub(crate) fn starter(self, state: &SkOpGlobalState, end: SkOpSpanBase) -> SkOpSpan {
        let result = if self.t(state) < end.t(state) {
            self
        } else {
            end
        };
        result.up_cast()
    }

    pub(crate) fn step(self, state: &SkOpGlobalState, end: SkOpSpanBase) -> i32 {
        if self.t(state) < end.t(state) {
            1
        } else {
            -1
        }
    }

    pub(crate) fn t(self, state: &SkOpGlobalState) -> f64 {
        state[self].pt_t.t
    }

    /// Skia asserts `!final()`; the cast itself needs no state.
    pub(crate) fn up_cast(self) -> SkOpSpan {
        SkOpSpan(self)
    }

    pub(crate) fn up_castable(self, state: &SkOpGlobalState) -> Option<SkOpSpan> {
        if self.r#final(state) {
            None
        } else {
            Some(self.up_cast())
        }
    }
}

impl SkOpSpan {
    pub(crate) fn already_added(self, state: &SkOpGlobalState) -> bool {
        state[self].already_added
    }

    pub(crate) fn compute_wind_sum(self, state: &mut SkOpGlobalState) -> i32 {
        let contour_head = state.contour_head();
        let mut wind_try = 0;
        while !self.sortable_top(state, *contour_head) && {
            wind_try += 1;
            wind_try < SkOpGlobalState::K_MAX_WINDING_TRIES
        } {}
        self.wind_sum(state)
    }

    /// `containsCoincidence(const SkOpSegment*)`.
    pub(crate) fn contains_coincidence_segment(
        self,
        state: &SkOpGlobalState,
        segment: SkOpSegment,
    ) -> bool {
        debug_assert!(self.segment(state) != segment);
        let mut next = state[self].coincident;
        loop {
            if next.segment(state) == segment {
                return true;
            }
            next = state[next].coincident;
            if next == self {
                break;
            }
        }
        false
    }

    /// `containsCoincidence(const SkOpSpan* coin)`.
    pub(crate) fn contains_coincidence(self, state: &SkOpGlobalState, coin: SkOpSpan) -> bool {
        debug_assert!(self != coin);
        let mut next = self;
        loop {
            next = state[next].coincident;
            if next == self {
                break;
            }
            if next == coin {
                return true;
            }
        }
        false
    }

    pub(crate) fn done(self, state: &SkOpGlobalState) -> bool {
        debug_assert!(!self.r#final(state));
        state[self].done
    }

    pub(crate) fn init(
        self,
        state: &mut SkOpGlobalState,
        parent: SkOpSegment,
        prev: Option<SkOpSpan>,
        t: f64,
        pt: &SkPoint,
    ) {
        debug_assert!(t != 1.0);
        self.init_base(state, parent, prev, t, pt);
        let data = &mut state[self];
        data.coincident = self;
        data.to_angle = None;
        data.opp_sum = SK_MIN_S32;
        data.wind_sum = data.opp_sum;
        data.wind_value = 1;
        data.opp_value = 0;
        data.top_t_try = 0;
        data.done = false;
        data.chased = data.done;
        parent.bump_count(state);
        state[self].already_added = false;
    }

    // Please keep this in sync with debugInsertCoincidence()
    /// `insertCoincidence(const SkOpSegment*, bool flipped, bool ordered)`.
    pub(crate) fn insert_coincidence_segment(
        self,
        state: &mut SkOpGlobalState,
        segment: SkOpSegment,
        flipped: bool,
        ordered: bool,
    ) -> bool {
        if self.contains_coincidence_segment(state, segment) {
            return true;
        }
        let pt_t = self.pt_t();
        let mut next = pt_t;
        loop {
            next = next.next(state);
            if next == pt_t {
                break;
            }
            if next.segment(state) == segment {
                let base = next.span(state);
                let span = if !ordered {
                    let span_end_pt_t = state[self]
                        .next
                        .expect("a span has a next span")
                        .contains_segment(state, segment);
                    let Some(span_end_pt_t) = span_end_pt_t else {
                        return false;
                    };
                    let span_end = span_end_pt_t.span(state);
                    let start = base.pt_t().starter(state, span_end.pt_t());
                    if start.span(state).up_castable(state).is_none() {
                        return false;
                    }
                    start.span(state).up_cast()
                } else if flipped {
                    let Some(prev) = base.prev(state) else {
                        return false;
                    };
                    prev
                } else {
                    if base.up_castable(state).is_none() {
                        return false;
                    }
                    base.up_cast()
                };
                self.insert_coincidence(state, span);
                return true;
            }
        }
        true
    }

    // Please keep this in sync with debugInsertCoincidence()
    /// `insertCoincidence(SkOpSpan* coin)`.
    pub(crate) fn insert_coincidence(self, state: &mut SkOpGlobalState, coin: SkOpSpan) {
        if self.contains_coincidence(state, coin) {
            debug_assert!(coin.contains_coincidence(state, self));
            return;
        }
        debug_assert!(self != coin);
        let coin_next = state[coin].coincident;
        state[coin].coincident = state[self].coincident;
        state[self].coincident = coin_next;
    }

    pub(crate) fn is_canceled(self, state: &SkOpGlobalState) -> bool {
        debug_assert!(!self.r#final(state));
        state[self].wind_value == 0 && state[self].opp_value == 0
    }

    pub(crate) fn mark_added(self, state: &mut SkOpGlobalState) {
        state[self].already_added = true;
    }

    pub(crate) fn next(self, state: &SkOpGlobalState) -> Option<SkOpSpanBase> {
        debug_assert!(!self.r#final(state));
        state[self].next
    }

    pub(crate) fn opp_sum(self, state: &SkOpGlobalState) -> i32 {
        debug_assert!(!self.r#final(state));
        state[self].opp_sum
    }

    pub(crate) fn opp_value(self, state: &SkOpGlobalState) -> i32 {
        debug_assert!(!self.r#final(state));
        state[self].opp_value
    }

    pub(crate) fn release(self, state: &mut SkOpGlobalState, kept: SkOpPtT) {
        debug_assert!(!self.r#final(state));
        let prev = self
            .prev(state)
            .expect("a released span has a previous span");
        let next = self.next(state).expect("a released span has a next span");
        prev.set_next(state, Some(next));
        next.set_prev(state, Some(prev));
        self.segment(state).release(state, self);
        if let Some(coincidence) = state.coincidence() {
            coincidence.fix_up(state, self.pt_t(), kept);
        }
        self.pt_t().set_deleted(state);
        let stop_pt_t = self.pt_t();
        let mut test_pt_t = stop_pt_t;
        let kept_span = kept.span(state);
        loop {
            if self == test_pt_t.span(state) {
                test_pt_t.set_span(state, kept_span);
            }
            test_pt_t = test_pt_t.next(state);
            if test_pt_t == stop_pt_t {
                break;
            }
        }
    }

    pub(crate) fn set_done(self, state: &mut SkOpGlobalState, done: bool) {
        debug_assert!(!self.r#final(state));
        state[self].done = done;
    }

    pub(crate) fn set_next(self, state: &mut SkOpGlobalState, next_t: Option<SkOpSpanBase>) {
        debug_assert!(!self.r#final(state));
        state[self].next = next_t;
    }

    pub(crate) fn set_opp_sum(self, state: &mut SkOpGlobalState, opp_sum: i32) {
        debug_assert!(!self.r#final(state));
        if state[self].opp_sum != SK_MIN_S32 && state[self].opp_sum != opp_sum {
            return;
        }
        state[self].opp_sum = opp_sum;
    }

    pub(crate) fn set_opp_value(self, state: &mut SkOpGlobalState, opp_value: i32) {
        debug_assert!(!self.r#final(state));
        debug_assert!(state[self].opp_sum == SK_MIN_S32);
        state[self].opp_value = opp_value;
    }

    pub(crate) fn set_to_angle(self, state: &mut SkOpGlobalState, angle: Option<SkOpAngle>) {
        debug_assert!(!self.r#final(state));
        state[self].to_angle = angle;
    }

    pub(crate) fn set_wind_sum(self, state: &mut SkOpGlobalState, wind_sum: i32) {
        debug_assert!(!self.r#final(state));
        if state[self].wind_sum != SK_MIN_S32 && state[self].wind_sum != wind_sum {
            return;
        }
        state[self].wind_sum = wind_sum;
    }

    pub(crate) fn set_wind_value(self, state: &mut SkOpGlobalState, wind_value: i32) {
        debug_assert!(!self.r#final(state));
        debug_assert!(wind_value >= 0);
        debug_assert!(state[self].wind_sum == SK_MIN_S32);
        state[self].wind_value = wind_value;
    }

    // `sortableTop` is defined in SkPathOpsWinding.cpp: see `path_ops_winding.rs`.

    pub(crate) fn to_angle(self, state: &SkOpGlobalState) -> Option<SkOpAngle> {
        debug_assert!(!self.r#final(state));
        state[self].to_angle
    }

    pub(crate) fn wind_sum(self, state: &SkOpGlobalState) -> i32 {
        debug_assert!(!self.r#final(state));
        state[self].wind_sum
    }

    pub(crate) fn wind_value(self, state: &SkOpGlobalState) -> i32 {
        state[self].wind_value
    }
}
