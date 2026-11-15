//! Port of Skia's src/pathops/SkPathOpsTypes.{h,cpp}, with `SkPathOp` from
//! include/pathops/SkPathOps.h.
//!
//! The ULPs comparisons, the epsilons that PathOps compares t values and coordinates with, and
//! `SkOpGlobalState`. The global state also owns every object Skia allocates from its arena:
//! each graph class's fields live in a `Vec` here, and the class itself is a handle into it.
//!
//! Skia overloads the comparisons for `float` and `double`; the `double` overloads (which
//! convert to float and call the `float` ones) keep the plain name, the `float` ones end in
//! `_f32`, so Rust's types pick the overload C++ would.

use crate::core::float_bits::{
    sk_float_as_2s_compliment, sk_ieee_double_divide, sk_is_finite_f32, std_max, DBL_EPSILON,
    FLT_EPSILON, SK_SCALAR_MAX,
};
use crate::core::path::SkPathVerb;
use crate::op_angle::{SkOpAngle, SkOpAngleData};
use crate::op_coincidence::{
    SkCoincidentSpans, SkCoincidentSpansData, SkOpCoincidence, SkOpCoincidenceData,
};
use crate::op_contour::{SkOpContour, SkOpContourData, SkOpContourHead};
use crate::op_segment::{SkOpSegment, SkOpSegmentData};
use crate::op_span::{SkOpSpan, SkOpSpanBase, SkOpSpanData};

/// `SkPathOp` is a boolean operation combining two paths, in Skia's order.
///
/// The first path is the one passed first to [`op`](crate::op), the second the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum SkPathOp {
    /// `Difference` keeps what is in the first path and not in the second.
    Difference = 0,
    /// `Intersect` keeps what is in both paths.
    Intersect = 1,
    /// `Union` keeps what is in either path.
    Union = 2,
    /// `Xor` keeps what is in exactly one of the paths.
    Xor = 3,
    /// `ReverseDifference` keeps what is in the second path and not in the first.
    ReverseDifference = 4,
}

/// `PathOpsError` is the failure of a path operation: Skia's `Op` or `Simplify` returned no
/// path.
///
/// Skia gives up when it cannot resolve the curves' intersections or windings consistently,
/// which happens for some near-coincident or degenerate geometry. No partial result is given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PathOpsError;

impl std::fmt::Display for PathOpsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the path operation could not be resolved")
    }
}

impl std::error::Error for PathOpsError {}

/// Declares a handle: a copyable index into one of `SkOpGlobalState`'s (or a TSect heap's)
/// arenas, standing for Skia's pointer to an object of the class it is named after. Its
/// default is an index no arena has, standing for a pointer Skia's arena left unset.
macro_rules! arena_handle {
    ($(#[$attr:meta])* $name:ident) => {
        $(#[$attr])*
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
        pub(crate) struct $name(pub(crate) u32);

        impl Default for $name {
            fn default() -> Self {
                Self(u32::MAX)
            }
        }
    };
}
pub(crate) use arena_handle;

/// `SkPathOpsMask`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub(crate) enum SkPathOpsMask {
    Winding = -1,
    #[default]
    No = 0,
    EvenOdd = 1,
}

/// `SkOpGlobalState`: Skia's global state, and the arena every PathOps object is made in.
///
/// Skia's constructor takes the contour head, a contour the caller made on its stack; here
/// `new` makes it, as the first contour. `SkOpCoincidence`'s constructor still sets
/// `coincidence`, and a second coincidence (`HandleCoincidence`'s `overlaps`) takes it over.
#[derive(Debug)]
pub(crate) struct SkOpGlobalState {
    pub(crate) contours: Vec<SkOpContourData>,
    pub(crate) segments: Vec<SkOpSegmentData>,
    /// Spans and span bases; each holds its embedded `SkOpPtT`.
    pub(crate) spans: Vec<SkOpSpanData>,
    pub(crate) angles: Vec<SkOpAngleData>,
    pub(crate) coincident_spans: Vec<SkCoincidentSpansData>,
    pub(crate) coincidences: Vec<SkOpCoincidenceData>,
    pub(crate) coincidence: Option<SkOpCoincidence>,
    pub(crate) contour_head: SkOpContourHead,
    pub(crate) allocated_op_span: bool,
}

impl SkOpGlobalState {
    pub(crate) const K_MAX_WINDING_TRIES: i32 = 10;

    /// `SkOpGlobalState(head, allocator)`, making the head contour as Skia's caller does.
    pub(crate) fn new() -> SkOpGlobalState {
        let mut state = SkOpGlobalState {
            contours: Vec::new(),
            segments: Vec::new(),
            spans: Vec::new(),
            angles: Vec::new(),
            coincident_spans: Vec::new(),
            coincidences: Vec::new(),
            coincidence: None,
            contour_head: SkOpContourHead(SkOpContour(0)),
            allocated_op_span: false,
        };
        let head = state.make_contour();
        state.contour_head = SkOpContourHead(head);
        state
    }

    pub(crate) fn allocated_op_span(&self) -> bool {
        self.allocated_op_span
    }

    pub(crate) fn coincidence(&self) -> Option<SkOpCoincidence> {
        self.coincidence
    }

    pub(crate) fn contour_head(&self) -> SkOpContourHead {
        self.contour_head
    }

    pub(crate) fn reset_allocated_op_span(&mut self) {
        self.allocated_op_span = false;
    }

    pub(crate) fn set_allocated_op_span(&mut self) {
        self.allocated_op_span = true;
    }

    pub(crate) fn set_coincidence(&mut self, coincidence: Option<SkOpCoincidence>) {
        self.coincidence = coincidence;
    }

    pub(crate) fn set_contour_head(&mut self, contour_head: SkOpContourHead) {
        self.contour_head = contour_head;
    }

    /// `allocator()->make<SkOpContour>()`: the constructor resets it; its embedded head
    /// segment is made with it.
    pub(crate) fn make_contour(&mut self) -> SkOpContour {
        let contour = SkOpContour(self.contours.len() as u32);
        self.contours.push(SkOpContourData::default());
        let head = self.make_segment();
        self.contours[contour.0 as usize].head = head;
        contour.reset(self);
        contour
    }

    /// `allocator()->make<SkOpSegment>()`: its embedded head span and tail span base are made
    /// with it.
    pub(crate) fn make_segment(&mut self) -> SkOpSegment {
        let segment = SkOpSegment(self.segments.len() as u32);
        self.segments.push(SkOpSegmentData::default());
        let head = self.make_span();
        let tail = self.make_span().0;
        let data = &mut self.segments[segment.0 as usize];
        data.head = head;
        data.tail = tail;
        segment
    }

    /// `allocator()->make<SkOpSpan>()`; a span base uses the same storage.
    pub(crate) fn make_span(&mut self) -> SkOpSpan {
        let span = SkOpSpan(SkOpSpanBase(self.spans.len() as u32));
        self.spans.push(SkOpSpanData::default());
        span
    }

    /// `allocator()->make<SkOpAngle>()`.
    pub(crate) fn make_angle(&mut self) -> SkOpAngle {
        let angle = SkOpAngle(self.angles.len() as u32);
        self.angles.push(SkOpAngleData::default());
        angle
    }

    /// `allocator()->make<SkCoincidentSpans>()`.
    pub(crate) fn make_coincident_spans(&mut self) -> SkCoincidentSpans {
        let spans = SkCoincidentSpans(self.coincident_spans.len() as u32);
        self.coincident_spans.push(SkCoincidentSpansData::default());
        spans
    }
}

fn arguments_denormalized(a: f32, b: f32, epsilon: i32) -> bool {
    let denormalized_check = FLT_EPSILON * epsilon as f32 / 2.0;
    a.abs() <= denormalized_check && b.abs() <= denormalized_check
}

// The ULP sums below wrap as the machine's adds do: Skia's `int` adds overflow only for NaNs with
// a payload within `epsilon` of the largest, where C++ leaves the result undefined.

// from http://randomascii.wordpress.com/2012/02/25/comparing-floating-point-numbers-2012-edition/
// FIXME: move to SkFloatBits.h
fn equal_ulps(a: f32, b: f32, epsilon: i32, depsilon: i32) -> bool {
    if arguments_denormalized(a, b, depsilon) {
        return true;
    }
    let a_bits = sk_float_as_2s_compliment(a);
    let b_bits = sk_float_as_2s_compliment(b);
    // Find the difference in ULPs.
    a_bits < b_bits.wrapping_add(epsilon) && b_bits < a_bits.wrapping_add(epsilon)
}

fn equal_ulps_no_normal_check(a: f32, b: f32, epsilon: i32, _depsilon: i32) -> bool {
    let a_bits = sk_float_as_2s_compliment(a);
    let b_bits = sk_float_as_2s_compliment(b);
    // Find the difference in ULPs.
    a_bits < b_bits.wrapping_add(epsilon) && b_bits < a_bits.wrapping_add(epsilon)
}

fn equal_ulps_pin(a: f32, b: f32, epsilon: i32, depsilon: i32) -> bool {
    if !sk_is_finite_f32(&[a, b]) {
        return false;
    }
    if arguments_denormalized(a, b, depsilon) {
        return true;
    }
    let a_bits = sk_float_as_2s_compliment(a);
    let b_bits = sk_float_as_2s_compliment(b);
    // Find the difference in ULPs.
    a_bits < b_bits.wrapping_add(epsilon) && b_bits < a_bits.wrapping_add(epsilon)
}

fn d_equal_ulps(a: f32, b: f32, epsilon: i32) -> bool {
    let a_bits = sk_float_as_2s_compliment(a);
    let b_bits = sk_float_as_2s_compliment(b);
    // Find the difference in ULPs.
    a_bits < b_bits.wrapping_add(epsilon) && b_bits < a_bits.wrapping_add(epsilon)
}

fn not_equal_ulps(a: f32, b: f32, epsilon: i32) -> bool {
    if arguments_denormalized(a, b, epsilon) {
        return false;
    }
    let a_bits = sk_float_as_2s_compliment(a);
    let b_bits = sk_float_as_2s_compliment(b);
    // Find the difference in ULPs.
    a_bits >= b_bits.wrapping_add(epsilon) || b_bits >= a_bits.wrapping_add(epsilon)
}

fn not_equal_ulps_pin(a: f32, b: f32, epsilon: i32) -> bool {
    if !sk_is_finite_f32(&[a, b]) {
        return false;
    }
    if arguments_denormalized(a, b, epsilon) {
        return false;
    }
    let a_bits = sk_float_as_2s_compliment(a);
    let b_bits = sk_float_as_2s_compliment(b);
    // Find the difference in ULPs.
    a_bits >= b_bits.wrapping_add(epsilon) || b_bits >= a_bits.wrapping_add(epsilon)
}

fn d_not_equal_ulps(a: f32, b: f32, epsilon: i32) -> bool {
    let a_bits = sk_float_as_2s_compliment(a);
    let b_bits = sk_float_as_2s_compliment(b);
    // Find the difference in ULPs.
    a_bits >= b_bits.wrapping_add(epsilon) || b_bits >= a_bits.wrapping_add(epsilon)
}

fn less_ulps(a: f32, b: f32, epsilon: i32) -> bool {
    if arguments_denormalized(a, b, epsilon) {
        return a <= b - FLT_EPSILON * epsilon as f32;
    }
    let a_bits = sk_float_as_2s_compliment(a);
    let b_bits = sk_float_as_2s_compliment(b);
    // Find the difference in ULPs.
    a_bits <= b_bits.wrapping_sub(epsilon)
}

fn less_or_equal_ulps(a: f32, b: f32, epsilon: i32) -> bool {
    if arguments_denormalized(a, b, epsilon) {
        return a < b + FLT_EPSILON * epsilon as f32;
    }
    let a_bits = sk_float_as_2s_compliment(a);
    let b_bits = sk_float_as_2s_compliment(b);
    // Find the difference in ULPs.
    a_bits < b_bits.wrapping_add(epsilon)
}

// Use Almost Equal when comparing coordinates. Use epsilon to compare T values.
pub(crate) fn almost_equal_ulps_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 16;
    equal_ulps(a, b, ULPS_EPSILON, ULPS_EPSILON)
}

pub(crate) fn almost_equal_ulps(a: f64, b: f64) -> bool {
    almost_equal_ulps_f32(a as f32, b as f32)
}

pub(crate) fn almost_equal_ulps_no_normal_check_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 16;
    equal_ulps_no_normal_check(a, b, ULPS_EPSILON, ULPS_EPSILON)
}

pub(crate) fn almost_equal_ulps_no_normal_check(a: f64, b: f64) -> bool {
    almost_equal_ulps_no_normal_check_f32(a as f32, b as f32)
}

pub(crate) fn almost_equal_ulps_pin_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 16;
    equal_ulps_pin(a, b, ULPS_EPSILON, ULPS_EPSILON)
}

pub(crate) fn almost_equal_ulps_pin(a: f64, b: f64) -> bool {
    almost_equal_ulps_pin_f32(a as f32, b as f32)
}

// Use Almost Dequal when comparing should not special case denormalized values.
pub(crate) fn almost_dequal_ulps_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 16;
    d_equal_ulps(a, b, ULPS_EPSILON)
}

pub(crate) fn almost_dequal_ulps(a: f64, b: f64) -> bool {
    if a.abs() < SK_SCALAR_MAX as f64 && b.abs() < SK_SCALAR_MAX as f64 {
        return almost_dequal_ulps_f32(a as f32, b as f32);
    }
    // We allow divide-by-zero here. It only happens if one of a,b is zero, and the other is NaN.
    // (Otherwise, we'd hit the condition above). Thus, if std::max returns 0, we compute NaN / 0,
    // which will produce NaN. The comparison will return false, which is the correct answer.
    sk_ieee_double_divide((a - b).abs(), std_max(a.abs(), b.abs())) < (FLT_EPSILON * 16.0) as f64
}

pub(crate) fn not_almost_equal_ulps_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 16;
    not_equal_ulps(a, b, ULPS_EPSILON)
}

pub(crate) fn not_almost_equal_ulps(a: f64, b: f64) -> bool {
    not_almost_equal_ulps_f32(a as f32, b as f32)
}

pub(crate) fn not_almost_equal_ulps_pin_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 16;
    not_equal_ulps_pin(a, b, ULPS_EPSILON)
}

pub(crate) fn not_almost_equal_ulps_pin(a: f64, b: f64) -> bool {
    not_almost_equal_ulps_pin_f32(a as f32, b as f32)
}

pub(crate) fn not_almost_dequal_ulps_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 16;
    d_not_equal_ulps(a, b, ULPS_EPSILON)
}

pub(crate) fn not_almost_dequal_ulps(a: f64, b: f64) -> bool {
    not_almost_dequal_ulps_f32(a as f32, b as f32)
}

// Use Almost Bequal when comparing coordinates in conjunction with between.
// equality using the same error term as between
pub(crate) fn almost_bequal_ulps_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 2;
    equal_ulps(a, b, ULPS_EPSILON, ULPS_EPSILON)
}

pub(crate) fn almost_bequal_ulps(a: f64, b: f64) -> bool {
    almost_bequal_ulps_f32(a as f32, b as f32)
}

pub(crate) fn almost_pequal_ulps_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 8;
    equal_ulps(a, b, ULPS_EPSILON, ULPS_EPSILON)
}

pub(crate) fn almost_pequal_ulps(a: f64, b: f64) -> bool {
    almost_pequal_ulps_f32(a as f32, b as f32)
}

pub(crate) fn roughly_equal_ulps_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 256;
    const D_ULPS_EPSILON: i32 = 1024;
    equal_ulps(a, b, ULPS_EPSILON, D_ULPS_EPSILON)
}

pub(crate) fn roughly_equal_ulps(a: f64, b: f64) -> bool {
    roughly_equal_ulps_f32(a as f32, b as f32)
}

pub(crate) fn almost_less_ulps_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 16;
    less_ulps(a, b, ULPS_EPSILON)
}

pub(crate) fn almost_less_or_equal_ulps_f32(a: f32, b: f32) -> bool {
    const ULPS_EPSILON: i32 = 16;
    less_or_equal_ulps(a, b, ULPS_EPSILON)
}

pub(crate) fn almost_between_ulps_f32(a: f32, b: f32, c: f32) -> bool {
    const ULPS_EPSILON: i32 = 2;
    if a <= c {
        less_or_equal_ulps(a, b, ULPS_EPSILON) && less_or_equal_ulps(b, c, ULPS_EPSILON)
    } else {
        less_or_equal_ulps(b, a, ULPS_EPSILON) && less_or_equal_ulps(c, b, ULPS_EPSILON)
    }
}

pub(crate) fn almost_between_ulps(a: f64, b: f64, c: f64) -> bool {
    almost_between_ulps_f32(a as f32, b as f32, c as f32)
}

// FLT_EPSILON == 1.19209290E-07 == 1 / (2 ^ 23)
// DBL_EPSILON == 2.22045e-16
// Skia computes the FLT_ ones in float (FLT_EPSILON is a float), then widens them.
pub(crate) const FLT_EPSILON_HALF: f64 = (FLT_EPSILON / 2.0) as f64;
pub(crate) const FLT_EPSILON_DOUBLE: f64 = (FLT_EPSILON * 2.0) as f64;
pub(crate) const FLT_EPSILON_ORDERABLE_ERR: f64 = (FLT_EPSILON * 16.0) as f64;
pub(crate) const FLT_EPSILON_INVERSE: f64 = (1.0 / FLT_EPSILON) as f64;
pub(crate) const DBL_EPSILON_ERR: f64 = DBL_EPSILON * 4.0; // FIXME: tune -- allow a few bits of error
pub(crate) const ROUGH_EPSILON: f64 = (FLT_EPSILON * 64.0) as f64;
pub(crate) const MORE_ROUGH_EPSILON: f64 = (FLT_EPSILON * 256.0) as f64;

/// `FLT_EPSILON` widened, as C++ promotes it next to a double.
const FLT_EPSILON_F64: f64 = FLT_EPSILON as f64;

pub(crate) fn zero_or_one(x: f64) -> bool {
    x == 0.0 || x == 1.0
}

pub(crate) fn approximately_zero(x: f64) -> bool {
    x.abs() < FLT_EPSILON_F64
}

pub(crate) fn precisely_zero(x: f64) -> bool {
    x.abs() < DBL_EPSILON_ERR
}

pub(crate) fn approximately_zero_half(x: f64) -> bool {
    x.abs() < FLT_EPSILON_HALF
}

pub(crate) fn approximately_zero_orderable(x: f64) -> bool {
    x.abs() < FLT_EPSILON_ORDERABLE_ERR
}

pub(crate) fn approximately_zero_inverse(x: f64) -> bool {
    x.abs() > FLT_EPSILON_INVERSE
}

pub(crate) fn approximately_zero_when_compared_to(x: f64, y: f64) -> bool {
    x == 0.0 || x.abs() < (y * FLT_EPSILON_F64).abs()
}

pub(crate) fn precisely_zero_when_compared_to(x: f64, y: f64) -> bool {
    x == 0.0 || x.abs() < (y * DBL_EPSILON).abs()
}

pub(crate) fn roughly_zero_when_compared_to(x: f64, y: f64) -> bool {
    x == 0.0 || x.abs() < (y * ROUGH_EPSILON).abs()
}

// Use this for comparing Ts in the range of 0 to 1. For general numbers (larger and smaller) use
// AlmostEqualUlps instead.
pub(crate) fn approximately_equal(x: f64, y: f64) -> bool {
    approximately_zero(x - y)
}

pub(crate) fn precisely_equal(x: f64, y: f64) -> bool {
    precisely_zero(x - y)
}

pub(crate) fn approximately_equal_half(x: f64, y: f64) -> bool {
    approximately_zero_half(x - y)
}

pub(crate) fn approximately_equal_orderable(x: f64, y: f64) -> bool {
    approximately_zero_orderable(x - y)
}

/// `1 - FLT_EPSILON` is computed in float, then widened.
pub(crate) fn approximately_greater_than_one(x: f64) -> bool {
    x > (1.0 - FLT_EPSILON) as f64
}

pub(crate) fn precisely_greater_than_one(x: f64) -> bool {
    x > 1.0 - DBL_EPSILON_ERR
}

pub(crate) fn approximately_less_than_zero(x: f64) -> bool {
    x < FLT_EPSILON_F64
}

pub(crate) fn precisely_less_than_zero(x: f64) -> bool {
    x < DBL_EPSILON_ERR
}

pub(crate) fn approximately_negative(x: f64) -> bool {
    x < FLT_EPSILON_F64
}

pub(crate) fn approximately_negative_orderable(x: f64) -> bool {
    x < FLT_EPSILON_ORDERABLE_ERR
}

pub(crate) fn precisely_negative(x: f64) -> bool {
    x < DBL_EPSILON_ERR
}

/// `1 + FLT_EPSILON` is computed in float, then widened.
pub(crate) fn approximately_one_or_less(x: f64) -> bool {
    x < (1.0 + FLT_EPSILON) as f64
}

pub(crate) fn approximately_one_or_less_double(x: f64) -> bool {
    x < 1.0 + FLT_EPSILON_DOUBLE
}

pub(crate) fn approximately_zero_or_more(x: f64) -> bool {
    x > -FLT_EPSILON_F64
}

pub(crate) fn approximately_zero_or_more_double(x: f64) -> bool {
    x > -FLT_EPSILON_DOUBLE
}

pub(crate) fn approximately_between_orderable(a: f64, b: f64, c: f64) -> bool {
    if a <= c {
        approximately_negative_orderable(a - b) && approximately_negative_orderable(b - c)
    } else {
        approximately_negative_orderable(b - a) && approximately_negative_orderable(c - b)
    }
}

pub(crate) fn approximately_between(a: f64, b: f64, c: f64) -> bool {
    if a <= c {
        approximately_negative(a - b) && approximately_negative(b - c)
    } else {
        approximately_negative(b - a) && approximately_negative(c - b)
    }
}

pub(crate) fn precisely_between(a: f64, b: f64, c: f64) -> bool {
    if a <= c {
        precisely_negative(a - b) && precisely_negative(b - c)
    } else {
        precisely_negative(b - a) && precisely_negative(c - b)
    }
}

// returns true if (a <= b <= c) || (a >= b >= c)
pub(crate) fn between(a: f64, b: f64, c: f64) -> bool {
    debug_assert!(
        ((a <= b && b <= c) || (a >= b && b >= c)) == ((a - b) * (c - b) <= 0.0)
            || (precisely_zero(a) && precisely_zero(b) && precisely_zero(c))
    );
    (a - b) * (c - b) <= 0.0
}

pub(crate) fn roughly_equal(x: f64, y: f64) -> bool {
    (x - y).abs() < ROUGH_EPSILON
}

pub(crate) fn roughly_negative(x: f64) -> bool {
    x < ROUGH_EPSILON
}

pub(crate) fn roughly_between(a: f64, b: f64, c: f64) -> bool {
    if a <= c {
        roughly_negative(a - b) && roughly_negative(b - c)
    } else {
        roughly_negative(b - a) && roughly_negative(c - b)
    }
}

pub(crate) fn more_roughly_equal(x: f64, y: f64) -> bool {
    (x - y).abs() < MORE_ROUGH_EPSILON
}

/// `SkPathOpsPointsToVerb`: 0 → move, 1 → line, 2 → quad, 3 → cubic.
pub(crate) fn sk_path_ops_points_to_verb(points: i32) -> SkPathVerb {
    let verb = (1 << points) >> 1;
    SkPathVerb::from_i32(verb)
}

/// `SkPathOpsVerbToPoints`: line → 1, quad and conic → 2, cubic → 3.
pub(crate) fn sk_path_ops_verb_to_points(verb: SkPathVerb) -> i32 {
    let verb = verb as i32;
    verb - ((verb + 1) >> 2)
}

pub(crate) fn sk_d_interp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/* Returns -1 if negative, 0 if zero, 1 if positive
 */

/* Returns 0 if negative, 1 if zero, 2 if positive
 */

/* Returns 1 if negative, 2 if zero, 4 if positive
 */

pub(crate) fn sk_pin_t(t: f64) -> f64 {
    if precisely_less_than_zero(t) {
        0.0
    } else if precisely_greater_than_one(t) {
        1.0
    } else {
        t
    }
}
