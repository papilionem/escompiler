//! Builtin function registry mapping names to native function implementations.
//!
//! The [`BuiltinRegistry`] provides a central lookup table for all built-in
//! JavaScript functions that are available to compiled code. The Cranelift and
//! LLVM backends use this to resolve calls to global functions like `isNaN`,
//! `parseInt`, etc.

use std::collections::HashMap;

use nanbox::JsValue;

/// Type alias for a native builtin function.
pub type NativeFn = fn(&[JsValue]) -> JsValue;

/// Registry of built-in functions available to compiled JavaScript code.
///
/// Maps function names (e.g. `"isNaN"`, `"parseInt"`) to their native Rust
/// implementations. The codegen backends look up function addresses from this
/// registry when lowering IR call instructions.
pub struct BuiltinRegistry {
    builtins: HashMap<String, NativeFn>,
}

impl BuiltinRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            builtins: HashMap::new(),
        }
    }

    /// Create a registry pre-populated with standard builtins.
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();
        reg.register("isNaN", builtin_is_nan);
        reg.register("isFinite", builtin_is_finite);
        reg.register("parseInt", builtin_parse_int);
        reg.register("parseFloat", builtin_parse_float);
        reg.register("encodeURIComponent", builtin_encode_uri_component);
        reg.register("decodeURIComponent", builtin_decode_uri_component);
        reg
    }

    /// Register a builtin function.
    pub fn register(&mut self, name: &str, func: NativeFn) {
        self.builtins.insert(name.to_string(), func);
    }

    /// Look up a builtin by name.
    pub fn get(&self, name: &str) -> Option<&NativeFn> {
        self.builtins.get(name)
    }

    /// Returns true if a builtin with the given name exists.
    pub fn contains(&self, name: &str) -> bool {
        self.builtins.contains_key(name)
    }

    /// Returns the number of registered builtins.
    pub fn len(&self) -> usize {
        self.builtins.len()
    }

    /// Returns true if no builtins are registered.
    pub fn is_empty(&self) -> bool {
        self.builtins.is_empty()
    }
}

impl Default for BuiltinRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

// === String extraction helpers ===

use runtime::string_ops::RtString;

/// Extract string data from a JsValue.
fn extract_string(val: &JsValue) -> Option<String> {
    if let Some(ptr) = val.as_string() {
        if ptr.is_null() {
            return Some(String::new());
        }
        let rt_str = unsafe {
            // SAFETY: ptr was created by string_from_data or make_string
            &*(ptr as *const RtString)
        };
        Some(rt_str.as_str().to_string())
    } else {
        None
    }
}

/// Create a new string JsValue from a Rust String.
fn make_string(s: String) -> JsValue {
    let rt_str = Box::new(RtString::new(s));
    let raw_ptr = Box::into_raw(rt_str) as *const ();
    JsValue::string(raw_ptr)
}

// === Global functions ===

/// Implementation of the global `isNaN()` function.
///
/// Coerces the argument to a number and returns true if the result is NaN.
fn builtin_is_nan(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    if let Some(n) = val.as_number() {
        JsValue::bool(n.is_nan())
    } else if val.is_undefined() {
        JsValue::bool(true)
    } else {
        JsValue::bool(false)
    }
}

/// Implementation of the global `isFinite()` function.
///
/// Coerces the argument to a number and returns true if finite.
fn builtin_is_finite(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    if let Some(n) = val.as_number() {
        JsValue::bool(n.is_finite())
    } else if val.is_int() {
        JsValue::bool(true)
    } else {
        JsValue::bool(false)
    }
}

/// Implementation of the global `parseInt(string, radix)` function.
///
/// Full spec: handles sign, `0x` prefix for hex, and radix 2-36.
/// Accepts both numeric and string arguments.
fn builtin_parse_int(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    let radix_arg = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)));

    // Fast path for numeric inputs
    if let Some(n) = val.as_int() {
        return JsValue::int(n);
    }
    if let Some(n) = val.as_number() {
        if n.is_finite() {
            return JsValue::number(n.trunc());
        }
        return JsValue::number(f64::NAN);
    }

    // String input path
    let text = match extract_string(&val) {
        Some(s) => s,
        None => return JsValue::number(f64::NAN),
    };

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return JsValue::number(f64::NAN);
    }

    let mut chars = trimmed.as_bytes();
    let negative = if chars.first() == Some(&b'-') {
        chars = &chars[1..];
        true
    } else if chars.first() == Some(&b'+') {
        chars = &chars[1..];
        false
    } else {
        false
    };

    let radix = if let Some(r) = radix_arg {
        if !(2..=36).contains(&r) {
            return JsValue::number(f64::NAN);
        }
        // Handle 0x prefix for radix 16
        if r == 16 && chars.len() >= 2 && chars[0] == b'0' && (chars[1] == b'x' || chars[1] == b'X')
        {
            chars = &chars[2..];
        }
        r as u32
    } else {
        // Auto-detect radix
        if chars.len() >= 2 && chars[0] == b'0' && (chars[1] == b'x' || chars[1] == b'X') {
            chars = &chars[2..];
            16
        } else {
            10
        }
    };

    // Parse digits
    let mut result: f64 = 0.0;
    let mut found_digit = false;
    for &ch in chars {
        let digit = match ch {
            b'0'..=b'9' => (ch - b'0') as u32,
            b'a'..=b'z' => (ch - b'a' + 10) as u32,
            b'A'..=b'Z' => (ch - b'A' + 10) as u32,
            _ => break,
        };
        if digit >= radix {
            break;
        }
        found_digit = true;
        result = result * (radix as f64) + (digit as f64);
    }

    if !found_digit {
        return JsValue::number(f64::NAN);
    }

    if negative {
        result = -result;
    }
    JsValue::number(result)
}

/// Implementation of the global `parseFloat(string)` function.
///
/// Parses a string argument and returns a floating point number.
fn builtin_parse_float(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);

    if let Some(n) = val.as_number() {
        return JsValue::number(n);
    }
    if let Some(n) = val.as_int() {
        return JsValue::number(n as f64);
    }

    let text = match extract_string(&val) {
        Some(s) => s,
        None => return JsValue::number(f64::NAN),
    };

    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return JsValue::number(f64::NAN);
    }

    // Try parsing as many leading characters as form a valid float
    // Find the longest prefix that parses as f64
    let mut best: Option<f64> = None;
    let bytes = trimmed.as_bytes();
    let mut end = 0;

    // Handle sign
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    // Integer part
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    // Decimal part
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
    }
    // Exponent
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        end += 1;
        if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
            end += 1;
        }
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
    }

    if end > 0
        && let Ok(n) = trimmed[..end].parse::<f64>()
    {
        best = Some(n);
    }

    // Special: "Infinity"
    if trimmed.starts_with("Infinity") {
        best = Some(f64::INFINITY);
    } else if trimmed.starts_with("-Infinity") {
        best = Some(f64::NEG_INFINITY);
    }

    match best {
        Some(n) => JsValue::number(n),
        None => JsValue::number(f64::NAN),
    }
}

/// Implementation of `encodeURIComponent(str)`.
///
/// Percent-encodes all characters except unreserved characters:
/// `A-Z a-z 0-9 - _ . ! ~ * ' ( )`
fn builtin_encode_uri_component(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    let text = match extract_string(&val) {
        Some(s) => s,
        None => return make_string("undefined".to_string()),
    };

    let mut result = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    make_string(result)
}

/// Implementation of `decodeURIComponent(str)`.
///
/// Decodes percent-encoded characters in the string.
fn builtin_decode_uri_component(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    let text = match extract_string(&val) {
        Some(s) => s,
        None => return make_string("undefined".to_string()),
    };

    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(byte_val) = u8::from_str_radix(hex_str, 16)
        {
            result.push(byte_val);
            i += 3;
            continue;
        }
        result.push(bytes[i]);
        i += 1;
    }

    match String::from_utf8(result) {
        Ok(s) => make_string(s),
        Err(_) => make_string(String::new()),
    }
}
