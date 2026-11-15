//! Port of Skia's src/pathops/SkIntersectionHelper.h: a cursor over a contour's segments, as
//! the intersection pass walks them pairwise.

use crate::core::path::SkPathVerb;
use crate::core::point::SkPoint;
use crate::op_contour::SkOpContour;
use crate::op_segment::SkOpSegment;
use crate::path_ops_bounds::SkPathOpsBounds;
use crate::path_ops_types::SkOpGlobalState;

/// `SkIntersectionHelper::SegmentType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub(crate) enum SegmentType {
    HorizontalLine = -1,
    VerticalLine = 0,
    Line = SkPathVerb::Line as i32,
    Quad = SkPathVerb::Quad as i32,
    Conic = SkPathVerb::Conic as i32,
    Cubic = SkPathVerb::Cubic as i32,
}

/// Skia leaves `fSegment` unset until `init`; it is null past the contour's last segment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SkIntersectionHelper {
    segment: Option<SkOpSegment>,
}

impl SkIntersectionHelper {
    pub(crate) fn advance(&mut self, state: &SkOpGlobalState) -> bool {
        self.segment = self.segment().next(state);
        self.segment.is_some()
    }

    pub(crate) fn bottom(&self, state: &SkOpGlobalState) -> f32 {
        self.bounds(state).bottom
    }

    pub(crate) fn bounds<'a>(&self, state: &'a SkOpGlobalState) -> &'a SkPathOpsBounds {
        self.segment().bounds(state)
    }

    pub(crate) fn init(&mut self, state: &SkOpGlobalState, contour: SkOpContour) {
        self.segment = Some(contour.first(state));
    }

    pub(crate) fn left(&self, state: &SkOpGlobalState) -> f32 {
        self.bounds(state).left
    }

    pub(crate) fn pts(&self, state: &SkOpGlobalState) -> [SkPoint; 4] {
        self.segment().pts(state)
    }

    pub(crate) fn right(&self, state: &SkOpGlobalState) -> f32 {
        self.bounds(state).right
    }

    pub(crate) fn segment(&self) -> SkOpSegment {
        self.segment.expect("the helper is on a segment")
    }

    pub(crate) fn segment_type(&self, state: &SkOpGlobalState) -> SegmentType {
        let segment = self.segment();
        let segment_type = match segment.verb(state) {
            SkPathVerb::Line => SegmentType::Line,
            SkPathVerb::Quad => SegmentType::Quad,
            SkPathVerb::Conic => SegmentType::Conic,
            SkPathVerb::Cubic => SegmentType::Cubic,
            verb => panic!("a segment's verb is a curve, not {verb:?}"),
        };
        if segment_type != SegmentType::Line {
            return segment_type;
        }
        if segment.is_horizontal(state) {
            return SegmentType::HorizontalLine;
        }
        if segment.is_vertical(state) {
            return SegmentType::VerticalLine;
        }
        SegmentType::Line
    }

    pub(crate) fn start_after(
        &mut self,
        state: &SkOpGlobalState,
        after: &SkIntersectionHelper,
    ) -> bool {
        self.segment = after.segment().next(state);
        self.segment.is_some()
    }

    pub(crate) fn top(&self, state: &SkOpGlobalState) -> f32 {
        self.bounds(state).top
    }

    pub(crate) fn weight(&self, state: &SkOpGlobalState) -> f32 {
        self.segment().weight(state)
    }

    pub(crate) fn x(&self, state: &SkOpGlobalState) -> f32 {
        self.bounds(state).left
    }

    pub(crate) fn x_flipped(&self, state: &SkOpGlobalState) -> bool {
        self.x(state) != self.pts(state)[0].x
    }

    pub(crate) fn y(&self, state: &SkOpGlobalState) -> f32 {
        self.bounds(state).top
    }

    pub(crate) fn y_flipped(&self, state: &SkOpGlobalState) -> bool {
        self.y(state) != self.pts(state)[0].y
    }
}
