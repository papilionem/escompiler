//! JavaScript value operations: arithmetic, comparison, and type conversion.
//!
//! Implements the core JS operators on NaN-boxed `JsValue` representations.
//! All operations follow ECMAScript semantics (e.g., `typeof null === "object"`,
//! `NaN !== NaN`, `null == undefined`).

use nanbox::JsValue;

use crate::internal_data::{InternalData, InternalKind, UnifiedObject};
use crate::string_ops;
use crate::symbol;
use crate::tagged_obj::{ObjTag, deref_tagged, read_obj_tag};

/// `ToNumber ( argument )` — ECMAScript abstract operation.
///
/// Converts a `JsValue` to an `f64` following the ECMAScript `ToNumber` algorithm.
///
/// [spec]: https://tc39.es/ecma262/#sec-tonumber
pub fn to_number(val: JsValue) -> f64 {
    // 1. If argument is a Number, return argument.
    if let Some(n) = val.as_int() {
        return n as f64;
    }
    if let Some(n) = val.as_number() {
        return n;
    }
    // 2. If argument is either a Symbol or a BigInt, throw a TypeError exception.
    if val.is_symbol() {
        throw_type_error("Cannot convert a Symbol value to a number");
        return f64::NAN;
    }
    // TODO: Step 2 — BigInt not yet supported
    // 3. If argument is undefined, return NaN.
    // (handled by fall-through at end)
    // 4. If argument is null, return +0.
    if val.is_null() {
        return 0.0;
    }
    // 5. If argument is true, return 1. If argument is false, return +0.
    if let Some(b) = val.as_bool() {
        return if b { 1.0 } else { 0.0 };
    }
    // 6. If argument is a String, return StringToNumber(argument).
    if val.is_string() {
        return string_to_number(val);
    }
    // 7. Assert: argument is an Object.
    // 8. Let primValue be ? ToPrimitive(argument, number).
    // 9. Assert: primValue is not an Object.
    // 10. Return ? ToNumber(primValue).
    if val.is_object() {
        let prim = to_primitive(val, ToPrimitiveHint::Number);
        // If ToPrimitive threw (e.g. non-callable @@toPrimitive, getter threw), propagate.
        if crate::exceptions::is_exception() {
            return f64::NAN;
        }
        // Prevent infinite recursion: if ToPrimitive returned an object, give up
        if prim.is_object() {
            return f64::NAN;
        }
        return to_number(prim);
    }
    // undefined → NaN (symbol handled above in step 2)
    f64::NAN
}

/// `ToIntegerOrInfinity ( argument )` — ECMAScript abstract operation.
///
/// Converts a value to an integer, +Infinity, or -Infinity.
///
/// [spec]: https://tc39.es/ecma262/#sec-tointegerorinfinity
pub fn to_integer_or_infinity(val: JsValue) -> f64 {
    // 1. Let number be ? ToNumber(argument).
    let number = to_number(val);
    // 2. If number is NaN, +0, or -0, return 0.
    if number.is_nan() || number == 0.0 {
        return 0.0;
    }
    // 3. If number is +Infinity, return +Infinity.
    // 4. If number is -Infinity, return -Infinity.
    if number.is_infinite() {
        return number;
    }
    // 5. Return truncate(R(number)).
    number.trunc()
}

/// `ToLength ( argument )` — ECMAScript abstract operation.
///
/// Converts a value to a valid array-like length in the range `[0, 2^53 - 1]`.
///
/// [spec]: https://tc39.es/ecma262/#sec-tolength
pub fn to_length(val: JsValue) -> u64 {
    // 1. Let len be ? ToIntegerOrInfinity(argument).
    let len = to_integer_or_infinity(val);
    // 2. If len <= 0, return +0.
    if len <= 0.0 {
        return 0;
    }
    // 3. Return min(len, 2^53 - 1).
    let max = (1u64 << 53) - 1;
    if len >= max as f64 { max } else { len as u64 }
}

/// `ToIndex ( value )` — ECMAScript abstract operation.
///
/// Converts a value to a valid typed-array/buffer index. Returns `Err` with
/// a RangeError `JsValue` if the index is out of the valid range `[0, 2^53 - 1]`.
///
/// [spec]: https://tc39.es/ecma262/#sec-toindex
pub fn to_index(val: JsValue) -> Result<u64, JsValue> {
    // 1. If value is undefined, return 0.
    if val.is_undefined() {
        return Ok(0);
    }
    // 2. Let integerIndex be ? ToIntegerOrInfinity(value).
    let integer_index = to_integer_or_infinity(val);
    // 3. If integerIndex is not in the inclusive interval from 0 to 2^53 - 1,
    //    throw a RangeError exception.
    let max = (1u64 << 53) - 1;
    if integer_index < 0.0 || integer_index > max as f64 {
        throw_range_error("Invalid index");
        return Err(JsValue::undefined());
    }
    // 4. Return integerIndex.
    Ok(integer_index as u64)
}

/// Returns `true` if `c` is an ECMAScript `WhiteSpace` or `LineTerminator` character.
///
/// Covers the full set from the ECMAScript specification:
/// - WhiteSpace (ES2024 section 12.2): TAB, VT, FF, SP, NBSP, BOM, and Unicode Zs category
/// - LineTerminator (ES2024 section 12.3): LF, CR, LS, PS
///
/// [spec-ws]: https://tc39.es/ecma262/#sec-white-space
/// [spec-lt]: https://tc39.es/ecma262/#sec-line-terminators
fn is_es_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'  // TAB
        | '\u{000B}'  // VT
        | '\u{000C}'  // FF
        | '\u{0020}'  // SP
        | '\u{00A0}'  // NBSP
        | '\u{FEFF}'  // BOM (ZWNBSP)
        | '\u{000A}'  // LF
        | '\u{000D}'  // CR
        | '\u{2028}'  // LS
        | '\u{2029}'  // PS
        | '\u{1680}'  // Ogham Space Mark
        | '\u{2000}'  // En Quad
        | '\u{2001}'  // Em Quad
        | '\u{2002}'  // En Space
        | '\u{2003}'  // Em Space
        | '\u{2004}'  // Three-Per-Em Space
        | '\u{2005}'  // Four-Per-Em Space
        | '\u{2006}'  // Six-Per-Em Space
        | '\u{2007}'  // Figure Space
        | '\u{2008}'  // Punctuation Space
        | '\u{2009}'  // Thin Space
        | '\u{200A}'  // Hair Space
        | '\u{202F}'  // Narrow No-Break Space
        | '\u{205F}'  // Medium Mathematical Space
        | '\u{3000}' // Ideographic Space
    )
}

/// Trim ECMAScript whitespace and line terminators from both ends of a string.
///
/// Used by `StringToNumber` to strip leading/trailing whitespace before parsing.
fn es_trim(s: &str) -> &str {
    s.trim_matches(is_es_whitespace)
}

/// `StringToNumber ( str )` — ECMAScript abstract operation.
///
/// Parses a NaN-boxed string value to a number following ECMAScript
/// `StringNumericLiteral` production rules.
///
/// [spec]: https://tc39.es/ecma262/#sec-stringtonumber
///
/// Handles all `StringNumericLiteral` productions:
/// - Empty/whitespace-only → `0`
/// - `"Infinity"` / `"+Infinity"` / `"-Infinity"` → `+/-Infinity`
/// - `"0x"` / `"0X"` hex prefix → hexadecimal integer
/// - `"0o"` / `"0O"` octal prefix → octal integer
/// - `"0b"` / `"0B"` binary prefix → binary integer
/// - `"010"` → `10` (NOT legacy octal; decimal per strict mode / ES2015+)
/// - Decimal integers and floats
/// - Unrecognized strings → `NaN`
fn string_to_number(val: JsValue) -> f64 {
    let s = string_ops::get_string_data(val);
    // 1. Let text be StringToCodePoints(str).
    // 2. Let literal be ParseText(text, StringNumericLiteral).
    // 3. If literal is a List of errors, return NaN.
    let trimmed = es_trim(&s);
    // 4. If literal is «» (empty), return +0.
    if trimmed.is_empty() {
        return 0.0;
    }
    // 5. Evaluate the MV (mathematical value) of the numeric literal.
    if trimmed == "Infinity" || trimmed == "+Infinity" {
        return f64::INFINITY;
    }
    if trimmed == "-Infinity" {
        return f64::NEG_INFINITY;
    }
    // Hex prefix: 0x / 0X
    if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
        return u64::from_str_radix(&trimmed[2..], 16)
            .map(|n| n as f64)
            .unwrap_or(f64::NAN);
    }
    // Octal prefix: 0o / 0O
    if trimmed.starts_with("0o") || trimmed.starts_with("0O") {
        return u64::from_str_radix(&trimmed[2..], 8)
            .map(|n| n as f64)
            .unwrap_or(f64::NAN);
    }
    // Binary prefix: 0b / 0B
    if trimmed.starts_with("0b") || trimmed.starts_with("0B") {
        return u64::from_str_radix(&trimmed[2..], 2)
            .map(|n| n as f64)
            .unwrap_or(f64::NAN);
    }
    // Decimal: "010" is decimal 10, not octal (ES2015+ / strict mode)
    trimmed.parse::<f64>().unwrap_or(f64::NAN)
}

/// `ToBoolean ( argument )` — ECMAScript abstract operation.
///
/// Converts a `JsValue` to `bool`. Falsy values are: `undefined`, `null`,
/// `false`, `+0`/`-0`, `NaN`, `""`, and `0n` (BigInt zero, not yet supported).
/// All other values, including all objects, are truthy.
///
/// [spec]: https://tc39.es/ecma262/#sec-toboolean
pub fn to_boolean(val: JsValue) -> bool {
    // 1. If argument is a Boolean, return argument.
    // 2. If argument is one of undefined, null, +0, -0, NaN, 0n, or "", return false.
    if val.is_falsy() {
        return false;
    }
    // Empty string is falsy but can't be detected at the nanbox level
    // (requires runtime pointer dereference to inspect string content).
    if val.is_string() && string_ops::is_empty_string(val) {
        return false;
    }
    // 3. NOTE: This step is replaced in section B.3.6.1 (document.all).
    // 4. Return true.
    true
}

/// Runtime-level falsiness check for `JsValue`.
///
/// This is the inverse of `to_boolean(val)`, returning `true` when the value
/// is falsy per ECMAScript semantics. Used internally for conditional branches
/// and logical operators.
pub fn is_falsy(val: JsValue) -> bool {
    !to_boolean(val)
}

// =========================================================================
// ToPrimitive
// =========================================================================

/// ECMAScript `ToPrimitive` hint for object-to-primitive coercion.
///
/// [spec]: https://tc39.es/ecma262/#sec-toprimitive
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToPrimitiveHint {
    /// Prefer numeric coercion: try `valueOf` first, then `toString`.
    Number,
    /// Prefer string coercion: try `toString` first, then `valueOf`.
    String,
    /// Default coercion (used by `+` and `==`): same order as `Number`.
    Default,
}

/// Default `toString()` result for an object based on its `InternalKind`.
///
/// Produces the spec-defined default string representation when an object
/// has no custom `toString` method:
/// - Arrays → comma-joined elements (matching `Array.prototype.toString()`)
/// - Errors → `"ErrorType: message"` (matching `Error.prototype.toString()`)
/// - Objects with `[Symbol.toStringTag]` → `"[object Tag]"`
/// - All other objects → `"[object Object]"`
///
/// This is an internal helper with no single spec algorithm equivalent.
fn default_object_to_string(bits: u64) -> JsValue {
    let tag = read_obj_tag(bits);
    if tag == Some(ObjTag::Unified as u8) {
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
        if let Some(u) = uni {
            // Arrays: join elements with commas (Array.prototype.toString)
            if u.kind == InternalKind::Array {
                let elements = u.array_elements();
                let parts: Vec<String> = elements
                    .iter()
                    .map(|elem| {
                        if elem.is_null() || elem.is_undefined() {
                            String::new()
                        } else {
                            crate::display::display_value(*elem)
                        }
                    })
                    .collect();
                let joined = parts.join(",");
                let rt_str = Box::new(string_ops::RtString::new(joined));
                let ptr = Box::into_raw(rt_str) as *const ();
                return JsValue::string(ptr);
            }
            // Errors: "ErrorType: message" (Error.prototype.toString)
            if u.kind == InternalKind::ErrorObj
                && let Some(InternalData::Error {
                    error_tag,
                    raw_message,
                    ..
                }) = u.internal_data()
            {
                let name = crate::exceptions::error_name(*error_tag);
                let msg_str = string_ops::get_string_data(JsValue::from_raw_bits(*raw_message));
                let s = if msg_str.is_empty() {
                    name.to_string()
                } else {
                    format!("{name}: {msg_str}")
                };
                let rt_str = Box::new(string_ops::RtString::new(s));
                let ptr = Box::into_raw(rt_str) as *const ();
                return JsValue::string(ptr);
            }
        }
    }
    // Check for [Symbol.toStringTag] property
    let tag_label = get_to_string_tag(bits);
    let s = format!("[object {tag_label}]");
    let rt_str = Box::new(string_ops::RtString::new(s));
    let ptr = Box::into_raw(rt_str) as *const ();
    JsValue::string(ptr)
}

/// Look up the `[Symbol.toStringTag]` property on an object.
///
/// Implements the tag-lookup portion of `Object.prototype.toString` (ES2024
/// section 20.1.3.6, steps 14-17). Returns the tag string if found
/// (e.g., `"Map"`, `"Set"`, or a custom tag), otherwise returns `"Object"`.
///
/// [spec]: https://tc39.es/ecma262/#sec-object.prototype.tostring
fn get_to_string_tag(bits: u64) -> String {
    let sym_key = JsValue::symbol(symbol::SYMBOL_TO_STRING_TAG).raw_bits();
    let tag_val_bits = crate::rt_api::__esc_rt_get_prop(bits, sym_key);
    let tag_val = JsValue::from_raw_bits(tag_val_bits);
    if tag_val.is_string() {
        return string_ops::get_string_data(tag_val);
    }
    // Default based on InternalKind if no toStringTag is set
    let obj_tag = read_obj_tag(bits);
    if obj_tag == Some(ObjTag::Unified as u8) {
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
        if let Some(u) = uni {
            return match u.kind {
                InternalKind::Array => "Array".to_string(),
                InternalKind::MapObj => "Map".to_string(),
                InternalKind::SetObj => "Set".to_string(),
                InternalKind::WeakMapObj => "WeakMap".to_string(),
                InternalKind::WeakSetObj => "WeakSet".to_string(),
                InternalKind::WeakRefObj => "WeakRef".to_string(),
                InternalKind::RegExpObj => "RegExp".to_string(),
                InternalKind::Promise => "Promise".to_string(),
                InternalKind::Generator => "Generator".to_string(),
                InternalKind::ErrorObj => "Error".to_string(),
                _ => "Object".to_string(),
            };
        }
    }
    "Object".to_string()
}

/// Try calling a method (valueOf or toString) on an object by looking up
/// the property and, if it's a callable closure, invoking it.
///
/// This is an internal helper used by `OrdinaryToPrimitive` (ES2024 7.1.1.1)
/// to perform step 5 ("Call the method").
///
/// Returns `Some(result)` if the method was found and called, `None` otherwise.
fn try_call_own_method(bits: u64, method: &str) -> Option<JsValue> {
    let tag = read_obj_tag(bits)?;
    if tag != ObjTag::Unified as u8 {
        return None;
    }
    // Look up the property via the shape table and proto chain
    let key = crate::rt_api::make_rt_string(method.to_string());
    let prop_bits = crate::rt_api::__esc_rt_get_prop(bits, key);
    let prop = JsValue::from_raw_bits(prop_bits);
    if prop.is_undefined() {
        // Fallback: for callable objects (Function/Closure/NativeFunc), "toString"
        // and "valueOf" are dispatched virtually, not stored as own properties.
        // Try the virtual dispatch path for these well-known methods.
        let uni = unsafe { deref_tagged::<UnifiedObject>(bits) }?;
        if uni.is_callable() {
            if method == "toString" {
                let result = crate::rt_api::dispatch_function_to_string(bits);
                return Some(JsValue::from_raw_bits(result));
            }
            if method == "valueOf" {
                // §20.2.3.6: Function.prototype.valueOf is Object.prototype.valueOf
                // which returns the object itself.
                return Some(JsValue::from_raw_bits(bits));
            }
        }
        return None;
    }
    // Check if the property is a callable closure/function
    if !prop.is_object() {
        return None;
    }
    let prop_tag = read_obj_tag(prop_bits)?;
    if prop_tag != ObjTag::Unified as u8 {
        return None;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(prop_bits) }?;
    if !uni.is_callable() {
        return None;
    }
    // Extract closure data to call it
    if let Some(InternalData::Function { code_idx, env, .. }) = uni.internal_data() {
        // Set `this` to the receiver object and call through the dispatch trampoline
        let result = unsafe {
            // SAFETY: We're calling a compiled closure with valid args.
            crate::rt_api::CURRENT_THIS.with(|cell| {
                let prev = cell.replace(bits);
                let prev_env = crate::rt_api::CURRENT_CLOSURE_ENV.with(|c| c.replace(*env));
                let prev_argc = crate::rt_api::CURRENT_ARGC.with(|c| c.replace(0));
                let prev_argv = crate::rt_api::CURRENT_ARGV.with(|c| c.replace(std::ptr::null()));
                let r = crate::rt_api::__esc_dispatch(*code_idx as i32, 0, std::ptr::null());
                crate::rt_api::CURRENT_THIS.with(|c| c.set(prev));
                crate::rt_api::CURRENT_CLOSURE_ENV.with(|c| c.set(prev_env));
                crate::rt_api::CURRENT_ARGC.with(|c| c.set(prev_argc));
                crate::rt_api::CURRENT_ARGV.with(|c| c.set(prev_argv));
                r
            })
        };
        // If the method threw (e.g. valueOf/toString that throw), propagate the exception.
        if crate::exceptions::is_exception() {
            return Some(JsValue::undefined());
        }
        return Some(JsValue::from_raw_bits(result));
    }
    // Native function
    if let Some(InternalData::NativeFunc { func, context }) = uni.internal_data() {
        // Set CURRENT_THIS so the NativeFunc sees the correct receiver
        // (e.g., Object.prototype.valueOf returns `this`).
        let prev = crate::rt_api::CURRENT_THIS.with(|c| c.replace(bits));
        let result = func(*context);
        crate::rt_api::CURRENT_THIS.with(|c| c.set(prev));
        // If the native function threw, propagate the exception.
        if crate::exceptions::is_exception() {
            return Some(JsValue::undefined());
        }
        return Some(JsValue::from_raw_bits(result));
    }
    None
}

/// Try calling a symbol-keyed method on an object with arguments.
///
/// Looks up the property keyed by `PropertyKey::Symbol(sym_id)` on the object,
/// and if it is callable, invokes it with the given arguments (as raw `u64` bits).
///
/// This is an internal dispatch helper used by `ToPrimitive` (to call
/// `[Symbol.toPrimitive]`) and by other symbol-protocol methods.
///
/// Returns `Some(result)` if the method was found and called, `None` otherwise.
pub(crate) fn try_call_symbol_method(bits: u64, sym_id: u32, args: &[u64]) -> Option<JsValue> {
    let tag = read_obj_tag(bits)?;
    if tag != ObjTag::Unified as u8 {
        return None;
    }
    // Look up the symbol-keyed property
    // §7.3.10 GetMethod: Let func be ? GetV(V, P).
    let sym_key = JsValue::symbol(sym_id).raw_bits();
    let prop_bits = crate::rt_api::__esc_rt_get_prop(bits, sym_key);
    // If get_prop threw (e.g. getter threw), propagate the exception.
    if crate::exceptions::is_exception() {
        // Return Some with undefined so caller propagates exception, not None which
        // would cause fallthrough to OrdinaryToPrimitive.
        return Some(JsValue::undefined());
    }
    let prop = JsValue::from_raw_bits(prop_bits);
    // §7.3.10 step 3: If func is either undefined or null, return undefined.
    if prop.is_undefined() || prop.is_null() {
        return None;
    }
    // §7.3.10 step 4: If IsCallable(func) is false, throw a TypeError exception.
    // We need prop to be a UnifiedObject that is callable.
    let prop_tag = read_obj_tag(prop_bits);
    if prop_tag != Some(ObjTag::Unified as u8) {
        throw_type_error("is not a function");
        return Some(JsValue::undefined());
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(prop_bits) }?;
    if !uni.is_callable() {
        throw_type_error("is not a function");
        return Some(JsValue::undefined());
    }
    // Extract closure data to call it with args
    if let Some(InternalData::Function { code_idx, env, .. }) = uni.internal_data() {
        let argc = args.len() as u32;
        let argv = if args.is_empty() {
            std::ptr::null()
        } else {
            args.as_ptr()
        };
        let result = unsafe {
            // SAFETY: We're calling a compiled closure with valid args.
            crate::rt_api::CURRENT_THIS.with(|cell| {
                let prev = cell.replace(bits);
                let prev_env = crate::rt_api::CURRENT_CLOSURE_ENV.with(|c| c.replace(*env));
                let prev_argc = crate::rt_api::CURRENT_ARGC.with(|c| c.replace(argc));
                let prev_argv = crate::rt_api::CURRENT_ARGV.with(|c| c.replace(argv));
                let r = crate::rt_api::__esc_dispatch(*code_idx as i32, argc as i32, argv);
                crate::rt_api::CURRENT_THIS.with(|c| c.set(prev));
                crate::rt_api::CURRENT_CLOSURE_ENV.with(|c| c.set(prev_env));
                crate::rt_api::CURRENT_ARGC.with(|c| c.set(prev_argc));
                crate::rt_api::CURRENT_ARGV.with(|c| c.set(prev_argv));
                r
            })
        };
        // If the function threw, propagate the exception.
        if crate::exceptions::is_exception() {
            return Some(JsValue::undefined());
        }
        return Some(JsValue::from_raw_bits(result));
    }
    // Native function — call with context
    if let Some(InternalData::NativeFunc { func, context }) = uni.internal_data() {
        let result = func(*context);
        // If the native function threw, propagate the exception.
        if crate::exceptions::is_exception() {
            return Some(JsValue::undefined());
        }
        return Some(JsValue::from_raw_bits(result));
    }
    None
}

/// Check if a tagged object is a Date object by inspecting its `InternalKind`.
///
/// Internal helper used by `ToPrimitive` to determine the effective hint
/// for Date objects (Date uses "string" instead of "default").
fn is_date_object(bits: u64) -> bool {
    let tag = read_obj_tag(bits);
    if tag == Some(ObjTag::Unified as u8) {
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
        if let Some(u) = uni {
            return u.kind == InternalKind::DateObj;
        }
    }
    false
}

/// Throw a TypeError via the runtime exception system.
///
/// Sets the pending exception. The caller should return a fallback value
/// after calling this, since the exception is only checked at the next
/// exception-check point in compiled code.
fn throw_type_error(msg: &str) {
    let msg_bits = crate::rt_api::make_rt_string(format!("TypeError: {msg}"));
    let err =
        crate::rt_api::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg_bits);
    crate::rt_api::__esc_rt_throw(err);
}

/// Throw a RangeError via the runtime exception system.
///
/// Sets the pending exception. The caller should return a fallback value
/// after calling this, since the exception is only checked at the next
/// exception-check point in compiled code.
fn throw_range_error(msg: &str) {
    let msg_bits = crate::rt_api::make_rt_string(format!("RangeError: {msg}"));
    let err =
        crate::rt_api::__esc_rt_create_error(crate::exceptions::error_tag::RANGE_ERROR, msg_bits);
    crate::rt_api::__esc_rt_throw(err);
}

/// `ToPrimitive ( input [ , preferredType ] )` — ECMAScript abstract operation.
///
/// Converts an ECMAScript language value to a primitive value. If `input` is
/// already a primitive, it is returned unchanged. For objects, checks for a
/// `[Symbol.toPrimitive]` method first, then falls back to `OrdinaryToPrimitive`.
///
/// [spec]: https://tc39.es/ecma262/#sec-toprimitive
pub fn to_primitive(val: JsValue, hint: ToPrimitiveHint) -> JsValue {
    // 1. If input is an Object, then
    if !val.is_object() {
        // 2. Return input. (already a primitive)
        return val;
    }

    let bits = val.raw_bits();

    // Fast path for wrapper objects: extract the wrapped primitive directly.
    // This is equivalent to calling valueOf() on the wrapper, which the spec
    // requires via OrdinaryToPrimitive, but we can shortcut since we control
    // the implementation.
    let unwrapped = crate::rt_api::unwrap_wrapper_object(bits);
    if unwrapped != bits {
        return JsValue::from_raw_bits(unwrapped);
    }

    // Resolve the effective hint: Date objects treat Default as String
    // (Date.prototype[@@toPrimitive] per ES2024 21.4.4.45)
    let effective_hint = if hint == ToPrimitiveHint::Default && is_date_object(bits) {
        ToPrimitiveHint::String
    } else {
        hint
    };

    // 1.a. Let exoticToPrim be ? GetMethod(input, @@toPrimitive).
    let hint_str = match effective_hint {
        ToPrimitiveHint::Number => "number",
        ToPrimitiveHint::String => "string",
        ToPrimitiveHint::Default => "default",
    };
    let hint_val = crate::rt_api::make_rt_string(hint_str.to_string());
    // 1.b. If exoticToPrim is not undefined, then
    if let Some(result) = try_call_symbol_method(bits, symbol::SYMBOL_TO_PRIMITIVE, &[hint_val]) {
        // 1.b.i. If result is not an Object, return result.
        if !result.is_object() {
            return result;
        }
        // 1.b.ii. Throw a TypeError exception.
        throw_type_error("Cannot convert object to primitive value");
        return default_object_to_string(bits);
    }

    // 1.c. If preferredType is not present, let preferredType be number.
    // (already resolved via effective_hint)

    // 1.d. Return ? OrdinaryToPrimitive(input, preferredType).
    // OrdinaryToPrimitive (ES2024 7.1.1.1):
    // 1. If hint is string, let methodNames be « "toString", "valueOf" ».
    // 2. Else, let methodNames be « "valueOf", "toString" ».
    let (first, second) = match effective_hint {
        ToPrimitiveHint::String => ("toString", "valueOf"),
        ToPrimitiveHint::Number | ToPrimitiveHint::Default => ("valueOf", "toString"),
    };

    // Track whether any method was found (even if it returned an object)
    let mut found_method = false;

    // 3. For each element name of methodNames, do
    //   a. Let method be ? Get(O, name).
    //   b. If IsCallable(method) is true, then
    //     i. Let result be ? Call(method, O).
    //     ii. If result is not an Object, return result.
    if let Some(result) = try_call_own_method(bits, first) {
        if !result.is_object() {
            return result;
        }
        found_method = true;
    }

    if let Some(result) = try_call_own_method(bits, second) {
        if !result.is_object() {
            return result;
        }
        found_method = true;
    }

    // 4. Throw a TypeError exception.
    // If custom methods were found but both returned objects, throw TypeError.
    // If no custom methods existed, fall back to the built-in default toString
    // (equivalent to Object.prototype.toString).
    if found_method {
        throw_type_error("Cannot convert object to primitive value");
    }

    default_object_to_string(bits)
}

/// `ApplyStringOrNumericBinaryOperator ( lval, opText, rval )` — for `+`.
///
/// Implements the `+` operator semantics from ES2024 section 13.15.3
/// (EvaluateStringOrNumericBinaryExpression) and the addition case of
/// `ApplyStringOrNumericBinaryOperator` (ES2024 section 6.1.6.1).
///
/// [spec]: https://tc39.es/ecma262/#sec-applystringornumericbinaryoperator
pub fn js_add(lhs: JsValue, rhs: JsValue) -> JsValue {
    // 1. If opText is +, then
    //   a. Let lprim be ? ToPrimitive(lval).
    let lp = if lhs.is_object() {
        to_primitive(lhs, ToPrimitiveHint::Default)
    } else {
        lhs
    };
    //   b. Let rprim be ? ToPrimitive(rval).
    let rp = if rhs.is_object() {
        to_primitive(rhs, ToPrimitiveHint::Default)
    } else {
        rhs
    };
    //   c. If lprim is a String or rprim is a String, then
    //     i. Let lstr be ? ToString(lprim).
    //     ii. Let rstr be ? ToString(rprim).
    //     iii. Return the string-concatenation of lstr and rstr.
    if lp.is_string() || rp.is_string() {
        return string_ops::string_concat(lp, rp);
    }
    //   d. Set lval to lprim. Set rval to rprim.
    // 2. NOTE: At this point, ... lval and rval are primitive values.
    // 3. Let lnum be ? ToNumeric(lval).
    // 4. Let rnum be ? ToNumeric(rval).
    let l = to_number(lp);
    let r = to_number(rp);
    // 6. Let operation be ... Number::add.
    // 7. Return operation(lnum, rnum).
    JsValue::number(l + r)
}

/// `ApplyStringOrNumericBinaryOperator ( lval, -, rval )` — subtraction.
///
/// [spec]: https://tc39.es/ecma262/#sec-applystringornumericbinaryoperator
pub fn js_sub(lhs: JsValue, rhs: JsValue) -> JsValue {
    // 3. Let lnum be ? ToNumeric(lval).
    let l = to_number(lhs);
    // 4. Let rnum be ? ToNumeric(rval).
    let r = to_number(rhs);
    // 6. Let operation be ... Number::subtract.
    // 7. Return operation(lnum, rnum).
    JsValue::number(l - r)
}

/// `ApplyStringOrNumericBinaryOperator ( lval, *, rval )` — multiplication.
///
/// [spec]: https://tc39.es/ecma262/#sec-applystringornumericbinaryoperator
pub fn js_mul(lhs: JsValue, rhs: JsValue) -> JsValue {
    // 3. Let lnum be ? ToNumeric(lval).
    let l = to_number(lhs);
    // 4. Let rnum be ? ToNumeric(rval).
    let r = to_number(rhs);
    // 6. Let operation be ... Number::multiply.
    // 7. Return operation(lnum, rnum).
    JsValue::number(l * r)
}

/// `ApplyStringOrNumericBinaryOperator ( lval, /, rval )` — division.
///
/// [spec]: https://tc39.es/ecma262/#sec-applystringornumericbinaryoperator
pub fn js_div(lhs: JsValue, rhs: JsValue) -> JsValue {
    // 3. Let lnum be ? ToNumeric(lval).
    let l = to_number(lhs);
    // 4. Let rnum be ? ToNumeric(rval).
    let r = to_number(rhs);
    // 6. Let operation be ... Number::divide.
    // 7. Return operation(lnum, rnum).
    JsValue::number(l / r)
}

/// `ApplyStringOrNumericBinaryOperator ( lval, %, rval )` — remainder.
///
/// [spec]: https://tc39.es/ecma262/#sec-applystringornumericbinaryoperator
pub fn js_mod(lhs: JsValue, rhs: JsValue) -> JsValue {
    // 3. Let lnum be ? ToNumeric(lval).
    let l = to_number(lhs);
    // 4. Let rnum be ? ToNumeric(rval).
    let r = to_number(rhs);
    // 6. Let operation be ... Number::remainder.
    // 7. Return operation(lnum, rnum).
    JsValue::number(l % r)
}

/// `UnaryExpression : - UnaryExpression` — unary numeric negation.
///
/// Converts the operand to a number via `ToNumeric`, then applies
/// `Number::unaryMinus`.
///
/// [spec]: https://tc39.es/ecma262/#sec-unary-minus-operator
pub fn js_neg(val: JsValue) -> JsValue {
    // 1. Let expr be ? Evaluation of UnaryExpression.
    // 2. Let oldValue be ? ToNumeric(? GetValue(expr)).
    let n = to_number(val);
    // 3. If oldValue is a Number, return Number::unaryMinus(oldValue).
    JsValue::number(-n)
}

/// `ApplyStringOrNumericBinaryOperator ( lval, **, rval )` — exponentiation.
///
/// [spec]: https://tc39.es/ecma262/#sec-applystringornumericbinaryoperator
pub fn js_exp(base: JsValue, exp: JsValue) -> JsValue {
    // 3. Let lnum be ? ToNumeric(lval).
    let b = to_number(base);
    // 4. Let rnum be ? ToNumeric(rval).
    let e = to_number(exp);
    // 6. Let operation be ... Number::exponentiate.
    // 7. Return operation(lnum, rnum).
    JsValue::number(es_exponentiate(b, e))
}

/// `Number::exponentiate ( base, exponent )` — ECMAScript numeric operation.
///
/// Implements the full exponentiation algorithm which differs from IEEE 754
/// `pow()` in several edge cases (see inline comments).
///
/// [spec]: https://tc39.es/ecma262/#sec-numeric-types-number-exponentiate
fn es_exponentiate(base: f64, exponent: f64) -> f64 {
    // 1. If exponent is NaN, return NaN.
    if exponent.is_nan() {
        return f64::NAN;
    }
    // 2. If exponent is either +0 or -0, return 1.
    if exponent == 0.0 {
        return 1.0;
    }
    // 3. If base is NaN, return NaN.
    if base.is_nan() {
        return f64::NAN;
    }
    // 4. If base is +Infinity, then (handled by f64::powf)
    // 5. If base is -Infinity, then (handled by f64::powf)
    // 6. If base is +0, then (handled by f64::powf)
    // 7. If base is -0, then (handled by f64::powf)
    // 8. If base > 0 and base is finite, then (partially handled by f64::powf)
    // 9. Assert: base is finite and is neither +0 nor -0.
    // 10. If exponent is +Infinity or exponent is -Infinity:
    //   a. If abs(base) = 1, return NaN.
    //   (This is where ES diverges from IEEE 754: pow(1, inf) = 1 in IEEE)
    if base.abs() == 1.0 && exponent.is_infinite() {
        return f64::NAN;
    }
    // All other cases: delegate to IEEE 754 pow
    base.powf(exponent)
}

/// Extract the numeric value from a `JsValue` as `f64`, treating both
/// `int` and `number` NaN-box tags uniformly.
///
/// Internal helper — no spec equivalent.
fn numeric_value(val: JsValue) -> Option<f64> {
    if let Some(n) = val.as_int() {
        Some(n as f64)
    } else {
        val.as_number()
    }
}

/// `IsStrictlyEqual ( x, y )` — ECMAScript abstract operation.
///
/// Implements the strict equality comparison (`===`) algorithm.
///
/// [spec]: https://tc39.es/ecma262/#sec-isstrictlyequal
pub fn strict_eq(lhs: JsValue, rhs: JsValue) -> bool {
    // 1. If Type(x) is not Type(y), return false.
    // (Handled below: fast-path bit comparison + cross int/number check)

    // Fast path: identical bit patterns are equal (except NaN)
    if lhs.raw_bits() == rhs.raw_bits() {
        // 2. If x is a Number, then
        //   a. Return Number::equal(x, y).
        // NaN !== NaN: if both are NaN, return false
        if let Some(n) = lhs.as_number() {
            return !n.is_nan();
        }
        return true;
    }

    // Cross int/number comparison: int(3) === number(3.0)
    // 2. If x is a Number, then
    //   a. Return Number::equal(x, y).
    if let (Some(a), Some(b)) = (numeric_value(lhs), numeric_value(rhs))
        && (lhs.is_int() || lhs.is_number())
        && (rhs.is_int() || rhs.is_number())
    {
        return a == b;
    }

    // 3. Return SameValueNonNumber(x, y).
    // String comparison by content: two different RtString pointers
    // with the same text should be ===.
    if lhs.is_string() && rhs.is_string() {
        let a = string_ops::get_string_data(lhs);
        let b = string_ops::get_string_data(rhs);
        return a == b;
    }

    // Different types or different object references → false
    false
}

/// `IsLooselyEqual ( x, y )` — ECMAScript abstract operation.
///
/// Implements the abstract equality comparison (`==`) algorithm, which
/// performs type coercion before comparing.
///
/// [spec]: https://tc39.es/ecma262/#sec-islooselyequal
pub fn abstract_eq(lhs: JsValue, rhs: JsValue) -> bool {
    // 1. If Type(x) is Type(y), then
    //   a. Return IsStrictlyEqual(x, y).
    if lhs.same_type_tag(&rhs) {
        return strict_eq(lhs, rhs);
    }

    // int and number are both numeric — compare as numbers
    if (lhs.is_int() || lhs.is_number()) && (rhs.is_int() || rhs.is_number()) {
        return strict_eq(lhs, rhs);
    }

    // 2. If x is null and y is undefined, return true.
    // 3. If x is undefined and y is null, return true.
    if lhs.is_nullish() && rhs.is_nullish() {
        return true;
    }

    // null/undefined != anything else
    if lhs.is_nullish() || rhs.is_nullish() {
        return false;
    }

    // 4. NOTE: This step is replaced in section B.3.6.2 (document.all).
    // 5. If x is a Number and y is a String, return IsLooselyEqual(x, ToNumber(y)).
    if (lhs.is_int() || lhs.is_number()) && rhs.is_string() {
        let rn = to_number(rhs);
        let ln = to_number(lhs);
        return ln == rn;
    }
    // 6. If x is a String and y is a Number, return IsLooselyEqual(ToNumber(x), y).
    if lhs.is_string() && (rhs.is_int() || rhs.is_number()) {
        let ln = to_number(lhs);
        let rn = to_number(rhs);
        return ln == rn;
    }

    // 7. If x is a BigInt and y is a String, ... (TODO: BigInt not yet supported)
    // 8. If x is a String and y is a BigInt, ... (TODO: BigInt not yet supported)

    // 9. If x is a Boolean, return IsLooselyEqual(ToNumber(x), y).
    if lhs.is_bool() {
        let ln = JsValue::number(to_number(lhs));
        return abstract_eq(ln, rhs);
    }
    // 10. If y is a Boolean, return IsLooselyEqual(x, ToNumber(y)).
    if rhs.is_bool() {
        let rn = JsValue::number(to_number(rhs));
        return abstract_eq(lhs, rn);
    }

    // 11. If x is either a String, a Number, a BigInt, or a Symbol and y is an Object,
    //     return IsLooselyEqual(x, ToPrimitive(y)).
    if lhs.is_object() && !rhs.is_object() {
        let lp = to_primitive(lhs, ToPrimitiveHint::Default);
        return abstract_eq(lp, rhs);
    }
    // 12. If x is an Object and y is either a String, a Number, a BigInt, or a Symbol,
    //     return IsLooselyEqual(ToPrimitive(x), y).
    if rhs.is_object() && !lhs.is_object() {
        let rp = to_primitive(rhs, ToPrimitiveHint::Default);
        return abstract_eq(lhs, rp);
    }

    // Both objects: reference equality (same bit pattern)
    // 13. If x is a BigInt and y is a Number, ... (TODO: BigInt)
    // 14. If x is a Number and y is a BigInt, ... (TODO: BigInt)
    if lhs.is_object() && rhs.is_object() {
        return lhs.raw_bits() == rhs.raw_bits();
    }

    // 15. Return false.
    false
}

/// Abstract Relational Comparison helper.
///
/// Implements the shared logic of the `IsLessThan ( x, y, LeftFirst )`
/// abstract operation (ES2024 section 7.2.13), which is used by all four
/// relational operators (`<`, `>`, `<=`, `>=`).
///
/// Returns `(l, r, is_string)` where:
/// - If `is_string` is `true`, `l` encodes lexicographic order (-1/0/+1)
/// - If `is_string` is `false`, `l` and `r` are numeric values to compare
///
/// [spec]: https://tc39.es/ecma262/#sec-islessthan
fn abstract_relational(lhs: JsValue, rhs: JsValue) -> (f64, f64, bool) {
    // 1. If LeftFirst is true, then
    //   a. Let px be ? ToPrimitive(x, number).
    let lp = if lhs.is_object() {
        to_primitive(lhs, ToPrimitiveHint::Number)
    } else {
        lhs
    };
    //   b. Let py be ? ToPrimitive(y, number).
    let rp = if rhs.is_object() {
        to_primitive(rhs, ToPrimitiveHint::Number)
    } else {
        rhs
    };
    // 3. If px is a String and py is a String, then
    //   a. ... perform string comparison.
    if lp.is_string() && rp.is_string() {
        let ls = string_ops::get_string_data(lp);
        let rs = string_ops::get_string_data(rp);
        // Encode string comparison as f64: -1 for less, 0 for equal, 1 for greater
        let cmp = ls.cmp(&rs);
        let val = match cmp {
            std::cmp::Ordering::Less => -1.0,
            std::cmp::Ordering::Equal => 0.0,
            std::cmp::Ordering::Greater => 1.0,
        };
        return (val, 0.0, true);
    }
    // 4. Else,
    //   a. ... (BigInt cases — TODO: not yet supported)
    //   d. Let nx be ? ToNumber(px).
    //   e. Let ny be ? ToNumber(py).
    (to_number(lp), to_number(rp), false)
}

/// `IsLessThan ( x, y, true )` — implements the `<` relational operator.
///
/// [spec]: https://tc39.es/ecma262/#sec-islessthan
pub fn js_lt(lhs: JsValue, rhs: JsValue) -> bool {
    let (l, r, is_string) = abstract_relational(lhs, rhs);
    if is_string {
        return l < 0.0;
    }
    // If either is NaN, comparison returns undefined (treated as false)
    l < r
}

/// `IsLessThan ( y, x, false )` — implements the `>` relational operator.
///
/// `x > y` is defined as `IsLessThan(y, x, false)` per ES2024 section 13.10.
///
/// [spec]: https://tc39.es/ecma262/#sec-islessthan
pub fn js_gt(lhs: JsValue, rhs: JsValue) -> bool {
    let (l, r, is_string) = abstract_relational(lhs, rhs);
    if is_string {
        return l > 0.0;
    }
    l > r
}

/// `IsLessThan ( y, x, false )` negated — implements the `<=` relational operator.
///
/// `x <= y` is defined as `not IsLessThan(y, x, false)` per ES2024 section 13.10.
///
/// [spec]: https://tc39.es/ecma262/#sec-islessthan
pub fn js_le(lhs: JsValue, rhs: JsValue) -> bool {
    let (l, r, is_string) = abstract_relational(lhs, rhs);
    if is_string {
        return l <= 0.0;
    }
    l <= r
}

/// `IsLessThan ( x, y, true )` negated — implements the `>=` relational operator.
///
/// `x >= y` is defined as `not IsLessThan(x, y, true)` per ES2024 section 13.10.
///
/// [spec]: https://tc39.es/ecma262/#sec-islessthan
pub fn js_ge(lhs: JsValue, rhs: JsValue) -> bool {
    let (l, r, is_string) = abstract_relational(lhs, rhs);
    if is_string {
        return l >= 0.0;
    }
    l >= r
}

/// `SameValue ( x, y )` — ECMAScript abstract operation.
///
/// Determines whether two values are the same value. Differs from
/// strict equality (`===`) in two ways:
/// - `SameValue(NaN, NaN)` is `true`
/// - `SameValue(+0, -0)` is `false`
///
/// Used by `Object.is()`.
///
/// [spec]: https://tc39.es/ecma262/#sec-samevalue
pub fn same_value(a: JsValue, b: JsValue) -> bool {
    // 1. If Type(x) is not Type(y), return false.
    // 2. If x is a Number, then
    if let (Some(x), Some(y)) = (numeric_value(a), numeric_value(b))
        && (a.is_int() || a.is_number())
        && (b.is_int() || b.is_number())
    {
        //   a. Return Number::sameValue(x, y).
        // Number::sameValue:
        //   1. If x is NaN and y is NaN, return true.
        if x.is_nan() && y.is_nan() {
            return true;
        }
        //   2. If x is +0 and y is -0, return false.
        //   3. If x is -0 and y is +0, return false.
        if x == 0.0 && y == 0.0 {
            return x.to_bits() == y.to_bits();
        }
        //   4. If x is y, return true.
        //   5. Return false.
        return x == y;
    }
    // 3. Return SameValueNonNumber(x, y).
    strict_eq(a, b)
}

/// `SameValueZero ( x, y )` — ECMAScript abstract operation.
///
/// Like `SameValue` but treats `+0` and `-0` as equal:
/// - `SameValueZero(NaN, NaN)` is `true`
/// - `SameValueZero(+0, -0)` is `true`
///
/// Used by `Map`, `Set`, `Array.prototype.includes`.
///
/// [spec]: https://tc39.es/ecma262/#sec-samevaluezero
pub fn same_value_zero(a: JsValue, b: JsValue) -> bool {
    // 1. If Type(x) is not Type(y), return false.
    // 2. If x is a Number, then
    if let (Some(x), Some(y)) = (numeric_value(a), numeric_value(b))
        && (a.is_int() || a.is_number())
        && (b.is_int() || b.is_number())
    {
        //   a. Return Number::sameValueZero(x, y).
        // Number::sameValueZero:
        //   1. If x is NaN and y is NaN, return true.
        if x.is_nan() && y.is_nan() {
            return true;
        }
        //   2. If x is +0 and y is -0, return true.
        //   3. If x is -0 and y is +0, return true.
        //   4. If x is y, return true.
        //   5. Return false.
        // (+0 == -0 in Rust's f64 comparison, so steps 2-4 collapse)
        return x == y;
    }
    // 3. Return SameValueNonNumber(x, y).
    strict_eq(a, b)
}

/// `typeof` operator — ECMAScript runtime semantics.
///
/// Returns the ECMAScript type string for a value based on the NaN-box tag.
/// Notably, `typeof null === "object"` per the spec.
///
/// For objects, inspects the `UnifiedObject` to check if the object implements
/// `[[Call]]` (is callable), returning `"function"` for callable objects and
/// `"object"` for non-callable objects per ES2024 Table 41.
///
/// [spec]: https://tc39.es/ecma262/#sec-typeof-operator
pub fn js_typeof(val: JsValue) -> &'static str {
    // Table 41: typeof Operator Results
    if val.is_undefined() {
        // Undefined → "undefined"
        "undefined"
    } else if val.is_null() {
        // Null → "object" (historical, per spec)
        "object"
    } else if val.is_bool() {
        // Boolean → "boolean"
        "boolean"
    } else if val.is_number() || val.is_int() {
        // Number → "number"
        "number"
    } else if val.is_string() {
        // String → "string"
        "string"
    } else if val.is_symbol() {
        // Symbol → "symbol"
        "symbol"
    } else if val.is_object() {
        // Object (implements [[Call]]) → "function"
        // Object (does not implement [[Call]]) → "object"
        let bits = val.raw_bits();
        let tag = read_obj_tag(bits);
        if tag == Some(ObjTag::Unified as u8) {
            // SAFETY: tag check confirms this is a unified object.
            let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
            if let Some(u) = uni
                && u.is_callable()
            {
                return "function";
            }
        }
        "object"
    } else {
        "undefined"
    }
}

/// Check if a property key is an ECMAScript integer index.
///
/// An integer index is a canonical numeric String value (ES2024 section 6.1.7)
/// whose numeric value is a non-negative integer in the range `0..2^32 - 2`.
/// The value `2^32 - 1` (4294967295) is NOT a valid array index (it is the
/// maximum array length, not a valid index).
///
/// Returns `Some(n)` if the key is an integer index, `None` otherwise.
///
/// [spec]: https://tc39.es/ecma262/#integer-index
pub fn is_integer_index(key: &str) -> Option<u32> {
    if key.is_empty() {
        return None;
    }
    // Reject leading zeros (except "0" itself) — "01" is not a canonical numeric string
    if key.len() > 1 && key.starts_with('0') {
        return None;
    }
    let n: u32 = key.parse().ok()?;
    // u32::MAX (4294967295) is NOT a valid array index per spec
    if n == u32::MAX {
        return None;
    }
    Some(n)
}

/// Sort property keys according to ECMAScript `OwnPropertyKeys` enumeration order.
///
/// Per ES2024 section 10.1.11.1 `OrdinaryOwnPropertyKeys`, own property
/// enumeration order is:
/// 1. Integer indices (array indices) in ascending numeric order
/// 2. String keys in property creation order
/// 3. Symbol keys in property creation order (not handled here)
///
/// Each key is a `(name, offset)` pair where `offset` represents creation order.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinaryownpropertykeys
pub fn sort_keys_spec_order(keys: &mut [(String, u32)]) {
    keys.sort_by(|(name_a, offset_a), (name_b, offset_b)| {
        let idx_a = is_integer_index(name_a);
        let idx_b = is_integer_index(name_b);
        match (idx_a, idx_b) {
            // Both integer indices: sort numerically
            (Some(a), Some(b)) => a.cmp(&b),
            // Integer index comes before string key
            (Some(_), None) => std::cmp::Ordering::Less,
            // String key comes after integer index
            (None, Some(_)) => std::cmp::Ordering::Greater,
            // Both string keys: preserve insertion order (by offset)
            (None, None) => offset_a.cmp(offset_b),
        }
    });
}
