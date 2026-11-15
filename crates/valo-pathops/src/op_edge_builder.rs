//! Port of Skia's src/pathops/SkOpEdgeBuilder.{h,cpp}: turning the input paths into contours
//! of segments, after dropping degenerate curves, closing open contours and splitting curves
//! the intersection cannot handle whole.
//!
//! The verbs collected here are Skia's legacy `SkPath::Verb` values in bytes, ending with
//! `K_DONE_VERB` after each path.

use crate::core::float_bits::{sk_is_finite_f32, sk_scalar_abs, std_min};
use crate::core::geometry::{sk_chop_quad_at_max_curvature, sk_find_quad_max_curvature, SkConic};
use crate::core::path::{SkPath, SkPathPriv, SkPathVerb};
use crate::core::point::SkPoint;
use crate::core::t_sort::sk_t_q_sort;
use crate::op_contour::{SkOpContourBuilder, SkOpContourHead};
use crate::path_ops_cubic::SkDCubic;
use crate::path_ops_point::SkDPoint;
use crate::path_ops_types::{
    sk_path_ops_verb_to_points, SkOpGlobalState, SkPathOpsMask, FLT_EPSILON_ORDERABLE_ERR,
};
use crate::reduce_order::SkReduceOrder;

/// `SkPath::kDone_Verb`, which ends each path's verbs in `path_verbs`.
pub(crate) const K_DONE_VERB: u8 = 6;

pub(crate) struct SkOpEdgeBuilder<'a> {
    pub(crate) path: &'a SkPath,
    pub(crate) path_pts: Vec<SkPoint>,
    pub(crate) weights: Vec<f32>,
    pub(crate) path_verbs: Vec<u8>,
    pub(crate) contour_builder: SkOpContourBuilder,
    pub(crate) contours_head: SkOpContourHead,
    pub(crate) xor_mask: [SkPathOpsMask; 2],
    pub(crate) second_half: i32,
    pub(crate) operand: bool,
    pub(crate) allow_open_contours: bool,
    pub(crate) unparseable: bool,
}

impl<'a> SkOpEdgeBuilder<'a> {
    /// `SkOpEdgeBuilder(const SkPath& path, SkOpContourHead* contours2,
    /// SkOpGlobalState* globalState)`: the state is the methods' parameter.
    pub(crate) fn new(path: &'a SkPath, contours2: SkOpContourHead) -> SkOpEdgeBuilder<'a> {
        let mut builder = SkOpEdgeBuilder {
            path,
            path_pts: Vec::new(),
            weights: Vec::new(),
            path_verbs: Vec::new(),
            contour_builder: SkOpContourBuilder::new(Some(contours2.0)),
            contours_head: contours2,
            xor_mask: [SkPathOpsMask::No; 2],
            second_half: 0,
            operand: false,
            allow_open_contours: false,
            unparseable: false,
        };
        builder.init();
        builder
    }

    pub(crate) fn add_operand(&mut self, path: &'a SkPath) {
        debug_assert!(!self.path_verbs.is_empty() && self.path_verbs.last() == Some(&K_DONE_VERB));
        self.path_verbs.pop();
        self.path = path;
        self.xor_mask[1] = if (self.path.fill_type() as i32 & 1) != 0 {
            SkPathOpsMask::EvenOdd
        } else {
            SkPathOpsMask::Winding
        };
        self.pre_fetch();
    }

    pub(crate) fn complete(&mut self, state: &mut SkOpGlobalState) {
        self.contour_builder.flush(state);
        let contour = self.contour_builder.contour();
        if let Some(contour) = contour {
            if contour.count(state) != 0 {
                contour.complete(state);
                self.contour_builder.set_contour(state, None);
            }
        }
    }

    pub(crate) fn finish(&mut self, state: &mut SkOpGlobalState) -> bool {
        self.operand = false;
        if self.unparseable || !self.walk(state) {
            return false;
        }
        self.complete(state);
        let contour = self.contour_builder.contour();
        if let Some(contour) = contour {
            if contour.count(state) == 0 {
                self.contours_head.remove(state, contour);
            }
        }
        true
    }

    pub(crate) fn init(&mut self) {
        self.operand = false;
        let mask = if (self.path.fill_type() as i32 & 1) != 0 {
            SkPathOpsMask::EvenOdd
        } else {
            SkPathOpsMask::Winding
        };
        self.xor_mask = [mask; 2];
        self.unparseable = false;
        self.second_half = self.pre_fetch();
    }

    pub(crate) fn unparseable(&self) -> bool {
        self.unparseable
    }

    pub(crate) fn xor_mask(&self) -> SkPathOpsMask {
        self.xor_mask[usize::from(self.operand)]
    }

    pub(crate) fn close_contour(&mut self, curve_end: &SkPoint, curve_start: &SkPoint) {
        if !SkDPoint::approximately_equal_sk_points(curve_end, curve_start) {
            self.path_verbs.push(SkPathVerb::Line as u8);
            self.path_pts.push(*curve_start);
        } else {
            let verb_count = self.path_verbs.len();
            let pts_count = self.path_pts.len();
            if SkPathVerb::Line as u8 == self.path_verbs[verb_count - 1]
                && self.path_pts[pts_count - 2] == *curve_start
            {
                self.path_verbs.pop();
                self.path_pts.pop();
            } else {
                self.path_pts[pts_count - 1] = *curve_start;
            }
        }
        self.path_verbs.push(SkPathVerb::Close as u8);
    }

    pub(crate) fn pre_fetch(&mut self) -> i32 {
        if !self.path.is_finite() {
            self.unparseable = true;
            return 0;
        }
        let mut curve_start = SkPoint::default();
        let mut curve = [SkPoint::default(); 4];
        let mut last_curve = false;
        for (path_verb, pts, w) in SkPathPriv::iterate(self.path) {
            let mut verb = path_verb;
            match verb {
                SkPathVerb::Move => {
                    if !self.allow_open_contours && last_curve {
                        self.close_contour(&curve[0], &curve_start);
                    }
                    self.path_verbs.push(verb as u8);
                    curve[0] = force_small_to_zero(&pts[0]);
                    self.path_pts.push(curve[0]);
                    curve_start = curve[0];
                    last_curve = false;
                    continue;
                }
                SkPathVerb::Line => {
                    curve[1] = force_small_to_zero(&pts[1]);
                    if SkDPoint::approximately_equal_sk_points(&curve[0], &curve[1]) {
                        let last_verb = *self.path_verbs.last().expect("a line follows a move");
                        if last_verb != SkPathVerb::Line as u8
                            && last_verb != SkPathVerb::Move as u8
                        {
                            curve[0] = curve[1];
                            *self.path_pts.last_mut().expect("a line follows a point") = curve[0];
                        }
                        continue; // skip degenerate points
                    }
                }
                SkPathVerb::Quad => {
                    curve[1] = force_small_to_zero(&pts[1]);
                    curve[2] = force_small_to_zero(&pts[2]);
                    verb = reduce_quad_in_place(&mut curve);
                    if verb == SkPathVerb::Move {
                        continue; // skip degenerate points
                    }
                }
                SkPathVerb::Conic => {
                    curve[1] = force_small_to_zero(&pts[1]);
                    curve[2] = force_small_to_zero(&pts[2]);
                    verb = reduce_quad_in_place(&mut curve);
                    if SkPathVerb::Quad == verb && 1.0 != w[0] {
                        verb = SkPathVerb::Conic;
                    } else if verb == SkPathVerb::Move {
                        continue; // skip degenerate points
                    }
                }
                SkPathVerb::Cubic => {
                    curve[1] = force_small_to_zero(&pts[1]);
                    curve[2] = force_small_to_zero(&pts[2]);
                    curve[3] = force_small_to_zero(&pts[3]);
                    let input = curve;
                    verb = SkReduceOrder::cubic(&input, &mut curve);
                    if verb == SkPathVerb::Move {
                        continue; // skip degenerate points
                    }
                }
                SkPathVerb::Close => {
                    self.close_contour(&curve[0], &curve_start);
                    last_curve = false;
                    continue;
                }
            }
            self.path_verbs.push(verb as u8);
            let pt_count = sk_path_ops_verb_to_points(verb) as usize;
            self.path_pts.extend_from_slice(&curve[1..1 + pt_count]);
            if verb == SkPathVerb::Conic {
                self.weights.push(w[0]);
            }
            curve[0] = curve[pt_count];
            last_curve = true;
        }
        if !self.allow_open_contours && last_curve {
            self.close_contour(&curve[0], &curve_start);
        }
        self.path_verbs.push(K_DONE_VERB);
        self.path_verbs.len() as i32 - 1
    }

    pub(crate) fn close(&mut self, state: &mut SkOpGlobalState) -> bool {
        self.complete(state);
        true
    }

    /// Skia's pointers into the verbs, points and weights are indices here.
    pub(crate) fn walk(&mut self, state: &mut SkOpGlobalState) -> bool {
        let mut verb_ptr = 0usize;
        let end_of_first_half = self.second_half as usize;
        let mut points_ptr = 0usize;
        let mut weight_ptr = 0usize;
        let mut contour = self.contour_builder.contour();
        let mut move_to_ptr_bump = 0usize;
        while self.path_verbs[verb_ptr] != K_DONE_VERB {
            let verb = SkPathVerb::from_i32(i32::from(self.path_verbs[verb_ptr]));
            if verb_ptr == end_of_first_half {
                self.operand = true;
            }
            verb_ptr += 1;
            match verb {
                SkPathVerb::Move => {
                    if let Some(current) = contour {
                        if current.count(state) != 0 {
                            if self.allow_open_contours {
                                self.complete(state);
                            } else if !self.close(state) {
                                return false;
                            }
                        }
                    }
                    let current = match contour {
                        Some(current) => current,
                        None => {
                            let appended = self.contours_head.append_contour(state);
                            contour = Some(appended);
                            self.contour_builder.set_contour(state, contour);
                            appended
                        }
                    };
                    current.init(
                        state,
                        self.operand,
                        self.xor_mask[usize::from(self.operand)] == SkPathOpsMask::EvenOdd,
                    );
                    points_ptr += move_to_ptr_bump;
                    move_to_ptr_bump = 1;
                    continue;
                }
                SkPathVerb::Line => {
                    self.contour_builder
                        .add_line(state, &self.path_pts[points_ptr..]);
                }
                // Skia's `goto addOneQuad` leaves the labeled block.
                SkPathVerb::Quad => 'add_one_quad: {
                    let points = &self.path_pts[points_ptr..];
                    let vec1 = points[1] - points[0];
                    let vec2 = points[2] - points[1];
                    if vec1.dot(&vec2) < 0.0 {
                        let mut pair = [SkPoint::default(); 5];
                        if sk_chop_quad_at_max_curvature(points, &mut pair) == 1 {
                            self.contour_builder
                                .add_quad(state, &self.path_pts[points_ptr..points_ptr + 3]);
                            break 'add_one_quad;
                        }
                        let pair_scalars: [f32; 10] = std::array::from_fn(|i| {
                            if i % 2 == 0 {
                                pair[i / 2].x
                            } else {
                                pair[i / 2].y
                            }
                        });
                        if !sk_is_finite_f32(&pair_scalars) {
                            return false;
                        }
                        for pt in pair.iter_mut() {
                            *pt = force_small_to_zero(pt);
                        }
                        let mut c_storage = [[SkPoint::default(); 2]; 2];
                        let v1 = SkReduceOrder::quad(&pair[0..3], &mut c_storage[0]);
                        let v2 = SkReduceOrder::quad(&pair[2..5], &mut c_storage[1]);
                        let can_add = can_add_curve(
                            v1,
                            if v1 != SkPathVerb::Line {
                                &mut pair[0..]
                            } else {
                                &mut c_storage[0]
                            },
                        ) && can_add_curve(
                            v2,
                            if v2 != SkPathVerb::Line {
                                &mut pair[2..]
                            } else {
                                &mut c_storage[1]
                            },
                        );
                        if can_add {
                            let curve1 = if v1 != SkPathVerb::Line {
                                &pair[0..]
                            } else {
                                &c_storage[0]
                            };
                            let curve2 = if v2 != SkPathVerb::Line {
                                &pair[2..]
                            } else {
                                &c_storage[1]
                            };
                            self.contour_builder.add_curve(state, v1, curve1, 1.0);
                            self.contour_builder.add_curve(state, v2, curve2, 1.0);
                            break 'add_one_quad;
                        }
                    }
                    // addOneQuad:
                    self.contour_builder
                        .add_quad(state, &self.path_pts[points_ptr..points_ptr + 3]);
                }
                SkPathVerb::Conic => 'conic: {
                    let points = &self.path_pts[points_ptr..];
                    let vec1 = points[1] - points[0];
                    let vec2 = points[2] - points[1];
                    let weight = self.weights[weight_ptr];
                    weight_ptr += 1;
                    if vec1.dot(&vec2) < 0.0 {
                        // FIXME: max curvature for conics hasn't been implemented; use placeholder
                        let max_curvature = sk_find_quad_max_curvature(points);
                        if 0.0 < max_curvature && max_curvature < 1.0 {
                            let conic = SkConic::new(points, weight);
                            let mut pair = [SkConic::default(); 2];
                            if !conic.chop_at(max_curvature, &mut pair) {
                                // if result can't be computed, use original
                                self.contour_builder.add_conic(
                                    state,
                                    &self.path_pts[points_ptr..points_ptr + 3],
                                    weight,
                                );
                                break 'conic;
                            }
                            let mut c_storage = [[SkPoint::default(); 3]; 2];
                            let v1 = SkReduceOrder::conic(&pair[0], &mut c_storage[0]);
                            let v2 = SkReduceOrder::conic(&pair[1], &mut c_storage[1]);
                            let [pair0, pair1] = &mut pair;
                            let can_add = can_add_curve(
                                v1,
                                if v1 != SkPathVerb::Line {
                                    &mut pair0.pts
                                } else {
                                    &mut c_storage[0]
                                },
                            ) && can_add_curve(
                                v2,
                                if v2 != SkPathVerb::Line {
                                    &mut pair1.pts
                                } else {
                                    &mut c_storage[1]
                                },
                            );
                            if can_add {
                                let curve1 = if v1 != SkPathVerb::Line {
                                    &pair[0].pts
                                } else {
                                    &c_storage[0]
                                };
                                let curve2 = if v2 != SkPathVerb::Line {
                                    &pair[1].pts
                                } else {
                                    &c_storage[1]
                                };
                                self.contour_builder.add_curve(state, v1, curve1, pair[0].w);
                                self.contour_builder.add_curve(state, v2, curve2, pair[1].w);
                                break 'conic;
                            }
                        }
                    }
                    self.contour_builder.add_conic(
                        state,
                        &self.path_pts[points_ptr..points_ptr + 3],
                        weight,
                    );
                }
                SkPathVerb::Cubic => {
                    // Split complex cubics (such as self-intersecting curves or
                    // ones with difficult curvature) in two before proceeding.
                    // This can be required for intersection to succeed.
                    let points = &self.path_pts[points_ptr..points_ptr + 4];
                    let mut split_t = [0.0f32; 3];
                    let breaks = SkDCubic::complex_break(points, &mut split_t);
                    if breaks == 0 {
                        self.contour_builder.add_cubic(state, points);
                    } else {
                        debug_assert!(breaks <= split_t.len() as i32);
                        let mut splits = [Splitsville::default(); 4];
                        debug_assert!(splits.len() == split_t.len() + 1);
                        let breaks_count = breaks as usize;
                        sk_t_q_sort(&mut split_t[..breaks_count]);
                        for index in 0..=breaks_count {
                            let split = &mut splits[index];
                            split.t[0] = if index != 0 {
                                f64::from(split_t[index - 1])
                            } else {
                                0.0
                            };
                            split.t[1] = if index < breaks_count {
                                f64::from(split_t[index])
                            } else {
                                1.0
                            };
                            let part = SkDCubic::sub_divide_pts(points, split.t[0], split.t[1]);
                            if !part.to_float_points(&mut split.pts) {
                                return false;
                            }
                            split.verb = SkReduceOrder::cubic(&split.pts, &mut split.reduced);
                            let curve = if SkPathVerb::Cubic == split.verb {
                                &mut split.pts
                            } else {
                                &mut split.reduced
                            };
                            split.can_add = can_add_curve(split.verb, curve);
                        }
                        for index in 0..=breaks_count {
                            if !splits[index].can_add {
                                continue;
                            }
                            let mut prior = index;
                            while prior > 0 && !splits[prior - 1].can_add {
                                prior -= 1;
                            }
                            if prior < index {
                                splits[index].t[0] = splits[prior].t[0];
                                splits[index].pts[0] = splits[prior].pts[0];
                            }
                            let mut next = index;
                            let break_limit = std_min(breaks, splits.len() as i32 - 1) as usize;
                            while next < break_limit && !splits[next + 1].can_add {
                                next += 1;
                            }
                            if next > index {
                                splits[index].t[1] = splits[next].t[1];
                                splits[index].pts[3] = splits[next].pts[3];
                            }
                            let split = &mut splits[index];
                            if prior < index || next > index {
                                split.verb = SkReduceOrder::cubic(&split.pts, &mut split.reduced);
                            }
                            let curve = if SkPathVerb::Cubic == split.verb {
                                &mut split.pts
                            } else {
                                &mut split.reduced
                            };
                            if !can_add_curve(split.verb, curve) {
                                return false;
                            }
                            self.contour_builder
                                .add_curve(state, split.verb, &*curve, 1.0);
                        }
                    }
                }
                SkPathVerb::Close => {
                    debug_assert!(contour.is_some());
                    if !self.close(state) {
                        return false;
                    }
                    contour = None;
                    continue;
                }
            }
            debug_assert!(contour.is_some());
            points_ptr += sk_path_ops_verb_to_points(verb) as usize;
        }
        self.contour_builder.flush(state);
        if let Some(current) = contour {
            if current.count(state) != 0 && !self.allow_open_contours && !self.close(state) {
                return false;
            }
        }
        true
    }
}

/// `walk()`'s local `struct Splitsville`: one piece of a cubic split at its complex breaks.
#[derive(Clone, Copy, Default)]
struct Splitsville {
    t: [f64; 2],
    pts: [SkPoint; 4],
    reduced: [SkPoint; 4],
    verb: SkPathVerb,
    can_add: bool,
}

// very tiny points cause numerical instability : don't allow them
fn force_small_to_zero(pt: &SkPoint) -> SkPoint {
    let mut ret = *pt;
    if f64::from(sk_scalar_abs(ret.x)) < FLT_EPSILON_ORDERABLE_ERR {
        ret.x = 0.0;
    }
    if f64::from(sk_scalar_abs(ret.y)) < FLT_EPSILON_ORDERABLE_ERR {
        ret.y = 0.0;
    }
    ret
}

#[allow(clippy::needless_range_loop)]
fn can_add_curve(verb: SkPathVerb, curve: &mut [SkPoint]) -> bool {
    if SkPathVerb::Move == verb {
        return false;
    }
    for index in 0..=sk_path_ops_verb_to_points(verb) as usize {
        curve[index] = force_small_to_zero(&curve[index]);
    }
    SkPathVerb::Line != verb || !SkDPoint::approximately_equal_sk_points(&curve[0], &curve[1])
}

/// `SkReduceOrder::Quad(curve, curve)`: Skia reads the whole quad before writing the reduced
/// points over it, so reading a copy is the same.
fn reduce_quad_in_place(curve: &mut [SkPoint; 4]) -> SkPathVerb {
    let input = *curve;
    SkReduceOrder::quad(&input, curve)
}
