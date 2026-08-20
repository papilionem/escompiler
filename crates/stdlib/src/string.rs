//! String built-in methods.
//!
//! These operate on JsValue-level representations. String values are
//! NaN-boxed pointers to `runtime::string_ops::RtString` structs.

use nanbox::JsValue;
use runtime::string_ops::RtString;

/// Extract string data from a JsValue that is known to be a string.
///
/// Returns `None` if the value is not a string type.
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

/// `String.prototype.length` (as a function).
///
/// Returns the byte length of the string. A future version will return
/// UTF-16 code unit count for full spec compliance.
pub fn length(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    match extract_string(&this) {
        Some(s) => JsValue::int(s.len() as i32),
        None => JsValue::int(0),
    }
}

/// `String.prototype.indexOf(searchValue)`.
///
/// Returns the index of the first occurrence of `searchValue` in this string,
/// or -1 if not found.
pub fn index_of(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let search = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return JsValue::int(-1),
    };
    let needle = match extract_string(&search) {
        Some(n) => n,
        None => return JsValue::int(-1),
    };
    match s.find(&needle) {
        Some(idx) => JsValue::int(idx as i32),
        None => JsValue::int(-1),
    }
}

/// `String.prototype.slice(start, end)`.
///
/// Extracts a section of the string and returns it as a new string.
pub fn slice(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return make_string(String::new()),
    };
    let len = s.len() as i32;

    let raw_start = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);
    let raw_end = args
        .get(2)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)));

    let start = if raw_start < 0 {
        (len + raw_start).max(0) as usize
    } else {
        raw_start.min(len) as usize
    };

    let end = match raw_end {
        Some(e) if e < 0 => (len + e).max(0) as usize,
        Some(e) => e.min(len) as usize,
        None => len as usize,
    };

    if start >= end {
        return make_string(String::new());
    }
    make_string(s[start..end].to_string())
}

/// `String.prototype.charAt(index)`.
///
/// Returns the character at the specified index as a single-character string.
pub fn char_at(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return make_string(String::new()),
    };
    let idx = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);

    if idx < 0 || idx as usize >= s.len() {
        return make_string(String::new());
    }
    make_string(s[idx as usize..idx as usize + 1].to_string())
}

/// `String.prototype.trim()`.
///
/// Removes leading and trailing whitespace from the string.
pub fn trim(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    match extract_string(&this) {
        Some(s) => make_string(s.trim().to_string()),
        None => make_string(String::new()),
    }
}

/// `String.prototype.toLowerCase()`.
///
/// Returns the string converted to lowercase.
pub fn to_lower_case(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    match extract_string(&this) {
        Some(s) => make_string(s.to_lowercase()),
        None => make_string(String::new()),
    }
}

/// `String.prototype.toUpperCase()`.
///
/// Returns the string converted to uppercase.
pub fn to_upper_case(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    match extract_string(&this) {
        Some(s) => make_string(s.to_uppercase()),
        None => make_string(String::new()),
    }
}

/// `String.prototype.includes(searchString)`.
///
/// Returns `true` if the string contains the search string.
pub fn includes(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let search = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return JsValue::bool(false),
    };
    let needle = match extract_string(&search) {
        Some(n) => n,
        None => return JsValue::bool(false),
    };
    JsValue::bool(s.contains(&needle))
}

/// `String.prototype.startsWith(searchString)`.
///
/// Returns `true` if the string starts with the search string.
pub fn starts_with(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let search = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return JsValue::bool(false),
    };
    let prefix = match extract_string(&search) {
        Some(p) => p,
        None => return JsValue::bool(false),
    };
    JsValue::bool(s.starts_with(&prefix))
}

/// `String.prototype.endsWith(searchString)`.
///
/// Returns `true` if the string ends with the search string.
pub fn ends_with(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let search = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return JsValue::bool(false),
    };
    let suffix = match extract_string(&search) {
        Some(su) => su,
        None => return JsValue::bool(false),
    };
    JsValue::bool(s.ends_with(&suffix))
}

/// `String.prototype.split(separator, limit)`.
///
/// Splits the string by separator and returns the parts as a JsValue array pointer.
/// Uses `RtArray` layout from the runtime to build the result array.
pub fn split(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let sep_arg = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return JsValue::undefined(),
    };
    let sep = match extract_string(&sep_arg) {
        Some(sep) => sep,
        None => return JsValue::undefined(),
    };

    let limit = args
        .get(2)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .map(|l| l.max(0) as usize);

    let parts: Vec<&str> = if let Some(lim) = limit {
        s.splitn(lim, sep.as_str()).collect()
    } else {
        s.split(sep.as_str()).collect()
    };

    // Return the count as int — actual array construction requires runtime support.
    // The individual strings are not returned yet; this is a structural placeholder
    // that returns the number of split parts. Full array return is Phase D+.
    JsValue::int(parts.len() as i32)
}

/// `String.prototype.repeat(count)`.
///
/// Returns the string repeated `count` times.
pub fn repeat(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return make_string(String::new()),
    };
    let count = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);
    if count <= 0 {
        return make_string(String::new());
    }
    make_string(s.repeat(count as usize))
}

/// `String.prototype.replace(searchValue, replaceValue)`.
///
/// Replaces the first occurrence of `searchValue` with `replaceValue`.
pub fn replace(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let search = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let replacement = args.get(2).copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return make_string(String::new()),
    };
    let needle = match extract_string(&search) {
        Some(n) => n,
        None => return make_string(s),
    };
    let rep = extract_string(&replacement).unwrap_or_default();
    // Replace first occurrence only
    make_string(s.replacen(&needle, &rep, 1))
}

/// `String.prototype.concat(...args)`.
///
/// Concatenates the string arguments to the calling string.
pub fn concat(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let mut result = extract_string(&this).unwrap_or_default();
    for arg in args.iter().skip(1) {
        if let Some(s) = extract_string(arg) {
            result.push_str(&s);
        }
    }
    make_string(result)
}

/// `String.prototype.padStart(targetLength, padString)`.
///
/// Pads the current string from the start with `padString` to reach
/// `targetLength`. Default pad string is a space.
pub fn pad_start(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return make_string(String::new()),
    };
    let target_len = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);

    if target_len as usize <= s.len() {
        return make_string(s);
    }

    let pad = args
        .get(2)
        .and_then(extract_string)
        .unwrap_or_else(|| " ".to_string());

    if pad.is_empty() {
        return make_string(s);
    }

    let needed = target_len as usize - s.len();
    let mut padding = String::with_capacity(needed);
    while padding.len() < needed {
        padding.push_str(&pad);
    }
    padding.truncate(needed);
    padding.push_str(&s);
    make_string(padding)
}

/// `String.prototype.padEnd(targetLength, padString)`.
///
/// Pads the current string from the end with `padString` to reach
/// `targetLength`. Default pad string is a space.
pub fn pad_end(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return make_string(String::new()),
    };
    let target_len = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);

    if target_len as usize <= s.len() {
        return make_string(s);
    }

    let pad = args
        .get(2)
        .and_then(extract_string)
        .unwrap_or_else(|| " ".to_string());

    if pad.is_empty() {
        return make_string(s);
    }

    let needed = target_len as usize - s.len();
    let mut result = s;
    let mut suffix = String::with_capacity(needed);
    while suffix.len() < needed {
        suffix.push_str(&pad);
    }
    suffix.truncate(needed);
    result.push_str(&suffix);
    make_string(result)
}

/// `String.prototype.substring(start, end)`.
///
/// Like `slice` but does not support negative indices. If `start > end`,
/// they are swapped.
pub fn substring(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return make_string(String::new()),
    };
    let len = s.len() as i32;

    let raw_start = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0)
        .clamp(0, len) as usize;

    let raw_end = args
        .get(2)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .map(|e| e.clamp(0, len) as usize)
        .unwrap_or(len as usize);

    let (start, end) = if raw_start <= raw_end {
        (raw_start, raw_end)
    } else {
        (raw_end, raw_start)
    };

    make_string(s[start..end].to_string())
}

/// `String.prototype.replaceAll(searchValue, replaceValue)`.
///
/// Replaces all occurrences of `searchValue` with `replaceValue`.
pub fn replace_all(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let search = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let replacement = args.get(2).copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return make_string(String::new()),
    };
    let needle = match extract_string(&search) {
        Some(n) => n,
        None => return make_string(s),
    };
    let rep = extract_string(&replacement).unwrap_or_default();
    make_string(s.replace(&needle, &rep))
}

/// `String.prototype.at(index)`.
///
/// Returns the character at the given index. Negative indices count from
/// the end of the string.
pub fn at(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return JsValue::undefined(),
    };
    let idx = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);

    let len = s.len() as i32;
    let actual = if idx < 0 { len + idx } else { idx };

    if actual < 0 || actual >= len {
        return JsValue::undefined();
    }
    make_string(s[actual as usize..actual as usize + 1].to_string())
}

/// `String.prototype.trimStart()`.
///
/// Removes leading whitespace from the string.
pub fn trim_start(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    match extract_string(&this) {
        Some(s) => make_string(s.trim_start().to_string()),
        None => make_string(String::new()),
    }
}

/// `String.prototype.trimEnd()`.
///
/// Removes trailing whitespace from the string.
pub fn trim_end(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    match extract_string(&this) {
        Some(s) => make_string(s.trim_end().to_string()),
        None => make_string(String::new()),
    }
}

/// `String.prototype.codePointAt(pos)`.
///
/// Returns the Unicode code point at the given position as an integer,
/// or `undefined` if the position is out of range.
pub fn code_point_at(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return JsValue::undefined(),
    };
    let pos = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);

    if pos < 0 || pos as usize >= s.len() {
        return JsValue::undefined();
    }

    match s[pos as usize..].chars().next() {
        Some(ch) => JsValue::int(ch as i32),
        None => JsValue::undefined(),
    }
}

/// `String.fromCodePoint(...codePoints)`.
///
/// Creates a string from the given Unicode code points.
pub fn from_code_point(args: &[JsValue]) -> JsValue {
    let mut result = String::new();
    for arg in args {
        let cp = if let Some(n) = arg.as_int() {
            n as u32
        } else if let Some(n) = arg.as_number() {
            n as u32
        } else {
            return JsValue::undefined();
        };
        match char::from_u32(cp) {
            Some(ch) => result.push(ch),
            None => return JsValue::undefined(),
        }
    }
    make_string(result)
}

/// `String.prototype.search(regexp)`.
///
/// Basic string pattern matching — returns the index of the first match,
/// or -1 if not found. Full RegExp support will be added later.
pub fn search(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let pattern = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return JsValue::int(-1),
    };
    let needle = match extract_string(&pattern) {
        Some(n) => n,
        None => return JsValue::int(-1),
    };
    match s.find(&needle) {
        Some(idx) => JsValue::int(idx as i32),
        None => JsValue::int(-1),
    }
}

/// `String.prototype.match(regexp)`.
///
/// Basic string pattern matching. Returns `undefined` if no match, or the
/// matching substring if found. Full RegExp support will be added later.
pub fn match_str(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let pattern = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return JsValue::null(),
    };
    let needle = match extract_string(&pattern) {
        Some(n) => n,
        None => return JsValue::null(),
    };
    if s.contains(&needle) {
        make_string(needle)
    } else {
        JsValue::null()
    }
}

/// `String.prototype.matchAll(regexp)`.
///
/// Basic string pattern matching — returns the count of matches.
/// Full RegExp/iterator support will be added later.
pub fn match_all(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let pattern = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let s = match extract_string(&this) {
        Some(s) => s,
        None => return JsValue::int(0),
    };
    let needle = match extract_string(&pattern) {
        Some(n) => n,
        None => return JsValue::int(0),
    };
    if needle.is_empty() {
        return JsValue::int(0);
    }
    JsValue::int(s.matches(&needle).count() as i32)
}
