//! Math and Number method dispatch.
//!
//! Contains `dispatch_math_method`, `dispatch_number_static_method`, and
//! `dispatch_number_instance_method`.

use nanbox::JsValue;

use super::{
    __esc_rt_create_error, __esc_rt_throw, extract_key_string, make_rt_string, read_argv,
    val_to_f64,
};

/// Dispatch a `Math` static method by name.
///
/// Implements the `Math` object methods from ES2024 \u{00a7}21.3.2.
/// Each branch corresponds to a specific `Math.xxx` function.
///
/// [spec]: https://tc39.es/ecma262/#sec-math-object
///
/// Returns `Some(bits)` if the method is a known Math method, `None` otherwise.
pub(crate) fn dispatch_math_method(method: &str, argc: u32, argv: *const u64) -> Option<u64> {
    let args = read_argv(argc, argv);

    let result = match method {
        // --- Math.abs ( x ) — ES2024 §21.3.2.1 ---
        // https://tc39.es/ecma262/#sec-math.abs
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, return NaN.
        // 3. If n is -0, return +0.
        // 4. If n is -Infinity, return +Infinity.
        // 5. If n < -0, return -n.
        // 6. Return n.
        "abs" => {
            // Step 1: ToNumber(x) — handled by val_to_f64; missing arg → NaN.
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-6: f64::abs handles all spec cases.
            JsValue::number(x.abs())
        }
        // --- Math.floor ( x ) — ES2024 §21.3.2.16 ---
        // https://tc39.es/ecma262/#sec-math.floor
        // 1. Let n be ? ToNumber(x).
        // 2. If n is not finite or n is +0 or n is -0, return n.
        // 3. If n > +0 but n < 1, return +0.
        // 4. Return the greatest (closest to +Infinity) integral Number value
        //    that is not greater than n.
        "floor" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-4: f64::floor handles all spec cases.
            JsValue::number(x.floor())
        }
        // --- Math.ceil ( x ) — ES2024 §21.3.2.6 ---
        // https://tc39.es/ecma262/#sec-math.ceil
        // 1. Let n be ? ToNumber(x).
        // 2. If n is not finite or n is +0 or n is -0, return n.
        // 3. If n < 0 but n > -1, return -0.
        // 4. Return the smallest (closest to -Infinity) integral Number value
        //    that is not less than n.
        "ceil" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-4: f64::ceil handles all spec cases.
            JsValue::number(x.ceil())
        }
        // --- Math.round ( x ) — ES2024 §21.3.2.28 ---
        // https://tc39.es/ecma262/#sec-math.round
        // 1. Let n be ? ToNumber(x).
        // 2. If n is not finite or n is an integral Number, return n.
        // 3. If n < 0.5 and n > +0, return +0.
        // 4. If n < -0 and n >= -0.5, return -0.
        // 5. Return the integral Number closest to n, preferring the Number
        //    closer to +Infinity in the case of a tie.
        "round" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // JS rounding: ties go toward +Infinity (not "round half to even").
            // Special case: Math.round(-0.5) must return -0 (not 0).
            let rounded = if x.is_nan() || x.is_infinite() {
                // Step 2: If n is not finite, return n.
                x
            } else {
                let floored = x.floor();
                let frac = x - floored;
                if frac < 0.5 {
                    // Step 3/general: fractional part < 0.5, round down.
                    floored
                } else if frac > 0.5 {
                    // Step 5: fractional part > 0.5, round up.
                    floored + 1.0
                } else {
                    // Step 5: Exactly 0.5 — round toward +Infinity.
                    // Step 4: For negative half-integers (e.g. -0.5), floored + 1.0 = 0.0
                    // but spec requires -0 when the result is zero from negative input.
                    let r = floored + 1.0;
                    if r == 0.0 && x.is_sign_negative() {
                        -0.0
                    } else {
                        r
                    }
                }
            };
            JsValue::number(rounded)
        }
        // --- Math.sqrt ( x ) — ES2024 §21.3.2.32 ---
        // https://tc39.es/ecma262/#sec-math.sqrt
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n is +0, n is -0, or n is +Infinity, return n.
        // 3. If n < -0, return NaN.
        // 4. Return the square root of n.
        "sqrt" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-4: f64::sqrt handles all spec cases.
            JsValue::number(x.sqrt())
        }
        // --- Math.pow ( base, exponent ) — ES2024 §21.3.2.26 ---
        // https://tc39.es/ecma262/#sec-math.pow
        // 1. Set base to ? ToNumber(base).
        // 2. Set exponent to ? ToNumber(exponent).
        // 3. Return Number::exponentiate(base, exponent).
        "pow" => {
            // Step 1: ToNumber(base).
            let base = args.first().map_or(f64::NAN, val_to_f64);
            // Step 2: ToNumber(exponent).
            let exp = args.get(1).map_or(f64::NAN, val_to_f64);
            // Step 3: Number::exponentiate(base, exponent).
            // TODO: Step 3 — f64::powf doesn't perfectly match the spec's
            // Number::exponentiate for all edge cases (e.g., 1**Infinity should be NaN
            // per spec but Rust returns 1.0). Most cases are handled correctly.
            JsValue::number(base.powf(exp))
        }
        // --- Math.max ( ...args ) — ES2024 §21.3.2.24 ---
        // https://tc39.es/ecma262/#sec-math.max
        // 1. Let coerced be a new empty List.
        // 2. For each element arg of args, do
        //    a. Let n be ? ToNumber(arg).
        //    b. Append n to coerced.
        // 3. Let highest be -Infinity.
        // 4. For each element number of coerced, do
        //    a. If number is NaN, return NaN.
        //    b. If number is +0 and highest is -0, set highest to +0.
        //    c. If number > highest, set highest to number.
        // 5. Return highest.
        "max" => {
            // Step 3: If no args, return -Infinity.
            if args.is_empty() {
                return Some(JsValue::number(f64::NEG_INFINITY).raw_bits());
            }
            // Step 3: Let highest be -Infinity.
            let mut result = f64::NEG_INFINITY;
            // Steps 2, 4: Iterate coerced values.
            for arg in &args {
                // Step 2a: ToNumber(arg).
                let n = val_to_f64(arg);
                // Step 4a: If number is NaN, return NaN.
                if n.is_nan() {
                    return Some(JsValue::number(f64::NAN).raw_bits());
                }
                // Steps 4b-4c: +0 > -0 for max purposes; if number > highest, update.
                if n > result || (n == 0.0 && result == 0.0 && result.is_sign_negative()) {
                    result = n;
                }
            }
            // Step 5: Return highest.
            JsValue::number(result)
        }
        // --- Math.min ( ...args ) — ES2024 §21.3.2.25 ---
        // https://tc39.es/ecma262/#sec-math.min
        // 1. Let coerced be a new empty List.
        // 2. For each element arg of args, do
        //    a. Let n be ? ToNumber(arg).
        //    b. Append n to coerced.
        // 3. Let lowest be +Infinity.
        // 4. For each element number of coerced, do
        //    a. If number is NaN, return NaN.
        //    b. If number is -0 and lowest is +0, set lowest to -0.
        //    c. If number < lowest, set lowest to number.
        // 5. Return lowest.
        "min" => {
            // Step 3: If no args, return +Infinity.
            if args.is_empty() {
                return Some(JsValue::number(f64::INFINITY).raw_bits());
            }
            // Step 3: Let lowest be +Infinity.
            let mut result = f64::INFINITY;
            // Steps 2, 4: Iterate coerced values.
            for arg in &args {
                // Step 2a: ToNumber(arg).
                let n = val_to_f64(arg);
                // Step 4a: If number is NaN, return NaN.
                if n.is_nan() {
                    return Some(JsValue::number(f64::NAN).raw_bits());
                }
                // Steps 4b-4c: -0 < +0 for min purposes; if number < lowest, update.
                if n < result || (n == 0.0 && result == 0.0 && n.is_sign_negative()) {
                    result = n;
                }
            }
            // Step 5: Return lowest.
            JsValue::number(result)
        }
        // --- Math.random ( ) — ES2024 §21.3.2.27 ---
        // https://tc39.es/ecma262/#sec-math.random
        // 1. Return a Number value with positive sign, greater than or equal to
        //    +0 but strictly less than 1, chosen randomly or pseudo-randomly
        //    with approximately uniform distribution over that range, using an
        //    implementation-defined algorithm or strategy.
        "random" => {
            // Step 1: Generate a random f64 in [0, 1).
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
        // --- Math.log ( x ) — ES2024 §21.3.2.20 ---
        // https://tc39.es/ecma262/#sec-math.log
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n < -0, or n is -Infinity, return NaN.
        // 3. If n is 1, return +0.
        // 4. If n is +0 or n is -0, return -Infinity.
        // 5. If n is +Infinity, return +Infinity.
        // 6. Return the natural logarithm of n.
        "log" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-6: f64::ln handles all spec cases.
            JsValue::number(x.ln())
        }
        // --- Math.log2 ( x ) — ES2024 §21.3.2.23 ---
        // https://tc39.es/ecma262/#sec-math.log2
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n < -0, or n is -Infinity, return NaN.
        // 3. If n is 1, return +0.
        // 4. If n is +0 or n is -0, return -Infinity.
        // 5. If n is +Infinity, return +Infinity.
        // 6. Return the base-2 logarithm of n.
        "log2" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-6: f64::log2 handles all spec cases.
            JsValue::number(x.log2())
        }
        // --- Math.log10 ( x ) — ES2024 §21.3.2.22 ---
        // https://tc39.es/ecma262/#sec-math.log10
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n < -0, or n is -Infinity, return NaN.
        // 3. If n is 1, return +0.
        // 4. If n is +0 or n is -0, return -Infinity.
        // 5. If n is +Infinity, return +Infinity.
        // 6. Return the base-10 logarithm of n.
        "log10" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-6: f64::log10 handles all spec cases.
            JsValue::number(x.log10())
        }
        // --- Math.log1p ( x ) — ES2024 §21.3.2.21 ---
        // https://tc39.es/ecma262/#sec-math.log1p
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n < -1, or n is -Infinity, return NaN.
        // 3. If n is -1, return -Infinity.
        // 4. If n is +0 or n is -0, return n.
        // 5. If n is +Infinity, return +Infinity.
        // 6. Return the natural logarithm of 1 + n (using an algorithm that
        //    is more precise than computing it naively).
        "log1p" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-6: f64::ln_1p handles all spec cases.
            JsValue::number(x.ln_1p())
        }
        // --- Math.exp ( x ) — ES2024 §21.3.2.14 ---
        // https://tc39.es/ecma262/#sec-math.exp
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN or n is +Infinity, return n.
        // 3. If n is +0 or n is -0, return 1.
        // 4. If n is -Infinity, return +0.
        // 5. Return the exponential function of n.
        "exp" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-5: f64::exp handles all spec cases.
            JsValue::number(x.exp())
        }
        // --- Math.expm1 ( x ) — ES2024 §21.3.2.15 ---
        // https://tc39.es/ecma262/#sec-math.expm1
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n is +0, or n is -0, return n.
        // 3. If n is +Infinity, return +Infinity.
        // 4. If n is -Infinity, return -1.
        // 5. Return the result of subtracting 1 from the exponential function of n
        //    (using an algorithm that is more precise than computing it naively).
        "expm1" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-5: f64::exp_m1 handles all spec cases.
            JsValue::number(x.exp_m1())
        }
        // --- Math.trunc ( x ) — ES2024 §21.3.2.35 ---
        // https://tc39.es/ecma262/#sec-math.trunc
        // 1. Let n be ? ToNumber(x).
        // 2. If n is not finite or n is +0 or n is -0, return n.
        // 3. If n < 1 and n > +0, return +0.
        // 4. If n > -1 and n < -0, return -0.
        // 5. Return the integral part of n, removing any fractional digits.
        "trunc" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-5: f64::trunc handles all spec cases.
            JsValue::number(x.trunc())
        }
        // --- Math.sign ( x ) — ES2024 §21.3.2.29 ---
        // https://tc39.es/ecma262/#sec-math.sign
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n is +0, or n is -0, return n.
        // 3. If n > +0, return 1.
        // 4. Return -1.
        "sign" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            let s = if x > 0.0 {
                // Step 3: If n > +0, return 1.
                1.0
            } else if x < 0.0 {
                // Step 4: Return -1.
                -1.0
            } else {
                // Step 2: If n is NaN, +0, or -0, return n.
                x // preserves +0, -0, NaN
            };
            JsValue::number(s)
        }
        // --- Math.cbrt ( x ) — ES2024 §21.3.2.5 ---
        // https://tc39.es/ecma262/#sec-math.cbrt
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n is +0, n is -0, n is +Infinity, or n is -Infinity, return n.
        // 3. Return the cube root of n.
        "cbrt" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-3: f64::cbrt handles all spec cases.
            JsValue::number(x.cbrt())
        }
        // --- Math.hypot ( ...args ) — ES2024 §21.3.2.18 ---
        // https://tc39.es/ecma262/#sec-math.hypot
        // 1. Let coerced be a new empty List.
        // 2. For each element arg of args, do
        //    a. Let n be ? ToNumber(arg).
        //    b. Append n to coerced.
        // 3. For each element number of coerced, do
        //    a. If number is +Infinity or -Infinity, return +Infinity.
        // 4. Let onlyZero be true.
        // 5. For each element number of coerced, do
        //    a. If number is NaN, return NaN.
        //    b. If number is not +0 and number is not -0, set onlyZero to false.
        // 6. If onlyZero is true, return +0.
        // 7. Return the square root of the sum of squares of the elements of coerced.
        "hypot" => {
            // If no args, return +0 (Step 6: onlyZero is true from empty list).
            if args.is_empty() {
                return Some(JsValue::number(0.0).raw_bits());
            }
            // Step 2: Coerce all arguments to numbers.
            let mut has_nan = false;
            let vals: Vec<f64> = args.iter().map(val_to_f64).collect();
            for &v in &vals {
                // Step 3a: If number is +/-Infinity, return +Infinity.
                if v.is_infinite() {
                    return Some(JsValue::number(f64::INFINITY).raw_bits());
                }
                // Step 5a: Track NaN.
                if v.is_nan() {
                    has_nan = true;
                }
            }
            // Step 5a: If any number is NaN (and none were Infinity), return NaN.
            if has_nan {
                return Some(JsValue::number(f64::NAN).raw_bits());
            }
            // Steps 6-7: Compute sqrt(sum of squares).
            let sum: f64 = vals.iter().map(|v| v * v).sum();
            JsValue::number(sum.sqrt())
        }
        // --- Math.fround ( x ) — ES2024 §21.3.2.17 ---
        // https://tc39.es/ecma262/#sec-math.fround
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, return NaN.
        // 3. If n is +0, -0, +Infinity, or -Infinity, return n.
        // 4. Let n32 be the result of converting n to IEEE 754-2019 binary32 format
        //    using roundTiesToEven mode.
        // 5. Let n64 be the result of converting n32 to IEEE 754-2019 binary64 format.
        // 6. Return n64.
        "fround" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-6: Cast to f32 then back to f64 performs the IEEE 754 round-trip.
            JsValue::number((x as f32) as f64)
        }
        // --- Math.clz32 ( x ) — ES2024 §21.3.2.7 ---
        // https://tc39.es/ecma262/#sec-math.clz32
        // 1. Let n be ? ToNumber(x).
        // 2. Let n32 be ! ToUint32(n).
        // 3. Let p be the number of leading zero bits in the unsigned 32-bit
        //    binary representation of n32.
        // 4. Return F(p).
        "clz32" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Step 2: ToUint32(n) — cast to i32 (Rust truncation matches ToInt32/ToUint32 for clz).
            let n = x as i32;
            // Steps 3-4: Count leading zeros and return.
            JsValue::number(n.leading_zeros() as f64)
        }
        // --- Math.imul ( x, y ) — ES2024 §21.3.2.19 ---
        // https://tc39.es/ecma262/#sec-math.imul
        // 1. Let a be ? ToNumber(x).
        // 2. Let b be ? ToNumber(y).
        // 3. Let a32 be ! ToUint32(a).
        // 4. Let b32 be ! ToUint32(b).
        // 5. Let product be (a32 * b32) modulo 2^32.
        // 6. If product >= 2^31, return F(product - 2^32); otherwise return F(product).
        "imul" => {
            // Steps 1, 3: ToNumber(x) then ToUint32.
            let a = args.first().map_or(f64::NAN, val_to_f64) as i32;
            // Steps 2, 4: ToNumber(y) then ToUint32.
            let b = args.get(1).map_or(f64::NAN, val_to_f64) as i32;
            // Steps 5-6: Wrapping multiply gives the correct 32-bit signed result.
            JsValue::number(a.wrapping_mul(b) as f64)
        }
        // --- Math.atan2 ( y, x ) — ES2024 §21.3.2.4 ---
        // https://tc39.es/ecma262/#sec-math.atan2
        // 1. Let ny be ? ToNumber(y).
        // 2. Let nx be ? ToNumber(x).
        // 3. If ny is NaN or nx is NaN, return NaN.
        // 4-17. (Various special cases for +/-0, +/-Infinity.)
        // 18. Return the arc tangent of ny / nx.
        "atan2" => {
            // Step 1: ToNumber(y).
            let y = args.first().map_or(f64::NAN, val_to_f64);
            // Step 2: ToNumber(x).
            let x = args.get(1).map_or(f64::NAN, val_to_f64);
            // Steps 3-18: f64::atan2 handles all spec cases.
            JsValue::number(y.atan2(x))
        }
        // --- Math.sin ( x ) — ES2024 §21.3.2.30 ---
        // https://tc39.es/ecma262/#sec-math.sin
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n is +0, or n is -0, return n.
        // 3. If n is +Infinity or n is -Infinity, return NaN.
        // 4. Return the sine of n.
        "sin" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-4: f64::sin handles all spec cases.
            JsValue::number(x.sin())
        }
        // --- Math.cos ( x ) — ES2024 §21.3.2.12 ---
        // https://tc39.es/ecma262/#sec-math.cos
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n is +Infinity, or n is -Infinity, return NaN.
        // 3. If n is +0 or n is -0, return 1.
        // 4. Return the cosine of n.
        "cos" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-4: f64::cos handles all spec cases.
            JsValue::number(x.cos())
        }
        // --- Math.tan ( x ) — ES2024 §21.3.2.33 ---
        // https://tc39.es/ecma262/#sec-math.tan
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n is +Infinity, or n is -Infinity, return NaN.
        // 3. If n is +0 or n is -0, return n.
        // 4. Return the tangent of n.
        "tan" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-4: f64::tan handles all spec cases.
            JsValue::number(x.tan())
        }
        // --- Math.asin ( x ) — ES2024 §21.3.2.3 ---
        // https://tc39.es/ecma262/#sec-math.asin
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n is +0, or n is -0, return n.
        // 3. If n > 1 or n < -1, return NaN.
        // 4. Return the arc sine of n.
        "asin" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-4: f64::asin handles all spec cases.
            JsValue::number(x.asin())
        }
        // --- Math.acos ( x ) — ES2024 §21.3.2.1 (note: spec reordered, §21.3.2.2) ---
        // https://tc39.es/ecma262/#sec-math.acos
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n > 1, or n < -1, return NaN.
        // 3. If n is 1, return +0.
        // 4. Return the arc cosine of n.
        "acos" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-4: f64::acos handles all spec cases.
            JsValue::number(x.acos())
        }
        // --- Math.atan ( x ) — ES2024 §21.3.2.4 (note: atan is at sec-math.atan) ---
        // https://tc39.es/ecma262/#sec-math.atan
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n is +0, or n is -0, return n.
        // 3. If n is +Infinity, return an approximation to +pi/2.
        // 4. If n is -Infinity, return an approximation to -pi/2.
        // 5. Return the arc tangent of n.
        "atan" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-5: f64::atan handles all spec cases.
            JsValue::number(x.atan())
        }
        // --- Math.sinh ( x ) — ES2024 §21.3.2.31 ---
        // https://tc39.es/ecma262/#sec-math.sinh
        // 1. Let n be ? ToNumber(x).
        // 2. If n is not finite or n is +0 or n is -0, return n.
        // 3. Return the hyperbolic sine of n.
        "sinh" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-3: f64::sinh handles all spec cases.
            JsValue::number(x.sinh())
        }
        // --- Math.cosh ( x ) — ES2024 §21.3.2.13 ---
        // https://tc39.es/ecma262/#sec-math.cosh
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, return NaN.
        // 3. If n is +Infinity or n is -Infinity, return +Infinity.
        // 4. If n is +0 or n is -0, return 1.
        // 5. Return the hyperbolic cosine of n.
        "cosh" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-5: f64::cosh handles all spec cases.
            JsValue::number(x.cosh())
        }
        // --- Math.tanh ( x ) — ES2024 §21.3.2.34 ---
        // https://tc39.es/ecma262/#sec-math.tanh
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n is +0, or n is -0, return n.
        // 3. If n is +Infinity, return 1.
        // 4. If n is -Infinity, return -1.
        // 5. Return the hyperbolic tangent of n.
        "tanh" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-5: f64::tanh handles all spec cases.
            JsValue::number(x.tanh())
        }
        // --- Math.asinh ( x ) — ES2024 §21.3.2.3 (sec-math.asinh) ---
        // https://tc39.es/ecma262/#sec-math.asinh
        // 1. Let n be ? ToNumber(x).
        // 2. If n is not finite or n is +0 or n is -0, return n.
        // 3. Return the inverse hyperbolic sine of n.
        "asinh" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-3: f64::asinh handles all spec cases.
            JsValue::number(x.asinh())
        }
        // --- Math.acosh ( x ) — ES2024 §21.3.2.1 (sec-math.acosh) ---
        // https://tc39.es/ecma262/#sec-math.acosh
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN or n < 1, return NaN.
        // 3. If n is 1, return +0.
        // 4. If n is +Infinity, return +Infinity.
        // 5. Return the inverse hyperbolic cosine of n.
        "acosh" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-5: f64::acosh handles all spec cases.
            JsValue::number(x.acosh())
        }
        // --- Math.atanh ( x ) — ES2024 §21.3.2.4 (sec-math.atanh) ---
        // https://tc39.es/ecma262/#sec-math.atanh
        // 1. Let n be ? ToNumber(x).
        // 2. If n is NaN, n > 1, or n < -1, return NaN.
        // 3. If n is 1, return +Infinity.
        // 4. If n is -1, return -Infinity.
        // 5. If n is +0 or n is -0, return n.
        // 6. Return the inverse hyperbolic tangent of n.
        "atanh" => {
            // Step 1: ToNumber(x).
            let x = args.first().map_or(f64::NAN, val_to_f64);
            // Steps 2-6: f64::atanh handles all spec cases.
            JsValue::number(x.atanh())
        }
        _ => return None,
    };
    Some(result.raw_bits())
}

/// Dispatch a `Number` static method by name (e.g. `Number.isInteger`).
///
/// Implements Number constructor static methods from ES2024 \u{00a7}21.1.2.
///
/// [spec]: https://tc39.es/ecma262/#sec-properties-of-the-number-constructor
///
/// Returns `Some(bits)` if the method is a known Number static method, `None` otherwise.
pub(crate) fn dispatch_number_static_method(
    method: &str,
    argc: u32,
    argv: *const u64,
) -> Option<u64> {
    let args = read_argv(argc, argv);
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);

    let result = match method {
        // --- Number.isInteger ( number ) — ES2024 §21.1.2.3 ---
        // https://tc39.es/ecma262/#sec-number.isinteger
        // 1. Return IsIntegralNumber(number).
        //
        // IsIntegralNumber ( argument ) — §7.2.6:
        // 1. If argument is not a Number, return false.
        // 2. If argument is NaN, +Infinity, or -Infinity, return false.
        // 3. If floor(abs(argument)) != abs(argument), return false.
        // 4. Return true.
        "isInteger" => {
            if let Some(n) = val.as_number() {
                // Steps 1-4 of IsIntegralNumber: finite and truncates to itself.
                JsValue::bool(n.is_finite() && n == n.trunc())
            } else if val.is_int() {
                // Small int representation — always an integer.
                JsValue::bool(true)
            } else {
                // Step 1 of IsIntegralNumber: not a Number → false.
                JsValue::bool(false)
            }
        }
        // --- Number.isFinite ( number ) — ES2024 §21.1.2.2 ---
        // https://tc39.es/ecma262/#sec-number.isfinite
        // 1. If number is not a Number, return false.
        // 2. If number is not finite, return false.
        // 3. Otherwise, return true.
        "isFinite" => {
            if let Some(n) = val.as_number() {
                // Steps 2-3: Check finiteness.
                JsValue::bool(n.is_finite())
            } else if val.is_int() {
                // Small int representation — always finite.
                JsValue::bool(true)
            } else {
                // Step 1: Not a Number → false.
                JsValue::bool(false)
            }
        }
        // --- Number.isNaN ( number ) — ES2024 §21.1.2.4 ---
        // https://tc39.es/ecma262/#sec-number.isnan
        // 1. If number is not a Number, return false.
        // 2. If number is NaN, return true.
        // 3. Otherwise, return false.
        "isNaN" => {
            if let Some(n) = val.as_number() {
                // Steps 2-3: Check for NaN.
                JsValue::bool(n.is_nan())
            } else {
                // Step 1: Not a Number → false.
                JsValue::bool(false)
            }
        }
        // --- Number.isSafeInteger ( number ) — ES2024 §21.1.2.5 ---
        // https://tc39.es/ecma262/#sec-number.issafeinteger
        // 1. If IsIntegralNumber(number) is false, return false.
        // 2. If abs(number) <= 2^53 - 1, return true.
        // 3. Otherwise, return false.
        "isSafeInteger" => {
            // Per spec: Number.isSafeInteger does NOT coerce its argument.
            // Returns false for non-number types.
            if let Some(n) = val.as_number() {
                // Step 1: IsIntegralNumber check (finite and truncates to itself).
                // Step 2: abs(number) <= 2^53 - 1.
                JsValue::bool(n.is_finite() && n == n.trunc() && n.abs() <= 9_007_199_254_740_991.0)
            } else if val.is_int() {
                // Small ints are always safe integers.
                JsValue::bool(true)
            } else {
                // Step 1: Not a Number → IsIntegralNumber returns false → false.
                JsValue::bool(false)
            }
        }
        // --- Number.parseInt ( string, radix ) — ES2024 §21.1.2.13 ---
        // https://tc39.es/ecma262/#sec-number.parseint
        // 1. Return ? parseInt(string, radix).
        // (The actual algorithm is in §19.2.5 — delegated to `es_parse_int`.)
        "parseInt" => {
            // Step 1: Delegate to global parseInt (ES2024 §19.2.5).
            let s = extract_key_string(val.raw_bits()).unwrap_or_default();
            let radix = args.get(1).and_then(|v| v.as_int()).unwrap_or(0);
            JsValue::number(super::es_parse_int(&s, radix))
        }
        // --- Number.parseFloat ( string ) — ES2024 §21.1.2.12 ---
        // https://tc39.es/ecma262/#sec-number.parsefloat
        // 1. Return ? parseFloat(string).
        // (The actual algorithm is in §19.2.4 — delegated to `es_parse_float`.)
        "parseFloat" => {
            // Step 1: Delegate to global parseFloat (ES2024 §19.2.4).
            let s = extract_key_string(val.raw_bits()).unwrap_or_default();
            JsValue::number(super::es_parse_float(&s))
        }
        _ => return None,
    };
    Some(result.raw_bits())
}

/// Dispatch a `Number` instance method (called on a numeric value).
///
/// Implements `Number.prototype` methods from ES2024 \u{00a7}21.1.3.
///
/// [spec]: https://tc39.es/ecma262/#sec-properties-of-the-number-prototype-object
///
/// Returns `Some(bits)` if the method is recognized, `None` otherwise.
pub(crate) fn dispatch_number_instance_method(
    val: JsValue,
    method: &str,
    argc: u32,
    argv: *const u64,
) -> Option<u64> {
    // thisNumberValue (ES2024 §21.1.3):
    // 1. If val is a Number, return val.
    // 2. If val is an Object with [[NumberData]], return [[NumberData]].
    // 3. Throw TypeError.
    let n = if val.is_number() || val.is_int() {
        val_to_f64(&val)
    } else {
        let unwrapped_bits = super::unwrap_wrapper_object(val.raw_bits());
        if unwrapped_bits != val.raw_bits() {
            val_to_f64(&JsValue::from_raw_bits(unwrapped_bits))
        } else {
            // Not a number and not a NumberObj — throw TypeError
            if matches!(
                method,
                "toString" | "valueOf" | "toFixed" | "toExponential" | "toPrecision"
            ) {
                let msg = format!("Number.prototype.{method} requires that 'this' be a Number");
                let msg_bits = super::make_rt_string(msg);
                let err = super::__esc_rt_create_error(
                    crate::exceptions::error_tag::TYPE_ERROR,
                    msg_bits,
                );
                super::__esc_rt_throw(err);
                return Some(JsValue::undefined().raw_bits());
            }
            return None;
        }
    };

    let result = match method {
        // --- Number.prototype.toString ( [ radix ] ) — ES2024 §21.1.3.6 ---
        // https://tc39.es/ecma262/#sec-number.prototype.tostring
        // 1. Let x be ? ThisNumberValue(this value).
        // 2. If radix is undefined, let radixMV be 10.
        // 3. Else, let radixMV be ? ToIntegerOrInfinity(radix).
        // 4. If radixMV is not in the inclusive interval from 2 to 36, throw a RangeError.
        // 5. Return Number::toString(x, radixMV).
        "toString" => {
            let args = read_argv(argc, argv);
            let radix_arg = args.first().copied();
            // Steps 2-3: Determine radix.
            let radix = if let Some(rv) = radix_arg {
                if rv.is_undefined() {
                    // Step 2: If radix is undefined, let radixMV be 10.
                    10
                } else {
                    // Step 3: ToIntegerOrInfinity(radix).
                    let r = val_to_f64(&rv) as i32;
                    // Step 4: If radixMV is not in [2, 36], throw a RangeError.
                    if !(2..=36).contains(&r) {
                        throw_range_error("toString() radix must be between 2 and 36");
                        return Some(JsValue::undefined().raw_bits());
                    }
                    r as u32
                }
            } else {
                // Step 2: No argument → radixMV = 10.
                10
            };

            // Step 5: Number::toString(x, radixMV).
            // Handle NaN and Infinity per Number::toString spec (§6.1.6.1.20).
            if n.is_nan() {
                return Some(make_rt_string("NaN".to_string()));
            }
            if n.is_infinite() {
                return Some(make_rt_string(if n > 0.0 {
                    "Infinity".to_string()
                } else {
                    "-Infinity".to_string()
                }));
            }

            if radix == 10 {
                return Some(make_rt_string(number_to_decimal_string(n)));
            }
            // Non-decimal radix for integer values
            if n == n.trunc() && n.is_finite() {
                let i = n as i64;
                let s = match radix {
                    2 => format!("{i:b}"),
                    8 => format!("{i:o}"),
                    16 => format!("{i:x}"),
                    _ => int_to_radix_string(i, radix),
                };
                return Some(make_rt_string(s));
            }
            return Some(make_rt_string(format!("{n}")));
        }
        // --- Number.prototype.toFixed ( fractionDigits ) — ES2024 §21.1.3.3 ---
        // https://tc39.es/ecma262/#sec-number.prototype.tofixed
        // 1. Let x be ? ThisNumberValue(this value).
        // 2. Let f be ? ToIntegerOrInfinity(fractionDigits).
        // 3. Assert: If fractionDigits is undefined, then f is 0.
        // 4. If f is not finite, throw a RangeError exception.
        // 5. If f < 0 or f > 100, throw a RangeError exception.
        // 6. If x is not finite, return Number::toString(x, 10).
        // 7. Set x to RoundMVResult(x, f).  (implementation-approximation)
        // 8-11. Format string with exactly f digits after the decimal point.
        // 12. Return the resulting String.
        "toFixed" => {
            let args = read_argv(argc, argv);
            // Steps 2-3: ToIntegerOrInfinity(fractionDigits); undefined → 0.
            let raw_digits = args
                .first()
                .map_or(0.0, |v| if v.is_undefined() { 0.0 } else { val_to_f64(v) });
            let digits = raw_digits as i64;
            // Steps 4-5: If f < 0 or f > 100 (or NaN), throw RangeError.
            if !(0..=100).contains(&digits) || raw_digits.is_nan() {
                throw_range_error("toFixed() digits argument must be between 0 and 100");
                return Some(JsValue::undefined().raw_bits());
            }
            let digits = digits as usize;
            // Step 6: If x is not finite, return Number::toString(x, 10).
            if n.is_nan() {
                return Some(make_rt_string("NaN".to_string()));
            }
            if n.is_infinite() {
                return Some(make_rt_string(if n > 0.0 {
                    "Infinity".to_string()
                } else {
                    "-Infinity".to_string()
                }));
            }
            // Steps 7-12: Format with exactly `digits` fraction digits.
            return Some(make_rt_string(format!("{n:.digits$}")));
        }
        // --- Number.prototype.toExponential ( fractionDigits ) — ES2024 §21.1.3.2 ---
        // https://tc39.es/ecma262/#sec-number.prototype.toexponential
        // 1. Let x be ? ThisNumberValue(this value).
        // 2. Let f be ? ToIntegerOrInfinity(fractionDigits).
        // 3. Assert: If fractionDigits is undefined, then f is 0.
        // 4. If x is not finite, return Number::toString(x, 10).
        // 5. If fractionDigits is not undefined, then
        //    a. If f < 0 or f > 100, throw a RangeError exception.
        // 6-16. Format in exponential notation.
        // 17. Return the resulting String.
        "toExponential" => {
            let args = read_argv(argc, argv);
            let frac_arg = args.first().copied();

            // Step 4: If x is not finite, return Number::toString(x, 10).
            if n.is_nan() {
                return Some(make_rt_string("NaN".to_string()));
            }
            if n.is_infinite() {
                return Some(make_rt_string(if n > 0.0 {
                    "Infinity".to_string()
                } else {
                    "-Infinity".to_string()
                }));
            }

            let has_frac_arg =
                frac_arg.is_some() && !frac_arg.as_ref().is_none_or(|v| v.is_undefined());

            if has_frac_arg {
                // Step 2: ToIntegerOrInfinity(fractionDigits).
                let raw_frac = val_to_f64(frac_arg.as_ref().unwrap_or(&JsValue::int(0)));
                let frac = raw_frac as i64;
                // Step 5a: If f < 0 or f > 100, throw a RangeError.
                if !(0..=100).contains(&frac) || raw_frac.is_nan() {
                    throw_range_error("toExponential() argument must be between 0 and 100");
                    return Some(JsValue::undefined().raw_bits());
                }
                // Steps 6-17: Format in exponential notation with exactly frac digits.
                return Some(make_rt_string(format_exponential(n, Some(frac as usize))));
            }
            // Steps 6-17: Format in exponential notation with minimum digits.
            return Some(make_rt_string(format_exponential(n, None)));
        }
        // --- Number.prototype.toPrecision ( precision ) — ES2024 §21.1.3.5 ---
        // https://tc39.es/ecma262/#sec-number.prototype.toprecision
        // 1. Let x be ? ThisNumberValue(this value).
        // 2. If precision is undefined, return ! ToString(x).
        // 3. Let p be ? ToIntegerOrInfinity(precision).
        // 4. If x is not finite, return Number::toString(x, 10).
        // 5. If p < 1 or p > 100, throw a RangeError exception.
        // 6-15. Format to p significant digits.
        // 16. Return the resulting String.
        "toPrecision" => {
            let args = read_argv(argc, argv);
            let prec_arg = args.first().copied();

            // Step 2: If precision is undefined, return ! ToString(x).
            if prec_arg.is_none() || prec_arg.as_ref().is_some_and(|v| v.is_undefined()) {
                return Some(make_rt_string(number_to_decimal_string(n)));
            }

            // Step 4: If x is not finite, return Number::toString(x, 10).
            if n.is_nan() {
                return Some(make_rt_string("NaN".to_string()));
            }
            if n.is_infinite() {
                return Some(make_rt_string(if n > 0.0 {
                    "Infinity".to_string()
                } else {
                    "-Infinity".to_string()
                }));
            }

            // Step 3: ToIntegerOrInfinity(precision).
            let raw_prec = val_to_f64(prec_arg.as_ref().unwrap_or(&JsValue::int(1)));
            let prec = raw_prec as i64;
            // Step 5: If p < 1 or p > 100, throw a RangeError.
            if !(1..=100).contains(&prec) || raw_prec.is_nan() {
                throw_range_error("toPrecision() argument must be between 1 and 100");
                return Some(JsValue::undefined().raw_bits());
            }
            // Steps 6-16: Format to p significant digits.
            return Some(make_rt_string(format_precision(n, prec as usize)));
        }
        // --- Number.prototype.valueOf ( ) — ES2024 §21.1.3.7 ---
        // https://tc39.es/ecma262/#sec-number.prototype.valueof
        // 1. Return ? ThisNumberValue(this value).
        "valueOf" => JsValue::number(n),
        _ => return None,
    };
    Some(result.raw_bits())
}

/// Convert a number to its standard decimal string representation (like JS `Number.toString()`).
///
/// Implements a simplified version of the `Number::toString ( x, 10 )` algorithm
/// from ES2024 \u{00a7}6.1.6.1.20.
///
/// [spec]: https://tc39.es/ecma262/#sec-numeric-types-number-tostring
///
/// Handles integers that fit in i64, and falls back to Rust's default float formatting.
fn number_to_decimal_string(n: f64) -> String {
    // Step 1: If x is NaN, return "NaN".
    if n.is_nan() {
        return "NaN".to_string();
    }
    // Steps 2-3: If x is +0 or -0, return "0". If x < -0, return concat("-", toString(-x)).
    // Step 4: If x is +Infinity, return "Infinity".
    if n.is_infinite() {
        return if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    // Steps 5-9: Produce the decimal string representation.
    if n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Convert an integer to an arbitrary radix string (2-36).
///
/// Internal helper for `Number.prototype.toString` with non-decimal radix.
/// Implements the digit-extraction loop from Number::toString (ES2024 \u{00a7}6.1.6.1.20).
fn int_to_radix_string(mut value: i64, radix: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }
    let negative = value < 0;
    if negative {
        value = -value;
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut result = Vec::new();
    let radix = radix as i64;
    while value > 0 {
        result.push(digits[(value % radix) as usize]);
        value /= radix;
    }
    if negative {
        result.push(b'-');
    }
    result.reverse();
    // SAFETY: digits are all ASCII characters
    unsafe { String::from_utf8_unchecked(result) }
}

/// Format a number in exponential notation.
///
/// Internal helper for `Number.prototype.toExponential` (ES2024 \u{00a7}21.1.3.2).
///
/// [spec]: https://tc39.es/ecma262/#sec-number.prototype.toexponential
///
/// If `fraction_digits` is `Some(n)`, produces exactly `n` digits after the decimal point.
/// If `None`, uses the minimum number of digits necessary.
fn format_exponential(n: f64, fraction_digits: Option<usize>) -> String {
    if n == 0.0 {
        let sign = if n.is_sign_negative() { "-" } else { "" };
        return match fraction_digits {
            Some(0) => format!("{sign}0e+0"),
            Some(fd) => format!("{sign}0.{}e+0", "0".repeat(fd)),
            None => format!("{sign}0e+0"),
        };
    }

    let negative = n < 0.0;
    let abs_n = n.abs();
    let exp = abs_n.log10().floor() as i32;
    let mantissa = abs_n / 10f64.powi(exp);

    let sign = if negative { "-" } else { "" };
    let exp_sign = if exp >= 0 { "+" } else { "" };

    match fraction_digits {
        Some(fd) => {
            if fd == 0 {
                let rounded = mantissa.round() as i64;
                // Handle rounding causing mantissa to reach 10
                if rounded >= 10 {
                    format!("{sign}1e{exp_sign}{}", exp + 1)
                } else {
                    format!("{sign}{rounded}e{exp_sign}{exp}")
                }
            } else {
                let factor = 10f64.powi(fd as i32);
                let rounded = (mantissa * factor).round() / factor;
                // Handle rounding causing mantissa to reach 10
                if rounded >= 10.0 {
                    let new_mantissa = rounded / 10.0;
                    let new_exp = exp + 1;
                    let new_exp_sign = if new_exp >= 0 { "+" } else { "" };
                    format!("{sign}{new_mantissa:.fd$}e{new_exp_sign}{new_exp}")
                } else {
                    format!("{sign}{rounded:.fd$}e{exp_sign}{exp}")
                }
            }
        }
        None => {
            // Use minimum digits necessary — format as full precision then strip trailing zeros
            // Use Rust's default formatting for the mantissa with plenty of digits
            let s = format!("{mantissa:.20}");
            let s = s.trim_end_matches('0');
            let s = s.trim_end_matches('.');
            format!("{sign}{s}e{exp_sign}{exp}")
        }
    }
}

/// Format a number to the specified precision (significant digits).
///
/// Implements the `Number.prototype.toPrecision ( precision )` algorithm
/// from ES2024 \u{00a7}21.1.3.5, steps 6-16 (formatting after validation).
///
/// [spec]: https://tc39.es/ecma262/#sec-number.prototype.toprecision
fn format_precision(n: f64, precision: usize) -> String {
    if n == 0.0 {
        let sign = if n.is_sign_negative() { "-" } else { "" };
        if precision == 1 {
            return format!("{sign}0");
        }
        return format!("{sign}0.{}", "0".repeat(precision - 1));
    }

    let negative = n < 0.0;
    let abs_n = n.abs();
    let sign = if negative { "-" } else { "" };

    // Compute the order of magnitude: e such that 10^(e) <= abs_n < 10^(e+1)
    let e = abs_n.log10().floor() as i64;

    // If precision > e+1 and e >= 0, or if e < 0, we may need decimal point
    // Use Rust's formatting with the right number of significant digits
    let digits_after_point = precision as i64 - e - 1;

    if e >= 0 && (e + 1) as usize <= precision {
        // Fixed notation: e.g. 123.46 for precision=5, e=2
        let dap = digits_after_point.max(0) as usize;
        if dap == 0 {
            // Need exactly `precision` significant digits, no decimal point
            let factor = 10f64.powi(e as i32 - precision as i32 + 1);
            let rounded = (abs_n / factor).round() * factor;
            format!("{sign}{rounded:.0}")
        } else {
            format!("{sign}{abs_n:.dap$}")
        }
    } else if e >= 0 {
        // Exponential notation: too many digits before decimal point
        // e.g., (123456).toPrecision(2) => "1.2e+5"
        let mantissa = abs_n / 10f64.powi(e as i32);
        let exp_sign = if e >= 0 { "+" } else { "" };
        if precision == 1 {
            let rounded = mantissa.round() as i64;
            if rounded >= 10 {
                format!("{sign}1e{exp_sign}{}", e + 1)
            } else {
                format!("{sign}{rounded}e{exp_sign}{e}")
            }
        } else {
            let fd = precision - 1;
            let factor = 10f64.powi(fd as i32);
            let rounded = (mantissa * factor).round() / factor;
            if rounded >= 10.0 {
                let new_mantissa = rounded / 10.0;
                let new_e = e + 1;
                let new_exp_sign = if new_e >= 0 { "+" } else { "" };
                format!("{sign}{new_mantissa:.fd$}e{new_exp_sign}{new_e}")
            } else {
                format!("{sign}{rounded:.fd$}e{exp_sign}{e}")
            }
        }
    } else {
        // e < 0: number is < 1, e.g. 0.000123
        // Fixed notation with leading zeros
        let dap = digits_after_point.max(0) as usize;
        format!("{sign}{abs_n:.dap$}")
    }
}

/// Helper to throw a `RangeError` with the given message string.
fn throw_range_error(msg: &str) {
    let msg_bits = make_rt_string(msg.to_string());
    let err = __esc_rt_create_error(crate::exceptions::error_tag::RANGE_ERROR, msg_bits);
    __esc_rt_throw(err);
}
