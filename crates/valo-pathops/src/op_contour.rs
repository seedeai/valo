//! Port of Skia's src/pathops/SkOpContour.{h,cpp}: a closed run of segments, the list of
//! contours an operation works on, and the builder that adds segments to a contour.
//!
//! `SkOpContour` is a handle into `SkOpGlobalState`; Skia embeds a contour's first segment,
//! which here is allocated with the contour and recorded by handle. `SkOpContourHead`, Skia's
//! subclass for the first contour of the list, wraps the contour's handle and dereferences to
//! it. Skia's `fState` back-pointer is the state parameter.

use std::ops::{Deref, Index, IndexMut};

use crate::core::path::SkPathVerb;
use crate::core::point::SkPoint;
use crate::op_segment::SkOpSegment;
use crate::op_span::SkOpSpan;
use crate::path_ops_bounds::SkPathOpsBounds;
use crate::path_ops_types::{arena_handle, SkOpGlobalState};

arena_handle! {
    /// Stands for `SkOpContour*`.
    SkOpContour
}

/// Stands for `SkOpContourHead*`: the first contour of the list.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub(crate) struct SkOpContourHead(pub(crate) SkOpContour);

impl Deref for SkOpContourHead {
    type Target = SkOpContour;

    fn deref(&self) -> &SkOpContour {
        &self.0
    }
}

/// `SkOpContour`'s fields.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkOpContourData {
    pub(crate) head: SkOpSegment,
    pub(crate) tail: Option<SkOpSegment>,
    pub(crate) next: Option<SkOpContour>,
    pub(crate) bounds: SkPathOpsBounds,
    pub(crate) count: i32,
    /// set by find top segment
    pub(crate) done: bool,
    /// true for the second argument to a binary operator
    pub(crate) operand: bool,
    /// set if original path had even-odd fill
    pub(crate) xor: bool,
    /// set if opposite path had even-odd fill
    pub(crate) opp_xor: bool,
}

impl Index<SkOpContour> for SkOpGlobalState {
    type Output = SkOpContourData;

    fn index(&self, contour: SkOpContour) -> &SkOpContourData {
        &self.contours[contour.0 as usize]
    }
}

impl IndexMut<SkOpContour> for SkOpGlobalState {
    fn index_mut(&mut self, contour: SkOpContour) -> &mut SkOpContourData {
        &mut self.contours[contour.0 as usize]
    }
}

impl SkOpContour {
    /// `operator<`: orders by the top of the bounds, then the left.
    pub(crate) fn less_than(self, state: &SkOpGlobalState, rh: SkOpContour) -> bool {
        let bounds = &state[self].bounds;
        let rh_bounds = &state[rh].bounds;
        if bounds.top == rh_bounds.top {
            bounds.left < rh_bounds.left
        } else {
            bounds.top < rh_bounds.top
        }
    }

    pub(crate) fn add_conic(self, state: &mut SkOpGlobalState, pts: &[SkPoint], weight: f32) {
        self.append_segment(state)
            .add_conic(state, pts, weight, self);
    }

    pub(crate) fn add_cubic(self, state: &mut SkOpGlobalState, pts: &[SkPoint]) {
        self.append_segment(state).add_cubic(state, pts, self);
    }

    pub(crate) fn add_line(self, state: &mut SkOpGlobalState, pts: &[SkPoint]) -> SkOpSegment {
        debug_assert!(pts[0] != pts[1]);
        self.append_segment(state).add_line(state, pts, self)
    }

    pub(crate) fn add_quad(self, state: &mut SkOpGlobalState, pts: &[SkPoint]) {
        self.append_segment(state).add_quad(state, pts, self);
    }

    pub(crate) fn append_segment(self, state: &mut SkOpGlobalState) -> SkOpSegment {
        let count = state[self].count;
        state[self].count += 1;
        let result = if count != 0 {
            state.make_segment()
        } else {
            state[self].head
        };
        let tail = state[self].tail;
        if let Some(tail) = tail {
            tail.set_next(state, Some(result));
        }
        state[self].tail = Some(result);
        result
    }

    pub(crate) fn bounds(self, state: &SkOpGlobalState) -> &SkPathOpsBounds {
        &state[self].bounds
    }

    pub(crate) fn calc_angles(self, state: &mut SkOpGlobalState) {
        debug_assert!(state[self].count > 0);
        let mut segment = Some(state[self].head);
        while let Some(current) = segment {
            current.calc_angles(state);
            segment = current.next(state);
        }
    }

    pub(crate) fn complete(self, state: &mut SkOpGlobalState) {
        self.set_bounds(state);
    }

    pub(crate) fn count(self, state: &SkOpGlobalState) -> i32 {
        state[self].count
    }

    pub(crate) fn done(self, state: &SkOpGlobalState) -> bool {
        state[self].done
    }

    // `findSortableTop` is defined in SkPathOpsWinding.cpp: see `path_ops_winding.rs`.

    pub(crate) fn first(self, state: &SkOpGlobalState) -> SkOpSegment {
        debug_assert!(state[self].count > 0);
        state[self].head
    }

    pub(crate) fn init(self, state: &mut SkOpGlobalState, operand: bool, is_xor: bool) {
        state[self].operand = operand;
        state[self].xor = is_xor;
    }

    pub(crate) fn is_xor(self, state: &SkOpGlobalState) -> bool {
        state[self].xor
    }

    // Please keep this aligned with debugMissingCoincidence()
    pub(crate) fn missing_coincidence(self, state: &mut SkOpGlobalState) -> bool {
        debug_assert!(state[self].count > 0);
        let mut segment = Some(state[self].head);
        let mut result = false;
        while let Some(current) = segment {
            if current.missing_coincidence(state) {
                result = true;
            }
            segment = current.next(state);
        }
        result
    }

    pub(crate) fn move_multiples(self, state: &mut SkOpGlobalState) -> bool {
        debug_assert!(state[self].count > 0);
        let mut segment = Some(state[self].head);
        while let Some(current) = segment {
            if !current.move_multiples(state) {
                return false;
            }
            segment = current.next(state);
        }
        true
    }

    pub(crate) fn move_nearby(self, state: &mut SkOpGlobalState) -> bool {
        debug_assert!(state[self].count > 0);
        let mut segment = Some(state[self].head);
        while let Some(current) = segment {
            if !current.move_nearby(state) {
                return false;
            }
            segment = current.next(state);
        }
        true
    }

    pub(crate) fn next(self, state: &SkOpGlobalState) -> Option<SkOpContour> {
        state[self].next
    }

    pub(crate) fn operand(self, state: &SkOpGlobalState) -> bool {
        state[self].operand
    }

    pub(crate) fn opp_xor(self, state: &SkOpGlobalState) -> bool {
        state[self].opp_xor
    }

    // `rayCheck` is defined in SkPathOpsWinding.cpp: see `path_ops_winding.rs`.

    pub(crate) fn reset(self, state: &mut SkOpGlobalState) {
        let data = &mut state[self];
        data.tail = None;
        data.next = None;
        data.count = 0;
        data.done = false;
    }

    pub(crate) fn set_bounds(self, state: &mut SkOpGlobalState) {
        debug_assert!(state[self].count > 0);
        let mut segment = state[self].head;
        let mut bounds = *segment.bounds(state);
        while let Some(next) = segment.next(state) {
            segment = next;
            bounds.add_bounds(segment.bounds(state));
        }
        state[self].bounds = bounds;
    }

    pub(crate) fn set_next(self, state: &mut SkOpGlobalState, contour: Option<SkOpContour>) {
        state[self].next = contour;
    }

    pub(crate) fn set_opp_xor(self, state: &mut SkOpGlobalState, is_opp_xor: bool) {
        state[self].opp_xor = is_opp_xor;
    }

    pub(crate) fn sort_angles(self, state: &mut SkOpGlobalState) -> bool {
        debug_assert!(state[self].count > 0);
        let mut segment = Some(state[self].head);
        while let Some(current) = segment {
            if !current.sort_angles(state) {
                return false;
            }
            segment = current.next(state);
        }
        true
    }

    /// Skia's `continue` in the do-while goes to the `next()` step.
    pub(crate) fn undone_span(self, state: &mut SkOpGlobalState) -> Option<SkOpSpan> {
        let mut test_segment = Some(state[self].head);
        while let Some(current) = test_segment {
            if !current.done(state) {
                return current.undone_span(state);
            }
            test_segment = current.next(state);
        }
        state[self].done = true;
        None
    }
}

impl SkOpContourHead {
    pub(crate) fn append_contour(self, state: &mut SkOpGlobalState) -> SkOpContour {
        let contour = state.make_contour();
        contour.set_next(state, None);
        let mut prev = self.0;
        while let Some(next) = prev.next(state) {
            prev = next;
        }
        prev.set_next(state, Some(contour));
        contour
    }

    pub(crate) fn remove(self, state: &mut SkOpGlobalState, contour: SkOpContour) {
        if contour == self.0 {
            debug_assert!(self.count(state) == 0);
            return;
        }
        debug_assert!(contour.next(state).is_none());
        let mut prev = self.0;
        loop {
            let next = prev.next(state);
            if next == Some(contour) {
                break;
            }
            prev = next.expect("the contour is in the list");
        }
        prev.set_next(state, None);
    }
}

/// `SkOpContourBuilder`: adds segments to a contour, holding back a line until the next
/// segment shows whether it doubles back over the line (both are then dropped).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SkOpContourBuilder {
    pub(crate) contour: Option<SkOpContour>,
    pub(crate) last_line: [SkPoint; 2],
    pub(crate) last_is_line: bool,
}

impl SkOpContourBuilder {
    pub(crate) fn new(contour: Option<SkOpContour>) -> SkOpContourBuilder {
        SkOpContourBuilder {
            contour,
            last_line: [SkPoint::default(); 2],
            last_is_line: false,
        }
    }

    pub(crate) fn add_conic(&mut self, state: &mut SkOpGlobalState, pts: &[SkPoint], weight: f32) {
        self.flush(state);
        self.built_contour().add_conic(state, pts, weight);
    }

    pub(crate) fn add_cubic(&mut self, state: &mut SkOpGlobalState, pts: &[SkPoint]) {
        self.flush(state);
        self.built_contour().add_cubic(state, pts);
    }

    /// Skia copies the points into the arena before adding the curve; the segment copies them
    /// here. `weight` defaults to 1 in Skia.
    pub(crate) fn add_curve(
        &mut self,
        state: &mut SkOpGlobalState,
        verb: SkPathVerb,
        pts: &[SkPoint],
        weight: f32,
    ) {
        if SkPathVerb::Line == verb {
            self.add_line(state, pts);
            return;
        }
        match verb {
            SkPathVerb::Quad => {
                self.add_quad(state, &pts[..3]);
            }
            SkPathVerb::Conic => {
                self.add_conic(state, &pts[..3], weight);
            }
            SkPathVerb::Cubic => {
                self.add_cubic(state, &pts[..4]);
            }
            _ => debug_assert!(false),
        }
    }

    pub(crate) fn add_line(&mut self, state: &mut SkOpGlobalState, pts: &[SkPoint]) {
        // if the previous line added is the exact opposite, eliminate both
        if self.last_is_line {
            if self.last_line[0] == pts[1] && self.last_line[1] == pts[0] {
                self.last_is_line = false;
                return;
            } else {
                self.flush(state);
            }
        }
        self.last_line = [pts[0], pts[1]];
        self.last_is_line = true;
    }

    pub(crate) fn add_quad(&mut self, state: &mut SkOpGlobalState, pts: &[SkPoint]) {
        self.flush(state);
        self.built_contour().add_quad(state, pts);
    }

    pub(crate) fn flush(&mut self, state: &mut SkOpGlobalState) {
        if !self.last_is_line {
            return;
        }
        let pt_storage = self.last_line;
        let _ = self.built_contour().add_line(state, &pt_storage);
        self.last_is_line = false;
    }

    /// Skia's `fContour`, which is set whenever a segment is added.
    fn built_contour(&self) -> SkOpContour {
        self.contour.expect("the builder has a contour")
    }

    pub(crate) fn contour(&self) -> Option<SkOpContour> {
        self.contour
    }

    pub(crate) fn set_contour(
        &mut self,
        state: &mut SkOpGlobalState,
        contour: Option<SkOpContour>,
    ) {
        self.flush(state);
        self.contour = contour;
    }
}
