//! Port of the parts of Skia's include/core/SkPath.h, src/core/SkPath.cpp, src/core/SkPathData.cpp
//! and src/core/SkPathPriv.{h,cpp} that PathOps calls: the path's storage, its validation,
//! bounds and queries, and its two iterations (`SkPath::Iter` and `SkPathPriv::Iterate`).
//!
//! Skia's path shares an immutable `SkPathData`; here the path owns its arrays. An invalid or
//! non-finite input becomes Skia's error path: no verbs, empty bounds, and `is_finite` false.

use crate::core::float_bits::{sk_is_finite_f32, std_max, std_min};
use crate::core::point::{SkPoint, SkVector};
use crate::core::rect::SkRect;

/// `SkPathFillType` says which points a path's contours enclose.
///
/// The winding rule counts how many times the contours wind around a point, the even-odd rule
/// whether they cross a ray from it an odd number of times; the inverse rules fill the
/// complement. The values are Skia's, so bit 0 is even-odd and bit 1 is inverse.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SkPathFillType {
    /// `Winding` fills where the contours wind a non-zero number of times; the default.
    #[default]
    Winding = 0,
    /// `EvenOdd` fills where the contours wind an odd number of times.
    EvenOdd = 1,
    /// `InverseWinding` fills where `Winding` does not.
    InverseWinding = 2,
    /// `InverseEvenOdd` fills where `EvenOdd` does not.
    InverseEvenOdd = 3,
}

impl SkPathFillType {
    pub(crate) fn from_bits(bits: u8) -> SkPathFillType {
        match bits & 3 {
            0 => SkPathFillType::Winding,
            1 => SkPathFillType::EvenOdd,
            2 => SkPathFillType::InverseWinding,
            _ => SkPathFillType::InverseEvenOdd,
        }
    }

    /// `SkPathFillType_IsInverse`.
    pub(crate) fn is_inverse(self) -> bool {
        (self as u8 & 2) != 0
    }

    /// `SkPathFillType_ToggleInverse`.
    pub(crate) fn toggle_inverse(self) -> SkPathFillType {
        SkPathFillType::from_bits(self as u8 ^ 2)
    }
}

/// `SkPathVerb` is one step of a path: where a contour starts, a segment, or a close.
///
/// A path stores one verb per step and, for each, the points it adds: `Move` one, `Line` one,
/// `Quad` and `Conic` two, `Cubic` three, `Close` none. A segment starts at the previous point.
/// The values are Skia's, and also stand for Skia's legacy `SkPath::Verb` inside PathOps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SkPathVerb {
    /// `Move` starts a contour at its point.
    #[default]
    Move = 0,
    /// `Line` draws a straight line to its point.
    Line = 1,
    /// `Quad` draws a quadratic Bézier through its control point to its end point.
    Quad = 2,
    /// `Conic` draws a conic section (a rational quadratic with its weight) to its end point.
    Conic = 3,
    /// `Cubic` draws a cubic Bézier through its two control points to its end point.
    Cubic = 4,
    /// `Close` joins the contour's last point back to its first.
    Close = 5,
}

impl SkPathVerb {
    /// The verb whose value is `value`; Skia casts the int, so a value past `Close` is a bug.
    pub(crate) fn from_i32(value: i32) -> SkPathVerb {
        match value {
            0 => SkPathVerb::Move,
            1 => SkPathVerb::Line,
            2 => SkPathVerb::Quad,
            3 => SkPathVerb::Conic,
            4 => SkPathVerb::Cubic,
            5 => SkPathVerb::Close,
            _ => panic!("not a path verb: {value}"),
        }
    }
}

/// `SkPath::AddPathMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkPathAddPathMode {
    /// `kAppend_AddPathMode`: the added path starts a new contour.
    Append,
    /// `kExtend_AddPathMode`: the added path's first point is joined to the last by a line.
    Extend,
}

/// `SkPathConvexity` (src/core/SkPathEnums.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkPathConvexity {
    ConvexCw,
    ConvexCcw,
    /// Known to not have a determinable direction, but convex.
    ConvexDegenerate,
    Concave,
    Unknown,
}

impl SkPathConvexity {
    /// `SkPathConvexity_IsConvex`.
    pub(crate) fn is_convex(self) -> bool {
        matches!(
            self,
            SkPathConvexity::ConvexCw
                | SkPathConvexity::ConvexCcw
                | SkPathConvexity::ConvexDegenerate
        )
    }
}

/// `kLine_SkPathSegmentMask` and its siblings.
pub(crate) const K_LINE_SK_PATH_SEGMENT_MASK: u8 = 1 << 0;
pub(crate) const K_QUAD_SK_PATH_SEGMENT_MASK: u8 = 1 << 1;
pub(crate) const K_CONIC_SK_PATH_SEGMENT_MASK: u8 = 1 << 2;
pub(crate) const K_CUBIC_SK_PATH_SEGMENT_MASK: u8 = 1 << 3;

/// `SkPath` is a shape made of contours of lines and curves, and the rule that fills it.
///
/// A path holds three arrays, read together: its verbs, the points the verbs add, and a weight
/// for each conic. A contour starts with a move and may end with a close; see [`SkPathVerb`]
/// for how many points each verb adds. The boolean operations read a path and return a new
/// one, so a path is a plain value that clones its arrays.
#[derive(Clone, Debug, PartialEq)]
pub struct SkPath {
    verbs: Vec<SkPathVerb>,
    points: Vec<SkPoint>,
    conic_weights: Vec<f32>,
    fill_type: SkPathFillType,
    /// `SkPathData::fBounds`, computed when the path is made.
    bounds: SkRect,
    /// `SkPathData::fSegmentMask`.
    segment_mask: u8,
    /// False for Skia's error singleton: the path made from invalid or non-finite input.
    is_finite: bool,
}

impl Default for SkPath {
    fn default() -> SkPath {
        SkPath::new()
    }
}

impl SkPath {
    /// `new` makes an empty path filled with the winding rule.
    pub fn new() -> SkPath {
        SkPath::with_fill_type(SkPathFillType::Winding)
    }

    /// `from_raw` makes a path from its verbs, points and conic weights, as Skia's
    /// `SkPath::Make` does.
    ///
    /// Valid arrays are stored exactly as given: no verbs are added, merged or dropped. The
    /// arrays are valid when the verbs are empty or start with a move, no move follows a move,
    /// only a move follows a close, the points and weights are exactly as many as the verbs
    /// add, every weight is finite and not negative, and every point (other than a trailing
    /// move's, which must still be finite) bounds finitely. Anything else gives Skia's error
    /// path: empty, with the given fill type, which the boolean operations treat as Skia does.
    pub fn from_raw(
        verbs: Vec<SkPathVerb>,
        points: Vec<SkPoint>,
        conic_weights: Vec<f32>,
        fill_type: SkPathFillType,
    ) -> SkPath {
        if !valid_path_data(&points, &verbs, &conic_weights) {
            return SkPath::error(fill_type);
        }
        SkPath::make_no_check(verbs, points, conic_weights, fill_type)
            .unwrap_or_else(|| SkPath::error(fill_type))
    }

    /// `verbs` returns the path's verbs, one per step.
    pub fn verbs(&self) -> &[SkPathVerb] {
        &self.verbs
    }

    /// `points` returns the path's points, in the order the verbs add them.
    pub fn points(&self) -> &[SkPoint] {
        &self.points
    }

    /// `conic_weights` returns one weight per conic verb, in order.
    pub fn conic_weights(&self) -> &[f32] {
        &self.conic_weights
    }

    /// `fill_type` returns the rule that fills the path.
    pub fn fill_type(&self) -> SkPathFillType {
        self.fill_type
    }

    /// `set_fill_type` changes the rule that fills the path; the geometry is unchanged.
    pub fn set_fill_type(&mut self, fill_type: SkPathFillType) {
        self.fill_type = fill_type;
    }

    /// `explicit SkPath(SkPathFillType)`.
    pub(crate) fn with_fill_type(fill_type: SkPathFillType) -> SkPath {
        SkPath {
            verbs: Vec::new(),
            points: Vec::new(),
            conic_weights: Vec::new(),
            fill_type,
            bounds: SkRect::make_empty(),
            segment_mask: 0,
            is_finite: true,
        }
    }

    /// `SkPath::MakeNullCheck` with a null `SkPathData`: the error singleton.
    pub(crate) fn error(fill_type: SkPathFillType) -> SkPath {
        SkPath {
            is_finite: false,
            ..SkPath::with_fill_type(fill_type)
        }
    }

    /// `SkPathData::MakeNoCheck` without known bounds: computes bounds and the segment mask,
    /// `None` when the bounds are not finite (`finishInit` failing).
    pub(crate) fn make_no_check(
        verbs: Vec<SkPathVerb>,
        points: Vec<SkPoint>,
        conic_weights: Vec<f32>,
        fill_type: SkPathFillType,
    ) -> Option<SkPath> {
        let mut path = SkPath {
            verbs,
            points,
            conic_weights,
            ..SkPath::with_fill_type(fill_type)
        };
        if path.points.is_empty() {
            return Some(path);
        }
        path.segment_mask = SkPathPriv::compute_segment_mask(&path.verbs);
        path.bounds = SkPathPriv::trimmed_bounds(&path.points, &path.verbs)?;
        Some(path)
    }

    /// `SkPathData::finishInit` with known bounds and segment mask.
    pub(crate) fn set_cached_bounds(&mut self, bounds: SkRect, segment_mask: u8) {
        self.bounds = bounds;
        self.segment_mask = segment_mask;
    }

    /// `SkPath::Rect(r)`: clockwise from the top-left corner, winding fill, or the error path
    /// when `r` is not finite.
    pub(crate) fn rect(r: &SkRect) -> SkPath {
        if !r.is_finite() {
            return SkPath::error(SkPathFillType::Winding);
        }
        SkPath {
            verbs: vec![
                SkPathVerb::Move,
                SkPathVerb::Line,
                SkPathVerb::Line,
                SkPathVerb::Line,
                SkPathVerb::Close,
            ],
            points: vec![
                SkPoint::new(r.left, r.top),
                SkPoint::new(r.right, r.top),
                SkPoint::new(r.right, r.bottom),
                SkPoint::new(r.left, r.bottom),
            ],
            conic_weights: Vec::new(),
            fill_type: SkPathFillType::Winding,
            bounds: r.make_sorted(),
            segment_mask: K_LINE_SK_PATH_SEGMENT_MASK,
            is_finite: true,
        }
    }

    pub(crate) fn get_fill_type(&self) -> SkPathFillType {
        self.fill_type
    }

    pub(crate) fn get_bounds(&self) -> &SkRect {
        &self.bounds
    }

    pub(crate) fn get_segment_masks(&self) -> u8 {
        self.segment_mask
    }

    pub(crate) fn is_finite(&self) -> bool {
        self.is_finite
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    pub(crate) fn is_inverse_fill_type(&self) -> bool {
        self.fill_type.is_inverse()
    }

    pub(crate) fn toggle_inverse_fill_type(&mut self) {
        self.fill_type = self.fill_type.toggle_inverse();
    }

    pub(crate) fn make_fill_type(&self, fill_type: SkPathFillType) -> SkPath {
        let mut copy = self.clone();
        copy.set_fill_type(fill_type);
        copy
    }

    /// `SkPath::isRect(SkRect* rect)` with `isClosed` and `direction` null.
    pub(crate) fn is_rect(&self, rect: &mut SkRect) -> bool {
        if let Some(rc) =
            SkPathPriv::is_rect_contour(&self.points, &self.verbs, self.get_segment_masks(), false)
        {
            *rect = rc.rect;
            return true;
        }
        false
    }

    /// `SkPath::isConvex`: Skia caches the computed convexity; this computes it each call.
    pub(crate) fn is_convex(&self) -> bool {
        self.get_convexity().is_convex()
    }

    /// `SkPath::getConvexity` through `computeConvexity`.
    pub(crate) fn get_convexity(&self) -> SkPathConvexity {
        let mut convexity = SkPathConvexity::Concave;
        if self.is_finite() {
            convexity =
                SkPathPriv::compute_convexity(&self.points, &self.verbs, &self.conic_weights);
        }
        debug_assert!(convexity != SkPathConvexity::Unknown);
        convexity
    }
}

/// `valid_path_data` (src/core/SkPathData.cpp).
fn valid_path_data(pts: &[SkPoint], vbs: &[SkPathVerb], conics: &[f32]) -> bool {
    const PTS_PER_VERB: [usize; 6] = [1, 1, 2, 2, 3, 0];
    if vbs.is_empty() {
        return pts.is_empty() && conics.is_empty();
    }
    // We must begin with a Move (unless we're empty)
    if vbs[0] != SkPathVerb::Move {
        return false;
    }
    let mut prev = SkPathVerb::Move;
    let mut point_count = 1;
    let mut conic_count = 0;
    for &curr in &vbs[1..] {
        if prev == SkPathVerb::Move && curr == SkPathVerb::Move {
            return false;
        }
        if prev == SkPathVerb::Close && curr != SkPathVerb::Move {
            return false;
        }
        point_count += PTS_PER_VERB[curr as usize];
        conic_count += usize::from(curr == SkPathVerb::Conic);
        prev = curr;
    }
    if pts.len() != point_count || conics.len() != conic_count {
        return false;
    }
    conics.iter().all(|&w| valid_conic_weight(w))
}

/// `valid_conic_weight` (src/core/SkPathData.cpp).
fn valid_conic_weight(w: f32) -> bool {
    w >= 0.0 && crate::core::float_bits::sk_is_finite_f32(&[w])
}

/// `SkPath::IterRec`: one verb and its points, the segment's start point first.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SkPathIterRec {
    pub(crate) verb: SkPathVerb,
    /// Stands for `fPoints`, a span into the iterator's storage: the first `point_count`.
    pub(crate) storage: [SkPoint; 4],
    pub(crate) point_count: usize,
}

impl SkPathIterRec {
    pub(crate) fn points(&self) -> &[SkPoint] {
        &self.storage[..self.point_count]
    }
}

/// `SkPath::Iter`: walks a path, giving each segment with its start point and, when
/// `force_close` is set, closing every contour with a line back to its start.
///
/// Skia's pointers into the path are indices here: `verb` into the verbs and `pt` into the
/// points.
pub(crate) struct SkPathIter<'a> {
    path: &'a SkPath,
    pt: usize,
    verb: usize,
    move_to: SkPoint,
    last_pt: SkPoint,
    force_close: bool,
    need_close: bool,
    storage: [SkPoint; 4],
}

/// `SkPath::kDone_Verb`, which `SkPath::Iter::next(SkPoint[4])` returns past the last verb.
const K_DONE_VERB: i32 = 6;

impl<'a> SkPathIter<'a> {
    pub(crate) fn new(path: &'a SkPath, force_close: bool) -> SkPathIter<'a> {
        SkPathIter {
            path,
            pt: 0,
            verb: 0,
            move_to: SkPoint::new(0.0, 0.0),
            last_pt: SkPoint::new(0.0, 0.0),
            force_close,
            need_close: false,
            storage: [SkPoint::default(); 4],
        }
    }

    fn auto_close(&mut self, pts: &mut [SkPoint; 4]) -> SkPathVerb {
        if self.last_pt != self.move_to {
            // A special case: if both points are NaN, SkPoint::operation== returns
            // false, but the iterator expects that they are treated as the same.
            // (consider SkPoint is a 2-dimension float point).
            if self.last_pt.x.is_nan()
                || self.last_pt.y.is_nan()
                || self.move_to.x.is_nan()
                || self.move_to.y.is_nan()
            {
                return SkPathVerb::Close;
            }
            pts[0] = self.last_pt;
            pts[1] = self.move_to;
            self.last_pt = self.move_to;
            return SkPathVerb::Line;
        }
        pts[0] = self.move_to;
        SkPathVerb::Close
    }

    /// `SkPath::Iter::next(SkPoint pts[4])`: the legacy verb, `K_DONE_VERB` at the end.
    fn next_legacy(&mut self, pts: &mut [SkPoint; 4]) -> i32 {
        let verbs = &self.path.verbs;
        if self.verb == verbs.len() {
            // Close the curve if requested and if there is some curve to close
            if self.need_close {
                if SkPathVerb::Line == self.auto_close(pts) {
                    return SkPathVerb::Line as i32;
                }
                self.need_close = false;
                return SkPathVerb::Close as i32;
            }
            return K_DONE_VERB;
        }
        let mut verb = verbs[self.verb];
        self.verb += 1;
        let src = &self.path.points;
        let mut src_pts = self.pt;
        match verb {
            SkPathVerb::Move => {
                if self.need_close {
                    self.verb -= 1; // move back one verb
                    verb = self.auto_close(pts);
                    if verb == SkPathVerb::Close {
                        self.need_close = false;
                    }
                    return verb as i32;
                }
                if self.verb == verbs.len() {
                    // might be a trailing moveto
                    return K_DONE_VERB;
                }
                self.move_to = src[src_pts];
                pts[0] = src[src_pts];
                src_pts += 1;
                self.last_pt = self.move_to;
                self.need_close = self.force_close;
            }
            SkPathVerb::Line => {
                pts[0] = self.last_pt;
                pts[1] = src[src_pts];
                self.last_pt = src[src_pts];
                src_pts += 1;
            }
            SkPathVerb::Conic | SkPathVerb::Quad => {
                pts[0] = self.last_pt;
                pts[1] = src[src_pts];
                pts[2] = src[src_pts + 1];
                self.last_pt = src[src_pts + 1];
                src_pts += 2;
            }
            SkPathVerb::Cubic => {
                pts[0] = self.last_pt;
                pts[1] = src[src_pts];
                pts[2] = src[src_pts + 1];
                pts[3] = src[src_pts + 2];
                self.last_pt = src[src_pts + 2];
                src_pts += 3;
            }
            SkPathVerb::Close => {
                verb = self.auto_close(pts);
                if verb == SkPathVerb::Line {
                    self.verb -= 1; // move back one verb
                } else {
                    self.need_close = false;
                }
                self.last_pt = self.move_to;
            }
        }
        self.pt = src_pts;
        verb as i32
    }

    /// `SkPath::Iter::next()`.
    pub(crate) fn next(&mut self) -> Option<SkPathIterRec> {
        const COUNTS: [usize; 6] = [1, 2, 3, 3, 4, 0];
        let mut storage = self.storage;
        let legacy_verb = self.next_legacy(&mut storage);
        self.storage = storage;
        if legacy_verb == K_DONE_VERB {
            return None;
        }
        let verb = SkPathVerb::from_i32(legacy_verb);
        Some(SkPathIterRec {
            verb,
            storage: self.storage,
            point_count: COUNTS[verb as usize],
        })
    }
}

/// Stands for Skia's `SkPathPriv`, a namespace of path helpers.
pub(crate) struct SkPathPriv;

/// `SkPathPriv::RectContour`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RectContour {
    pub(crate) rect: SkRect,
}

impl SkPathPriv {
    /// `SkPathPriv::Iterate(path)`: no steps for a non-finite path.
    pub(crate) fn iterate(path: &SkPath) -> SkPathPrivIterate<'_> {
        let mut iterate = SkPathPriv::iterate_raw(&path.verbs, &path.points, &path.conic_weights);
        // Don't allow iteration through non-finite points.
        if !path.is_finite() {
            iterate.verb = iterate.verbs.len();
        }
        iterate
    }

    /// `SkPathPriv::Iterate(verbs, points, weights)`.
    pub(crate) fn iterate_raw<'a>(
        verbs: &'a [SkPathVerb],
        points: &'a [SkPoint],
        weights: &'a [f32],
    ) -> SkPathPrivIterate<'a> {
        SkPathPrivIterate {
            verbs,
            points,
            weights,
            verb: 0,
            point: 0,
            weight: 0,
        }
    }

    /// `SkPathPriv::PtsInVerb`: the points a verb adds, not counting its start point.
    pub(crate) fn pts_in_verb(verb: SkPathVerb) -> usize {
        const PTS_IN_VERB: [usize; 6] = [1, 1, 2, 2, 3, 0];
        PTS_IN_VERB[verb as usize]
    }

    /// `SkPathPriv::ComputeSegmentMask` (src/core/SkPathRaw.cpp).
    pub(crate) fn compute_segment_mask(verbs: &[SkPathVerb]) -> u8 {
        const VERB_TO_SEGMENT_MASK: [u8; 6] = [
            0, // move
            K_LINE_SK_PATH_SEGMENT_MASK,
            K_QUAD_SK_PATH_SEGMENT_MASK,
            K_CONIC_SK_PATH_SEGMENT_MASK,
            K_CUBIC_SK_PATH_SEGMENT_MASK,
            0, // close
        ];
        let mut mask = 0;
        for &v in verbs {
            mask |= VERB_TO_SEGMENT_MASK[v as usize];
        }
        mask
    }

    /// `SkPathPriv::TrimmedBounds`: empty if there are no points, `None` if the bounds are not
    /// finite.
    pub(crate) fn trimmed_bounds(pts: &[SkPoint], vbs: &[SkPathVerb]) -> Option<SkRect> {
        let mut pts = pts;
        // Does a trailing kMove verb contribute to the bounds?
        // - only if it is the only verb in the path
        // - otherwise we ignore it when computing bounds
        if vbs.len() > 1 && vbs[vbs.len() - 1] == SkPathVerb::Move {
            debug_assert!(!pts.is_empty());
            // While trailing moves do not contribute to the bounds, we still reject them.
            if !pts[pts.len() - 1].is_finite() {
                return None;
            }
            pts = &pts[..pts.len() - 1];
        }
        SkRect::bounds(pts)
    }

    /// `SkPathPriv::FindLastMoveToIndex`.
    pub(crate) fn find_last_move_to_index(verbs: &[SkPathVerb], pt_count: usize) -> i32 {
        if verbs.is_empty() {
            debug_assert!(pt_count == 0);
            return -1;
        }
        debug_assert!(verbs[0] == SkPathVerb::Move);
        debug_assert!(pt_count > 0);
        let mut pt_index = pt_count as i32 - 1;
        for &verb in verbs.iter().rev() {
            if verb == SkPathVerb::Move {
                break;
            }
            pt_index -= SkPathPriv::pts_in_verb(verb) as i32;
        }
        debug_assert!(pt_index >= 0);
        pt_index
    }

    /// `SkPathPriv::IsRectContour` (src/core/SkPathPriv.cpp), with its file-local helpers
    /// `trivial_rect` and `rect_make_dir`.
    pub(crate) fn is_rect_contour(
        pts: &[SkPoint],
        vbs: &[SkPathVerb],
        segment_mask: u8,
        allow_partial: bool,
    ) -> Option<RectContour> {
        let pt_span = pts;
        let vb_span = vbs;
        if segment_mask != K_LINE_SK_PATH_SEGMENT_MASK || pt_span.len() < 4 || vb_span.len() < 4 {
            return None;
        }

        if let Some(rc) = trivial_rect(pt_span, vb_span) {
            return Some(rc);
        }

        let mut curr_verb: usize = 0;
        let verb_cnt = vb_span.len();

        let mut corners: i32 = 0;
        // Skia's point pointers are indices into `pt_span`.
        let mut close_xy: SkPoint; // used to determine if final line falls on a diagonal
        let mut line_start = SkPoint::default(); // used to construct line from previous point
        let mut first_pt: Option<usize> = None; // first point in the rect (last of first moves)
        let mut last_pt: Option<usize> = None; // last point in the rect (last of lines or first if closed)
        let mut first_corner = SkPoint::default();
        let mut third_corner = SkPoint::default();
        let mut pts: usize = 0;
        line_start.set(0.0, 0.0);
        let mut directions: [i8; 5] = [-1, -1, -1, -1, -1]; // -1 to 3; -1 is uninitialized
        let mut closed_or_moved = false;
        let mut auto_close = false;
        let mut insert_close = false;
        while curr_verb < verb_cnt && (!allow_partial || !auto_close) {
            let verb = if insert_close {
                SkPathVerb::Close
            } else {
                vb_span[curr_verb]
            };
            match verb {
                SkPathVerb::Close | SkPathVerb::Line => 'line: {
                    if verb == SkPathVerb::Close {
                        auto_close = true;
                        insert_close = false;
                    }
                    if SkPathVerb::Close != verb {
                        last_pt = Some(pts);
                    }
                    let line_end = if SkPathVerb::Close == verb {
                        pt_span[first_pt.expect("a rect contour starts with a move")]
                    } else {
                        pts += 1;
                        pt_span[pts - 1]
                    };
                    let line_delta = line_end - line_start;
                    if line_delta.x != 0.0 && line_delta.y != 0.0 {
                        return None; // diagonal
                    }
                    if !line_delta.is_finite() {
                        return None; // path contains infinity or NaN
                    }
                    if line_start == line_end {
                        break 'line; // single point on side OK
                    }
                    let next_direction = rect_make_dir(line_delta.x, line_delta.y) as i8; // 0 to 3
                    if 0 == corners {
                        directions[0] = next_direction;
                        corners = 1;
                        closed_or_moved = false;
                        line_start = line_end;
                        break 'line;
                    }
                    if closed_or_moved {
                        return None; // closed followed by a line
                    }
                    if auto_close && next_direction == directions[0] {
                        break 'line; // colinear with first
                    }
                    closed_or_moved = auto_close;
                    if directions[(corners - 1) as usize] == next_direction {
                        if 3 == corners && SkPathVerb::Line == verb {
                            third_corner = line_end;
                        }
                        line_start = line_end;
                        break 'line; // colinear segment
                    }
                    directions[corners as usize] = next_direction;
                    corners += 1;
                    // opposite lines must point in opposite directions; xoring them should equal 2
                    match corners {
                        2 => {
                            first_corner = line_start;
                        }
                        3 => {
                            if (directions[0] ^ directions[2]) != 2 {
                                return None;
                            }
                            third_corner = line_end;
                        }
                        4 => {
                            if (directions[1] ^ directions[3]) != 2 {
                                return None;
                            }
                        }
                        _ => return None, // too many direction changes
                    }
                    line_start = line_end;
                }
                SkPathVerb::Quad | SkPathVerb::Conic | SkPathVerb::Cubic => {
                    return None; // quadratic, cubic not allowed
                }
                SkPathVerb::Move => {
                    if allow_partial && !auto_close && directions[0] >= 0 {
                        insert_close = true;
                        // Skia's `goto addMissingClose` skips the increment below.
                        curr_verb -= 1; // try move again afterwards
                        continue;
                    }
                    if corners == 0 {
                        first_pt = Some(pts);
                    } else {
                        let first = pt_span[first_pt.expect("a rect contour starts with a move")];
                        let last = pt_span[last_pt.expect("a corner follows a line")];
                        close_xy = first - last;
                        if close_xy.x != 0.0 && close_xy.y != 0.0 {
                            return None; // we're diagonal, abort
                        }
                    }
                    line_start = pt_span[pts];
                    pts += 1;
                    closed_or_moved = true;
                }
            }
            curr_verb += 1;
        }
        // Success if 4 corners and first point equals last
        if !(3..=4).contains(&corners) {
            return None;
        }
        // check if close generates diagonal
        close_xy = pt_span[first_pt.expect("a rect contour starts with a move")]
            - pt_span[last_pt.expect("a corner follows a line")];
        if close_xy.x != 0.0 && close_xy.y != 0.0 {
            return None;
        }

        // `SkRect::set(p0, p1)`.
        let bounds = |a: SkPoint, b: SkPoint| {
            SkRect::make_ltrb(
                std_min(a.x, b.x),
                std_min(a.y, b.y),
                std_max(a.x, b.x),
                std_max(a.y, b.y),
            )
        };

        Some(RectContour {
            rect: bounds(first_corner, third_corner),
        })
    }

    /// `SkPathPriv::ComputeConvexity` (src/core/SkPathPriv.cpp), with its `Convexicator`.
    pub(crate) fn compute_convexity(
        pts: &[SkPoint],
        verbs: &[SkPathVerb],
        conic_weights: &[f32],
    ) -> SkPathConvexity {
        let mut points = pts;
        let mut vbs = verbs;
        // callers need to give us finite values
        debug_assert!(SkRect::bounds(points).is_some());

        trim_trailing_moves(&mut points, &mut vbs);

        if vbs.is_empty() {
            return SkPathConvexity::ConvexDegenerate;
        }

        // Check to see if path changes direction more than three times as quick concave test
        if Convexicator::is_concave_by_sign(points) {
            return SkPathConvexity::Concave;
        }

        let mut contour_count = 0;
        let mut needs_close = false;
        let mut state = Convexicator::default();

        // Skia's `SkPathIter`, which gives the same points for every verb but a close, whose
        // points this does not read; the trailing moves it would trim are trimmed above.
        for (verb, pts, _) in SkPathPriv::iterate_raw(vbs, points, conic_weights) {
            // Looking for the last moveTo before non-move verbs start
            if contour_count == 0 {
                if verb == SkPathVerb::Move {
                    state.set_move_pt(&pts[0]);
                } else {
                    // Starting the actual contour, fall through to c=1 to add the points
                    contour_count += 1;
                    needs_close = true;
                }
            }
            // Accumulating points into the Convexicator until we hit a close or another move
            if contour_count == 1 {
                if verb == SkPathVerb::Close || verb == SkPathVerb::Move {
                    if !state.close() {
                        return SkPathConvexity::Concave;
                    }
                    needs_close = false;
                    contour_count += 1;
                } else {
                    // lines add 1 point, cubics add 3, conics and quads add 2
                    let count = SkPathPriv::pts_in_verb(verb);
                    debug_assert!(count > 0);
                    for pt in &pts[1..=count] {
                        if !state.add_pt(pt) {
                            return SkPathConvexity::Concave;
                        }
                    }
                }
            } else {
                // The first contour has closed and anything other than spurious trailing moves means
                // there's multiple contours and the path can't be convex
                if verb != SkPathVerb::Move {
                    return SkPathConvexity::Concave;
                }
            }
        }

        // If the path isn't explicitly closed do so implicitly
        if needs_close && !state.close() {
            return SkPathConvexity::Concave;
        }

        let first_dir = state.get_first_direction();
        if first_dir == SkPathFirstDirection::Unknown && state.reversals() >= 3 {
            return SkPathConvexity::Concave;
        }
        sk_path_first_direction_to_convexity(first_dir)
    }
}

fn rect_make_dir(dx: f32, dy: f32) -> i32 {
    (i32::from(0.0 != dx)) | (i32::from(dx > 0.0 || dy > 0.0) << 1)
}

// Quick check for a "trivial" rect, i.e. a rect created via SkPath::Rect(),
// SkPathBuilder::addRect(), etc.
fn trivial_rect(pts: &[SkPoint], vbs: &[SkPathVerb]) -> Option<RectContour> {
    const G_TRIVIAL_VERBS: [SkPathVerb; 5] = [
        SkPathVerb::Move,
        SkPathVerb::Line,
        SkPathVerb::Line,
        SkPathVerb::Line,
        SkPathVerb::Close,
    ];
    if pts.len() != 4 || vbs.len() != G_TRIVIAL_VERBS.len() || vbs != G_TRIVIAL_VERBS {
        return None;
    }

    let v0 = pts[1] - pts[0];
    let v1 = pts[2] - pts[1];
    let v2 = pts[3] - pts[2];
    let v3 = pts[0] - pts[3];

    let axis_aligned_orthogonal = |a: &SkVector, b: &SkVector| -> bool {
        // Assuming one of the vectors is known to be axis-aligned,
        // check whether the other one is orthogonal (and implicitly axis-aligned).
        // I.e. we have exactly one vertical vector (fX == 0, fY != 0) and one
        // horizontal vector (fX != 0, fY == 0).
        ((a.x == 0.0) ^ (b.x == 0.0)) & ((a.y == 0.0) ^ (b.y == 0.0))
    };

    // We have a rect iff the side vectors are axis aligned and form 3 right corners.
    // This can be further reduced to one axis aligned vector and 3 orthogonal vectors.
    // Note: bitwise operators are measurably faster in micro benchmarks (no short-circuiting?).
    if !(
        // Axis-aligned v0.
        ((v0.x == 0.0) ^ (v0.y == 0.0))
            // Orthogonal vectors, in alternating dimensions.
            & axis_aligned_orthogonal(&v0, &v1)
            & axis_aligned_orthogonal(&v1, &v2)
            & axis_aligned_orthogonal(&v2, &v3)
    ) {
        return None;
    }

    let rect = SkRect::make_ltrb(pts[0].x, pts[0].y, pts[2].x, pts[2].y).make_sorted();

    Some(RectContour { rect })
}

/// `SkPathFirstDirection` (src/core/SkPathEnums.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SkPathFirstDirection {
    Cw,
    Ccw,
    Unknown,
}

/// `SkPathFirstDirection_ToConvexity` (src/core/SkPathEnums.h).
fn sk_path_first_direction_to_convexity(dir: SkPathFirstDirection) -> SkPathConvexity {
    match dir {
        SkPathFirstDirection::Cw => SkPathConvexity::ConvexCw,
        SkPathFirstDirection::Ccw => SkPathConvexity::ConvexCcw,
        SkPathFirstDirection::Unknown => SkPathConvexity::ConvexDegenerate,
    }
}

fn sign(x: f32) -> i32 {
    i32::from(x < 0.0)
}
const K_VALUE_NEVER_RETURNED_BY_SIGN: i32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirChange {
    Unknown,
    Left,
    Right,
    Straight,
    Backwards, // if double back, allow simple lines to be convex
    Invalid,
}

// only valid for a single contour
struct Convexicator {
    first_pt: SkPoint,   // The first point of the contour, e.g. moveTo(x,y)
    first_vec: SkVector, // The direction leaving fFirstPt to the next vertex

    last_pt: SkPoint,   // The last point passed to addPt()
    last_vec: SkVector, // The direction that brought the path to fLastPt

    expected_dir: DirChange,
    first_direction: SkPathFirstDirection,
    reversals: i32,
}

impl Default for Convexicator {
    fn default() -> Convexicator {
        Convexicator {
            first_pt: SkPoint::new(0.0, 0.0),
            first_vec: SkPoint::new(0.0, 0.0),
            last_pt: SkPoint::new(0.0, 0.0),
            last_vec: SkPoint::new(0.0, 0.0),
            expected_dir: DirChange::Invalid,
            first_direction: SkPathFirstDirection::Unknown,
            reversals: 0,
        }
    }
}

impl Convexicator {
    /** The direction returned is only valid if the path is determined convex */
    fn get_first_direction(&self) -> SkPathFirstDirection {
        self.first_direction
    }

    fn set_move_pt(&mut self, pt: &SkPoint) {
        self.first_pt = *pt;
        self.last_pt = *pt;
        self.expected_dir = DirChange::Invalid;
    }

    fn add_pt(&mut self, pt: &SkPoint) -> bool {
        if self.last_pt == *pt {
            return true;
        }
        // should only be true for first non-zero vector after setMovePt was called. It is possible
        // we doubled backed at the start so need to check if fLastVec is zero or not.
        if self.first_pt == self.last_pt
            && self.expected_dir == DirChange::Invalid
            && self.last_vec.x == 0.0
            && self.last_vec.y == 0.0
        {
            self.last_vec = *pt - self.last_pt;
            self.first_vec = self.last_vec;
        } else if !self.add_vec(&(*pt - self.last_pt)) {
            return false;
        }
        self.last_pt = *pt;
        true
    }

    fn is_concave_by_sign(points: &[SkPoint]) -> bool {
        let count = points.len();
        if count <= 3 {
            // point, line, or triangle are always convex
            return false;
        }

        let mut curr_pt = points[0];
        let first_pt = curr_pt;
        let mut dxes = 0;
        let mut dyes = 0;
        let mut last_sx = K_VALUE_NEVER_RETURNED_BY_SIGN;
        let mut last_sy = K_VALUE_NEVER_RETURNED_BY_SIGN;
        // Skia walks `points[1..]`, then, pointing at its local `firstPt`, the one vector back
        // to the first point.
        let mut rest = &points[1..];
        let first_only = [first_pt];
        for outer_loop in 0..2 {
            for next in rest {
                let vec = *next - curr_pt;
                if !((0.0 == vec.x) & (0.0 == vec.y)) {
                    // give up if vector construction failed
                    if !vec.is_finite() {
                        return true; // treat as concave
                    }
                    let sx = sign(vec.x);
                    let sy = sign(vec.y);
                    dxes += i32::from(sx != last_sx);
                    dyes += i32::from(sy != last_sy);
                    if dxes > 3 || dyes > 3 {
                        return true;
                    }
                    last_sx = sx;
                    last_sy = sy;
                }
                curr_pt = *next;
                if outer_loop != 0 {
                    break;
                }
            }
            rest = &first_only;
        }
        false // that is, it may be convex, don't know yet
    }

    fn close(&mut self) -> bool {
        // If this was an explicit close, there was already a lineTo to fFirstPoint, so this
        // addPt() is a no-op. Otherwise, the addPt implicitly closes the contour. In either case,
        // we have to check the direction change along the first vector in case it is concave.
        let first_pt = self.first_pt;
        if !self.add_pt(&first_pt) {
            return false;
        }
        let first_vec = self.first_vec;
        self.add_vec(&first_vec)
    }

    fn reversals(&self) -> i32 {
        self.reversals
    }

    fn direction_change(&self, cur_vec: &SkVector) -> DirChange {
        let cross = SkPoint::cross_product(&self.last_vec, cur_vec);
        if !sk_is_finite_f32(&[cross]) {
            return DirChange::Unknown;
        }
        if cross == 0.0 {
            return if self.last_vec.dot(cur_vec) < 0.0 {
                DirChange::Backwards
            } else {
                DirChange::Straight
            };
        }
        if 1 == sk_scalar_sign_as_int(cross) {
            DirChange::Right
        } else {
            DirChange::Left
        }
    }

    fn add_vec(&mut self, cur_vec: &SkVector) -> bool {
        let dir = self.direction_change(cur_vec);
        match dir {
            DirChange::Left | DirChange::Right => {
                if DirChange::Invalid == self.expected_dir {
                    self.expected_dir = dir;
                    self.first_direction = if DirChange::Right == dir {
                        SkPathFirstDirection::Cw
                    } else {
                        SkPathFirstDirection::Ccw
                    };
                } else if dir != self.expected_dir {
                    self.first_direction = SkPathFirstDirection::Unknown;
                    return false;
                }
                self.last_vec = *cur_vec;
            }
            DirChange::Straight => {}
            DirChange::Backwards => {
                //  allow path to reverse direction twice
                //    Given path.moveTo(0, 0); path.lineTo(1, 1);
                //    - 1st reversal: direction change formed by line (0,0 1,1), line (1,1 0,0)
                //    - 2nd reversal: direction change formed by line (1,1 0,0), line (0,0 1,1)
                self.last_vec = *cur_vec;
                self.reversals += 1;
                return self.reversals < 3;
            }
            DirChange::Unknown => {
                return false;
            }
            DirChange::Invalid => {
                panic!("Use of invalid direction change flag");
            }
        }
        true
    }
}

/// `SkScalarSignAsInt` (include/core/SkScalar.h).
fn sk_scalar_sign_as_int(x: f32) -> i32 {
    if x < 0.0 {
        -1
    } else {
        i32::from(x > 0.0)
    }
}

fn trim_trailing_moves(pts: &mut &[SkPoint], vbs: &mut &[SkPathVerb]) {
    let mut vb_count = vbs.len();
    while vb_count > 0 && vbs[vb_count - 1] == SkPathVerb::Move {
        vb_count -= 1;
    }
    let delta = vbs.len() - vb_count;
    if delta != 0 {
        debug_assert!(pts.len() >= delta);
        *pts = &pts[..pts.len() - delta];
        *vbs = &vbs[..vbs.len() - delta];
    }
}

/// `SkPathPriv::Iterate`'s range (`SkPath::RangeIter`): each verb with its points, the
/// segment's start point first, and the conic weights from the current one on.
///
/// Skia yields a pointer to the points, peeking one back for every verb but a move (a close
/// gets the contour's last point); here that is a slice of the points the verb reads: one for
/// a move or close, two for a line, three for a quad or conic, four for a cubic. Skia's
/// weight pointer is a slice starting at the current weight, so `w[0]` is Skia's `*w`.
pub(crate) struct SkPathPrivIterate<'a> {
    verbs: &'a [SkPathVerb],
    points: &'a [SkPoint],
    weights: &'a [f32],
    verb: usize,
    point: usize,
    weight: usize,
}

impl<'a> Iterator for SkPathPrivIterate<'a> {
    type Item = (SkPathVerb, &'a [SkPoint], &'a [f32]);

    fn next(&mut self) -> Option<Self::Item> {
        if self.verb == self.verbs.len() {
            return None;
        }
        let verb = self.verbs[self.verb];
        let (backset, read, advance) = match verb {
            SkPathVerb::Move => (0, 1, 1),
            SkPathVerb::Line => (1, 2, 1),
            SkPathVerb::Quad | SkPathVerb::Conic => (1, 3, 2),
            SkPathVerb::Cubic => (1, 4, 3),
            SkPathVerb::Close => (1, 1, 0),
        };
        let start = self.point - backset;
        let pts = &self.points[start..start + read];
        let weights = &self.weights[self.weight..];
        self.verb += 1;
        self.point += advance;
        if verb == SkPathVerb::Conic {
            self.weight += 1;
        }
        Some((verb, pts, weights))
    }
}

/// `SkPathRaw` (src/core/SkPathRaw.h): a path's arrays borrowed with their bounds. Its
/// convexity is left out: PathOps only asks for it unresolved.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SkPathRaw<'a> {
    pub(crate) points: &'a [SkPoint],
    pub(crate) verbs: &'a [SkPathVerb],
    pub(crate) conics: &'a [f32],
    pub(crate) bounds: SkRect,
    pub(crate) segment_mask: u8,
}
