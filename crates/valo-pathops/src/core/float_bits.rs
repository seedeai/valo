//! Port of Skia's src/base/SkFloatBits.h, the parts of include/private/base/SkFloatingPoint.h,
//! SkMath.h, SkTPin.h, SkTemplates.h and include/core/SkScalar.h that PathOps calls.
//!
//! `std::min`, `std::max`, `SkTPin` and `SkTAbs` are ported with C++'s exact comparisons, which
//! differ from `f64::min`/`max`/`clamp`/`abs` around NaN and signed zero: use these, never std's.

use std::ops::Neg;

/// `SK_MaxS32`.
pub(crate) const SK_MAX_S32: i32 = i32::MAX;
/// `SK_MinS32`: Skia's is `-SK_MaxS32`, one more than `i32::MIN`.
pub(crate) const SK_MIN_S32: i32 = -SK_MAX_S32;
/// `SK_NaN32`: `INT32_MIN`.
pub(crate) const SK_NAN_32: i32 = i32::MIN;

/// `SK_ScalarMax`: Skia's `3.402823466e+38f`, which rounds to `f32::MAX`.
pub(crate) const SK_SCALAR_MAX: f32 = f32::MAX;
/// `SK_ScalarInfinity`.
pub(crate) const SK_SCALAR_INFINITY: f32 = f32::INFINITY;
/// `SK_ScalarNaN` (`SK_FloatNaN`, a quiet NaN).
pub(crate) const SK_SCALAR_NAN: f32 = f32::NAN;
/// `SK_Scalar1`.
pub(crate) const SK_SCALAR_1: f32 = 1.0;

/// `FLT_EPSILON`.
pub(crate) const FLT_EPSILON: f32 = f32::EPSILON;
/// `DBL_EPSILON`.
pub(crate) const DBL_EPSILON: f64 = f64::EPSILON;
/// `FLT_MAX`.
pub(crate) const FLT_MAX: f32 = f32::MAX;
/// `DBL_MAX`.
pub(crate) const DBL_MAX: f64 = f64::MAX;

/// `std::min(a, b)`: `(b < a) ? b : a`.
#[inline]
pub(crate) fn std_min<T: PartialOrd>(a: T, b: T) -> T {
    if b < a {
        b
    } else {
        a
    }
}

/// `std::max(a, b)`: `(a < b) ? b : a`.
#[inline]
pub(crate) fn std_max<T: PartialOrd>(a: T, b: T) -> T {
    if a < b {
        b
    } else {
        a
    }
}

/// `SkTPin(x, lo, hi)`: `std::max(lo, std::min(x, hi))`, so NaN pins to `lo`.
#[inline]
pub(crate) fn sk_t_pin<T: PartialOrd>(x: T, lo: T, hi: T) -> T {
    std_max(lo, std_min(x, hi))
}

/// `SkTAbs(value)`: negates when `value < 0`, so `-0.0` and NaN come back unchanged.
#[inline]
pub(crate) fn sk_t_abs<T: Copy + PartialOrd + Neg<Output = T> + From<i8>>(value: T) -> T {
    if value < T::from(0) {
        -value
    } else {
        value
    }
}

/// `SkScalarAbs(x)`: `std::fabs`.
#[inline]
pub(crate) fn sk_scalar_abs(x: f32) -> f32 {
    x.abs()
}

/// `SkIsNaN(x)`.
#[inline]
#[allow(clippy::eq_op)]
pub(crate) fn sk_is_nan_f32(x: f32) -> bool {
    x != x
}

/// `SkIsNaN(x)` for doubles.
#[inline]
#[allow(clippy::eq_op)]
pub(crate) fn sk_is_nan(x: f64) -> bool {
    x != x
}

/// `SkIsFinite(x, values...)` for floats: `(x - x) * values...` is NaN unless all are finite.
#[inline]
#[allow(clippy::eq_op)]
pub(crate) fn sk_is_finite_f32(values: &[f32]) -> bool {
    let x = values[0];
    let mut prod = x - x;
    for &value in &values[1..] {
        prod *= value;
    }
    prod == prod
}

/// `sk_ieee_double_divide(numer, denom)`: IEEE division, dividing by zero allowed.
#[inline]
pub(crate) fn sk_ieee_double_divide(numer: f64, denom: f64) -> f64 {
    numer / denom
}

/// `SkFloat2Bits(x)`.
#[inline]
pub(crate) fn sk_float_2_bits(x: f32) -> i32 {
    x.to_bits() as i32
}

/// `SkSignBitTo2sCompliment(x)`.
#[inline]
pub(crate) fn sk_sign_bit_to_2s_compliment(mut x: i32) -> i32 {
    if x < 0 {
        x &= 0x7FFF_FFFF;
        x = -x;
    }
    x
}

/// `SkFloatAs2sCompliment(x)`: the float's bits as an int that orders like the float.
#[inline]
pub(crate) fn sk_float_as_2s_compliment(x: f32) -> i32 {
    sk_sign_bit_to_2s_compliment(sk_float_2_bits(x))
}

/// `SkCLZ(x)`.
#[inline]
pub(crate) fn sk_clz(x: u32) -> i32 {
    x.leading_zeros() as i32
}

/// `SkNextLog2(value)`: `32 - SkCLZ(value - 1)`; `value` must not be zero.
#[inline]
pub(crate) fn sk_next_log2(value: u32) -> i32 {
    debug_assert!(value != 0);
    32 - sk_clz(value - 1)
}
