//! Port of Skia's src/pathops/SkOpCoincidence.{h,cpp}: the runs where two segments coincide,
//! and their resolution into the windings of the spans they cover.
//!
//! `SkCoincidentSpans` and `SkOpCoincidence` are handles into `SkOpGlobalState`. An operation
//! has two coincidences (`HandleCoincidence` makes a second, `overlaps`, whose constructor
//! takes over the global state's pointer), so `SkOpCoincidence` indexes its own arena rather
//! than standing for a single set of fields.
//!
//! Skia walks its lists through `SkCoincidentSpans**`, the address of a list head (`&fHead`,
//! `&fTop`) or of a node's `fNext`; `CoinListLink` names that address.

use std::ops::{Index, IndexMut};

use crate::core::float_bits::{std_max, std_min};
use crate::core::point::SkPoint;
use crate::intersections::SkIntersections;
use crate::op_segment::SkOpSegment;
use crate::op_span::{Collapsed, SkOpPtT, SkOpSpan, SkOpSpanBase};
use crate::path_ops_curve::CURVE_INTERSECT_RAY;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::SkDPoint;
use crate::path_ops_types::{
    arena_handle, between, sk_path_ops_verb_to_points, zero_or_one, SkOpGlobalState,
};

arena_handle! {
    /// Stands for `SkCoincidentSpans*`.
    SkCoincidentSpans
}

/// `SkCoincidentSpans`'s fields; `init` zeroes them, which is all `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SkCoincidentSpansData {
    pub(crate) next: Option<SkCoincidentSpans>,
    pub(crate) coin_pt_t_start: Option<SkOpPtT>,
    pub(crate) coin_pt_t_end: Option<SkOpPtT>,
    pub(crate) opp_pt_t_start: Option<SkOpPtT>,
    pub(crate) opp_pt_t_end: Option<SkOpPtT>,
}

impl Index<SkCoincidentSpans> for SkOpGlobalState {
    type Output = SkCoincidentSpansData;

    fn index(&self, spans: SkCoincidentSpans) -> &SkCoincidentSpansData {
        &self.coincident_spans[spans.0 as usize]
    }
}

impl IndexMut<SkCoincidentSpans> for SkOpGlobalState {
    fn index_mut(&mut self, spans: SkCoincidentSpans) -> &mut SkCoincidentSpansData {
        &mut self.coincident_spans[spans.0 as usize]
    }
}

/// Stands for `SkCoincidentSpans**`: where a coincidence list's link is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoinListLink {
    /// `&coincidence->fHead`.
    Head(SkOpCoincidence),
    /// `&coincidence->fTop`.
    Top(SkOpCoincidence),
    /// `spans->nextPtr()`: `&spans->fNext`.
    Next(SkCoincidentSpans),
}

impl CoinListLink {
    /// `*headPtr`.
    pub(crate) fn get(self, state: &SkOpGlobalState) -> Option<SkCoincidentSpans> {
        match self {
            CoinListLink::Head(coincidence) => state[coincidence].head,
            CoinListLink::Top(coincidence) => state[coincidence].top,
            CoinListLink::Next(spans) => state[spans].next,
        }
    }

    /// `*headPtr = value`.
    pub(crate) fn set(self, state: &mut SkOpGlobalState, value: Option<SkCoincidentSpans>) {
        match self {
            CoinListLink::Head(coincidence) => state[coincidence].head = value,
            CoinListLink::Top(coincidence) => state[coincidence].top = value,
            CoinListLink::Next(spans) => state[spans].next = value,
        }
    }
}

/// `SkCoincidentSpans::correctOneEnd`'s getter, a pointer to member function in Skia.
pub(crate) type CoinGetEnd = fn(SkCoincidentSpans, &SkOpGlobalState) -> SkOpPtT;
/// `SkCoincidentSpans::correctOneEnd`'s setter, a pointer to member function in Skia.
pub(crate) type CoinSetEnd = fn(SkCoincidentSpans, &mut SkOpGlobalState, SkOpPtT);

impl SkCoincidentSpans {
    // out of line since this function is referenced by address
    pub(crate) fn coin_pt_t_end(self, state: &SkOpGlobalState) -> SkOpPtT {
        state[self]
            .coin_pt_t_end
            .expect("a set coincident span has its ends")
    }

    // out of line since this function is referenced by address
    pub(crate) fn coin_pt_t_start(self, state: &SkOpGlobalState) -> SkOpPtT {
        state[self]
            .coin_pt_t_start
            .expect("a set coincident span has its ends")
    }

    // These return non-const pointers so that, as copies, they can be added
    // to a new span pair
    pub(crate) fn coin_pt_t_end_writable(self, state: &SkOpGlobalState) -> SkOpPtT {
        state[self]
            .coin_pt_t_end
            .expect("a set coincident span has its ends")
    }

    pub(crate) fn coin_pt_t_start_writable(self, state: &SkOpGlobalState) -> SkOpPtT {
        state[self]
            .coin_pt_t_start
            .expect("a set coincident span has its ends")
    }

    // returns true if coincident span's start and end are the same
    pub(crate) fn collapsed(self, state: &SkOpGlobalState, test: SkOpPtT) -> bool {
        let coin_pt_t_start = self.coin_pt_t_start(state);
        let coin_pt_t_end = self.coin_pt_t_end(state);
        let opp_pt_t_start = self.opp_pt_t_start(state);
        let opp_pt_t_end = self.opp_pt_t_end(state);
        (coin_pt_t_start == test && coin_pt_t_end.contains_pt_t(state, test))
            || (coin_pt_t_end == test && coin_pt_t_start.contains_pt_t(state, test))
            || (opp_pt_t_start == test && opp_pt_t_end.contains_pt_t(state, test))
            || (opp_pt_t_end == test && opp_pt_t_start.contains_pt_t(state, test))
    }

    // returns true if both points are inside this
    pub(crate) fn contains(self, state: &SkOpGlobalState, s: SkOpPtT, e: SkOpPtT) -> bool {
        let (mut s, mut e) = (s, e);
        if state[s].t > state[e].t {
            std::mem::swap(&mut s, &mut e);
        }
        let coin_pt_t_start = self.coin_pt_t_start(state);
        if s.segment(state) == coin_pt_t_start.segment(state) {
            state[coin_pt_t_start].t <= state[s].t
                && state[e].t <= state[self.coin_pt_t_end(state)].t
        } else {
            debug_assert!(s.segment(state) == self.opp_pt_t_start(state).segment(state));
            let mut opp_ts = state[self.opp_pt_t_start(state)].t;
            let mut opp_te = state[self.opp_pt_t_end(state)].t;
            if opp_ts > opp_te {
                std::mem::swap(&mut opp_ts, &mut opp_te);
            }
            opp_ts <= state[s].t && state[e].t <= opp_te
        }
    }

    // FIXME: member pointers have fallen out of favor and can be replaced with
    // an alternative approach.
    // makes all span ends agree with the segment's spans that define them
    pub(crate) fn correct_ends(self, state: &mut SkOpGlobalState) {
        self.correct_one_end(
            state,
            SkCoincidentSpans::coin_pt_t_start,
            SkCoincidentSpans::set_coin_pt_t_start,
        );
        self.correct_one_end(
            state,
            SkCoincidentSpans::coin_pt_t_end,
            SkCoincidentSpans::set_coin_pt_t_end,
        );
        self.correct_one_end(
            state,
            SkCoincidentSpans::opp_pt_t_start,
            SkCoincidentSpans::set_opp_pt_t_start,
        );
        self.correct_one_end(
            state,
            SkCoincidentSpans::opp_pt_t_end,
            SkCoincidentSpans::set_opp_pt_t_end,
        );
    }

    // sets the span's end to the ptT referenced by the previous-next
    pub(crate) fn correct_one_end(
        self,
        state: &mut SkOpGlobalState,
        get_end: CoinGetEnd,
        set_end: CoinSetEnd,
    ) {
        let orig_pt_t = get_end(self, state);
        let orig_span = orig_pt_t.span(state);
        let prev = orig_span.prev(state);
        let test_pt_t = match prev {
            Some(prev) => prev.next(state).expect("a span has a next").pt_t(),
            None => orig_span
                .up_cast()
                .next(state)
                .expect("a span has a next")
                .prev(state)
                .expect("a span's next has a prev")
                .pt_t(),
        };
        if orig_pt_t != test_pt_t {
            set_end(self, state, test_pt_t);
        }
    }

    // expand the range by checking adjacent spans for coincidence
    pub(crate) fn expand(self, state: &mut SkOpGlobalState) -> bool {
        let mut expanded = false;
        let segment = self.coin_pt_t_start(state).segment(state);
        let opp_segment = self.opp_pt_t_start(state).segment(state);
        loop {
            let start = self.coin_pt_t_start(state).span(state).up_cast();
            let Some(prev) = start.prev(state) else {
                break;
            };
            let Some(opp_pt_t) = prev.contains_segment(state, opp_segment) else {
                break;
            };
            let mid_t = (prev.t(state) + start.t(state)) / 2.0;
            if !segment.is_close(state, mid_t, opp_segment) {
                break;
            }
            self.set_starts(state, prev.pt_t(), opp_pt_t);
            expanded = true;
        }
        loop {
            let end = self.coin_pt_t_end(state).span(state);
            let next = if end.r#final(state) {
                None
            } else {
                end.up_cast().next(state)
            };
            if next.is_some_and(|next| next.deleted(state)) {
                break;
            }
            let Some(next) = next else {
                break;
            };
            let Some(opp_pt_t) = next.contains_segment(state, opp_segment) else {
                break;
            };
            let mid_t = (end.t(state) + next.t(state)) / 2.0;
            if !segment.is_close(state, mid_t, opp_segment) {
                break;
            }
            self.set_ends(state, next.pt_t(), opp_pt_t);
            expanded = true;
        }
        expanded
    }

    // increase the range of this span
    pub(crate) fn extend(
        self,
        state: &mut SkOpGlobalState,
        coin_pt_t_start: SkOpPtT,
        coin_pt_t_end: SkOpPtT,
        opp_pt_t_start: SkOpPtT,
        opp_pt_t_end: SkOpPtT,
    ) -> bool {
        let mut result = false;
        if state[self.coin_pt_t_start(state)].t > state[coin_pt_t_start].t
            || (if self.flipped(state) {
                state[self.opp_pt_t_start(state)].t < state[opp_pt_t_start].t
            } else {
                state[self.opp_pt_t_start(state)].t > state[opp_pt_t_start].t
            })
        {
            self.set_starts(state, coin_pt_t_start, opp_pt_t_start);
            result = true;
        }
        if state[self.coin_pt_t_end(state)].t < state[coin_pt_t_end].t
            || (if self.flipped(state) {
                state[self.opp_pt_t_end(state)].t > state[opp_pt_t_end].t
            } else {
                state[self.opp_pt_t_end(state)].t < state[opp_pt_t_end].t
            })
        {
            self.set_ends(state, coin_pt_t_end, opp_pt_t_end);
            result = true;
        }
        result
    }

    pub(crate) fn flipped(self, state: &SkOpGlobalState) -> bool {
        let data = &state[self];
        let opp_start = data
            .opp_pt_t_start
            .expect("a set coincident span has its ends");
        let opp_end = data
            .opp_pt_t_end
            .expect("a set coincident span has its ends");
        state[opp_start].t > state[opp_end].t
    }

    pub(crate) fn init(self, state: &mut SkOpGlobalState) {
        state[self] = SkCoincidentSpansData::default();
    }

    pub(crate) fn next(self, state: &SkOpGlobalState) -> Option<SkCoincidentSpans> {
        state[self].next
    }

    pub(crate) fn next_ptr(self) -> CoinListLink {
        CoinListLink::Next(self)
    }

    // out of line since this function is referenced by address
    pub(crate) fn opp_pt_t_start(self, state: &SkOpGlobalState) -> SkOpPtT {
        state[self]
            .opp_pt_t_start
            .expect("a set coincident span has its ends")
    }

    // out of line since this function is referenced by address
    pub(crate) fn opp_pt_t_end(self, state: &SkOpGlobalState) -> SkOpPtT {
        state[self]
            .opp_pt_t_end
            .expect("a set coincident span has its ends")
    }

    // These return non-const pointers so that, as copies, they can be added
    // to a new span pair
    pub(crate) fn opp_pt_t_start_writable(self, state: &SkOpGlobalState) -> SkOpPtT {
        state[self]
            .opp_pt_t_start
            .expect("a set coincident span has its ends")
    }

    pub(crate) fn opp_pt_t_end_writable(self, state: &SkOpGlobalState) -> SkOpPtT {
        state[self]
            .opp_pt_t_end
            .expect("a set coincident span has its ends")
    }

    // A coincident span is unordered if the pairs of points in the main and opposite curves'
    // t values do not ascend or descend. For instance, if a tightly arced quadratic is
    // coincident with another curve, it may intersect it out of order.
    pub(crate) fn ordered(self, state: &SkOpGlobalState, result: &mut bool) -> bool {
        let start = self.coin_pt_t_start(state).span(state);
        let end = self.coin_pt_t_end(state).span(state);
        let mut next = start.up_cast().next(state).expect("a span has a next");
        if next == end {
            *result = true;
            return true;
        }
        let flipped = self.flipped(state);
        let opp_seg = self.opp_pt_t_start(state).segment(state);
        let mut opp_last_t = state[self.opp_pt_t_start(state)].t;
        loop {
            let Some(opp) = next.contains_segment(state, opp_seg) else {
                return false;
            };
            if (opp_last_t > state[opp].t) != flipped {
                *result = false;
                return true;
            }
            opp_last_t = state[opp].t;
            if next == end {
                break;
            }
            if next.up_castable(state).is_none() {
                *result = false;
                return true;
            }
            next = next.up_cast().next(state).expect("a span has a next");
        }
        *result = true;
        true
    }

    // set the range of this span
    pub(crate) fn set(
        self,
        state: &mut SkOpGlobalState,
        next: Option<SkCoincidentSpans>,
        coin_pt_t_start: SkOpPtT,
        coin_pt_t_end: SkOpPtT,
        opp_pt_t_start: SkOpPtT,
        opp_pt_t_end: SkOpPtT,
    ) {
        debug_assert!(SkOpCoincidence::ordered_pt_t(
            state,
            coin_pt_t_start,
            opp_pt_t_start
        ));
        state[self].next = next;
        self.set_starts(state, coin_pt_t_start, opp_pt_t_start);
        self.set_ends(state, coin_pt_t_end, opp_pt_t_end);
    }

    pub(crate) fn set_coin_pt_t_end(self, state: &mut SkOpGlobalState, pt_t: SkOpPtT) {
        debug_assert!(state[self]
            .coin_pt_t_start
            .is_none_or(|start| start.segment(state) == pt_t.segment(state)));
        state[self].coin_pt_t_end = Some(pt_t);
        pt_t.set_coincident(state);
    }

    pub(crate) fn set_coin_pt_t_start(self, state: &mut SkOpGlobalState, pt_t: SkOpPtT) {
        debug_assert!(state[self]
            .coin_pt_t_end
            .is_none_or(|end| end.segment(state) == pt_t.segment(state)));
        state[self].coin_pt_t_start = Some(pt_t);
        pt_t.set_coincident(state);
    }

    pub(crate) fn set_ends(
        self,
        state: &mut SkOpGlobalState,
        coin_pt_t_end: SkOpPtT,
        opp_pt_t_end: SkOpPtT,
    ) {
        self.set_coin_pt_t_end(state, coin_pt_t_end);
        self.set_opp_pt_t_end(state, opp_pt_t_end);
    }

    pub(crate) fn set_opp_pt_t_end(self, state: &mut SkOpGlobalState, pt_t: SkOpPtT) {
        debug_assert!(state[self]
            .opp_pt_t_start
            .is_none_or(|start| start.segment(state) == pt_t.segment(state)));
        state[self].opp_pt_t_end = Some(pt_t);
        pt_t.set_coincident(state);
    }

    pub(crate) fn set_opp_pt_t_start(self, state: &mut SkOpGlobalState, pt_t: SkOpPtT) {
        debug_assert!(state[self]
            .opp_pt_t_end
            .is_none_or(|end| end.segment(state) == pt_t.segment(state)));
        state[self].opp_pt_t_start = Some(pt_t);
        pt_t.set_coincident(state);
    }

    pub(crate) fn set_starts(
        self,
        state: &mut SkOpGlobalState,
        coin_pt_t_start: SkOpPtT,
        opp_pt_t_start: SkOpPtT,
    ) {
        self.set_coin_pt_t_start(state, coin_pt_t_start);
        self.set_opp_pt_t_start(state, opp_pt_t_start);
    }
}

arena_handle! {
    /// Stands for `SkOpCoincidence*`.
    SkOpCoincidence
}

/// `SkOpCoincidence`'s fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SkOpCoincidenceData {
    pub(crate) head: Option<SkCoincidentSpans>,
    pub(crate) top: Option<SkCoincidentSpans>,
}

impl Index<SkOpCoincidence> for SkOpGlobalState {
    type Output = SkOpCoincidenceData;

    fn index(&self, coincidence: SkOpCoincidence) -> &SkOpCoincidenceData {
        &self.coincidences[coincidence.0 as usize]
    }
}

impl IndexMut<SkOpCoincidence> for SkOpGlobalState {
    fn index_mut(&mut self, coincidence: SkOpCoincidence) -> &mut SkOpCoincidenceData {
        &mut self.coincidences[coincidence.0 as usize]
    }
}

/// `check->contains(ptT)` where Skia may pass a null `ptT`: `SkOpPtT::contains(const SkOpPtT*)`
/// then finds no pt-t equal to it in the loop and returns false.
fn contains_maybe_null(state: &SkOpGlobalState, pt_t: SkOpPtT, check: Option<SkOpPtT>) -> bool {
    match check {
        Some(check) => pt_t.contains_pt_t(state, check),
        None => false,
    }
}

impl SkOpCoincidence {
    /// `SkOpCoincidence(SkOpGlobalState*)`: empty, and now the state's coincidence.
    pub(crate) fn new(state: &mut SkOpGlobalState) -> SkOpCoincidence {
        let coincidence = SkOpCoincidence(state.coincidences.len() as u32);
        state.coincidences.push(SkOpCoincidenceData::default());
        state.set_coincidence(Some(coincidence));
        coincidence
    }

    /// Both of Skia's `add` overloads: the private one only casts away `const`.
    // adds a new coincident pair
    pub(crate) fn add(
        self,
        state: &mut SkOpGlobalState,
        coin_pt_t_start: SkOpPtT,
        coin_pt_t_end: SkOpPtT,
        opp_pt_t_start: SkOpPtT,
        opp_pt_t_end: SkOpPtT,
    ) {
        // OPTIMIZE: caller should have already sorted
        if !SkOpCoincidence::ordered_pt_t(state, coin_pt_t_start, opp_pt_t_start) {
            if state[opp_pt_t_start].t < state[opp_pt_t_end].t {
                self.add(
                    state,
                    opp_pt_t_start,
                    opp_pt_t_end,
                    coin_pt_t_start,
                    coin_pt_t_end,
                );
            } else {
                self.add(
                    state,
                    opp_pt_t_end,
                    opp_pt_t_start,
                    coin_pt_t_end,
                    coin_pt_t_start,
                );
            }
            return;
        }
        debug_assert!(SkOpCoincidence::ordered_pt_t(
            state,
            coin_pt_t_start,
            opp_pt_t_start
        ));
        // choose the ptT at the front of the list to track
        let coin_pt_t_start = coin_pt_t_start.span(state).pt_t();
        let coin_pt_t_end = coin_pt_t_end.span(state).pt_t();
        let opp_pt_t_start = opp_pt_t_start.span(state).pt_t();
        let opp_pt_t_end = opp_pt_t_end.span(state).pt_t();
        let coin_rec = state.make_coincident_spans();
        coin_rec.init(state);
        let head = state[self].head;
        coin_rec.set(
            state,
            head,
            coin_pt_t_start,
            coin_pt_t_end,
            opp_pt_t_start,
            opp_pt_t_end,
        );
        state[self].head = Some(coin_rec);
    }

    /// `addEndMovedSpans()`.
    /*  If A is coincident with B and B includes an endpoint, and A's matching point
        is not the endpoint (i.e., there's an implied line connecting B-end and A)
        then assume that the same implied line may intersect another curve close to B.
        Since we only care about coincidence that was undetected, look at the
        ptT list on B-segment adjacent to the B-end/A ptT loop (not in the loop, but
        next door) and see if the A matching point is close enough to form another
        coincident pair. If so, check for a new coincident span between B-end/A ptT loop
        and the adjacent ptT loop.
    */
    pub(crate) fn add_end_moved_spans(self, state: &mut SkOpGlobalState) -> bool {
        let Some(mut span) = state[self].head else {
            return true;
        };
        state[self].top = Some(span);
        state[self].head = None;
        loop {
            if state[span.coin_pt_t_start(state)].pt != state[span.opp_pt_t_start(state)].pt {
                if 1.0 == state[span.coin_pt_t_start(state)].t {
                    return false;
                }
                let on_end = state[span.coin_pt_t_start(state)].t == 0.0;
                let o_on_end = zero_or_one(state[span.opp_pt_t_start(state)].t);
                if on_end {
                    if !o_on_end {
                        // if both are on end, any nearby intersect was already found
                        let opp_pt_t_start = span.opp_pt_t_start(state);
                        if !self.add_end_moved_spans_pt_t(state, opp_pt_t_start) {
                            return false;
                        }
                    }
                } else if o_on_end {
                    let coin_pt_t_start = span.coin_pt_t_start(state);
                    if !self.add_end_moved_spans_pt_t(state, coin_pt_t_start) {
                        return false;
                    }
                }
            }
            if state[span.coin_pt_t_end(state)].pt != state[span.opp_pt_t_end(state)].pt {
                let on_end = state[span.coin_pt_t_end(state)].t == 1.0;
                let o_on_end = zero_or_one(state[span.opp_pt_t_end(state)].t);
                if on_end {
                    if !o_on_end {
                        let opp_pt_t_end = span.opp_pt_t_end(state);
                        if !self.add_end_moved_spans_pt_t(state, opp_pt_t_end) {
                            return false;
                        }
                    }
                } else if o_on_end {
                    let coin_pt_t_end = span.coin_pt_t_end(state);
                    if !self.add_end_moved_spans_pt_t(state, coin_pt_t_end) {
                        return false;
                    }
                }
            }
            match span.next(state) {
                Some(next) => span = next,
                None => break,
            }
        }
        self.restore_head(state);
        true
    }

    // for each coincident pair, match the spans
    // if the spans don't match, add the missing pt to the segment and loop it in the opposite span
    pub(crate) fn add_expanded(self, state: &mut SkOpGlobalState) -> bool {
        let Some(mut coin) = state[self].head else {
            return true;
        };
        loop {
            let start_pt_t = coin.coin_pt_t_start(state);
            let o_start_pt_t = coin.opp_pt_t_start(state);
            let mut prior_t = state[start_pt_t].t;
            let mut o_prior_t = state[o_start_pt_t].t;
            if !start_pt_t.contains_pt_t(state, o_start_pt_t) {
                return false;
            }
            let start = start_pt_t.span(state);
            let o_start = o_start_pt_t.span(state);
            let mut end = coin.coin_pt_t_end(state).span(state);
            let mut o_end = coin.opp_pt_t_end(state).span(state);
            if o_end.deleted(state) {
                return false;
            }
            if start.up_castable(state).is_none() {
                return false;
            }
            let mut test = start.up_cast().next(state).expect("a span has a next");
            if !coin.flipped(state) && o_start.up_castable(state).is_none() {
                return false;
            }
            let o_test = if coin.flipped(state) {
                o_start.prev(state).map(SkOpSpanBase::from)
            } else {
                o_start.up_cast().next(state)
            };
            let Some(mut o_test) = o_test else {
                return false;
            };
            let seg = start.segment(state);
            let o_seg = o_start.segment(state);
            while test != end || o_test != o_end {
                let contained_opp = test.pt_t().contains_segment(state, o_seg);
                let contained_this = o_test.pt_t().contains_segment(state, seg);
                if contained_opp.is_none() || contained_this.is_none() {
                    // choose the ends, or the first common pt-t list shared by both
                    let next_t;
                    let o_next_t;
                    if let Some(contained_opp) = contained_opp {
                        next_t = test.t(state);
                        o_next_t = state[contained_opp].t;
                    } else if let Some(contained_this) = contained_this {
                        next_t = state[contained_this].t;
                        o_next_t = o_test.t(state);
                    } else {
                        // iterate through until a pt-t list found that contains the other
                        let mut walk = test;
                        let mut walk_opp;
                        loop {
                            if walk.up_castable(state).is_none() {
                                return false;
                            }
                            walk = walk.up_cast().next(state).expect("a span has a next");
                            walk_opp = walk.pt_t().contains_segment(state, o_seg);
                            if !(walk_opp.is_none()
                                && walk != coin.coin_pt_t_end(state).span(state))
                            {
                                break;
                            }
                        }
                        let Some(walk_opp) = walk_opp else {
                            return false;
                        };
                        next_t = walk.t(state);
                        o_next_t = state[walk_opp].t;
                    }
                    // use t ranges to guess which one is missing
                    let start_range = next_t - prior_t;
                    if start_range == 0.0 {
                        return false;
                    }
                    let start_part = (test.t(state) - prior_t) / start_range;
                    let o_start_range = o_next_t - o_prior_t;
                    if o_start_range == 0.0 {
                        return false;
                    }
                    let o_start_part = (o_test.t(state) - o_prior_t) / o_start_range;
                    if start_part == o_start_part {
                        return false;
                    }
                    let add_to_opp = if contained_opp.is_none() && contained_this.is_none() {
                        start_part < o_start_part
                    } else {
                        contained_this.is_some()
                    };
                    let mut start_over = false;
                    let success = if add_to_opp {
                        o_seg.add_expanded(
                            state,
                            o_prior_t + o_start_range * start_part,
                            test,
                            &mut start_over,
                        )
                    } else {
                        seg.add_expanded(
                            state,
                            prior_t + start_range * o_start_part,
                            o_test,
                            &mut start_over,
                        )
                    };
                    if !success {
                        return false;
                    }
                    if start_over {
                        test = start;
                        o_test = o_start;
                    }
                    end = coin.coin_pt_t_end(state).span(state);
                    o_end = coin.opp_pt_t_end(state).span(state);
                }
                if test != end {
                    if test.up_castable(state).is_none() {
                        return false;
                    }
                    prior_t = test.t(state);
                    test = test.up_cast().next(state).expect("a span has a next");
                }
                if o_test != o_end {
                    o_prior_t = o_test.t(state);
                    let o_next = if coin.flipped(state) {
                        o_test.prev(state).map(SkOpSpanBase::from)
                    } else {
                        if o_test.up_castable(state).is_none() {
                            return false;
                        }
                        o_test.up_cast().next(state)
                    };
                    let Some(o_next) = o_next else {
                        return false;
                    };
                    o_test = o_next;
                }
            }
            match coin.next(state) {
                Some(next) => coin = next,
                None => break,
            }
        }
        true
    }

    // detects overlaps of different coincident runs on same segment
    // does not detect overlaps for pairs without any segments in common
    // returns true if caller should loop again
    pub(crate) fn add_missing(self, state: &mut SkOpGlobalState, added: &mut bool) -> bool {
        let outer = state[self].head;
        *added = false;
        let Some(mut outer) = outer else {
            return true;
        };
        state[self].top = Some(outer);
        state[self].head = None;
        loop {
            // addifmissing can modify the list that this is walking
            // save head so that walker can iterate over old data unperturbed
            // addifmissing adds to head freely then add saved head in the end
            let ocs = outer.coin_pt_t_start(state);
            if ocs.deleted(state) {
                return false;
            }
            let outer_coin = ocs.segment(state);
            if outer_coin.done(state) {
                return false;
            }
            let oos = outer.opp_pt_t_start(state);
            if oos.deleted(state) {
                return true;
            }
            let outer_opp = oos.segment(state);
            let mut inner = outer;
            while let Some(next) = inner.next(state) {
                inner = next;
                let mut over_s = 0.0;
                let mut over_e = 0.0;
                let ics = inner.coin_pt_t_start(state);
                if ics.deleted(state) {
                    return false;
                }
                let inner_coin = ics.segment(state);
                if inner_coin.done(state) {
                    return false;
                }
                let ios = inner.opp_pt_t_start(state);
                if ios.deleted(state) {
                    return false;
                }
                let inner_opp = ios.segment(state);
                if outer_coin == inner_coin {
                    let oce = outer.coin_pt_t_end(state);
                    if oce.deleted(state) {
                        return true;
                    }
                    let ice = inner.coin_pt_t_end(state);
                    if ice.deleted(state) {
                        return false;
                    }
                    if outer_opp != inner_opp
                        && self.overlap(state, ocs, oce, ics, ice, &mut over_s, &mut over_e)
                    {
                        let over1s = ocs.starter(state, oce);
                        let over2s = ics.starter(state, ice);
                        if !self.add_if_missing(
                            state, over1s, over2s, over_s, over_e, outer_opp, inner_opp, added,
                        ) {
                            return false;
                        }
                    }
                } else if outer_coin == inner_opp {
                    let oce = outer.coin_pt_t_end(state);
                    if oce.deleted(state) {
                        return false;
                    }
                    let ioe = inner.opp_pt_t_end(state);
                    if ioe.deleted(state) {
                        return false;
                    }
                    if outer_opp != inner_coin
                        && self.overlap(state, ocs, oce, ios, ioe, &mut over_s, &mut over_e)
                    {
                        let over1s = ocs.starter(state, oce);
                        let over2s = ios.starter(state, ioe);
                        if !self.add_if_missing(
                            state, over1s, over2s, over_s, over_e, outer_opp, inner_coin, added,
                        ) {
                            return false;
                        }
                    }
                } else if outer_opp == inner_coin {
                    let ooe = outer.opp_pt_t_end(state);
                    if ooe.deleted(state) {
                        return false;
                    }
                    let ice = inner.coin_pt_t_end(state);
                    if ice.deleted(state) {
                        return false;
                    }
                    debug_assert!(outer_coin != inner_opp);
                    if self.overlap(state, oos, ooe, ics, ice, &mut over_s, &mut over_e) {
                        let over1s = oos.starter(state, ooe);
                        let over2s = ics.starter(state, ice);
                        if !self.add_if_missing(
                            state, over1s, over2s, over_s, over_e, outer_coin, inner_opp, added,
                        ) {
                            return false;
                        }
                    }
                } else if outer_opp == inner_opp {
                    let ooe = outer.opp_pt_t_end(state);
                    if ooe.deleted(state) {
                        return false;
                    }
                    let ioe = inner.opp_pt_t_end(state);
                    if ioe.deleted(state) {
                        return true;
                    }
                    debug_assert!(outer_coin != inner_coin);
                    if self.overlap(state, oos, ooe, ios, ioe, &mut over_s, &mut over_e) {
                        let over1s = oos.starter(state, ooe);
                        let over2s = ios.starter(state, ioe);
                        if !self.add_if_missing(
                            state, over1s, over2s, over_s, over_e, outer_coin, inner_coin, added,
                        ) {
                            return false;
                        }
                    }
                }
            }
            match outer.next(state) {
                Some(next) => outer = next,
                None => break,
            }
        }
        self.restore_head(state);
        true
    }

    // walk span sets in parallel, moving winding from one to the other
    pub(crate) fn apply(self, state: &mut SkOpGlobalState) -> bool {
        let Some(mut coin) = state[self].head else {
            return true;
        };
        loop {
            'body: {
                let start_span = coin.coin_pt_t_start_writable(state).span(state);
                if start_span.up_castable(state).is_none() {
                    return false;
                }
                let mut start = start_span.up_cast();
                if start.deleted(state) {
                    break 'body;
                }
                let end = coin.coin_pt_t_end(state).span(state);
                if start != start.starter(state, end) {
                    return false;
                }
                let flipped = coin.flipped(state);
                let o_start_base = (if flipped {
                    coin.opp_pt_t_end_writable(state)
                } else {
                    coin.opp_pt_t_start_writable(state)
                })
                .span(state);
                if o_start_base.up_castable(state).is_none() {
                    return false;
                }
                let mut o_start = o_start_base.up_cast();
                if o_start.deleted(state) {
                    break 'body;
                }
                let o_end = (if flipped {
                    coin.opp_pt_t_start(state)
                } else {
                    coin.opp_pt_t_end(state)
                })
                .span(state);
                debug_assert!(o_start == o_start.starter(state, o_end));
                let segment = start.segment(state);
                let o_segment = o_start.segment(state);
                let operand_swap = segment.operand(state) != o_segment.operand(state);
                if flipped {
                    if o_end.deleted(state) {
                        break 'body;
                    }
                    loop {
                        let o_next = o_start.next(state).expect("a span has a next");
                        if o_next == o_end {
                            break;
                        }
                        if o_next.up_castable(state).is_none() {
                            return false;
                        }
                        o_start = o_next.up_cast();
                    }
                }
                loop {
                    let mut wind_value = start.wind_value(state);
                    let mut opp_value = start.opp_value(state);
                    let mut o_wind_value = o_start.wind_value(state);
                    let mut o_opp_value = o_start.opp_value(state);
                    // winding values are added or subtracted depending on direction and wind type
                    // same or opposite values are summed depending on the operand value
                    let mut wind_diff = if operand_swap {
                        o_opp_value
                    } else {
                        o_wind_value
                    };
                    let mut o_wind_diff = if operand_swap { opp_value } else { wind_value };
                    if !flipped {
                        wind_diff = -wind_diff;
                        o_wind_diff = -o_wind_diff;
                    }
                    let mut add_to_start = wind_value != 0
                        && (wind_value > wind_diff
                            || (wind_value == wind_diff && o_wind_value <= o_wind_diff));
                    if if add_to_start {
                        start.done(state)
                    } else {
                        o_start.done(state)
                    } {
                        add_to_start ^= true;
                    }
                    if add_to_start {
                        if operand_swap {
                            std::mem::swap(&mut o_wind_value, &mut o_opp_value);
                        }
                        if flipped {
                            wind_value -= o_wind_value;
                            opp_value -= o_opp_value;
                        } else {
                            wind_value += o_wind_value;
                            opp_value += o_opp_value;
                        }
                        if segment.is_xor(state) {
                            wind_value &= 1;
                        }
                        if segment.opp_xor(state) {
                            opp_value &= 1;
                        }
                        o_wind_value = 0;
                        o_opp_value = 0;
                    } else {
                        if operand_swap {
                            std::mem::swap(&mut wind_value, &mut opp_value);
                        }
                        if flipped {
                            o_wind_value -= wind_value;
                            o_opp_value -= opp_value;
                        } else {
                            o_wind_value += wind_value;
                            o_opp_value += opp_value;
                        }
                        if o_segment.is_xor(state) {
                            o_wind_value &= 1;
                        }
                        if o_segment.opp_xor(state) {
                            o_opp_value &= 1;
                        }
                        wind_value = 0;
                        opp_value = 0;
                    }
                    if wind_value <= -1 {
                        return false;
                    }
                    start.set_wind_value(state, wind_value);
                    start.set_opp_value(state, opp_value);
                    if o_wind_value <= -1 {
                        return false;
                    }
                    o_start.set_wind_value(state, o_wind_value);
                    o_start.set_opp_value(state, o_opp_value);
                    if wind_value == 0 && opp_value == 0 {
                        segment.mark_done(state, start);
                    }
                    if o_wind_value == 0 && o_opp_value == 0 {
                        o_segment.mark_done(state, o_start);
                    }
                    let next = start.next(state).expect("a span has a next");
                    let o_next = if flipped {
                        o_start.prev(state).map(SkOpSpanBase::from)
                    } else {
                        o_start.next(state)
                    };
                    if next == end {
                        break;
                    }
                    if next.up_castable(state).is_none() {
                        return false;
                    }
                    start = next.up_cast();
                    // if the opposite ran out too soon, just reuse the last span
                    let o_next = match o_next {
                        Some(o_next) if o_next.up_castable(state).is_some() => o_next,
                        _ => o_start.0,
                    };
                    o_start = o_next.up_cast();
                }
            }
            match coin.next(state) {
                Some(next) => coin = next,
                None => break,
            }
        }
        true
    }

    /// `contains(const SkOpPtT* coinPtTStart, const SkOpPtT* coinPtTEnd,
    /// const SkOpPtT* oppPtTStart, const SkOpPtT* oppPtTEnd) const`.
    pub(crate) fn contains(
        self,
        state: &SkOpGlobalState,
        coin_pt_t_start: SkOpPtT,
        coin_pt_t_end: SkOpPtT,
        opp_pt_t_start: SkOpPtT,
        opp_pt_t_end: SkOpPtT,
    ) -> bool {
        let Some(mut test) = state[self].head else {
            return false;
        };
        let (mut coin_pt_t_start, mut coin_pt_t_end) = (coin_pt_t_start, coin_pt_t_end);
        let (mut opp_pt_t_start, mut opp_pt_t_end) = (opp_pt_t_start, opp_pt_t_end);
        let mut coin_seg = coin_pt_t_start.segment(state);
        let mut opp_seg = opp_pt_t_start.segment(state);
        if !SkOpCoincidence::ordered_pt_t(state, coin_pt_t_start, opp_pt_t_start) {
            std::mem::swap(&mut coin_seg, &mut opp_seg);
            std::mem::swap(&mut coin_pt_t_start, &mut opp_pt_t_start);
            std::mem::swap(&mut coin_pt_t_end, &mut opp_pt_t_end);
            if state[coin_pt_t_start].t > state[coin_pt_t_end].t {
                std::mem::swap(&mut coin_pt_t_start, &mut coin_pt_t_end);
                std::mem::swap(&mut opp_pt_t_start, &mut opp_pt_t_end);
            }
        }
        let opp_min_t = std_min(state[opp_pt_t_start].t, state[opp_pt_t_end].t);
        let opp_max_t = std_max(state[opp_pt_t_start].t, state[opp_pt_t_end].t);
        loop {
            'body: {
                if coin_seg != test.coin_pt_t_start(state).segment(state) {
                    break 'body;
                }
                if state[coin_pt_t_start].t < state[test.coin_pt_t_start(state)].t {
                    break 'body;
                }
                if state[coin_pt_t_end].t > state[test.coin_pt_t_end(state)].t {
                    break 'body;
                }
                if opp_seg != test.opp_pt_t_start(state).segment(state) {
                    break 'body;
                }
                if opp_min_t
                    < std_min(
                        state[test.opp_pt_t_start(state)].t,
                        state[test.opp_pt_t_end(state)].t,
                    )
                {
                    break 'body;
                }
                if opp_max_t
                    > std_max(
                        state[test.opp_pt_t_start(state)].t,
                        state[test.opp_pt_t_end(state)].t,
                    )
                {
                    break 'body;
                }
                return true;
            }
            match test.next(state) {
                Some(next) => test = next,
                None => break,
            }
        }
        false
    }

    pub(crate) fn correct_ends(self, state: &mut SkOpGlobalState) {
        let Some(mut coin) = state[self].head else {
            return;
        };
        loop {
            coin.correct_ends(state);
            match coin.next(state) {
                Some(next) => coin = next,
                None => break,
            }
        }
    }

    // expand the range by checking adjacent spans for coincidence
    pub(crate) fn expand(self, state: &mut SkOpGlobalState) -> bool {
        let Some(mut coin) = state[self].head else {
            return false;
        };
        let mut expanded = false;
        loop {
            if coin.expand(state) {
                // check to see if multiple spans expanded so they are now identical
                let mut test = state[self].head.expect("the list holds coin");
                'tests: loop {
                    'body: {
                        if coin == test {
                            break 'body;
                        }
                        if coin.coin_pt_t_start(state) == test.coin_pt_t_start(state)
                            && coin.opp_pt_t_start(state) == test.opp_pt_t_start(state)
                        {
                            self.release_coin(state, CoinListLink::Head(self), test);
                            break 'tests;
                        }
                    }
                    match test.next(state) {
                        Some(next) => test = next,
                        None => break,
                    }
                }
                expanded = true;
            }
            match coin.next(state) {
                Some(next) => coin = next,
                None => break,
            }
        }
        expanded
    }

    // if there is an existing pair that overlaps the addition, extend it
    pub(crate) fn extend(
        self,
        state: &mut SkOpGlobalState,
        coin_pt_t_start: SkOpPtT,
        coin_pt_t_end: SkOpPtT,
        opp_pt_t_start: SkOpPtT,
        opp_pt_t_end: SkOpPtT,
    ) -> bool {
        let Some(mut test) = state[self].head else {
            return false;
        };
        let (mut coin_pt_t_start, mut coin_pt_t_end) = (coin_pt_t_start, coin_pt_t_end);
        let (mut opp_pt_t_start, mut opp_pt_t_end) = (opp_pt_t_start, opp_pt_t_end);
        let mut coin_seg = coin_pt_t_start.segment(state);
        let mut opp_seg = opp_pt_t_start.segment(state);
        if !SkOpCoincidence::ordered_pt_t(state, coin_pt_t_start, opp_pt_t_start) {
            std::mem::swap(&mut coin_seg, &mut opp_seg);
            std::mem::swap(&mut coin_pt_t_start, &mut opp_pt_t_start);
            std::mem::swap(&mut coin_pt_t_end, &mut opp_pt_t_end);
            if state[coin_pt_t_start].t > state[coin_pt_t_end].t {
                std::mem::swap(&mut coin_pt_t_start, &mut coin_pt_t_end);
                std::mem::swap(&mut opp_pt_t_start, &mut opp_pt_t_end);
            }
        }
        let opp_min_t = std_min(state[opp_pt_t_start].t, state[opp_pt_t_end].t);
        loop {
            'body: {
                if coin_seg != test.coin_pt_t_start(state).segment(state) {
                    break 'body;
                }
                if opp_seg != test.opp_pt_t_start(state).segment(state) {
                    break 'body;
                }
                let o_test_min_t = std_min(
                    state[test.opp_pt_t_start(state)].t,
                    state[test.opp_pt_t_end(state)].t,
                );
                let o_test_max_t = std_max(
                    state[test.opp_pt_t_start(state)].t,
                    state[test.opp_pt_t_end(state)].t,
                );
                if (state[test.coin_pt_t_start(state)].t <= state[coin_pt_t_end].t
                    && state[coin_pt_t_start].t <= state[test.coin_pt_t_end(state)].t)
                    || (o_test_min_t <= o_test_max_t && opp_min_t <= o_test_max_t)
                {
                    test.extend(
                        state,
                        coin_pt_t_start,
                        coin_pt_t_end,
                        opp_pt_t_start,
                        opp_pt_t_end,
                    );
                    return true;
                }
            }
            match test.next(state) {
                Some(next) => test = next,
                None => break,
            }
        }
        false
    }

    /// `const` in Skia; it adds to `overlaps`.
    pub(crate) fn find_overlaps(
        self,
        state: &mut SkOpGlobalState,
        overlaps: SkOpCoincidence,
    ) -> bool {
        state[overlaps].top = None;
        state[overlaps].head = None;
        let mut outer = state[self].head;
        while let Some(outer_spans) = outer {
            let outer_coin = outer_spans.coin_pt_t_start(state).segment(state);
            let outer_opp = outer_spans.opp_pt_t_start(state).segment(state);
            let mut inner = outer_spans;
            while let Some(next) = inner.next(state) {
                inner = next;
                let inner_coin = inner.coin_pt_t_start(state).segment(state);
                if outer_coin == inner_coin {
                    continue; // both winners are the same segment, so there's no additional overlap
                }
                let inner_opp = inner.opp_pt_t_start(state).segment(state);
                let mut overlap_s = None;
                let mut overlap_e = None;
                if (outer_opp == inner_coin
                    && SkOpPtT::overlaps(
                        state,
                        outer_spans.opp_pt_t_start(state),
                        outer_spans.opp_pt_t_end(state),
                        inner.coin_pt_t_start(state),
                        inner.coin_pt_t_end(state),
                        &mut overlap_s,
                        &mut overlap_e,
                    ))
                    || (outer_coin == inner_opp
                        && SkOpPtT::overlaps(
                            state,
                            outer_spans.coin_pt_t_start(state),
                            outer_spans.coin_pt_t_end(state),
                            inner.opp_pt_t_start(state),
                            inner.opp_pt_t_end(state),
                            &mut overlap_s,
                            &mut overlap_e,
                        ))
                    || (outer_opp == inner_opp
                        && SkOpPtT::overlaps(
                            state,
                            outer_spans.opp_pt_t_start(state),
                            outer_spans.opp_pt_t_end(state),
                            inner.opp_pt_t_start(state),
                            inner.opp_pt_t_end(state),
                            &mut overlap_s,
                            &mut overlap_e,
                        ))
                {
                    let overlap_s = overlap_s.expect("Overlaps returned true");
                    let overlap_e = overlap_e.expect("Overlaps returned true");
                    if !overlaps.add_overlap(
                        state, outer_coin, outer_opp, inner_coin, inner_opp, overlap_s, overlap_e,
                    ) {
                        return false;
                    }
                }
            }
            outer = outer_spans.next(state);
        }
        true
    }

    /// `fixUp(SkOpPtT* deleted, const SkOpPtT* kept)`.
    pub(crate) fn fix_up(self, state: &mut SkOpGlobalState, deleted: SkOpPtT, kept: SkOpPtT) {
        if state[self].head.is_some() {
            self.fix_up_list(state, CoinListLink::Head(self), deleted, kept);
        }
        if state[self].top.is_some() {
            self.fix_up_list(state, CoinListLink::Top(self), deleted, kept);
        }
    }

    pub(crate) fn is_empty(self, state: &SkOpGlobalState) -> bool {
        state[self].head.is_none() && state[self].top.is_none()
    }

    /* this sets up the coincidence links in the segments when the coincidence crosses multiple spans */
    pub(crate) fn mark(self, state: &mut SkOpGlobalState) -> bool {
        let Some(mut coin) = state[self].head else {
            return true;
        };
        loop {
            let start_base = coin.coin_pt_t_start_writable(state).span(state);
            if start_base.up_castable(state).is_none() {
                return false;
            }
            let start = start_base.up_cast();
            if start.deleted(state) {
                return false;
            }
            let end = coin.coin_pt_t_end_writable(state).span(state);
            let mut o_start = coin.opp_pt_t_start_writable(state).span(state);
            let mut o_end = coin.opp_pt_t_end_writable(state).span(state);
            if o_end.deleted(state) {
                return false;
            }
            let flipped = coin.flipped(state);
            if flipped {
                std::mem::swap(&mut o_start, &mut o_end);
            }
            /* coin and opp spans may not match up. Mark the ends, and then let the interior
            get marked as many times as the spans allow */
            if o_start.up_castable(state).is_none() {
                return false;
            }
            start.insert_coincidence(state, o_start.up_cast());
            end.insert_coin_end(state, o_end);
            let segment = start.segment(state);
            let o_segment = o_start.segment(state);
            let mut next = start.0;
            let mut o_next = o_start;
            let mut ordered = false;
            if !coin.ordered(state, &mut ordered) {
                return false;
            }
            loop {
                next = next.up_cast().next(state).expect("a span has a next");
                if next == end {
                    break;
                }
                if next.up_castable(state).is_none() {
                    return false;
                }
                if !next
                    .up_cast()
                    .insert_coincidence_segment(state, o_segment, flipped, ordered)
                {
                    return false;
                }
            }
            loop {
                o_next = o_next.up_cast().next(state).expect("a span has a next");
                if o_next == o_end {
                    break;
                }
                if o_next.up_castable(state).is_none() {
                    return false;
                }
                if !o_next
                    .up_cast()
                    .insert_coincidence_segment(state, segment, flipped, ordered)
                {
                    return false;
                }
            }
            match coin.next(state) {
                Some(next) => coin = next,
                None => break,
            }
        }
        true
    }

    /// `markCollapsed(SkOpPtT*)`.
    pub(crate) fn mark_collapsed(self, state: &mut SkOpGlobalState, test: SkOpPtT) {
        self.mark_collapsed_list(state, CoinListLink::Head(self), test);
        self.mark_collapsed_list(state, CoinListLink::Top(self), test);
    }

    /// `static Ordered(const SkOpPtT* coinPtTStart, const SkOpPtT* oppPtTStart)`.
    pub(crate) fn ordered_pt_t(
        state: &SkOpGlobalState,
        coin_pt_t_start: SkOpPtT,
        opp_pt_t_start: SkOpPtT,
    ) -> bool {
        SkOpCoincidence::ordered(
            state,
            coin_pt_t_start.segment(state),
            opp_pt_t_start.segment(state),
        )
    }

    /// `static Ordered(const SkOpSegment* coin, const SkOpSegment* opp)`.
    ///
    /// Skia walks the points' `SkScalar`s as one array, x then y of each point.
    pub(crate) fn ordered(state: &SkOpGlobalState, coin: SkOpSegment, opp: SkOpSegment) -> bool {
        if (coin.verb(state) as i32) < (opp.verb(state) as i32) {
            return true;
        }
        if (coin.verb(state) as i32) > (opp.verb(state) as i32) {
            return false;
        }
        let count = (sk_path_ops_verb_to_points(coin.verb(state)) + 1) * 2;
        let c_pts = coin.pts(state);
        let o_pts = opp.pts(state);
        let scalar = |pts: &[SkPoint; 4], index: usize| {
            let pt = pts[index / 2];
            if index.is_multiple_of(2) {
                pt.x
            } else {
                pt.y
            }
        };
        for index in 0..count as usize {
            let c_pt = scalar(&c_pts, index);
            let o_pt = scalar(&o_pts, index);
            if c_pt < o_pt {
                return true;
            }
            if c_pt > o_pt {
                return false;
            }
        }
        true
    }

    /// `release(const SkOpSegment*)`.
    pub(crate) fn release(self, state: &mut SkOpGlobalState, deleted: SkOpSegment) {
        self.release_segment_list(state, CoinListLink::Head(self), deleted);
        self.release_segment_list(state, CoinListLink::Top(self), deleted);
    }

    /// `releaseDeleted()`.
    pub(crate) fn release_deleted(self, state: &mut SkOpGlobalState) {
        self.release_deleted_list(state, CoinListLink::Head(self));
        self.release_deleted_list(state, CoinListLink::Top(self));
    }

    /// `addEndMovedSpans(const SkOpSpan* base, const SkOpSpanBase* testSpan)`.
    pub(crate) fn add_end_moved_spans_span(
        self,
        state: &mut SkOpGlobalState,
        base: SkOpSpan,
        test_span: SkOpSpanBase,
    ) -> bool {
        let mut test_pt_t = test_span.pt_t();
        let stop_pt_t = test_pt_t;
        let base_seg = base.segment(state);
        let mut escape_hatch = 100000; // this is 100 times larger than the debugLoopLimit test
        loop {
            test_pt_t = test_pt_t.next(state);
            if test_pt_t == stop_pt_t {
                break;
            }
            escape_hatch -= 1;
            if escape_hatch <= 0 {
                return false; // if triggered (likely by a fuzz-generated test) too complex to succeed
            }
            let test_seg = test_pt_t.segment(state);
            if test_pt_t.deleted(state) {
                continue;
            }
            if test_seg == base_seg {
                continue;
            }
            if test_pt_t.span(state).pt_t() != test_pt_t {
                continue;
            }
            if self.contains_segment(state, base_seg, test_seg, state[test_pt_t].t) {
                continue;
            }
            // intersect perp with base->ptT() with testPtT->segment()
            let dxdy = base_seg.d_slope_at_t(state, base.t(state));
            let pt = base.pt(state);
            let ray = SkDLine {
                pts: [
                    SkDPoint {
                        x: pt.x as f64,
                        y: pt.y as f64,
                    },
                    SkDPoint {
                        x: pt.x as f64 + dxdy.y,
                        y: pt.y as f64 - dxdy.x,
                    },
                ],
            };
            let mut i = SkIntersections::new();
            CURVE_INTERSECT_RAY[test_seg.verb(state) as usize]
                .expect("a segment's verb has a curve")(
                &test_seg.pts(state),
                test_seg.weight(state),
                &ray,
                &mut i,
            );
            for index in 0..i.used() as usize {
                let t = i[0][index];
                if !between(0.0, t, 1.0) {
                    continue;
                }
                let opp_pt = *i.pt(index);
                if !opp_pt.approximately_equal_sk_point(&pt) {
                    continue;
                }
                let writable_seg = test_seg;
                // Skia dereferences addT's result unchecked.
                let opp_start = writable_seg
                    .add_t(state, t)
                    .expect("addT found or made the pt-t");
                if opp_start == test_pt_t {
                    continue;
                }
                let writable_base = base;
                opp_start.span(state).add_opp(state, writable_base.0);
                if opp_start.deleted(state) {
                    continue;
                }
                let mut coin_seg = base.segment(state);
                let mut opp_seg = opp_start.segment(state);
                let mut coin_ts;
                let mut coin_te;
                let mut opp_ts;
                let mut opp_te;
                if SkOpCoincidence::ordered(state, coin_seg, opp_seg) {
                    coin_ts = base.t(state);
                    coin_te = test_span.t(state);
                    opp_ts = state[opp_start].t;
                    opp_te = state[test_pt_t].t;
                } else {
                    std::mem::swap(&mut coin_seg, &mut opp_seg);
                    coin_ts = state[opp_start].t;
                    coin_te = state[test_pt_t].t;
                    opp_ts = base.t(state);
                    opp_te = test_span.t(state);
                }
                if coin_ts > coin_te {
                    std::mem::swap(&mut coin_ts, &mut coin_te);
                    std::mem::swap(&mut opp_ts, &mut opp_te);
                }
                let mut added = false;
                if !self.add_or_overlap(
                    state, coin_seg, opp_seg, coin_ts, coin_te, opp_ts, opp_te, &mut added,
                ) {
                    return false;
                }
            }
        }
        true
    }

    /// `addEndMovedSpans(const SkOpPtT* ptT)`.
    pub(crate) fn add_end_moved_spans_pt_t(
        self,
        state: &mut SkOpGlobalState,
        pt_t: SkOpPtT,
    ) -> bool {
        if pt_t.span(state).up_castable(state).is_none() {
            return false;
        }
        let base = pt_t.span(state).up_cast();
        let Some(prev) = base.prev(state) else {
            return false;
        };
        if !prev.is_canceled(state) {
            let base_prev = base.prev(state).expect("checked above").0;
            if !self.add_end_moved_spans_span(state, base, base_prev) {
                return false;
            }
        }
        if !base.is_canceled(state) {
            let base_next = base.next(state).expect("a span has a next");
            if !self.add_end_moved_spans_span(state, base, base_next) {
                return false;
            }
        }
        true
    }

    // note that over1s, over1e, over2s, over2e are ordered
    pub(crate) fn add_if_missing(
        self,
        state: &mut SkOpGlobalState,
        over1s: SkOpPtT,
        over2s: SkOpPtT,
        t_start: f64,
        t_end: f64,
        coin_seg: SkOpSegment,
        opp_seg: SkOpSegment,
        added: &mut bool,
    ) -> bool {
        debug_assert!(t_start < t_end);
        debug_assert!(over1s.segment(state) == over2s.segment(state));
        debug_assert!(over1s.segment(state) != coin_seg);
        debug_assert!(over1s.segment(state) != opp_seg);
        debug_assert!(coin_seg != opp_seg);
        let mut coin_ts = SkOpCoincidence::t_range(state, over1s, t_start, coin_seg);
        let mut coin_te = SkOpCoincidence::t_range(state, over1s, t_end, coin_seg);
        let mut result = coin_seg.collapsed(state, coin_ts, coin_te);
        if Collapsed::No != result {
            return Collapsed::Yes == result;
        }
        let mut opp_ts = SkOpCoincidence::t_range(state, over2s, t_start, opp_seg);
        let mut opp_te = SkOpCoincidence::t_range(state, over2s, t_end, opp_seg);
        result = opp_seg.collapsed(state, opp_ts, opp_te);
        if Collapsed::No != result {
            return Collapsed::Yes == result;
        }
        if coin_ts > coin_te {
            std::mem::swap(&mut coin_ts, &mut coin_te);
            std::mem::swap(&mut opp_ts, &mut opp_te);
        }
        let _ = self.add_or_overlap(
            state, coin_seg, opp_seg, coin_ts, coin_te, opp_ts, opp_te, added,
        );
        true
    }

    // If this is called by addEndMovedSpans(), a returned false propogates out to an abort.
    // If this is called by AddIfMissing(), a returned false indicates there was nothing to add
    pub(crate) fn add_or_overlap(
        self,
        state: &mut SkOpGlobalState,
        coin_seg: SkOpSegment,
        opp_seg: SkOpSegment,
        coin_ts: f64,
        coin_te: f64,
        opp_ts: f64,
        opp_te: f64,
        added: &mut bool,
    ) -> bool {
        let mut overlaps: Vec<SkCoincidentSpans> = Vec::new();
        let Some(top) = state[self].top else {
            return false;
        };
        if !self.check_overlap(
            state,
            top,
            coin_seg,
            opp_seg,
            coin_ts,
            coin_te,
            opp_ts,
            opp_te,
            &mut overlaps,
        ) {
            return true;
        }
        if let Some(head) = state[self].head {
            if !self.check_overlap(
                state,
                head,
                coin_seg,
                opp_seg,
                coin_ts,
                coin_te,
                opp_ts,
                opp_te,
                &mut overlaps,
            ) {
                return true;
            }
        }
        let overlap = overlaps.first().copied();
        for &test in overlaps.iter().skip(1) {
            // combine overlaps before continuing
            let overlap = overlap.expect("overlaps is not empty");
            if state[overlap.coin_pt_t_start(state)].t > state[test.coin_pt_t_start(state)].t {
                let test_coin_pt_t_start = test.coin_pt_t_start(state);
                overlap.set_coin_pt_t_start(state, test_coin_pt_t_start);
            }
            if state[overlap.coin_pt_t_end(state)].t < state[test.coin_pt_t_end(state)].t {
                let test_coin_pt_t_end = test.coin_pt_t_end(state);
                overlap.set_coin_pt_t_end(state, test_coin_pt_t_end);
            }
            if if overlap.flipped(state) {
                state[overlap.opp_pt_t_start(state)].t < state[test.opp_pt_t_start(state)].t
            } else {
                state[overlap.opp_pt_t_start(state)].t > state[test.opp_pt_t_start(state)].t
            } {
                let test_opp_pt_t_start = test.opp_pt_t_start(state);
                overlap.set_opp_pt_t_start(state, test_opp_pt_t_start);
            }
            if if overlap.flipped(state) {
                state[overlap.opp_pt_t_end(state)].t > state[test.opp_pt_t_end(state)].t
            } else {
                state[overlap.opp_pt_t_end(state)].t < state[test.opp_pt_t_end(state)].t
            } {
                let test_opp_pt_t_end = test.opp_pt_t_end(state);
                overlap.set_opp_pt_t_end(state, test_opp_pt_t_end);
            }
            if state[self].head.is_none()
                || !self.release_coin(state, CoinListLink::Head(self), test)
            {
                let released = self.release_coin(state, CoinListLink::Top(self), test);
                debug_assert!(released);
            }
        }
        let mut cs = coin_seg.existing(state, coin_ts, Some(opp_seg));
        let mut ce = coin_seg.existing(state, coin_te, Some(opp_seg));
        if let (Some(overlap), Some(cs), Some(ce)) = (overlap, cs, ce) {
            if overlap.contains(state, cs, ce) {
                return true;
            }
        }
        if cs == ce && cs.is_some() {
            return false;
        }
        let mut os = opp_seg.existing(state, opp_ts, Some(coin_seg));
        let mut oe = opp_seg.existing(state, opp_te, Some(coin_seg));
        if let (Some(overlap), Some(os), Some(oe)) = (overlap, os, oe) {
            if overlap.contains(state, os, oe) {
                return true;
            }
        }
        if cs.is_some_and(|cs| cs.deleted(state)) {
            return false;
        }
        if os.is_some_and(|os| os.deleted(state)) {
            return false;
        }
        if ce.is_some_and(|ce| ce.deleted(state)) {
            return false;
        }
        if oe.is_some_and(|oe| oe.deleted(state)) {
            return false;
        }
        let cs_existing = if cs.is_none() {
            coin_seg.existing(state, coin_ts, None)
        } else {
            None
        };
        let ce_existing = if ce.is_none() {
            coin_seg.existing(state, coin_te, None)
        } else {
            None
        };
        if cs_existing.is_some() && cs_existing == ce_existing {
            return false;
        }
        if let Some(ce_existing) = ce_existing {
            if Some(ce_existing) == cs
                || contains_maybe_null(state, ce_existing, cs_existing.or(cs))
            {
                return false;
            }
        }
        let os_existing = if os.is_none() {
            opp_seg.existing(state, opp_ts, None)
        } else {
            None
        };
        let oe_existing = if oe.is_none() {
            opp_seg.existing(state, opp_te, None)
        } else {
            None
        };
        if os_existing.is_some() && os_existing == oe_existing {
            return false;
        }
        if let Some(os_existing) = os_existing {
            if Some(os_existing) == oe
                || contains_maybe_null(state, os_existing, oe_existing.or(oe))
            {
                return false;
            }
        }
        if let Some(oe_existing) = oe_existing {
            if Some(oe_existing) == os
                || contains_maybe_null(state, oe_existing, os_existing.or(os))
            {
                return false;
            }
        }
        if cs.is_none() || os.is_none() {
            let cs_writable = match cs {
                Some(cs) => Some(cs),
                None => coin_seg.add_t(state, coin_ts),
            };
            if cs_writable == ce {
                return true;
            }
            let os_writable = match os {
                Some(os) => Some(os),
                None => opp_seg.add_t(state, opp_ts),
            };
            let (Some(cs_writable), Some(os_writable)) = (cs_writable, os_writable) else {
                return false;
            };
            let os_writable_span = os_writable.span(state);
            cs_writable.span(state).add_opp(state, os_writable_span);
            cs = Some(cs_writable);
            os = os_writable.active(state);
            if os.is_none() {
                return false;
            }
            if ce.is_some_and(|ce| ce.deleted(state)) || oe.is_some_and(|oe| oe.deleted(state)) {
                return false;
            }
        }
        if ce.is_none() || oe.is_none() {
            // Skia dereferences addT's results unchecked.
            let ce_writable = match ce {
                Some(ce) => ce,
                None => coin_seg
                    .add_t(state, coin_te)
                    .expect("addT found or made the pt-t"),
            };
            let oe_writable = match oe {
                Some(oe) => oe,
                None => opp_seg
                    .add_t(state, opp_te)
                    .expect("addT found or made the pt-t"),
            };
            let oe_writable_span = oe_writable.span(state);
            if !ce_writable.span(state).add_opp(state, oe_writable_span) {
                return false;
            }
            ce = Some(ce_writable);
            oe = Some(oe_writable);
        }
        let mut cs = cs.expect("cs is set above");
        let mut os = os.expect("os is set above");
        let mut ce = ce.expect("ce is set above");
        let mut oe = oe.expect("oe is set above");
        if cs.deleted(state) {
            return false;
        }
        if os.deleted(state) {
            return false;
        }
        if ce.deleted(state) {
            return false;
        }
        if oe.deleted(state) {
            return false;
        }
        if cs.contains_pt_t(state, ce) || os.contains_pt_t(state, oe) {
            return false;
        }
        let mut result = true;
        if let Some(overlap) = overlap {
            if overlap.coin_pt_t_start(state).segment(state) == coin_seg {
                result = overlap.extend(state, cs, ce, os, oe);
            } else {
                if state[os].t > state[oe].t {
                    std::mem::swap(&mut cs, &mut ce);
                    std::mem::swap(&mut os, &mut oe);
                }
                result = overlap.extend(state, os, oe, cs, ce);
            }
        } else {
            self.add(state, cs, ce, os, oe);
        }
        if result {
            *added = true;
        }
        true
    }

    pub(crate) fn add_overlap(
        self,
        state: &mut SkOpGlobalState,
        seg1: SkOpSegment,
        seg1o: SkOpSegment,
        seg2: SkOpSegment,
        seg2o: SkOpSegment,
        over_s: SkOpPtT,
        over_e: SkOpPtT,
    ) -> bool {
        let s1 = over_s.find(state, seg1);
        let e1 = over_e.find(state, seg1);
        let Some(mut s1) = s1 else {
            return false;
        };
        let Some(mut e1) = e1 else {
            return false;
        };
        if s1
            .starter(state, e1)
            .span(state)
            .up_cast()
            .wind_value(state)
            == 0
        {
            let s1o = over_s.find(state, seg1o);
            let e1o = over_e.find(state, seg1o);
            let Some(s1o) = s1o else {
                return false;
            };
            let Some(e1o) = e1o else {
                return false;
            };
            s1 = s1o;
            e1 = e1o;
            if s1
                .starter(state, e1)
                .span(state)
                .up_cast()
                .wind_value(state)
                == 0
            {
                return true;
            }
        }
        let s2 = over_s.find(state, seg2);
        let e2 = over_e.find(state, seg2);
        let Some(mut s2) = s2 else {
            return false;
        };
        let Some(mut e2) = e2 else {
            return false;
        };
        if s2
            .starter(state, e2)
            .span(state)
            .up_cast()
            .wind_value(state)
            == 0
        {
            let s2o = over_s.find(state, seg2o);
            let e2o = over_e.find(state, seg2o);
            let Some(s2o) = s2o else {
                return false;
            };
            let Some(e2o) = e2o else {
                return false;
            };
            s2 = s2o;
            e2 = e2o;
            if s2
                .starter(state, e2)
                .span(state)
                .up_cast()
                .wind_value(state)
                == 0
            {
                return true;
            }
        }
        if s1.segment(state) == s2.segment(state) {
            return true;
        }
        if state[s1].t > state[e1].t {
            std::mem::swap(&mut s1, &mut e1);
            std::mem::swap(&mut s2, &mut e2);
        }
        self.add(state, s1, e1, s2, e2);
        true
    }

    // return true if span overlaps existing and needs to adjust the coincident list
    pub(crate) fn check_overlap(
        self,
        state: &SkOpGlobalState,
        check: SkCoincidentSpans,
        coin_seg: SkOpSegment,
        opp_seg: SkOpSegment,
        coin_ts: f64,
        coin_te: f64,
        opp_ts: f64,
        opp_te: f64,
        overlaps: &mut Vec<SkCoincidentSpans>,
    ) -> bool {
        if !SkOpCoincidence::ordered(state, coin_seg, opp_seg) {
            if opp_ts < opp_te {
                return self.check_overlap(
                    state, check, opp_seg, coin_seg, opp_ts, opp_te, coin_ts, coin_te, overlaps,
                );
            }
            return self.check_overlap(
                state, check, opp_seg, coin_seg, opp_te, opp_ts, coin_te, coin_ts, overlaps,
            );
        }
        let (mut opp_ts, mut opp_te) = (opp_ts, opp_te);
        let swap_opp = opp_ts > opp_te;
        if swap_opp {
            std::mem::swap(&mut opp_ts, &mut opp_te);
        }
        let mut check = check;
        loop {
            'body: {
                if check.coin_pt_t_start(state).segment(state) != coin_seg {
                    break 'body;
                }
                if check.opp_pt_t_start(state).segment(state) != opp_seg {
                    break 'body;
                }
                let check_ts = state[check.coin_pt_t_start(state)].t;
                let check_te = state[check.coin_pt_t_end(state)].t;
                let coin_outside = coin_te < check_ts || coin_ts > check_te;
                let mut o_check_ts = state[check.opp_pt_t_start(state)].t;
                let mut o_check_te = state[check.opp_pt_t_end(state)].t;
                if swap_opp {
                    if o_check_ts <= o_check_te {
                        return false;
                    }
                    std::mem::swap(&mut o_check_ts, &mut o_check_te);
                }
                let opp_outside = opp_te < o_check_ts || opp_ts > o_check_te;
                if coin_outside && opp_outside {
                    break 'body;
                }
                let coin_inside = coin_te <= check_te && coin_ts >= check_ts;
                let opp_inside = opp_te <= o_check_te && opp_ts >= o_check_ts;
                if coin_inside && opp_inside {
                    // already included, do nothing
                    return false;
                }
                overlaps.push(check); // partial overlap, extend existing entry
            }
            match check.next(state) {
                Some(next) => check = next,
                None => break,
            }
        }
        true
    }

    /// `contains(const SkOpSegment* seg, const SkOpSegment* opp, double oppT) const`.
    pub(crate) fn contains_segment(
        self,
        state: &SkOpGlobalState,
        seg: SkOpSegment,
        opp: SkOpSegment,
        opp_t: f64,
    ) -> bool {
        if self.contains_coin(state, state[self].head, seg, opp, opp_t) {
            return true;
        }
        if self.contains_coin(state, state[self].top, seg, opp, opp_t) {
            return true;
        }
        false
    }

    /// `contains(const SkCoincidentSpans* coin, const SkOpSegment* seg,
    /// const SkOpSegment* opp, double oppT) const`.
    pub(crate) fn contains_coin(
        self,
        state: &SkOpGlobalState,
        coin: Option<SkCoincidentSpans>,
        seg: SkOpSegment,
        opp: SkOpSegment,
        opp_t: f64,
    ) -> bool {
        let Some(mut coin) = coin else {
            return false;
        };
        loop {
            if coin.coin_pt_t_start(state).segment(state) == seg
                && coin.opp_pt_t_start(state).segment(state) == opp
                && between(
                    state[coin.opp_pt_t_start(state)].t,
                    opp_t,
                    state[coin.opp_pt_t_end(state)].t,
                )
            {
                return true;
            }
            if coin.opp_pt_t_start(state).segment(state) == seg
                && coin.coin_pt_t_start(state).segment(state) == opp
                && between(
                    state[coin.coin_pt_t_start(state)].t,
                    opp_t,
                    state[coin.coin_pt_t_end(state)].t,
                )
            {
                return true;
            }
            match coin.next(state) {
                Some(next) => coin = next,
                None => break,
            }
        }
        false
    }

    /// `fixUp(SkCoincidentSpans** headPtr, SkOpPtT* deleted, const SkOpPtT* kept)`.
    pub(crate) fn fix_up_list(
        self,
        state: &mut SkOpGlobalState,
        head_ptr: CoinListLink,
        deleted: SkOpPtT,
        kept: SkOpPtT,
    ) {
        let mut head_ptr = head_ptr;
        while let Some(coin) = head_ptr.get(state) {
            if coin.coin_pt_t_start(state) == deleted {
                if coin.coin_pt_t_end(state).span(state) == kept.span(state) {
                    head_ptr.set(state, coin.next(state));
                    continue;
                }
                coin.set_coin_pt_t_start(state, kept);
            }
            if coin.coin_pt_t_end(state) == deleted {
                if coin.coin_pt_t_start(state).span(state) == kept.span(state) {
                    head_ptr.set(state, coin.next(state));
                    continue;
                }
                coin.set_coin_pt_t_end(state, kept);
            }
            if coin.opp_pt_t_start(state) == deleted {
                if coin.opp_pt_t_end(state).span(state) == kept.span(state) {
                    head_ptr.set(state, coin.next(state));
                    continue;
                }
                coin.set_opp_pt_t_start(state, kept);
            }
            if coin.opp_pt_t_end(state) == deleted {
                if coin.opp_pt_t_start(state).span(state) == kept.span(state) {
                    head_ptr.set(state, coin.next(state));
                    continue;
                }
                coin.set_opp_pt_t_end(state, kept);
            }
            head_ptr = coin.next_ptr();
        }
    }

    /// `markCollapsed(SkCoincidentSpans** headPtr, SkOpPtT* test)`.
    pub(crate) fn mark_collapsed_list(
        self,
        state: &mut SkOpGlobalState,
        head_ptr: CoinListLink,
        test: SkOpPtT,
    ) {
        let mut head_ptr = head_ptr;
        while let Some(coin) = head_ptr.get(state) {
            if coin.collapsed(state, test) {
                if zero_or_one(state[coin.coin_pt_t_start(state)].t)
                    && zero_or_one(state[coin.coin_pt_t_end(state)].t)
                {
                    coin.coin_pt_t_start_writable(state)
                        .segment(state)
                        .mark_all_done(state);
                }
                if zero_or_one(state[coin.opp_pt_t_start(state)].t)
                    && zero_or_one(state[coin.opp_pt_t_end(state)].t)
                {
                    coin.opp_pt_t_start_writable(state)
                        .segment(state)
                        .mark_all_done(state);
                }
                head_ptr.set(state, coin.next(state));
            } else {
                head_ptr = coin.next_ptr();
            }
        }
    }

    pub(crate) fn overlap(
        self,
        state: &SkOpGlobalState,
        coin_start1: SkOpPtT,
        coin_end1: SkOpPtT,
        coin_start2: SkOpPtT,
        coin_end2: SkOpPtT,
        over_s: &mut f64,
        over_e: &mut f64,
    ) -> bool {
        debug_assert!(coin_start1.segment(state) == coin_start2.segment(state));
        *over_s = std_max(
            std_min(state[coin_start1].t, state[coin_end1].t),
            std_min(state[coin_start2].t, state[coin_end2].t),
        );
        *over_e = std_min(
            std_max(state[coin_start1].t, state[coin_end1].t),
            std_max(state[coin_start2].t, state[coin_end2].t),
        );
        *over_s < *over_e
    }

    /// `release(SkCoincidentSpans** headPtr, SkCoincidentSpans* remove)`.
    pub(crate) fn release_coin(
        self,
        state: &mut SkOpGlobalState,
        head_ptr: CoinListLink,
        remove: SkCoincidentSpans,
    ) -> bool {
        let mut head_ptr = head_ptr;
        while let Some(coin) = head_ptr.get(state) {
            if coin == remove {
                head_ptr.set(state, coin.next(state));
                return true;
            }
            head_ptr = coin.next_ptr();
        }
        false
    }

    /// `release(SkCoincidentSpans** headPtr, const SkOpSegment* deleted)`.
    pub(crate) fn release_segment_list(
        self,
        state: &mut SkOpGlobalState,
        head_ptr: CoinListLink,
        deleted: SkOpSegment,
    ) {
        debug_assert!(head_ptr == CoinListLink::Head(self) || head_ptr == CoinListLink::Top(self));
        let mut head_ptr = head_ptr;
        while let Some(coin) = head_ptr.get(state) {
            if coin.coin_pt_t_start(state).segment(state) == deleted
                || coin.coin_pt_t_end(state).segment(state) == deleted
                || coin.opp_pt_t_start(state).segment(state) == deleted
                || coin.opp_pt_t_end(state).segment(state) == deleted
            {
                head_ptr.set(state, coin.next(state));
            } else {
                head_ptr = coin.next_ptr();
            }
        }
    }

    /// `releaseDeleted(SkCoincidentSpans** headPtr)`.
    pub(crate) fn release_deleted_list(self, state: &mut SkOpGlobalState, head_ptr: CoinListLink) {
        debug_assert!(head_ptr == CoinListLink::Head(self) || head_ptr == CoinListLink::Top(self));
        let mut head_ptr = head_ptr;
        while let Some(coin) = head_ptr.get(state) {
            if coin.coin_pt_t_start(state).deleted(state) {
                head_ptr.set(state, coin.next(state));
            } else {
                head_ptr = coin.next_ptr();
            }
        }
    }

    pub(crate) fn restore_head(self, state: &mut SkOpGlobalState) {
        let mut head_ptr = CoinListLink::Head(self);
        while let Some(coin) = head_ptr.get(state) {
            head_ptr = coin.next_ptr();
        }
        let top = state[self].top;
        head_ptr.set(state, top);
        state[self].top = None;
        // segments may have collapsed in the meantime; remove empty referenced segments
        head_ptr = CoinListLink::Head(self);
        while let Some(test) = head_ptr.get(state) {
            if test.coin_pt_t_start(state).segment(state).done(state)
                || test.opp_pt_t_start(state).segment(state).done(state)
            {
                head_ptr.set(state, test.next(state));
                continue;
            }
            head_ptr = test.next_ptr();
        }
    }

    // given a t span, map the same range on the coincident span
    /*
    the curves may not scale linearly, so interpolation may only happen within known points
    remap over1s, over1e, cointPtTStart, coinPtTEnd to smallest range that captures over1s
    then repeat to capture over1e
    */
    /// return coinPtT->segment()->t mapped from overS->fT <= t <= overE->fT
    pub(crate) fn t_range(
        state: &SkOpGlobalState,
        over_s: SkOpPtT,
        t: f64,
        coin_seg: SkOpSegment,
    ) -> f64 {
        let mut work = over_s.span(state);
        let mut found_start = None;
        let mut found_end = None;
        let mut coin_start = None;
        let mut coin_end = None;
        loop {
            match work.contains_segment(state, coin_seg) {
                None => {
                    if work.r#final(state) {
                        break;
                    }
                }
                Some(contained) => {
                    if work.t(state) <= t {
                        coin_start = Some(contained);
                        found_start = Some(work.pt_t());
                    }
                    if work.t(state) >= t {
                        coin_end = Some(contained);
                        found_end = Some(work.pt_t());
                        break;
                    }
                }
            }
            match work.up_cast().next(state) {
                Some(next) => work = next,
                None => break,
            }
        }
        let (Some(coin_start), Some(coin_end)) = (coin_start, coin_end) else {
            return 1.0;
        };
        let found_start = found_start.expect("set with coinStart");
        let found_end = found_end.expect("set with coinEnd");
        // while overS->fT <=t and overS contains coinSeg
        let denom = state[found_end].t - state[found_start].t;
        let s_ratio = if denom != 0.0 {
            (t - state[found_start].t) / denom
        } else {
            1.0
        };
        state[coin_start].t + (state[coin_end].t - state[coin_start].t) * s_ratio
    }
}
