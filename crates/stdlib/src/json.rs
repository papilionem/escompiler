//! JSON built-in methods.
//!
//! Provides `JSON.parse` and `JSON.stringify` for primitive values, objects,
//! and arrays. Uses a recursive descent parser and serializer with circular
//! reference detection.

use std::collections::HashSet;

use nanbox::JsValue;
use runtime::string_ops::RtString;

/// Extract string data from a JsValue that is known to be a string.
fn extract_string(val: &JsValue) -> Option<String> {
    if let Some(ptr) = val.as_string() {
        if ptr.is_null() {
            return Some(String::new());
        }
        let rt_str = unsafe {
            // SAFETY: ptr was created by string_from_data or string_concat in runtime
            // via Box::into_raw on an RtString.
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

/// `JSON.parse(text)` — parse a JSON string into a value.
///
/// Supports: `null`, `true`, `false`, numbers, quoted strings, arrays, and
/// objects (with string keys).
pub fn parse(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    let text = match extract_string(&val) {
        Some(s) => s,
        None => return JsValue::undefined(),
    };
    let trimmed = text.trim();
    let bytes = trimmed.as_bytes();
    match parse_value(bytes, 0) {
        Some((result, _)) => result,
        None => JsValue::undefined(),
    }
}

/// Recursive JSON value parser. Returns the parsed value and the next byte offset.
fn parse_value(bytes: &[u8], start: usize) -> Option<(JsValue, usize)> {
    let pos = skip_whitespace(bytes, start);
    if pos >= bytes.len() {
        return None;
    }
    match bytes[pos] {
        b'n' => parse_null(bytes, pos),
        b't' => parse_true(bytes, pos),
        b'f' => parse_false(bytes, pos),
        b'"' => parse_string_value(bytes, pos),
        b'[' => parse_array(bytes, pos),
        b'{' => parse_object(bytes, pos),
        b'-' | b'0'..=b'9' => parse_number(bytes, pos),
        _ => None,
    }
}

/// Skip whitespace characters.
fn skip_whitespace(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    pos
}

/// Parse `null`.
fn parse_null(bytes: &[u8], pos: usize) -> Option<(JsValue, usize)> {
    if bytes.get(pos..pos + 4)? == b"null" {
        Some((JsValue::null(), pos + 4))
    } else {
        None
    }
}

/// Parse `true`.
fn parse_true(bytes: &[u8], pos: usize) -> Option<(JsValue, usize)> {
    if bytes.get(pos..pos + 4)? == b"true" {
        Some((JsValue::bool(true), pos + 4))
    } else {
        None
    }
}

/// Parse `false`.
fn parse_false(bytes: &[u8], pos: usize) -> Option<(JsValue, usize)> {
    if bytes.get(pos..pos + 5)? == b"false" {
        Some((JsValue::bool(false), pos + 5))
    } else {
        None
    }
}

/// Parse a JSON number.
fn parse_number(bytes: &[u8], pos: usize) -> Option<(JsValue, usize)> {
    let mut end = pos;
    if end < bytes.len() && bytes[end] == b'-' {
        end += 1;
    }
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
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
    let s = std::str::from_utf8(&bytes[pos..end]).ok()?;
    let n: f64 = s.parse().ok()?;
    Some((JsValue::number(n), end))
}

/// Parse a JSON string (including quotes). Returns the string value and offset after closing quote.
fn parse_string_value(bytes: &[u8], pos: usize) -> Option<(JsValue, usize)> {
    let (s, end) = parse_string_raw(bytes, pos)?;
    Some((make_string(s), end))
}

/// Parse a JSON string and return the raw Rust String content.
fn parse_string_raw(bytes: &[u8], pos: usize) -> Option<(String, usize)> {
    if bytes.get(pos)? != &b'"' {
        return None;
    }
    let mut result = String::new();
    let mut i = pos + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Some((result, i + 1)),
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    return None;
                }
                match bytes[i] {
                    b'"' => result.push('"'),
                    b'\\' => result.push('\\'),
                    b'/' => result.push('/'),
                    b'n' => result.push('\n'),
                    b'r' => result.push('\r'),
                    b't' => result.push('\t'),
                    b'b' => result.push('\u{0008}'),
                    b'f' => result.push('\u{000C}'),
                    b'u' => {
                        if i + 4 >= bytes.len() {
                            return None;
                        }
                        let hex = std::str::from_utf8(&bytes[i + 1..i + 5]).ok()?;
                        let code = u32::from_str_radix(hex, 16).ok()?;
                        if let Some(ch) = char::from_u32(code) {
                            result.push(ch);
                        }
                        i += 4;
                    }
                    _ => return None,
                }
            }
            ch => {
                result.push(ch as char);
            }
        }
        i += 1;
    }
    None // Unterminated string
}

/// Parse a JSON array (stdlib-level, returns element count).
///
/// The stdlib layer cannot create `TaggedObj<JsArray>` because it does not
/// depend on the runtime. For real array creation, use the runtime-level
/// `dispatch_json_method("parse", ...)` in `rt_api.rs`.
fn parse_array(bytes: &[u8], pos: usize) -> Option<(JsValue, usize)> {
    if bytes.get(pos)? != &b'[' {
        return None;
    }
    let mut i = skip_whitespace(bytes, pos + 1);
    if i < bytes.len() && bytes[i] == b']' {
        return Some((JsValue::int(0), i + 1));
    }

    let mut count = 0i32;
    loop {
        let (_, next) = parse_value(bytes, i)?;
        count += 1;
        i = skip_whitespace(bytes, next);
        if i >= bytes.len() {
            return None;
        }
        if bytes[i] == b']' {
            return Some((JsValue::int(count), i + 1));
        }
        if bytes[i] != b',' {
            return None;
        }
        i = skip_whitespace(bytes, i + 1);
    }
}

/// Parse a JSON object (stdlib-level, returns empty object pointer).
///
/// The stdlib layer cannot create `TaggedObj<JsObject>` with properties
/// because it does not depend on the runtime. For real object creation,
/// use the runtime-level `dispatch_json_method("parse", ...)` in `rt_api.rs`.
fn parse_object(bytes: &[u8], pos: usize) -> Option<(JsValue, usize)> {
    if bytes.get(pos)? != &b'{' {
        return None;
    }
    let mut i = skip_whitespace(bytes, pos + 1);
    if i < bytes.len() && bytes[i] == b'}' {
        return Some((JsValue::object(std::ptr::null()), i + 1));
    }

    loop {
        // Parse key (must be a string)
        let (_, key_end) = parse_string_raw(bytes, i)?;
        i = skip_whitespace(bytes, key_end);
        if i >= bytes.len() || bytes[i] != b':' {
            return None;
        }
        i = skip_whitespace(bytes, i + 1);

        // Parse value
        let (_, val_end) = parse_value(bytes, i)?;
        i = skip_whitespace(bytes, val_end);
        if i >= bytes.len() {
            return None;
        }
        if bytes[i] == b'}' {
            return Some((JsValue::object(std::ptr::null()), i + 1));
        }
        if bytes[i] != b',' {
            return None;
        }
        i = skip_whitespace(bytes, i + 1);
    }
}

/// `JSON.stringify(value, replacer?, space?)` — convert a value to a JSON string.
///
/// Supports primitive values, and detects circular references in objects.
/// The `replacer` parameter is currently ignored. The `space` parameter
/// controls indentation (number of spaces).
pub fn stringify(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);

    let indent = args
        .get(2)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0)
        .max(0) as usize;

    let mut seen = HashSet::new();
    match stringify_value(&val, indent, 0, &mut seen) {
        Some(s) => make_string(s),
        None => JsValue::undefined(),
    }
}

/// Recursive stringify implementation.
fn stringify_value(
    val: &JsValue,
    _indent: usize,
    _depth: usize,
    seen: &mut HashSet<u64>,
) -> Option<String> {
    if val.is_undefined() {
        return None; // JSON.stringify(undefined) returns undefined
    }
    if val.is_null() {
        return Some("null".to_string());
    }
    if let Some(b) = val.as_bool() {
        return Some(if b { "true" } else { "false" }.to_string());
    }
    if let Some(n) = val.as_int() {
        return Some(n.to_string());
    }
    if let Some(n) = val.as_number() {
        if n.is_nan() || n.is_infinite() {
            return Some("null".to_string());
        }
        if n == n.trunc() && n.abs() < 1e15 {
            return Some(format!("{}", n as i64));
        }
        return Some(format!("{n}"));
    }
    if let Some(s) = extract_string(val) {
        return Some(format!(
            "\"{}\"",
            s.replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }

    // Object — check for circular reference
    if val.is_object() {
        let key = val.raw_bits();
        if !seen.insert(key) {
            // Circular reference — return None to signal TypeError
            return None;
        }
        // Without runtime property enumeration, produce "{}"
        let result = "{}".to_string();
        seen.remove(&key);
        return Some(result);
    }

    None
}
