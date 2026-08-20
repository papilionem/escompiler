//! Number built-in methods and properties.
//!
//! Implements the JavaScript `Number` global object with its static
//! methods and constants. Uses [`crate::math::to_f64`] for value coercion.

use nanbox::JsValue;

use crate::math::to_f64;

// === Constants ===

/// `Number.MAX_SAFE_INTEGER` (2^53 - 1).
pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
/// `Number.MIN_SAFE_INTEGER` (-(2^53 - 1)).
pub const MIN_SAFE_INTEGER: f64 = -9_007_199_254_740_991.0;
/// `Number.EPSILON` — the smallest representable difference between 1.0 and the next f64.
pub const EPSILON: f64 = f64::EPSILON;
/// `Number.MAX_VALUE`.
pub const MAX_VALUE: f64 = f64::MAX;
/// `Number.MIN_VALUE` — the smallest positive subnormal f64 (5e-324).
///
/// This is NOT `f64::MIN_POSITIVE` (smallest *normal* positive), but rather
/// the absolute smallest representable positive value, including subnormals.
// SAFETY: f64::from_bits(1) is the subnormal 5e-324, which is Number.MIN_VALUE in JS.
pub const MIN_VALUE: f64 = 5e-324;
/// `Number.POSITIVE_INFINITY`.
pub const POSITIVE_INFINITY: f64 = f64::INFINITY;
/// `Number.NEGATIVE_INFINITY`.
pub const NEGATIVE_INFINITY: f64 = f64::NEG_INFINITY;
/// `Number.NaN`.
pub const NAN: f64 = f64::NAN;

// === Methods ===

/// `Number.isInteger(value)` — returns true if the value is a finite integer.
pub fn is_integer(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    if let Some(n) = val.as_number() {
        JsValue::bool(n.is_finite() && n == n.trunc())
    } else if val.is_int() {
        JsValue::bool(true)
    } else {
        JsValue::bool(false)
    }
}

/// `Number.isFinite(value)` — returns true if the value is a finite number.
///
/// Unlike the global `isFinite()`, does NOT coerce the argument.
pub fn is_finite(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    if let Some(n) = val.as_number() {
        JsValue::bool(n.is_finite())
    } else if val.is_int() {
        JsValue::bool(true)
    } else {
        JsValue::bool(false)
    }
}

/// `Number.isNaN(value)` — returns true only if the value is exactly NaN.
///
/// Unlike the global `isNaN()`, does NOT coerce the argument.
pub fn is_nan(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    if let Some(n) = val.as_number() {
        JsValue::bool(n.is_nan())
    } else {
        JsValue::bool(false)
    }
}

/// `Number.isSafeInteger(value)` — true if the value is an integer in the safe range.
pub fn is_safe_integer(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    let n = to_f64(&val);
    JsValue::bool(n.is_finite() && n == n.trunc() && n.abs() <= MAX_SAFE_INTEGER)
}

/// `Number.parseInt(string, radix)` — simplified numeric-only version.
///
/// Full string parsing will be added in Phase D.
pub fn parse_int(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    if let Some(n) = val.as_int() {
        return JsValue::int(n);
    }
    if let Some(n) = val.as_number()
        && n.is_finite()
    {
        return JsValue::number(n.trunc());
    }
    JsValue::number(f64::NAN)
}

/// `Number.parseFloat(string)` — simplified numeric-only version.
///
/// Full string parsing will be added in Phase D.
pub fn parse_float(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    if let Some(n) = val.as_number() {
        return JsValue::number(n);
    }
    if let Some(n) = val.as_int() {
        return JsValue::number(n as f64);
    }
    JsValue::number(f64::NAN)
}
