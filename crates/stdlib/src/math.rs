//! Math built-in object methods.
//!
//! Implements the JavaScript `Math` global object with its static
//! methods and constants. All methods work on NaN-boxed JsValues.

use nanbox::JsValue;

// === Constants ===

/// Math.PI
pub const PI: f64 = std::f64::consts::PI;
/// Math.E
pub const E: f64 = std::f64::consts::E;
/// Math.LN2
pub const LN2: f64 = std::f64::consts::LN_2;
/// Math.LN10
pub const LN10: f64 = std::f64::consts::LN_10;
/// Math.LOG2E
pub const LOG2E: f64 = std::f64::consts::LOG2_E;
/// Math.LOG10E
pub const LOG10E: f64 = std::f64::consts::LOG10_E;
/// Math.SQRT2
pub const SQRT2: f64 = std::f64::consts::SQRT_2;

// === Helper to extract f64 from JsValue ===

/// Extract a numeric value from a [`JsValue`], converting ints and bools to f64.
///
/// Follows the JS `ToNumber` abstract operation (simplified):
/// - number -> as-is
/// - int -> widened to f64
/// - bool -> 1.0 / 0.0
/// - null -> 0.0
/// - everything else -> NaN
pub fn to_f64(val: &JsValue) -> f64 {
    if let Some(n) = val.as_number() {
        n
    } else if let Some(n) = val.as_int() {
        n as f64
    } else if let Some(b) = val.as_bool() {
        if b { 1.0 } else { 0.0 }
    } else if val.is_null() {
        0.0
    } else {
        f64::NAN
    }
}

// === Methods ===

/// Math.abs(x)
pub fn abs(args: &[JsValue]) -> JsValue {
    let x = args.first().map_or(f64::NAN, to_f64);
    JsValue::number(x.abs())
}

/// Math.floor(x)
pub fn floor(args: &[JsValue]) -> JsValue {
    let x = args.first().map_or(f64::NAN, to_f64);
    JsValue::number(x.floor())
}

/// Math.ceil(x)
pub fn ceil(args: &[JsValue]) -> JsValue {
    let x = args.first().map_or(f64::NAN, to_f64);
    JsValue::number(x.ceil())
}

/// Math.round(x)
///
/// Uses JavaScript rounding semantics where 0.5 rounds toward +Infinity.
pub fn round(args: &[JsValue]) -> JsValue {
    let x = args.first().map_or(f64::NAN, to_f64);
    JsValue::number(js_round(x))
}

/// Math.max(...args)
///
/// Returns `-Infinity` when called with no arguments.
/// Returns `NaN` if any argument is NaN.
pub fn max(args: &[JsValue]) -> JsValue {
    if args.is_empty() {
        return JsValue::number(f64::NEG_INFINITY);
    }
    let mut result = f64::NEG_INFINITY;
    for arg in args {
        let n = to_f64(arg);
        if n.is_nan() {
            return JsValue::number(f64::NAN);
        }
        if n > result {
            result = n;
        }
    }
    JsValue::number(result)
}

/// Math.min(...args)
///
/// Returns `+Infinity` when called with no arguments.
/// Returns `NaN` if any argument is NaN.
pub fn min(args: &[JsValue]) -> JsValue {
    if args.is_empty() {
        return JsValue::number(f64::INFINITY);
    }
    let mut result = f64::INFINITY;
    for arg in args {
        let n = to_f64(arg);
        if n.is_nan() {
            return JsValue::number(f64::NAN);
        }
        if n < result {
            result = n;
        }
    }
    JsValue::number(result)
}

/// Math.sqrt(x)
pub fn sqrt(args: &[JsValue]) -> JsValue {
    let x = args.first().map_or(f64::NAN, to_f64);
    JsValue::number(x.sqrt())
}

/// Math.pow(base, exponent)
pub fn pow(args: &[JsValue]) -> JsValue {
    let base = args.first().map_or(f64::NAN, to_f64);
    let exp = args.get(1).map_or(f64::NAN, to_f64);
    JsValue::number(base.powf(exp))
}

/// Math.random() -- returns a random number in [0, 1).
///
/// Uses the host ABI `__esc_host_random_bytes` for entropy, producing
/// a uniform distribution across the 53-bit mantissa of an f64.
pub fn random(_args: &[JsValue]) -> JsValue {
    let mut buf = [0u8; 8];
    // SAFETY: buf is a valid 8-byte writable buffer on the stack.
    unsafe {
        host::abi::__esc_host_random_bytes(buf.as_mut_ptr(), 8);
    }
    let raw = u64::from_le_bytes(buf);
    // Mask to 53 bits (mantissa of f64), divide to get [0, 1)
    let masked = raw & ((1u64 << 53) - 1);
    JsValue::number(masked as f64 / (1u64 << 53) as f64)
}

/// Math.sign(x)
///
/// Returns 1.0, -1.0, or 0.0 (preserving +0/-0). Returns NaN for NaN input.
pub fn sign(args: &[JsValue]) -> JsValue {
    let x = args.first().map_or(f64::NAN, to_f64);
    if x.is_nan() {
        JsValue::number(f64::NAN)
    } else if x > 0.0 {
        JsValue::number(1.0)
    } else if x < 0.0 {
        JsValue::number(-1.0)
    } else {
        JsValue::number(x) // preserves +0/-0
    }
}

/// Math.trunc(x)
pub fn trunc(args: &[JsValue]) -> JsValue {
    let x = args.first().map_or(f64::NAN, to_f64);
    JsValue::number(x.trunc())
}

/// Math.log(x) — natural logarithm.
pub fn log(args: &[JsValue]) -> JsValue {
    let x = args.first().map_or(f64::NAN, to_f64);
    JsValue::number(x.ln())
}

/// Math.sin(x)
pub fn sin(args: &[JsValue]) -> JsValue {
    let x = args.first().map_or(f64::NAN, to_f64);
    JsValue::number(x.sin())
}

/// Math.cos(x)
pub fn cos(args: &[JsValue]) -> JsValue {
    let x = args.first().map_or(f64::NAN, to_f64);
    JsValue::number(x.cos())
}

/// JS-compatible rounding (rounds .5 toward +Infinity, not "round half to even").
fn js_round(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() {
        return x;
    }
    let floored = x.floor();
    if x - floored >= 0.5 {
        floored + 1.0
    } else {
        floored
    }
}
