//! JsValue display formatting for console output and string coercion.
//!
//! Converts NaN-boxed `JsValue` instances to human-readable strings,
//! following JavaScript conventions (e.g., `typeof null === "object"`,
//! integer-like floats without trailing `.0`).

use nanbox::JsValue;

use crate::internal_data::{InternalData, InternalKind, UnifiedObject};
use crate::tagged_obj::{ObjTag, read_obj_tag};

/// Format a `JsValue` as a string for console output.
///
/// Follows JavaScript display conventions:
/// - `undefined` → `"undefined"`
/// - `null` → `"null"`
/// - Booleans → `"true"` / `"false"`
/// - Integers → decimal with no fraction
/// - Numbers → JS-like formatting (no trailing `.0` for whole numbers)
/// - `NaN` → `"NaN"`, infinities → `"Infinity"` / `"-Infinity"`
/// - Strings → the string data
/// - Objects → `"[object Object]"` (arrays and functions have special display)
/// - Symbols → `"Symbol()"`
pub fn display_value(val: JsValue) -> String {
    if val.is_undefined() {
        return "undefined".to_string();
    }
    if val.is_null() {
        return "null".to_string();
    }
    if let Some(b) = val.as_bool() {
        return b.to_string();
    }
    if let Some(n) = val.as_int() {
        return n.to_string();
    }
    if let Some(n) = val.as_number() {
        return display_number(n);
    }
    if val.is_string() {
        return crate::string_ops::get_string_data(val);
    }
    if val.is_object() {
        // Try unwrapping wrapper objects first (BooleanObj, NumberObj, StringObj)
        let unwrapped = crate::rt_api::unwrap_wrapper_object(val.raw_bits());
        if unwrapped != val.raw_bits() {
            return display_value(JsValue::from_raw_bits(unwrapped));
        }
        let tag = read_obj_tag(val.raw_bits());
        // Unified objects with special toString behavior
        if tag == Some(ObjTag::Unified as u8) {
            let uni = unsafe {
                // SAFETY: tag check confirms this is a unified object.
                crate::tagged_obj::deref_tagged::<UnifiedObject>(val.raw_bits())
            };
            if let Some(u) = uni {
                if u.kind == InternalKind::ErrorObj
                    && let Some(InternalData::Error { message, .. }) = u.internal_data()
                {
                    return crate::string_ops::get_string_data(JsValue::from_raw_bits(*message));
                }
                if u.kind == InternalKind::RegExpObj
                    && let Some(InternalData::RegExp { inner }) = u.internal_data()
                    && let Some(re) = inner.downcast_ref::<crate::regexp_bridge::JsRegExpData>()
                {
                    return format!("/{}/{}", re.inner.pattern, re.flags_string());
                }
            }
        }
        return "[object Object]".to_string();
    }
    if let Some(id) = val.as_symbol() {
        return crate::symbol::symbol_to_string(id);
    }
    "undefined".to_string()
}

/// Format a number following JavaScript conventions.
///
/// - `NaN` → `"NaN"`
/// - `Infinity` → `"Infinity"` / `"-Infinity"`
/// - Integer-valued floats → no trailing `.0`
/// - Very large (>=1e21) or very small (<1e-6) → scientific notation
/// - Other floats → standard decimal representation matching JS output
pub fn display_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    if n == 0.0 {
        return "0".to_string();
    }
    // Use JS-like formatting: no trailing .0 for integer-valued floats.
    // JS uses integer notation for values < 1e21 (values >= 1e21 get
    // exponential notation in format_js_number).
    if n == n.trunc() && n.abs() < 1e21 {
        // Format with zero decimal places to get the integer representation.
        // Using {:.0} avoids issues with i64/u64 overflow for large values.
        return format!("{:.0}", n);
    }
    // JavaScript uses exponential notation for very large or very small numbers.
    // Use Rust's {:e} formatting then convert to JS-style: "1.5e+100" → "1.5e+100"
    // JS uses lowercase 'e' with explicit '+' for positive exponents.
    format_js_number(n)
}

/// Format a float using JavaScript's `Number.prototype.toString()` algorithm.
///
/// JavaScript displays numbers using the shortest representation that
/// round-trips back to the same `f64` value, with exponential notation
/// for magnitudes >= 1e21 or when the base-10 exponent is <= -7
/// (i.e., the number would need more than 6 leading zeros after the
/// decimal point).
fn format_js_number(n: f64) -> String {
    let abs = n.abs();
    let negative = n < 0.0;
    let prefix = if negative { "-" } else { "" };

    // Compute the base-10 exponent for scientific notation threshold.
    // JS uses exponential notation when:
    //   - abs >= 1e21 (exponent >= 21)
    //   - exponent <= -7 (more than 6 leading zeros after decimal)
    let use_scientific = if abs >= 1e21 {
        true
    } else if abs > 0.0 {
        // floor(log10(abs)) gives the exponent e where abs = m * 10^e, 1 <= m < 10
        let exp = abs.log10().floor() as i32;
        exp <= -7
    } else {
        false
    };

    if use_scientific {
        // Use Rust's {:e} then fix up to match JS formatting.
        let s = format!("{abs:e}");
        if let Some(pos) = s.find('e') {
            let (mantissa, exp_part) = s.split_at(pos);
            let exp_str = &exp_part[1..]; // skip 'e'
            let exp_val: i32 = exp_str.parse().unwrap_or_default();
            let mantissa = clean_mantissa(mantissa);
            // Use e+ for positive exponents, e- for negative.
            let sign = if exp_val >= 0 { "+" } else { "" };
            return format!("{prefix}{mantissa}e{sign}{exp_val}");
        }
        return format!("{prefix}{s}");
    }

    // For normal range numbers, Rust's `{}` format produces the shortest
    // decimal representation that round-trips for f64.
    let s = format!("{abs}");
    format!("{prefix}{s}")
}

/// Remove trailing zeros after the decimal point in a mantissa string.
///
/// Preserves at least one digit after the decimal point if one exists.
/// If the mantissa has no decimal point, returns it unchanged.
fn clean_mantissa(s: &str) -> &str {
    if let Some(dot) = s.find('.') {
        let trimmed = s.trim_end_matches('0');
        // If trimming removed everything after the dot, keep just the integer part
        if trimmed.len() == dot + 1 {
            &s[..dot]
        } else {
            trimmed
        }
    } else {
        s
    }
}
