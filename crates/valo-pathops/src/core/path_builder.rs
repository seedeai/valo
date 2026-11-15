//! Port of the parts of Skia's include/core/SkPathBuilder.h and src/core/SkPathBuilder.cpp that
//! PathOps calls, with `SkPathPriv::ReversePathTo` / `ReverseAddPath` and
//! `SkPathPriv::Raw(const SkPathBuilder&)`.
//!
//! Skia's builder also tracks volatility, whether it holds an oval or rounded rect, and a
//! cached convexity; none of them changes what PathOps writes, so they are left out. Every
//! matrix PathOps passes is the identity, so the transforming variants are ported without one.

use crate::core::float_bits::sk_is_finite_f32;
use crate::core::path::{
    SkPath, SkPathAddPathMode, SkPathFillType, SkPathPriv, SkPathRaw, SkPathVerb,
    K_CONIC_SK_PATH_SEGMENT_MASK, K_CUBIC_SK_PATH_SEGMENT_MASK, K_LINE_SK_PATH_SEGMENT_MASK,
    K_QUAD_SK_PATH_SEGMENT_MASK,
};
use crate::core::point::SkPoint;
use crate::core::rect::SkRect;

/// `SkPathBuilder::Reserve`: how `addRaw` grows the arrays. It changes no result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkPathBuilderReserve {
    Exact,
}

/// `SkPathBuilder` builds an [`SkPath`] from drawing commands, as Skia's `SkPathBuilder` does.
///
/// It stores what Skia's builder stores for the same calls, which is what a path operation sees:
/// a move after a move replaces it, a segment after a close starts at the closed contour's first
/// point, a conic of weight 1 is a quadratic, and a second close in a row is ignored. Build a path
/// with it, rather than with [`SkPath::from_raw`], when the commands come from a caller that
/// draws (as dart:ui's `Path` does) and may repeat a move.
#[derive(Clone, Debug, PartialEq)]
pub struct SkPathBuilder {
    pts: Vec<SkPoint>,
    verbs: Vec<SkPathVerb>,
    conic_weights: Vec<f32>,
    fill_type: SkPathFillType,
    segment_mask: u8,
    /// Index of the last move's point; -1 (illegal) when there is none.
    last_move_index: i32,
}

impl Default for SkPathBuilder {
    fn default() -> SkPathBuilder {
        SkPathBuilder::new()
    }
}

impl SkPathBuilder {
    /// `new` makes an empty builder whose path is filled with the winding rule.
    pub fn new() -> SkPathBuilder {
        SkPathBuilder {
            pts: Vec::new(),
            verbs: Vec::new(),
            conic_weights: Vec::new(),
            fill_type: SkPathFillType::Winding,
            segment_mask: 0,
            last_move_index: -1,
        }
    }

    /// `with_fill_type` makes an empty builder whose path is filled with `fill_type`.
    pub fn with_fill_type(fill_type: SkPathFillType) -> SkPathBuilder {
        let mut builder = SkPathBuilder::new();
        builder.fill_type = fill_type;
        builder
    }

    /// `operator=(const SkPath&)`.
    pub(crate) fn assign_path(&mut self, src: &SkPath) {
        self.reset();
        self.fill_type = src.get_fill_type();
        if src.is_empty() {
            return;
        }
        let raw = SkPathRaw {
            points: src.points(),
            verbs: src.verbs(),
            conics: src.conic_weights(),
            bounds: *src.get_bounds(),
            segment_mask: src.get_segment_masks(),
        };
        self.add_raw(&raw, SkPathBuilderReserve::Exact);
        // These are not part of SkPathRaw, so we set them separately
        self.last_move_index = SkPathPriv::find_last_move_to_index(&self.verbs, self.pts.len());
    }

    pub(crate) fn reset(&mut self) -> &mut SkPathBuilder {
        self.pts.clear();
        self.verbs.clear();
        self.conic_weights.clear();
        self.fill_type = SkPathFillType::Winding;
        // these are internal state
        self.segment_mask = 0;
        self.last_move_index = -1; // illegal
        self
    }

    pub(crate) fn points(&self) -> &[SkPoint] {
        &self.pts
    }

    pub(crate) fn verbs(&self) -> &[SkPathVerb] {
        &self.verbs
    }

    pub(crate) fn conic_weights(&self) -> &[f32] {
        &self.conic_weights
    }

    pub(crate) fn count_points(&self) -> i32 {
        self.pts.len() as i32
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    pub(crate) fn get_last_pt(&self) -> Option<SkPoint> {
        self.pts.last().copied()
    }

    fn ensure_move(&mut self) {
        if self.verbs.is_empty() {
            self.move_to(SkPoint::new(0.0, 0.0));
        } else if self.verbs[self.verbs.len() - 1] == SkPathVerb::Close {
            let pt = self.pts[self.last_move_index as usize];
            self.move_to(pt);
        }
    }

    /// `move_to` starts a contour at `pt`, replacing the point of a move just before it.
    pub fn move_to(&mut self, pt: SkPoint) -> &mut SkPathBuilder {
        if self.verbs.last() == Some(&SkPathVerb::Move) {
            let last = self.pts.len() - 1;
            self.pts[last] = pt;
        } else {
            self.last_move_index = self.pts.len() as i32;
            self.pts.push(pt);
            self.verbs.push(SkPathVerb::Move);
        }
        self
    }

    /// `line_to` adds a line to `pt`.
    pub fn line_to(&mut self, pt: SkPoint) -> &mut SkPathBuilder {
        self.ensure_move();
        self.pts.push(pt);
        self.verbs.push(SkPathVerb::Line);
        self.segment_mask |= K_LINE_SK_PATH_SEGMENT_MASK;
        self
    }

    /// `quad_to` adds a quadratic through control point `pt1` to `pt2`.
    pub fn quad_to(&mut self, pt1: SkPoint, pt2: SkPoint) -> &mut SkPathBuilder {
        self.ensure_move();
        self.pts.push(pt1);
        self.pts.push(pt2);
        self.verbs.push(SkPathVerb::Quad);
        self.segment_mask |= K_QUAD_SK_PATH_SEGMENT_MASK;
        self
    }

    /// `conic_to` adds a conic through control point `pt1` to `pt2` with weight `w`.
    ///
    /// A weight of 1 adds a quadratic, a non-finite weight two lines.
    pub fn conic_to(&mut self, pt1: SkPoint, pt2: SkPoint, w: f32) -> &mut SkPathBuilder {
        self.ensure_move();
        self.pts.push(pt1);
        self.pts.push(pt2);
        if w == 1.0 {
            self.verbs.push(SkPathVerb::Quad);
            self.segment_mask |= K_QUAD_SK_PATH_SEGMENT_MASK;
        } else if sk_is_finite_f32(&[w]) {
            self.verbs.push(SkPathVerb::Conic);
            self.conic_weights.push(w);
            self.segment_mask |= K_CONIC_SK_PATH_SEGMENT_MASK;
        } else {
            self.verbs.push(SkPathVerb::Line);
            self.verbs.push(SkPathVerb::Line);
            self.segment_mask |= K_LINE_SK_PATH_SEGMENT_MASK;
        }
        self
    }

    /// `cubic_to` adds a cubic through control points `pt1` and `pt2` to `pt3`.
    pub fn cubic_to(
        &mut self,
        pt1: SkPoint,
        pt2: SkPoint,
        pt3: SkPoint,
    ) -> &mut SkPathBuilder {
        self.ensure_move();
        self.pts.push(pt1);
        self.pts.push(pt2);
        self.pts.push(pt3);
        self.verbs.push(SkPathVerb::Cubic);
        self.segment_mask |= K_CUBIC_SK_PATH_SEGMENT_MASK;
        self
    }

    /// `close` closes the current contour; a second close in a row is ignored.
    pub fn close(&mut self) -> &mut SkPathBuilder {
        if let Some(&last) = self.verbs.last() {
            if last != SkPathVerb::Close {
                self.ensure_move();
                self.verbs.push(SkPathVerb::Close);
            }
        }
        self
    }

    /// `snapshot(nullptr)`: the path so far, or the error path when its bounds are not finite.
    pub(crate) fn snapshot(&self) -> SkPath {
        match SkPathPriv::raw_builder(self) {
            Some(raw) => {
                if raw.verbs.is_empty() {
                    return SkPath::with_fill_type(self.fill_type);
                }
                path_from_raw_with_bounds(&raw, self.fill_type)
            }
            None => SkPath::error(self.fill_type),
        }
    }

    /// `detach` is the path built so far, leaving the builder empty: Skia's `detach()`.
    ///
    /// A path whose points do not bound finitely is Skia's error path: empty, with the
    /// builder's fill type.
    pub fn detach(&mut self) -> SkPath {
        let path = self.snapshot();
        self.reset();
        path
    }

    /// `addRaw`: replays the raw path's verbs, restoring a trailing move.
    pub(crate) fn add_raw(
        &mut self,
        raw: &SkPathRaw,
        reserve: SkPathBuilderReserve,
    ) -> &mut SkPathBuilder {
        match reserve {
            SkPathBuilderReserve::Exact => {
                self.pts.reserve_exact(raw.points.len());
                self.verbs.reserve_exact(raw.verbs.len());
                self.conic_weights.reserve_exact(raw.conics.len());
            }
        }
        for (verb, pts, w) in SkPathPriv::iterate_raw(raw.verbs, raw.points, raw.conics) {
            match verb {
                SkPathVerb::Move => self.move_to(pts[0]),
                SkPathVerb::Line => self.line_to(pts[1]),
                SkPathVerb::Quad => self.quad_to(pts[1], pts[2]),
                SkPathVerb::Conic => self.conic_to(pts[1], pts[2], w[0]),
                SkPathVerb::Cubic => self.cubic_to(pts[1], pts[2], pts[3]),
                SkPathVerb::Close => self.close(),
            };
        }
        let has_trailing_move = |vbs: &[SkPathVerb]| vbs.last() == Some(&SkPathVerb::Move);
        // if the iterator 'trimmed' off a trialing move, we restore it here
        if has_trailing_move(raw.verbs) && !has_trailing_move(&self.verbs) {
            self.move_to(raw.points[raw.points.len() - 1]);
        }
        self
    }

    /// `addPath(src, SkMatrix::I(), mode)`.
    pub(crate) fn add_path(&mut self, src: &SkPath, mode: SkPathAddPathMode) -> &mut SkPathBuilder {
        if src.is_empty() {
            return self;
        }
        let can_replace_this =
            (mode == SkPathAddPathMode::Append && self.verbs.len() <= 1) || self.verbs.is_empty();
        if can_replace_this {
            let fill_type = self.fill_type;
            self.assign_path(src);
            self.fill_type = fill_type;
            return self;
        }
        if SkPathAddPathMode::Append == mode {
            // If the current builder ends with a moveTo and src starts with one (which is always
            // true if non-empty), we must discard the builder moveTo in order to maintain
            // internal consistency after append (no repeating moveTos).
            if self.verbs.last() == Some(&SkPathVerb::Move) && !src.is_empty() {
                self.verbs.pop();
                self.pts.pop();
            }
            let last_move_to_index =
                SkPathPriv::find_last_move_to_index(src.verbs(), src.points().len());
            self.last_move_index = last_move_to_index + self.count_points();
            // growForVerbsInPath, then the identity map of the points
            self.segment_mask |= src.get_segment_masks();
            self.verbs.extend_from_slice(src.verbs());
            self.pts.extend_from_slice(src.points());
            self.conic_weights.extend_from_slice(src.conic_weights());
            return self;
        }
        let mut first_verb = true;
        for (verb, pts, w) in SkPathPriv::iterate(src) {
            match verb {
                SkPathVerb::Move => {
                    if first_verb && mode == SkPathAddPathMode::Extend && !self.is_empty() {
                        self.ensure_move(); // In case last contour is closed
                        let last_pt = self.get_last_pt();
                        // don't add lineTo if it is degenerate
                        if last_pt != Some(pts[0]) {
                            self.line_to(pts[0]);
                        }
                    } else {
                        self.move_to(pts[0]);
                    }
                }
                SkPathVerb::Line => {
                    self.line_to(pts[1]);
                }
                SkPathVerb::Quad => {
                    self.quad_to(pts[1], pts[2]);
                }
                SkPathVerb::Conic => {
                    self.conic_to(pts[1], pts[2], w[0]);
                }
                SkPathVerb::Cubic => {
                    self.cubic_to(pts[1], pts[2], pts[3]);
                }
                SkPathVerb::Close => {
                    self.close();
                }
            }
            first_verb = false;
        }
        self
    }

    /// `computeFiniteBounds`.
    pub(crate) fn compute_finite_bounds(&self) -> Option<SkRect> {
        SkPathPriv::trimmed_bounds(&self.pts, &self.verbs)
    }

    /// `privateReversePathTo`: ignores the last point of the 1st contour.
    fn private_reverse_path_to(&mut self, path: &SkPath) -> &mut SkPathBuilder {
        let verbs = path.verbs();
        if verbs.is_empty() {
            return self;
        }
        let points = path.points();
        let weights = path.conic_weights();
        let mut verb_index = verbs.len();
        // Skia's point pointer may step one before the array (never read there): signed.
        let mut pts = points.len() as isize - 1;
        let mut conic_weights = weights.len();
        let at = |index: isize| points[index as usize];
        while verb_index > 0 {
            verb_index -= 1;
            let v = verbs[verb_index];
            pts -= SkPathPriv::pts_in_verb(v) as isize;
            match v {
                SkPathVerb::Move => {
                    // if the path has multiple contours, stop after reversing the last
                    return self;
                }
                SkPathVerb::Line => {
                    self.line_to(at(pts));
                }
                SkPathVerb::Quad => {
                    self.quad_to(at(pts + 1), at(pts));
                }
                SkPathVerb::Conic => {
                    conic_weights -= 1;
                    self.conic_to(at(pts + 1), at(pts), weights[conic_weights]);
                }
                SkPathVerb::Cubic => {
                    self.cubic_to(at(pts + 2), at(pts + 1), at(pts));
                }
                SkPathVerb::Close => {}
            }
        }
        self
    }

    /// `privateReverseAddPath`.
    fn private_reverse_add_path(&mut self, src: &SkPath) -> &mut SkPathBuilder {
        let verbs = src.verbs();
        if verbs.is_empty() {
            return self;
        }
        let points = src.points();
        let weights = src.conic_weights();
        let mut verb_index = verbs.len();
        // Skia's point pointer may step one before the array (never read there): signed.
        let mut pts = points.len() as isize;
        let mut conic_weights = weights.len();
        let at = |index: isize| points[index as usize];
        let mut need_move = true;
        let mut need_close = false;
        while verb_index > 0 {
            verb_index -= 1;
            let v = verbs[verb_index];
            let n = SkPathPriv::pts_in_verb(v) as isize;
            if need_move {
                pts -= 1;
                self.move_to(at(pts));
                need_move = false;
            }
            pts -= n;
            match v {
                SkPathVerb::Move => {
                    if need_close {
                        self.close();
                        need_close = false;
                    }
                    need_move = true;
                    pts += 1; // so we see the point in "if (needMove)" above
                }
                SkPathVerb::Line => {
                    self.line_to(at(pts));
                }
                SkPathVerb::Quad => {
                    self.quad_to(at(pts + 1), at(pts));
                }
                SkPathVerb::Conic => {
                    conic_weights -= 1;
                    self.conic_to(at(pts + 1), at(pts), weights[conic_weights]);
                }
                SkPathVerb::Cubic => {
                    self.cubic_to(at(pts + 2), at(pts + 1), at(pts));
                }
                SkPathVerb::Close => {
                    need_close = true;
                }
            }
        }
        self
    }
}

/// `SkPathData::MakeTransform(raw, SkMatrix::I())` for a non-empty raw path: the arrays
/// copied, the raw bounds sorted and its segment mask kept (`finishInit` with both given).
fn path_from_raw_with_bounds(raw: &SkPathRaw, fill_type: SkPathFillType) -> SkPath {
    let mut path = SkPath::make_no_check(
        raw.verbs.to_vec(),
        raw.points.to_vec(),
        raw.conics.to_vec(),
        fill_type,
    )
    .unwrap_or_else(|| SkPath::error(fill_type));
    if !raw.points.is_empty() {
        path.set_cached_bounds(raw.bounds.make_sorted(), raw.segment_mask);
    }
    path
}

impl SkPathPriv {
    /// `SkPathPriv::Raw(const SkPathBuilder&, SkResolveConvexity::kNo)`: `None` when the
    /// bounds are not finite.
    pub(crate) fn raw_builder(builder: &SkPathBuilder) -> Option<SkPathRaw<'_>> {
        let bounds = builder.compute_finite_bounds()?;
        Some(SkPathRaw {
            points: builder.points(),
            verbs: builder.verbs(),
            conics: builder.conic_weights(),
            bounds,
            segment_mask: builder.segment_mask,
        })
    }

    /// `SkPathPriv::ReversePathTo`.
    pub(crate) fn reverse_path_to(builder: &mut SkPathBuilder, reverse_me: &SkPath) {
        builder.private_reverse_path_to(reverse_me);
    }

    /// `SkPathPriv::ReverseAddPath`.
    pub(crate) fn reverse_add_path(builder: &mut SkPathBuilder, reverse_me: &SkPath) {
        builder.private_reverse_add_path(reverse_me);
    }
}
