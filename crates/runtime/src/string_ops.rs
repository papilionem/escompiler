//! Runtime string operations for NaN-boxed string values.
//!
//! Provides a simple heap-allocated `RtString` and functions to create,
//! concatenate, and extract string data from NaN-boxed `JsValue` pointers.
//! In the final implementation, these will integrate with `strings::JsString`.
//!
//! All index-based operations use UTF-16 code unit indices, matching
//! ECMAScript semantics where `String.prototype.length` returns the number
//! of 16-bit code units, not bytes or Unicode codepoints.
//!
//! ## Spec References
//!
//! - String type: <https://tc39.es/ecma262/#sec-ecmascript-language-types-string-type> (§6.1.4)
//! - ToString: <https://tc39.es/ecma262/#sec-tostring> (§7.1.17)
//! - String concatenation (+ operator): <https://tc39.es/ecma262/#sec-addition-operator-plus> (§13.15.3)
//! - String.prototype.indexOf: <https://tc39.es/ecma262/#sec-string.prototype.indexof> (§22.1.3.9)
//! - String.prototype.lastIndexOf: <https://tc39.es/ecma262/#sec-string.prototype.lastindexof> (§22.1.3.10)
//! - String.prototype.slice: <https://tc39.es/ecma262/#sec-string.prototype.slice> (§22.1.3.22)
//! - String.prototype.charCodeAt: <https://tc39.es/ecma262/#sec-string.prototype.charcodeat> (§22.1.3.2)

use nanbox::JsValue;

/// A simple heap-allocated runtime string.
///
/// Represents a String value as defined in §6.1.4 of the ECMAScript spec:
/// a finite ordered sequence of zero or more 16-bit unsigned integer values
/// ("elements"), up to a maximum length of 2^53 - 1.
///
/// In the final implementation, this will use `strings::JsString`
/// with Latin1/UTF-16 dual encoding. For now, uses a standard Rust `String`
/// (UTF-8 internally, with UTF-16 conversion for spec-compliant operations).
///
/// [spec]: https://tc39.es/ecma262/#sec-ecmascript-language-types-string-type (§6.1.4)
pub struct RtString {
    data: String,
}

impl RtString {
    /// Creates a new runtime string from a Rust `String`.
    pub fn new(s: String) -> Self {
        Self { data: s }
    }

    /// Returns a reference to the underlying string data.
    pub fn as_str(&self) -> &str {
        &self.data
    }

    /// Returns the byte length of the string (Rust/UTF-8).
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }

    /// Returns the length in UTF-16 code units (ECMAScript `.length`).
    ///
    /// This implements the `length` property of String objects (§10.4.3.3):
    /// the number of elements (16-bit code units) in the String value.
    ///
    /// For ASCII/Latin1 strings this equals the byte length. For strings
    /// containing supplementary characters (emoji, CJK supplementary, etc.),
    /// each such character counts as 2 UTF-16 code units (a surrogate pair).
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-properties-of-string-instances-length (§10.4.3.3)
    pub fn utf16_len(&self) -> usize {
        self.data.encode_utf16().count()
    }

    /// Returns `true` if the string is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

// ---------------------------------------------------------------------------
// UTF-16 ↔ byte index helpers
// ---------------------------------------------------------------------------

/// Convert a UTF-16 code unit index to a UTF-8 byte index within `s`.
///
/// ECMAScript strings are sequences of UTF-16 code units (§6.1.4), but this
/// runtime stores strings as UTF-8. This helper maps from spec-level indices
/// to internal byte indices.
///
/// Returns `None` if `utf16_idx` is out of range or points to the second
/// half of a surrogate pair (which has no corresponding byte boundary).
pub fn utf16_index_to_byte(s: &str, utf16_idx: usize) -> Option<usize> {
    let mut cu_pos: usize = 0;
    for (bi, ch) in s.char_indices() {
        if cu_pos == utf16_idx {
            return Some(bi);
        }
        let cu_len = ch.len_utf16();
        if cu_len == 2 && utf16_idx == cu_pos + 1 {
            // Points at the low surrogate — no byte boundary here.
            return None;
        }
        cu_pos += cu_len;
    }
    if cu_pos == utf16_idx {
        Some(s.len())
    } else {
        None
    }
}

/// Convert a UTF-8 byte index to a UTF-16 code unit index.
///
/// The inverse of [`utf16_index_to_byte`]. Maps from internal byte indices
/// back to spec-level UTF-16 code unit indices.
///
/// Returns `None` if `byte_idx` is not a valid char boundary.
pub fn byte_index_to_utf16(s: &str, byte_idx: usize) -> Option<usize> {
    if byte_idx > s.len() {
        return None;
    }
    if !s.is_char_boundary(byte_idx) {
        return None;
    }
    let mut utf16_idx: usize = 0;
    for (bi, ch) in s.char_indices() {
        if bi == byte_idx {
            return Some(utf16_idx);
        }
        utf16_idx += ch.len_utf16();
    }
    if byte_idx == s.len() {
        Some(utf16_idx)
    } else {
        None
    }
}

/// Return the UTF-16 code unit at index `utf16_idx`.
///
/// Implements the core of `String.prototype.charCodeAt ( pos )` (§22.1.3.2):
/// returns the numeric value of the code unit at the given UTF-16 index.
///
/// [spec]: https://tc39.es/ecma262/#sec-string.prototype.charcodeat (§22.1.3.2)
///
/// # Spec Algorithm (charCodeAt)
///
/// 1. Let O be ? RequireObjectCoercible(**this** value).
/// 2. Let S be ? ToString(O).
/// 3. Let position be ? ToIntegerOrInfinity(pos).
/// 4. Let size be the length of S.
/// 5. If position < 0 or position >= size, return NaN.
/// 6. Return the Number value for the numeric value of the code unit at
///    index position within the String S.
///
/// Note: Steps 1-4 are handled by the caller. This function implements
/// steps 5-6 — returning `None` for the NaN/out-of-range case.
///
/// For BMP characters, returns the code unit directly. For supplementary
/// characters encoded as a surrogate pair, returns the high or low surrogate
/// depending on whether the index points to the first or second code unit.
pub fn char_at_utf16(s: &str, utf16_idx: usize) -> Option<u16> {
    let mut cu_pos: usize = 0;
    for ch in s.chars() {
        let cu_len = ch.len_utf16();
        if cu_pos == utf16_idx {
            // Return the first code unit of this character.
            let mut buf = [0u16; 2];
            ch.encode_utf16(&mut buf);
            return Some(buf[0]);
        }
        if cu_len == 2 && utf16_idx == cu_pos + 1 {
            // Return the low surrogate.
            let mut buf = [0u16; 2];
            ch.encode_utf16(&mut buf);
            return Some(buf[1]);
        }
        cu_pos += cu_len;
    }
    None
}

/// Slice a string by UTF-16 code unit indices `[start..end)`.
///
/// Implements the core extraction logic used by `String.prototype.slice` (§22.1.3.22)
/// and `String.prototype.substring` (§22.1.3.24). Both resolve their `start`/`end`
/// arguments to UTF-16 code unit positions and extract the substring between them.
///
/// [spec]: https://tc39.es/ecma262/#sec-string.prototype.slice (§22.1.3.22)
///
/// # Spec Algorithm (slice — extraction portion)
///
/// After resolving `from` and `to` indices:
/// 7. Let span be max(to - from, 0).
/// 8. Return the substring of S from from to from + span.
///
/// Handles surrogate pair boundaries: if `start` or `end` falls within a
/// surrogate pair, the boundary is adjusted to the containing character's
/// byte boundary.
pub fn slice_utf16(s: &str, start: usize, end: usize) -> String {
    let utf16_len = s.encode_utf16().count();
    let start = start.min(utf16_len);
    let end = end.min(utf16_len);
    if start >= end {
        return String::new();
    }
    let byte_start = utf16_index_to_byte_clamped(s, start);
    let byte_end = utf16_index_to_byte_clamped(s, end);
    s[byte_start..byte_end].to_string()
}

/// Convert a UTF-16 code unit index to a byte index, clamping to the nearest
/// character boundary. Unlike [`utf16_index_to_byte`], never returns `None`.
fn utf16_index_to_byte_clamped(s: &str, utf16_idx: usize) -> usize {
    let mut cu_pos: usize = 0;
    for (bi, ch) in s.char_indices() {
        if cu_pos >= utf16_idx {
            return bi;
        }
        cu_pos += ch.len_utf16();
    }
    s.len()
}

/// `String.prototype.indexOf ( searchString [ , position ] )`
///
/// Searches for the first occurrence of `needle` in `haystack`, starting
/// the search at UTF-16 code unit index `from_utf16`.
///
/// [spec]: https://tc39.es/ecma262/#sec-string.prototype.indexof (§22.1.3.9)
///
/// # Spec Algorithm
///
/// 1. Let O be ? RequireObjectCoercible(**this** value).
/// 2. Let S be ? ToString(O).
/// 3. Let searchStr be ? ToString(searchString).
/// 4. Let pos be ? ToIntegerOrInfinity(position).
/// 5. Assert: If position is undefined, then pos is 0.
/// 6. Let len be the length of S.
/// 7. Let start be the result of clamping pos between 0 and len.
/// 8. Return StringIndexOf(S, searchStr, start).
///
/// Note: Steps 1-7 are handled by the caller. This function implements
/// step 8 (StringIndexOf) — returning `None` when not found (which the
/// caller converts to -1).
///
/// Returns the UTF-16 code unit index of the first match, or `None`.
pub fn index_of_utf16(haystack: &str, needle: &str, from_utf16: usize) -> Option<usize> {
    // Convert from_utf16 to a byte index, clamped.
    let from_byte = utf16_index_to_byte_clamped(haystack, from_utf16);
    let tail = &haystack[from_byte..];
    match tail.find(needle) {
        Some(byte_offset) => {
            // Convert the byte offset back to UTF-16.
            let abs_byte = from_byte + byte_offset;
            byte_index_to_utf16(haystack, abs_byte)
        }
        None => None,
    }
}

/// `String.prototype.lastIndexOf ( searchString [ , position ] )`
///
/// Searches for the last occurrence of `needle` in `haystack`.
///
/// [spec]: https://tc39.es/ecma262/#sec-string.prototype.lastindexof (§22.1.3.10)
///
/// # Spec Algorithm
///
/// 1. Let O be ? RequireObjectCoercible(**this** value).
/// 2. Let S be ? ToString(O).
/// 3. Let searchStr be ? ToString(searchString).
/// 4. Let numPos be ? ToNumber(position).
/// 5. Assert: If position is undefined, then numPos is NaN.
/// 6. If numPos is NaN, let pos be +Infinity; else let pos be ToIntegerOrInfinity(numPos).
/// 7. Let len be the length of S.
/// 8. Let start be the result of clamping pos between 0 and len.
/// 9. Let searchLen be the length of searchStr.
/// 10. Return the largest possible non-negative integer k <= start such that
///     StringIndexOf(S, searchStr, k) is k, or -1 if no such k exists.
///
/// Note: Steps 1-8 are handled by the caller. This simplified version
/// always searches from the end (equivalent to position = +Infinity).
///
/// Returns the UTF-16 code unit index of the last match, or `None`.
pub fn last_index_of_utf16(haystack: &str, needle: &str) -> Option<usize> {
    match haystack.rfind(needle) {
        Some(byte_offset) => byte_index_to_utf16(haystack, byte_offset),
        None => None,
    }
}

/// Create a runtime string from raw UTF-8 bytes and return as a NaN-boxed `JsValue`.
///
/// This is an internal runtime function for materializing String values from
/// compiled constants. It corresponds to the String type (§6.1.4) — creating
/// a new String value from raw byte data.
///
/// [spec]: https://tc39.es/ecma262/#sec-ecmascript-language-types-string-type (§6.1.4)
///
/// # Safety
///
/// The caller must guarantee that `ptr` points to valid UTF-8 data of length `len`.
/// This is satisfied by compiled string constants emitted by the compiler.
pub unsafe fn string_from_data(ptr: *const u8, len: usize) -> JsValue {
    if ptr.is_null() || len == 0 {
        let rt_str = Box::new(RtString::new(String::new()));
        let raw_ptr = Box::into_raw(rt_str) as *const ();
        return JsValue::string(raw_ptr);
    }
    let data = unsafe {
        // SAFETY: Caller guarantees ptr/len are valid UTF-8 string data from a compiled constant.
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len))
    };
    let rt_str = Box::new(RtString::new(data.to_string()));
    let raw_ptr = Box::into_raw(rt_str) as *const ();
    JsValue::string(raw_ptr)
}

/// Concatenate two `JsValue`s into a new string `JsValue`.
///
/// Implements the string concatenation case of the Addition Operator (§13.15.3)
/// and the abstract operation `ApplyStringOrNumericBinaryOperator` (§13.15.4).
///
/// [spec]: https://tc39.es/ecma262/#sec-addition-operator-plus (§13.15.3)
///
/// # Spec Algorithm (EvaluateStringOrNumericBinaryExpression — §13.15.4)
///
/// When the `+` operator is applied and at least one operand is a String:
///
/// 1. Let lref be ? Evaluation of leftOperand.
/// 2. Let lval be ? GetValue(lref).
/// 3. Let rref be ? Evaluation of rightOperand.
/// 4. Let rval be ? GetValue(rref).
/// 5. Return ? ApplyStringOrNumericBinaryOperator(lval, opText, rval).
///
/// ## ApplyStringOrNumericBinaryOperator (§13.15.4, for `+`)
///
/// 1. If opText is `+`, then
///    a. Let lprim be ? ToPrimitive(lval).
///    b. Let rprim be ? ToPrimitive(rval).
///    c. If lprim is a String or rprim is a String, then
///    i. Let lstr be ? ToString(lprim).
///    ii. Let rstr be ? ToString(rprim).
///    iii. Return the string-concatenation of lstr and rstr.
///
/// Note: Steps 1a-1b (ToPrimitive) and type checking (1c) are handled by
/// the compiler/caller. This function implements steps 1c.i-iii.
pub fn string_concat(a: JsValue, b: JsValue) -> JsValue {
    // 1c.i. Let lstr be ? ToString(lprim).
    let a_str = to_string_repr(a);
    // 1c.ii. Let rstr be ? ToString(rprim).
    let b_str = to_string_repr(b);
    // 1c.iii. Return the string-concatenation of lstr and rstr.
    let result = format!("{a_str}{b_str}");
    let rt_str = Box::new(RtString::new(result));
    let raw_ptr = Box::into_raw(rt_str) as *const ();
    JsValue::string(raw_ptr)
}

/// `ToString ( argument )` — abstract operation
///
/// Converts a `JsValue` to its string representation following the
/// ToString abstract operation.
///
/// [spec]: https://tc39.es/ecma262/#sec-tostring (§7.1.17)
///
/// # Spec Algorithm
///
/// The spec defines ToString as a type-switch:
///
/// | Argument Type | Result |
/// |--------------|--------|
/// | Undefined    | `"undefined"` |
/// | Null         | `"null"` |
/// | Boolean      | `"true"` if true, `"false"` if false |
/// | Number       | Number::toString(argument) |
/// | String       | Return argument (identity) |
/// | Symbol       | Throw a TypeError exception |
/// | BigInt       | BigInt::toString(argument) |
/// | Object       | ToPrimitive(argument, string), then ToString on result |
///
/// Note: The Symbol → TypeError case and Object → ToPrimitive path are
/// handled at the caller level. This function delegates to `display_value`
/// for the non-string type conversions.
fn to_string_repr(val: JsValue) -> String {
    if val.is_string() {
        // String → Return argument (identity).
        get_string_data(val)
    } else {
        // Undefined/Null/Boolean/Number → type-specific string representation.
        crate::display::display_value(val)
    }
}

/// Extract string data from a `JsValue`.
///
/// Returns the underlying String value. This is an internal runtime operation
/// that extracts the character sequence from a NaN-boxed string pointer.
///
/// Returns an empty string if the value is not a string or the pointer is null.
///
/// # Safety
///
/// The string pointer must have been created by `string_from_data` or
/// `string_concat` (i.e., via `Box::into_raw` on an `RtString`).
pub fn get_string_data(val: JsValue) -> String {
    if let Some(ptr) = val.as_string() {
        if ptr.is_null() {
            return String::new();
        }
        let rt_str = unsafe {
            // SAFETY: ptr was created by string_from_data or string_concat via Box::into_raw.
            &*(ptr as *const RtString)
        };
        rt_str.data.clone()
    } else {
        String::new()
    }
}

/// Returns `true` if `val` is a string-tagged `JsValue` whose content is empty.
///
/// The empty string `""` is significant in ECMAScript — it is falsy (§7.1.2
/// ToBoolean), has `.length === 0`, and is the identity element for string
/// concatenation.
///
/// Returns `false` for non-string values. A null string pointer is treated as
/// empty (consistent with `get_string_data` which returns `""` for null).
pub fn is_empty_string(val: JsValue) -> bool {
    if !val.is_string() {
        return false;
    }
    let Some(ptr) = val.as_string() else {
        return false;
    };
    if ptr.is_null() {
        return true; // null pointer → treat as empty
    }
    // SAFETY: ptr was created by string_from_data or string_concat via Box::into_raw on an RtString.
    let rt_str = unsafe { &*(ptr as *const RtString) };
    rt_str.is_empty()
}

/// Free a runtime string `JsValue`, dropping the heap-allocated `RtString`.
///
/// This is an internal runtime deallocation function. ECMAScript itself has
/// no explicit free operation — this is part of the AOT compiler's
/// deterministic memory management (ARC/zone-based, not GC).
///
/// # Safety
///
/// The string pointer must have been created by `string_from_data` or
/// `string_concat`, and must not be used after this call.
pub fn free_string(val: JsValue) {
    if let Some(ptr) = val.as_string()
        && !ptr.is_null()
    {
        unsafe {
            // SAFETY: ptr was created by Box::into_raw on an RtString.
            drop(Box::from_raw(ptr as *mut RtString));
        }
    }
}
