//! String method dispatch.
//!
//! Contains `dispatch_string_method` for `String.prototype` methods and
//! `dispatch_string_static_method` for `String.fromCharCode` etc.

use nanbox::JsValue;

use crate::internal_data::{InternalData, InternalKind, UnifiedObject};
use crate::string_ops;
use crate::tagged_obj::{ObjTag, deref_tagged_mut, read_obj_tag};
use crate::{exceptions, rt_api};

use super::{
    __esc_rt_array_push, __esc_rt_create_array, __esc_rt_regexp_exec, __esc_rt_set_prop,
    create_array_from_elements, extract_key_string, make_rt_string, normalize_index, read_argv,
};

/// Convert a JsValue argument to an integer using spec-correct coercion.
///
/// Applies `ToIntegerOrInfinity` (ES2024 §7.1.5) which handles objects via
/// ToPrimitive → ToNumber → truncation. This replaces `v.as_int().unwrap_or(default)`
/// which only handles primitive int values.
fn arg_to_int(v: &JsValue, default: i32) -> i32 {
    if v.is_undefined() {
        return default;
    }
    crate::value_ops::to_integer_or_infinity(*v) as i32
}

/// Convert a JsValue to a string using spec-correct `ToString` (ES2024 §7.1.17).
///
/// Handles all types: strings, booleans, numbers, null, undefined, objects.
/// This replaces `extract_key_string(v.raw_bits()).unwrap_or_default()` which
/// only handles string and int values.
fn arg_to_string(v: &JsValue) -> String {
    if v.is_undefined() {
        return "undefined".to_string();
    }
    if v.is_null() {
        return "null".to_string();
    }
    if let Some(b) = v.as_bool() {
        return if b { "true" } else { "false" }.to_string();
    }
    if let Some(ptr) = v.as_string() {
        if ptr.is_null() {
            return String::new();
        }
        let rt_str = unsafe {
            // SAFETY: string pointer was created by runtime string allocation.
            &*(ptr as *const string_ops::RtString)
        };
        return rt_str.as_str().to_owned();
    }
    // Numbers, objects, symbols — use display_value
    crate::display::display_value(*v)
}

/// Apply ES spec replacement patterns (`$$`, `$&`, `` $` ``, `$'`) in a
/// replacement string for `replace` / `replaceAll`.
///
/// Implements the `GetSubstitution` abstract operation.
///
/// [spec]: https://tc39.es/ecma262/#sec-getsubstitution
fn apply_replacement_pattern(
    replacement: &str,
    matched: &str,
    original: &str,
    match_start: usize,
    match_end: usize,
) -> String {
    let mut result = String::with_capacity(replacement.len());
    let mut chars = replacement.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' {
            match chars.peek() {
                // $$ → literal "$"
                Some('$') => {
                    result.push('$');
                    chars.next();
                }
                // $& → the matched substring
                Some('&') => {
                    result.push_str(matched);
                    chars.next();
                }
                // $` → the portion of the string before the match
                Some('`') => {
                    result.push_str(&original[..match_start]);
                    chars.next();
                }
                // $' → the portion of the string after the match
                Some('\'') => {
                    result.push_str(&original[match_end..]);
                    chars.next();
                }
                // TODO: Step 11.g — $<Name> for named capture groups
                // TODO: Step 11.h — $n / $nn for numbered capture groups
                _ => {
                    result.push('$');
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Check whether a replacement string contains any ES spec replacement patterns.
fn has_replacement_patterns(s: &str) -> bool {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b'$' && matches!(bytes[i + 1], b'$' | b'&' | b'`' | b'\'') {
            return true;
        }
    }
    false
}

/// Check if a NaN-boxed value is a unified object with `InternalKind::RegExpObj`.
fn is_unified_regexp(bits: u64, tag: Option<u8>) -> bool {
    if tag != Some(ObjTag::Unified as u8) {
        return false;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(bits)
    };
    uni.is_some_and(|u| u.kind == InternalKind::RegExpObj)
}

/// Access `JsRegExpData` mutably from a unified `InternalKind::RegExpObj`,
/// calling `f` with the data.
///
/// Returns `None` if the value is not a regexp or the data cannot be accessed.
fn with_regexp_data_mut<F, R>(bits: u64, tag: Option<u8>, f: F) -> Option<R>
where
    F: FnOnce(&mut crate::regexp_bridge::JsRegExpData) -> R,
{
    if tag != Some(ObjTag::Unified as u8) {
        return None;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(bits)
    }?;
    if let Some(InternalData::RegExp { inner }) = uni.internal_data_mut() {
        let re = inner.downcast_mut::<crate::regexp_bridge::JsRegExpData>()?;
        return Some(f(re));
    }
    None
}

/// Dispatch a `String.prototype` method call by name.
///
/// Handles 25+ methods including `toUpperCase`, `indexOf`, `split`, `match`,
/// `search`, `replace`, `slice`, `padStart`, etc. Returns the NaN-boxed result
/// directly, or `undefined` for unknown method names.
///
/// Each branch implements the corresponding ES2024 spec algorithm.
///
/// [spec]: https://tc39.es/ecma262/#sec-properties-of-the-string-prototype-object
pub(crate) fn dispatch_string_method(obj: u64, method: &str, argc: u32, argv: *const u64) -> u64 {
    // thisStringValue (ES2024 §22.1.3): if this is a String wrapper object,
    // unwrap to get the primitive string value.
    let unwrapped = rt_api::unwrap_wrapper_object(obj);
    let v = JsValue::from_raw_bits(unwrapped);

    // ES spec: String.prototype methods require `this` to be coercible to Object.
    // RequireObjectCoercible (§7.2.1) throws TypeError for null/undefined.
    if v.is_null() || v.is_undefined() {
        let msg = rt_api::make_rt_string(format!(
            "TypeError: String.prototype.{method} called on null or undefined"
        ));
        let err = rt_api::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        rt_api::__esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // Step 2: Let S be ? ToString(O).
    // If this is already a string, use it directly. Otherwise convert via ToString.
    let (str_data_owned, str_data_ref);
    if let Some(ptr) = v.as_string() {
        if ptr.is_null() {
            str_data_owned = String::new();
            str_data_ref = str_data_owned.as_str();
        } else {
            let s = unsafe {
                // SAFETY: string pointer was created by runtime string_from_data or string_concat.
                &*(ptr as *const string_ops::RtString)
            };
            str_data_owned = s.as_str().to_owned();
            str_data_ref = str_data_owned.as_str();
        }
    } else {
        // Non-string primitive: convert via ToString (§7.1.17)
        // This handles boolean.trim() → "true".trim(), number.trim() → "42".trim() etc.
        str_data_owned = crate::display::display_value(v);
        str_data_ref = str_data_owned.as_str();
    }
    let str_data = str_data_ref;
    let args = read_argv(argc, argv);

    match method {
        // =====================================================================
        // String.prototype.toUpperCase ( )
        // §22.1.3.30
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.touppercase
        // =====================================================================
        // 1. Let O be ? RequireObjectCoercible(this value).
        //    (handled above)
        // 2. Let S be ? ToString(O).
        //    (str_data is already the string)
        // 3. Let sText be StringToCodePoints(S).
        // 4. Let upperText be toUppercase(sText) (Unicode Default Case Conversion).
        // 5. Return CodePointsToString(upperText).
        "toUpperCase" => make_rt_string(str_data.to_uppercase()),

        // =====================================================================
        // String.prototype.toLowerCase ( )
        // §22.1.3.28
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.tolowercase
        // =====================================================================
        // 1. Let O be ? RequireObjectCoercible(this value).
        //    (handled above)
        // 2. Let S be ? ToString(O).
        //    (str_data is already the string)
        // 3. Let sText be StringToCodePoints(S).
        // 4. Let lowerText be toLowercase(sText) (Unicode Default Case Conversion).
        // 5. Return CodePointsToString(lowerText).
        "toLowerCase" => make_rt_string(str_data.to_lowercase()),

        // =====================================================================
        // String.prototype.localeCompare ( that )
        // §22.1.3.10
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.localecompare
        // =====================================================================
        "localeCompare" => {
            let that = args.first().map(arg_to_string).unwrap_or_default();
            // Simplified: use Rust's Ord comparison (lexicographic by Unicode code point).
            // A full implementation would use ICU/locale-aware comparison.
            let result = str_data.cmp(&*that);
            JsValue::int(match result {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            })
            .raw_bits()
        }

        // =====================================================================
        // String.prototype.trim ( )
        // §22.1.3.31
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.trim
        // =====================================================================
        // 1. Let S be ? thisStringValue(this value).
        //    (str_data is already the string)
        // 2. Return TrimString(S, start+end).
        //    TrimString §22.1.3.34.1:
        //    1. Let str be ? RequireObjectCoercible(string).
        //    2. Let S be ? ToString(str).
        //    3. (where = start+end) Remove leading and trailing white space.
        //    4. Return the result.
        "trim" => make_rt_string(str_data.trim().to_string()),

        // =====================================================================
        // String.prototype.trimStart ( )
        // §22.1.3.33
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.trimstart
        // =====================================================================
        // 1. Let S be ? thisStringValue(this value).
        // 2. Return TrimString(S, start).
        "trimStart" => make_rt_string(str_data.trim_start().to_string()),

        // =====================================================================
        // String.prototype.trimEnd ( )
        // §22.1.3.32
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.trimend
        // =====================================================================
        // 1. Let S be ? thisStringValue(this value).
        // 2. Return TrimString(S, end).
        "trimEnd" => make_rt_string(str_data.trim_end().to_string()),

        // =====================================================================
        // String.prototype.charAt ( pos )
        // §22.1.3.1
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.charat
        // =====================================================================
        "charAt" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let position be ? ToIntegerOrInfinity(pos).
            let idx = args.first().map_or(0, |v| arg_to_int(v, 0)).max(0) as usize;
            // 4. Let size be the length of S.
            // 5. If position < 0 or position >= size, return the empty String.
            // 6. Return the substring of S from position to position + 1.
            match string_ops::char_at_utf16(str_data, idx) {
                Some(cu) => {
                    let ch = char::from_u32(cu as u32)
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    make_rt_string(ch)
                }
                None => make_rt_string(String::new()),
            }
        }

        // =====================================================================
        // String.prototype.indexOf ( searchString [ , position ] )
        // §22.1.3.9
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.indexof
        // =====================================================================
        "indexOf" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let searchStr be ? ToString(searchString).
            let search = args
                .first()
                .map(arg_to_string)
                .unwrap_or_else(|| "undefined".to_string());
            // 4. Let pos be ? ToIntegerOrInfinity(position).
            // 5. Assert: If position is undefined, then pos is 0.
            let from_idx = args.get(1).map_or(0, |v| arg_to_int(v, 0)).max(0) as usize;
            // 6. Let len be the length of S.
            // 7. Let start be the result of clamping pos between 0 and len.
            // 8. Return StringIndexOf(S, searchStr, start).
            let idx = string_ops::index_of_utf16(str_data, &search, from_idx)
                .map(|i| i as i32)
                .unwrap_or(-1);
            JsValue::int(idx).raw_bits()
        }

        // =====================================================================
        // String.prototype.lastIndexOf ( searchString [ , position ] )
        // §22.1.3.10
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.lastindexof
        // =====================================================================
        "lastIndexOf" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let searchStr be ? ToString(searchString).
            let search = args
                .first()
                .map(arg_to_string)
                .unwrap_or_else(|| "undefined".to_string());
            // 4. Let numPos be ? ToNumber(position).
            // 5. Assert: If position is undefined, then numPos is NaN.
            // 6. If numPos is NaN, let pos be +∞; otherwise let pos be ToIntegerOrInfinity(numPos).
            // TODO: Step 4-6 — position parameter is not supported for lastIndexOf
            // 7. Let len be the length of S.
            // 8. Let start be the result of clamping pos between 0 and len.
            // 9. Return the largest index i such that StringIndexOf(S, searchStr, i) = i and i <= start;
            //    or -1 if no such index exists.
            let idx = string_ops::last_index_of_utf16(str_data, &search)
                .map(|i| i as i32)
                .unwrap_or(-1);
            JsValue::int(idx).raw_bits()
        }

        // =====================================================================
        // String.prototype.includes ( searchString [ , position ] )
        // §22.1.3.8
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.includes
        // =====================================================================
        "includes" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let isRegExp be ? IsRegExp(searchString).
            // 4. If isRegExp is true, throw a TypeError exception.
            if let Some(first) = args.first() {
                let first_bits = first.raw_bits();
                let first_tag = read_obj_tag(first_bits);
                if is_unified_regexp(first_bits, first_tag) {
                    let msg = rt_api::make_rt_string(
                        "TypeError: First argument to String.prototype.includes must not be a regular expression".to_string(),
                    );
                    let err = rt_api::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                    rt_api::__esc_rt_throw(err);
                    return JsValue::undefined().raw_bits();
                }
            }
            // 5. Let searchStr be ? ToString(searchString).
            let search = args
                .first()
                .map(arg_to_string)
                .unwrap_or_else(|| "undefined".to_string());
            // 6. Let pos be ? ToIntegerOrInfinity(position).
            // 7. Assert: If position is undefined, then pos is 0.
            let position = args.get(1).map_or(0, |v| arg_to_int(v, 0)).max(0) as usize;
            // 8. Let len be the length of S.
            let utf16_len = str_data.encode_utf16().count();
            // 9. Let start be the result of clamping pos between 0 and len.
            if position >= utf16_len {
                return JsValue::bool(search.is_empty()).raw_bits();
            }
            // 10. Let index be StringIndexOf(S, searchStr, start).
            // 11. If index is not -1, return true.
            // 12. Return false.
            let byte_pos = string_ops::utf16_index_to_byte(str_data, position).unwrap_or(0);
            JsValue::bool(str_data[byte_pos..].contains(&*search)).raw_bits()
        }

        // =====================================================================
        // String.prototype.startsWith ( searchString [ , position ] )
        // §22.1.3.25
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.startswith
        // =====================================================================
        "startsWith" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let isRegExp be ? IsRegExp(searchString).
            // 4. If isRegExp is true, throw a TypeError exception.
            if let Some(first) = args.first() {
                let first_bits = first.raw_bits();
                let first_tag = read_obj_tag(first_bits);
                if is_unified_regexp(first_bits, first_tag) {
                    let msg = rt_api::make_rt_string(
                        "TypeError: First argument to String.prototype.startsWith must not be a regular expression".to_string(),
                    );
                    let err = rt_api::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                    rt_api::__esc_rt_throw(err);
                    return JsValue::undefined().raw_bits();
                }
            }
            // 5. Let searchStr be ? ToString(searchString).
            let prefix = args
                .first()
                .map(arg_to_string)
                .unwrap_or_else(|| "undefined".to_string());
            // 6. Let pos be ? ToIntegerOrInfinity(position).
            // 7. Assert: If position is undefined, then pos is 0.
            let position = args.get(1).map_or(0, |v| arg_to_int(v, 0)).max(0) as usize;
            // 8. Let len be the length of S.
            // 9. Let start be the result of clamping pos between 0 and len.
            let byte_pos = string_ops::utf16_index_to_byte(str_data, position).unwrap_or(0);
            // 10. Let searchLength be the length of searchStr.
            // 11. If searchLength + start > len, return false.
            // 12. If the substring of S from start to start + searchLength is searchStr, return true.
            // 13. Return false.
            JsValue::bool(str_data[byte_pos..].starts_with(&*prefix)).raw_bits()
        }

        // =====================================================================
        // String.prototype.endsWith ( searchString [ , endPosition ] )
        // §22.1.3.7
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.endswith
        // =====================================================================
        "endsWith" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let isRegExp be ? IsRegExp(searchString).
            // 4. If isRegExp is true, throw a TypeError exception.
            if let Some(first) = args.first() {
                let first_bits = first.raw_bits();
                let first_tag = read_obj_tag(first_bits);
                if is_unified_regexp(first_bits, first_tag) {
                    let msg = rt_api::make_rt_string(
                        "TypeError: First argument to String.prototype.endsWith must not be a regular expression".to_string(),
                    );
                    let err = rt_api::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                    rt_api::__esc_rt_throw(err);
                    return JsValue::undefined().raw_bits();
                }
            }
            // 5. Let searchStr be ? ToString(searchString).
            let suffix = args
                .first()
                .map(arg_to_string)
                .unwrap_or_else(|| "undefined".to_string());
            // 6. Let len be the length of S.
            let utf16_len = str_data.encode_utf16().count() as i32;
            // 7. If endPosition is undefined, let pos be len; else let pos be ? ToIntegerOrInfinity(endPosition).
            // 8. Let end be the result of clamping pos between 0 and len.
            let end_pos = args
                .get(1)
                .map_or(utf16_len, |v| arg_to_int(v, utf16_len))
                .max(0)
                .min(utf16_len) as usize;
            // 9. Let searchLength be the length of searchStr.
            // 10. If searchLength > end, return false.
            // 11. Let start be end - searchLength.
            // 12. If the substring of S from start to end is searchStr, return true.
            // 13. Return false.
            let byte_end =
                string_ops::utf16_index_to_byte(str_data, end_pos).unwrap_or(str_data.len());
            JsValue::bool(str_data[..byte_end].ends_with(&*suffix)).raw_bits()
        }

        // =====================================================================
        // String.prototype.slice ( start, end )
        // §22.1.3.22
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.slice
        // =====================================================================
        "slice" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let len be the length of S.
            let len = str_data.encode_utf16().count() as i32;
            // 4. Let intStart be ? ToIntegerOrInfinity(start).
            let start = args.first().map_or(0, |v| arg_to_int(v, 0));
            // 5. If intEnd is undefined, let intEnd be len; else let intEnd be ? ToIntegerOrInfinity(end).
            let end = args.get(1).map_or(len, |v| arg_to_int(v, len));
            // 6. If intStart < 0, let from be max(len + intStart, 0); else let from be min(intStart, len).
            let start = normalize_index(start, len);
            // 7. If intEnd < 0, let to be max(len + intEnd, 0); else let to be min(intEnd, len).
            let end = normalize_index(end, len);
            // 8. If from >= to, return the empty String.
            if start >= end {
                return make_rt_string(String::new());
            }
            // 9. Return the substring of S from from to to.
            make_rt_string(string_ops::slice_utf16(str_data, start, end))
        }

        // =====================================================================
        // String.prototype.substring ( start, end )
        // §22.1.3.26
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.substring
        // =====================================================================
        "substring" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let len be the length of S.
            let len = str_data.encode_utf16().count() as i32;
            // 4. Let intStart be ? ToIntegerOrInfinity(start).
            let start = args
                .first()
                .map_or(0, |v| arg_to_int(v, 0))
                // 6. Let finalStart be the result of clamping intStart between 0 and len.
                .max(0)
                .min(len);
            // 5. If end is undefined, let intEnd be len; else let intEnd be ? ToIntegerOrInfinity(end).
            let end = args
                .get(1)
                .map_or(len, |v| arg_to_int(v, len))
                // 7. Let finalEnd be the result of clamping intEnd between 0 and len.
                .max(0)
                .min(len);
            // 8. Let from be min(finalStart, finalEnd).
            // 9. Let to be max(finalStart, finalEnd).
            let (start, end) = if start <= end {
                (start as usize, end as usize)
            } else {
                (end as usize, start as usize)
            };
            // 10. Return the substring of S from from to to.
            make_rt_string(string_ops::slice_utf16(str_data, start, end))
        }

        // =====================================================================
        // String.prototype.split ( separator, limit )
        // §22.1.3.24
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.split
        // =====================================================================
        "split" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. If separator is neither undefined nor null, then
            //    a. Let splitter be ? GetMethod(separator, @@split).
            //    b. If splitter is not undefined, return ? Call(splitter, separator, « O, limit »).
            let first_arg = args.first().map_or(0u64, |v| v.raw_bits());
            let tag = read_obj_tag(first_arg);
            if is_unified_regexp(first_arg, tag) {
                // Delegate to RegExp.prototype[Symbol.split](string, limit)
                let str_bits = make_rt_string(str_data.to_string());
                let limit_bits = args
                    .get(1)
                    .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
                let split_argv = [str_bits, limit_bits];
                return super::dispatch_regexp::dispatch_regexp_method(
                    first_arg,
                    "Symbol.split",
                    2,
                    split_argv.as_ptr(),
                );
            }
            // 3. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 4. If limit is undefined, let lim be 2^32 - 1; else let lim be ? ToUint32(limit).
            let limit = args.get(1).and_then(|v| {
                v.as_int()
                    .map(|n| n as usize)
                    .or_else(|| v.as_number().map(|n| n as usize))
            });
            // 5. Let R be ? ToString(separator).
            let sep = args.first().and_then(|v| {
                if v.is_undefined() {
                    None // undefined separator means no splitting
                } else {
                    Some(arg_to_string(v))
                }
            });
            let make_str_val = |s: &str| -> JsValue {
                let rt = Box::new(string_ops::RtString::new(s.to_string()));
                let ptr = Box::into_raw(rt) as *const ();
                JsValue::string(ptr)
            };
            // 6. If lim = 0, return CreateArrayFromList(« »).
            // 7-12. (Splitting algorithm — see spec for full steps)
            let parts: Vec<JsValue> = match sep {
                Some(ref s) if s.is_empty() => {
                    // Empty separator: split into individual UTF-16 code units.
                    // For BMP chars this is the same as chars(), but supplementary
                    // characters (emoji etc.) are split into two surrogate halves.
                    let utf16_len = str_data.encode_utf16().count();
                    let iter = (0..utf16_len).map(|i| {
                        let cu = string_ops::char_at_utf16(str_data, i).unwrap_or(0);
                        let ch = char::from_u32(cu as u32)
                            .map(|c| c.to_string())
                            .unwrap_or_default();
                        make_str_val(&ch)
                    });
                    match limit {
                        Some(lim) => iter.take(lim).collect(),
                        None => iter.collect(),
                    }
                }
                Some(ref s) => {
                    let iter = str_data.split(s.as_str()).map(&make_str_val);
                    match limit {
                        Some(lim) => iter.take(lim).collect(),
                        None => iter.collect(),
                    }
                }
                // 13. If separator is undefined, return CreateArrayFromList(« S »).
                None => vec![make_str_val(str_data)],
            };
            // 14. Return A (the result array).
            create_array_from_elements(parts)
        }

        // =====================================================================
        // String.prototype.repeat ( count )
        // §22.1.3.18
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.repeat
        // =====================================================================
        "repeat" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let n be ? ToIntegerOrInfinity(count).
            // 4. If n < 0 or n = +Infinity, throw a RangeError exception.
            let raw_count = args.first().copied().unwrap_or(JsValue::undefined());
            if let Some(n) = raw_count.as_number() {
                if n < 0.0 || n.is_infinite() {
                    let msg = rt_api::make_rt_string(
                        "RangeError: Invalid count value for String.prototype.repeat".to_string(),
                    );
                    let err =
                        rt_api::__esc_rt_create_error(exceptions::error_tag::RANGE_ERROR, msg);
                    rt_api::__esc_rt_throw(err);
                    return JsValue::undefined().raw_bits();
                }
            } else if let Some(n) = raw_count.as_int()
                && n < 0
            {
                let msg = rt_api::make_rt_string(
                    "RangeError: Invalid count value for String.prototype.repeat".to_string(),
                );
                let err = rt_api::__esc_rt_create_error(exceptions::error_tag::RANGE_ERROR, msg);
                rt_api::__esc_rt_throw(err);
                return JsValue::undefined().raw_bits();
            }
            let count = arg_to_int(&raw_count, 0).max(0) as usize;
            // 5. If n is 0, return the empty String.
            // 6. Return the String value consisting of n copies of S.
            make_rt_string(str_data.repeat(count))
        }

        // =====================================================================
        // String.prototype.replace ( searchValue, replaceValue )
        // §22.1.3.19
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.replace
        // =====================================================================
        "replace" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. If searchValue is neither undefined nor null, then
            //    a. Let replacer be ? GetMethod(searchValue, @@replace).
            //    b. If replacer is not undefined, return ? Call(replacer, searchValue, « O, replaceValue »).
            let first_arg = args.first().map_or(0u64, |v| v.raw_bits());
            let tag = read_obj_tag(first_arg);
            if is_unified_regexp(first_arg, tag) {
                // Delegate to RegExp.prototype[Symbol.replace](string, replacement)
                let str_bits = make_rt_string(str_data.to_string());
                let repl_bits = args
                    .get(1)
                    .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
                let replace_argv = [str_bits, repl_bits];
                return super::dispatch_regexp::dispatch_regexp_method(
                    first_arg,
                    "Symbol.replace",
                    2,
                    replace_argv.as_ptr(),
                );
            }
            // 3. Let string be ? ToString(O).
            // 4. Let searchString be ? ToString(searchValue).
            let search = args.first().map(arg_to_string).unwrap_or_default();
            // 5. Let functionalReplace be IsCallable(replaceValue).
            let repl_val = args.get(1).copied().unwrap_or(JsValue::undefined());
            let functional_replace =
                repl_val.is_object() && super::dispatch_core::is_callable(repl_val.raw_bits());
            // 6. If functionalReplace is false, let replaceValue be ? ToString(replaceValue).
            let replacement = if functional_replace {
                String::new()
            } else {
                arg_to_string(&repl_val)
            };
            // 7-14. Find and replace.
            // 12. If functionalReplace is true, call the replacement function.
            if functional_replace {
                if let Some(byte_start) = if search.is_empty() {
                    Some(0usize)
                } else {
                    str_data.find(&*search)
                } {
                    let char_pos = str_data[..byte_start].chars().count();
                    let matched_str = make_rt_string(search.to_string());
                    let pos_val = JsValue::int(char_pos as i32).raw_bits();
                    let str_val = make_rt_string(str_data.to_string());
                    let call_args = [matched_str, pos_val, str_val];
                    let result = unsafe {
                        super::dispatch_core::__esc_rt_call_indirect(
                            repl_val.raw_bits(),
                            3,
                            call_args.as_ptr(),
                        )
                    };
                    let result_str = arg_to_string(&JsValue::from_raw_bits(result));
                    let byte_end = byte_start + search.len();
                    let mut out = String::with_capacity(str_data.len());
                    out.push_str(&str_data[..byte_start]);
                    out.push_str(&result_str);
                    out.push_str(&str_data[byte_end..]);
                    return make_rt_string(out);
                }
                return make_rt_string(str_data.to_string());
            }
            if search.is_empty() {
                // Empty search matches at position 0
                if has_replacement_patterns(&replacement) {
                    let expanded = apply_replacement_pattern(&replacement, "", str_data, 0, 0);
                    make_rt_string(format!("{expanded}{str_data}"))
                } else {
                    make_rt_string(format!("{replacement}{str_data}"))
                }
            } else if has_replacement_patterns(&replacement) {
                match str_data.find(&*search) {
                    Some(byte_start) => {
                        let byte_end = byte_start + search.len();
                        let expanded = apply_replacement_pattern(
                            &replacement,
                            &search,
                            str_data,
                            byte_start,
                            byte_end,
                        );
                        let mut result = String::with_capacity(str_data.len());
                        result.push_str(&str_data[..byte_start]);
                        result.push_str(&expanded);
                        result.push_str(&str_data[byte_end..]);
                        make_rt_string(result)
                    }
                    None => make_rt_string(str_data.to_string()),
                }
            } else {
                make_rt_string(str_data.replacen(&*search, &replacement, 1))
            }
        }

        // =====================================================================
        // String.prototype.replaceAll ( searchValue, replaceValue )
        // §22.1.3.20
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.replaceall
        // =====================================================================
        "replaceAll" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. If searchValue is neither undefined nor null, then
            //    a. Let isRegExp be ? IsRegExp(searchValue).
            //    b. If isRegExp is true, then
            //       i. Let flags be ? Get(searchValue, "flags").
            //       ii. If flags does not contain "g", throw a TypeError exception.
            let first_arg = args.first().map_or(0u64, |v| v.raw_bits());
            let tag = read_obj_tag(first_arg);
            if is_unified_regexp(first_arg, tag) {
                // Check global flag -- TypeError if not set
                let has_global =
                    with_regexp_data_mut(first_arg, tag, |re_data| re_data.inner.flags.global)
                        .unwrap_or(false);
                if !has_global {
                    let msg = rt_api::make_rt_string(
                        "TypeError: String.prototype.replaceAll called with a non-global RegExp argument".to_string(),
                    );
                    let err = rt_api::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                    rt_api::__esc_rt_throw(err);
                    return JsValue::undefined().raw_bits();
                }
                //    c. Let replacer be ? GetMethod(searchValue, @@replace).
                //    d. If replacer is not undefined, return ? Call(replacer, searchValue, « O, replaceValue »).
                // Delegate to RegExp.prototype[Symbol.replace](string, replacement)
                let str_bits = make_rt_string(str_data.to_string());
                let repl_bits = args
                    .get(1)
                    .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
                let replace_argv = [str_bits, repl_bits];
                return super::dispatch_regexp::dispatch_regexp_method(
                    first_arg,
                    "Symbol.replace",
                    2,
                    replace_argv.as_ptr(),
                );
            }
            // 3. Let string be ? ToString(O).
            // 4. Let searchString be ? ToString(searchValue).
            let search = args.first().map(arg_to_string).unwrap_or_default();
            // 5. Let functionalReplace be IsCallable(replaceValue).
            let repl_val = args.get(1).copied().unwrap_or(JsValue::undefined());
            let functional_replace =
                repl_val.is_object() && super::dispatch_core::is_callable(repl_val.raw_bits());
            // 6. If functionalReplace is false, let replaceValue be ? ToString(replaceValue).
            let replacement = if functional_replace {
                String::new()
            } else {
                arg_to_string(&repl_val)
            };
            // 12. If functionalReplace, call replacer for each match.
            if functional_replace {
                let search_len = search.len();
                let mut result = String::new();
                let mut last_end = 0usize;
                if search.is_empty() {
                    // Empty search: insert replacement between every character
                    let mut char_idx = 0usize;
                    for ch in str_data.chars() {
                        let matched_str = make_rt_string(String::new());
                        let pos_val = JsValue::int(char_idx as i32).raw_bits();
                        let str_val = make_rt_string(str_data.to_string());
                        let call_args = [matched_str, pos_val, str_val];
                        let res = unsafe {
                            super::dispatch_core::__esc_rt_call_indirect(
                                repl_val.raw_bits(),
                                3,
                                call_args.as_ptr(),
                            )
                        };
                        result.push_str(&arg_to_string(&JsValue::from_raw_bits(res)));
                        result.push(ch);
                        char_idx += 1;
                    }
                    // Final insertion after last character
                    let matched_str = make_rt_string(String::new());
                    let pos_val = JsValue::int(char_idx as i32).raw_bits();
                    let str_val = make_rt_string(str_data.to_string());
                    let call_args = [matched_str, pos_val, str_val];
                    let res = unsafe {
                        super::dispatch_core::__esc_rt_call_indirect(
                            repl_val.raw_bits(),
                            3,
                            call_args.as_ptr(),
                        )
                    };
                    result.push_str(&arg_to_string(&JsValue::from_raw_bits(res)));
                } else {
                    while let Some(pos) = str_data[last_end..].find(&*search) {
                        let byte_start = last_end + pos;
                        result.push_str(&str_data[last_end..byte_start]);
                        let char_pos = str_data[..byte_start].chars().count();
                        let matched_str = make_rt_string(search.to_string());
                        let pos_val = JsValue::int(char_pos as i32).raw_bits();
                        let str_val = make_rt_string(str_data.to_string());
                        let call_args = [matched_str, pos_val, str_val];
                        let res = unsafe {
                            super::dispatch_core::__esc_rt_call_indirect(
                                repl_val.raw_bits(),
                                3,
                                call_args.as_ptr(),
                            )
                        };
                        result.push_str(&arg_to_string(&JsValue::from_raw_bits(res)));
                        last_end = byte_start + search_len;
                    }
                    result.push_str(&str_data[last_end..]);
                }
                return make_rt_string(result);
            }
            // 7-15. String replacement path.
            if has_replacement_patterns(&replacement) {
                if search.is_empty() {
                    // Empty search: insert replacement between every char
                    let mut result = String::new();
                    let mut byte_pos = 0;
                    for ch in str_data.chars() {
                        let expanded = apply_replacement_pattern(
                            &replacement,
                            "",
                            str_data,
                            byte_pos,
                            byte_pos,
                        );
                        result.push_str(&expanded);
                        result.push(ch);
                        byte_pos += ch.len_utf8();
                    }
                    let expanded =
                        apply_replacement_pattern(&replacement, "", str_data, byte_pos, byte_pos);
                    result.push_str(&expanded);
                    make_rt_string(result)
                } else {
                    let search_len = search.len();
                    let mut result = String::new();
                    let mut last_end = 0;
                    while let Some(pos) = str_data[last_end..].find(&*search) {
                        let byte_start = last_end + pos;
                        let byte_end = byte_start + search_len;
                        result.push_str(&str_data[last_end..byte_start]);
                        let expanded = apply_replacement_pattern(
                            &replacement,
                            &search,
                            str_data,
                            byte_start,
                            byte_end,
                        );
                        result.push_str(&expanded);
                        last_end = byte_end;
                    }
                    result.push_str(&str_data[last_end..]);
                    make_rt_string(result)
                }
            } else {
                make_rt_string(str_data.replace(&*search, &replacement))
            }
        }

        // =====================================================================
        // String.prototype.concat ( ...args )
        // §22.1.3.4
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.concat
        // =====================================================================
        "concat" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            let mut result = str_data.to_string();
            // 3. Let R be S.
            // 4. For each element next of args, do
            for arg in &args {
                //    a. Let nextString be ? ToString(next).
                let s = arg_to_string(arg);
                //    b. Set R to the string-concatenation of R and nextString.
                result.push_str(&s);
            }
            // 5. Return R.
            make_rt_string(result)
        }

        // =====================================================================
        // String.prototype.padStart ( maxLength [ , fillString ] )
        // §22.1.3.17
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.padstart
        // =====================================================================
        "padStart" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let intMaxLength be ? ToLength(maxLength).
            let target_len = args.first().map_or(0, |v| arg_to_int(v, 0)) as usize;
            // 4. Let stringLength be the length of S.
            let str_utf16_len = str_data.encode_utf16().count();
            // 5. If intMaxLength <= stringLength, return S.
            // 6. If fillString is undefined, let filler be the String value consisting
            //    solely of the code unit 0x0020 (SPACE); else let filler be ? ToString(fillString).
            let pad_str = args.get(1).map_or_else(
                || " ".to_string(),
                |v| {
                    if v.is_undefined() {
                        " ".to_string()
                    } else {
                        arg_to_string(v)
                    }
                },
            );
            let pad_utf16_len = pad_str.encode_utf16().count();
            // 7. If filler is the empty String, return S.
            if str_utf16_len >= target_len || pad_utf16_len == 0 {
                return make_rt_string(str_data.to_string());
            }
            // 8. Let fillLen be intMaxLength - stringLength.
            let fill_len = target_len - str_utf16_len;
            // 9. Let truncatedStringFiller be the String value consisting of repeated
            //    concatenations of filler truncated to length fillLen.
            let repeated = pad_str.repeat((fill_len / pad_utf16_len) + 1);
            let fill = string_ops::slice_utf16(&repeated, 0, fill_len);
            // 10. Return the string-concatenation of truncatedStringFiller and S.
            make_rt_string(format!("{fill}{str_data}"))
        }

        // =====================================================================
        // String.prototype.padEnd ( maxLength [ , fillString ] )
        // §22.1.3.16
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.padend
        // =====================================================================
        "padEnd" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let intMaxLength be ? ToLength(maxLength).
            let target_len = args.first().map_or(0, |v| arg_to_int(v, 0)) as usize;
            // 4. Let stringLength be the length of S.
            let str_utf16_len = str_data.encode_utf16().count();
            // 5. If intMaxLength <= stringLength, return S.
            // 6. If fillString is undefined, let filler be the String value consisting
            //    solely of the code unit 0x0020 (SPACE); else let filler be ? ToString(fillString).
            let pad_str = args.get(1).map_or_else(
                || " ".to_string(),
                |v| {
                    if v.is_undefined() {
                        " ".to_string()
                    } else {
                        arg_to_string(v)
                    }
                },
            );
            let pad_utf16_len = pad_str.encode_utf16().count();
            // 7. If filler is the empty String, return S.
            if str_utf16_len >= target_len || pad_utf16_len == 0 {
                return make_rt_string(str_data.to_string());
            }
            // 8. Let fillLen be intMaxLength - stringLength.
            let fill_len = target_len - str_utf16_len;
            // 9. Let truncatedStringFiller be the String value consisting of repeated
            //    concatenations of filler truncated to length fillLen.
            let repeated = pad_str.repeat((fill_len / pad_utf16_len) + 1);
            let fill = string_ops::slice_utf16(&repeated, 0, fill_len);
            // 10. Return the string-concatenation of S and truncatedStringFiller.
            make_rt_string(format!("{str_data}{fill}"))
        }

        // =====================================================================
        // String.prototype.charCodeAt ( pos )
        // §22.1.3.2
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.charcodeat
        // =====================================================================
        "charCodeAt" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let position be ? ToIntegerOrInfinity(pos).
            let idx = args.first().map_or(0, |v| arg_to_int(v, 0)) as usize;
            // 4. Let size be the length of S.
            // 5. If position < 0 or position >= size, return NaN.
            // 6. Return the Number value for the numeric value of the code unit at
            //    index position within S.
            match string_ops::char_at_utf16(str_data, idx) {
                Some(cu) => JsValue::int(cu as i32).raw_bits(),
                None => JsValue::number(f64::NAN).raw_bits(),
            }
        }

        // =====================================================================
        // String.prototype.codePointAt ( pos )
        // §22.1.3.3
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.codepointat
        // =====================================================================
        "codePointAt" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let position be ? ToIntegerOrInfinity(pos).
            let idx = args.first().map_or(0, |v| arg_to_int(v, 0)) as usize;
            // 4. Let size be the length of S.
            // 5. If position < 0 or position >= size, return undefined.
            // 6. Let cp be CodePointAt(S, position).
            // 7. Return 𝔽(cp.[[CodePoint]]).
            //
            // CodePointAt ( string, position ) — §11.1.5:
            // 1. Let size be the length of string.
            // 2. Assert: position >= 0 and position < size.
            // 3. Let first be the code unit at index position within string.
            // 4. Let cp be the code point whose numeric value is that of first.
            // 5. If first is not a leading surrogate or trailing surrogate, return Record { [[CodePoint]]: cp, ... }.
            // 6. If first is a trailing surrogate or position + 1 = size, return Record { [[CodePoint]]: cp, [[CodeUnitCount]]: 1, [[IsUnpairedSurrogate]]: true }.
            // 7. Let second be the code unit at index position + 1 within string.
            // 8. If second is not a trailing surrogate, return Record { [[CodePoint]]: cp, [[CodeUnitCount]]: 1, [[IsUnpairedSurrogate]]: true }.
            // 9. Set cp to UTF16SurrogatePairToCodePoint(first, second).
            // 10. Return Record { [[CodePoint]]: cp, [[CodeUnitCount]]: 2, [[IsUnpairedSurrogate]]: false }.
            match string_ops::char_at_utf16(str_data, idx) {
                Some(hi) if (0xD800..=0xDBFF).contains(&hi) => {
                    // High surrogate -- check if next code unit is a low surrogate.
                    match string_ops::char_at_utf16(str_data, idx + 1) {
                        Some(lo) if (0xDC00..=0xDFFF).contains(&lo) => {
                            let cp = 0x10000 + ((hi as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00);
                            JsValue::int(cp as i32).raw_bits()
                        }
                        _ => JsValue::int(hi as i32).raw_bits(),
                    }
                }
                Some(cu) => JsValue::int(cu as i32).raw_bits(),
                None => JsValue::undefined().raw_bits(),
            }
        }

        // =====================================================================
        // String.prototype.match ( regexp )
        // §22.1.3.12
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.match
        // =====================================================================
        "match" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. If regexp is neither undefined nor null, then
            //    a. Let matcher be ? GetMethod(regexp, @@match).
            //    b. If matcher is not undefined, return ? Call(matcher, regexp, « O »).
            let first_arg = args.first().map_or(0u64, |v| v.raw_bits());
            let tag = read_obj_tag(first_arg);
            let is_regexp = is_unified_regexp(first_arg, tag);
            if is_regexp {
                let match_result = with_regexp_data_mut(first_arg, tag, |re_data| {
                    if re_data.inner.flags.global {
                        // RegExp.prototype[@@match] §22.2.6.8 (global case):
                        // Collect all matches and return array of matched strings.
                        let matches = re_data.inner.match_all(str_data);
                        if matches.is_empty() {
                            JsValue::null().raw_bits()
                        } else {
                            let elements: Vec<JsValue> = matches
                                .iter()
                                .map(|m| {
                                    JsValue::from_raw_bits(make_rt_string(m.full_match.clone()))
                                })
                                .collect();
                            create_array_from_elements(elements)
                        }
                    } else {
                        // RegExp.prototype[@@match] §22.2.6.8 (non-global case):
                        // Return ? RegExpExec(R, S).
                        __esc_rt_regexp_exec(first_arg, obj)
                    }
                });
                match_result.unwrap_or(JsValue::null().raw_bits())
            } else {
                // 3. Let string be ? ToString(O).
                // 4. Let rx be ? RegExpCreate(regexp, undefined).
                let pattern = {
                    let v = JsValue::from_raw_bits(first_arg);
                    arg_to_string(&v)
                };
                // 5. Return ? Invoke(rx, @@match, « string »).
                match crate::regexp_bridge::JsRegExpData::new(&pattern, "") {
                    Ok(mut re_data) => match re_data.inner.exec(str_data) {
                        Some(m) => {
                            let arr_bits = __esc_rt_create_array(0);
                            let full_bits = make_rt_string(m.full_match);
                            __esc_rt_array_push(arr_bits, full_bits);
                            for group in &m.groups {
                                match group {
                                    Some(s) => {
                                        let s_bits = make_rt_string(s.clone());
                                        __esc_rt_array_push(arr_bits, s_bits);
                                    }
                                    None => {
                                        __esc_rt_array_push(
                                            arr_bits,
                                            JsValue::undefined().raw_bits(),
                                        );
                                    }
                                }
                            }
                            let index_key = make_rt_string("index".to_string());
                            // m.index is a byte index; convert to UTF-16 for ES semantics.
                            let utf16_idx = string_ops::byte_index_to_utf16(str_data, m.index)
                                .unwrap_or(m.index);
                            let index_val = JsValue::number(utf16_idx as f64).raw_bits();
                            __esc_rt_set_prop(arr_bits, index_key, index_val);
                            arr_bits
                        }
                        None => JsValue::null().raw_bits(),
                    },
                    Err(_) => JsValue::null().raw_bits(),
                }
            }
        }

        // =====================================================================
        // String.prototype.search ( regexp )
        // §22.1.3.21
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.search
        // =====================================================================
        "search" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. If regexp is neither undefined nor null, then
            //    a. Let searcher be ? GetMethod(regexp, @@search).
            //    b. If searcher is not undefined, return ? Call(searcher, regexp, « O »).
            let first_arg = args.first().map_or(0u64, |v| v.raw_bits());
            let tag = read_obj_tag(first_arg);
            let is_regexp = is_unified_regexp(first_arg, tag);
            if is_regexp {
                let search_result = with_regexp_data_mut(first_arg, tag, |re_data| {
                    // RegExp.prototype[@@search] §22.2.6.10:
                    // 1-2. (Type checks — handled by is_unified_regexp)
                    // 3. Let previousLastIndex be ? Get(rx, "lastIndex").
                    let saved = re_data.inner.last_index;
                    // 4. If SameValue(previousLastIndex, +0𝔽) is false, then
                    //    a. Perform ? Set(rx, "lastIndex", +0𝔽, true).
                    re_data.inner.last_index = 0;
                    // 5. Let result be ? RegExpExec(rx, S).
                    let result = re_data.inner.exec(str_data);
                    // 6. Let currentLastIndex be ? Get(rx, "lastIndex").
                    // 7. If SameValue(currentLastIndex, previousLastIndex) is false, then
                    //    a. Perform ? Set(rx, "lastIndex", previousLastIndex, true).
                    re_data.inner.last_index = saved;
                    // 8. If result is null, return -1𝔽.
                    // 9. Return ? Get(result, "index").
                    match result {
                        Some(m) => {
                            // m.index is a byte index; convert to UTF-16.
                            let utf16_idx = string_ops::byte_index_to_utf16(str_data, m.index)
                                .unwrap_or(m.index);
                            JsValue::int(utf16_idx as i32).raw_bits()
                        }
                        None => JsValue::int(-1).raw_bits(),
                    }
                });
                search_result.unwrap_or(JsValue::int(-1).raw_bits())
            } else {
                // 3. Let string be ? ToString(O).
                // 4. Let rx be ? RegExpCreate(regexp, undefined).
                let pattern = {
                    let v = JsValue::from_raw_bits(first_arg);
                    arg_to_string(&v)
                };
                // 5. Return ? Invoke(rx, @@search, « string »).
                match crate::regexp_bridge::JsRegExpData::new(&pattern, "") {
                    Ok(mut re_data) => match re_data.inner.exec(str_data) {
                        Some(m) => {
                            // m.index is a byte index; convert to UTF-16.
                            let utf16_idx = string_ops::byte_index_to_utf16(str_data, m.index)
                                .unwrap_or(m.index);
                            JsValue::int(utf16_idx as i32).raw_bits()
                        }
                        None => JsValue::int(-1).raw_bits(),
                    },
                    Err(_) => JsValue::int(-1).raw_bits(),
                }
            }
        }

        // =====================================================================
        // String.prototype.at ( index )
        // §22.1.3.1
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.at
        // =====================================================================
        "at" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. Let len be the length of S.
            let utf16_len = str_data.encode_utf16().count() as i32;
            // 4. Let relativeIndex be ? ToIntegerOrInfinity(index).
            let index = args.first().map_or(0, |v| arg_to_int(v, 0));
            // 5. If relativeIndex >= 0, then
            //    a. Let k be relativeIndex.
            // 6. Else,
            //    a. Let k be len + relativeIndex.
            let actual = if index < 0 { utf16_len + index } else { index };
            // 7. If k < 0 or k >= len, return undefined.
            if actual < 0 || actual >= utf16_len {
                return JsValue::undefined().raw_bits();
            }
            // 8. Return the substring of S from k to k + 1.
            match string_ops::char_at_utf16(str_data, actual as usize) {
                Some(cu) => {
                    let ch = char::from_u32(cu as u32)
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    make_rt_string(ch)
                }
                None => JsValue::undefined().raw_bits(),
            }
        }

        // =====================================================================
        // String.prototype.matchAll ( regexp )
        // §22.1.3.13
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.matchall
        // =====================================================================
        "matchAll" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. If regexp is neither undefined nor null, then
            //    a. Let isRegExp be ? IsRegExp(regexp).
            //    b. If isRegExp is true, then
            //       i. Let flags be ? Get(regexp, "flags").
            //       ii. Perform ? RequireObjectCoercible(flags).
            //       iii. If flags does not contain "g", throw a TypeError exception.
            let first_arg = args.first().map_or(0u64, |v| v.raw_bits());
            let tag = read_obj_tag(first_arg);
            if is_unified_regexp(first_arg, tag) {
                // RegExp must have global flag -- TypeError if not
                let has_global =
                    with_regexp_data_mut(first_arg, tag, |re_data| re_data.inner.flags.global)
                        .unwrap_or(false);
                if !has_global {
                    let msg = rt_api::make_rt_string(
                        "TypeError: String.prototype.matchAll called with a non-global RegExp argument".to_string(),
                    );
                    let err = rt_api::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                    rt_api::__esc_rt_throw(err);
                    return JsValue::undefined().raw_bits();
                }
                //    c. Let matcher be ? GetMethod(regexp, @@matchAll).
                //    d. If matcher is not undefined, return ? Call(matcher, regexp, « O »).
                // NOTE: We inline the @@matchAll logic here rather than dispatching to
                // RegExp.prototype[@@matchAll], collecting all matches eagerly instead
                // of returning a lazy iterator (which the spec requires).
                // TODO: Step 2.c-d — return a proper RegExpStringIterator per §22.2.6.9
                let arr_bits = __esc_rt_create_array(0);
                with_regexp_data_mut(first_arg, tag, |re_data| {
                    re_data.inner.last_index = 0;
                    let matches = re_data.inner.match_all(str_data);
                    for m in &matches {
                        let match_arr = __esc_rt_create_array(0);
                        __esc_rt_array_push(match_arr, make_rt_string(m.full_match.clone()));
                        // Add capture groups
                        for group in &m.groups {
                            let g_bits = match group {
                                Some(s) => make_rt_string(s.clone()),
                                None => JsValue::undefined().raw_bits(),
                            };
                            __esc_rt_array_push(match_arr, g_bits);
                        }
                        // Set index property (byte -> UTF-16)
                        let utf16_idx =
                            string_ops::byte_index_to_utf16(str_data, m.index).unwrap_or(m.index);
                        let index_key = make_rt_string("index".to_string());
                        let index_val = JsValue::number(utf16_idx as f64).raw_bits();
                        __esc_rt_set_prop(match_arr, index_key, index_val);
                        __esc_rt_array_push(arr_bits, match_arr);
                    }
                });
                return arr_bits;
            }
            // 3. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 4. Let rx be ? RegExpCreate(regexp, "g").
            // TODO: Step 4 — should create regexp with "g" flag per spec
            let pattern = {
                let v = JsValue::from_raw_bits(first_arg);
                arg_to_string(&v)
            };
            // 5. Return ? Invoke(rx, @@matchAll, « S »).
            let arr_bits = __esc_rt_create_array(0);
            if pattern.is_empty() {
                // Empty pattern matches at every position
                let utf16_len = str_data.encode_utf16().count();
                for i in 0..=utf16_len {
                    let match_arr = __esc_rt_create_array(0);
                    __esc_rt_array_push(match_arr, make_rt_string(String::new()));
                    let index_key = make_rt_string("index".to_string());
                    let index_val = JsValue::number(i as f64).raw_bits();
                    __esc_rt_set_prop(match_arr, index_key, index_val);
                    __esc_rt_array_push(arr_bits, match_arr);
                }
            } else {
                let mut start = 0;
                while let Some(pos) = str_data[start..].find(&*pattern) {
                    let abs_pos = start + pos;
                    let match_arr = __esc_rt_create_array(0);
                    __esc_rt_array_push(match_arr, make_rt_string(pattern.clone()));
                    // Convert byte index to UTF-16 index for ES semantics
                    let utf16_idx =
                        string_ops::byte_index_to_utf16(str_data, abs_pos).unwrap_or(abs_pos);
                    let index_key = make_rt_string("index".to_string());
                    let index_val = JsValue::number(utf16_idx as f64).raw_bits();
                    __esc_rt_set_prop(match_arr, index_key, index_val);
                    __esc_rt_array_push(arr_bits, match_arr);
                    start = abs_pos + pattern.len();
                }
            }
            arr_bits
        }

        // =====================================================================
        // String.prototype.normalize ( [ form ] )
        // §22.1.3.14
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.normalize
        // =====================================================================
        "normalize" => {
            // 1. Let O be ? RequireObjectCoercible(this value).
            //    (handled above)
            // 2. Let S be ? ToString(O).
            //    (str_data is already the string)
            // 3. If form is undefined, let f be "NFC".
            // 4. Else, let f be ? ToString(form).
            // 5. If f is not one of "NFC", "NFD", "NFKC", or "NFKD", throw a RangeError.
            // 6. Let ns be the String value that is the result of normalizing S into
            //    the normalization form named by f (Unicode Standard Annex #15).
            // 7. Return ns.
            // TODO: Full Unicode normalization (NFC, NFD, NFKC, NFKD) — requires
            // a unicode-normalization crate. For now, return the string unchanged
            // (correct for ASCII-only input).
            make_rt_string(str_data.to_string())
        }

        // =====================================================================
        // String.prototype.toString ( ) / String.prototype.valueOf ( )
        // §22.1.3.27 (toString) / §22.1.3.35 (valueOf)
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.tostring
        // [spec]: https://tc39.es/ecma262/#sec-string.prototype.valueof
        // =====================================================================
        "toString" | "valueOf" => {
            // 1. Return ? thisStringValue(this value).
            //    thisStringValue:
            //    1. If Type(value) is String, return value.
            //    2. If Type(value) is Object and value has a [[StringData]] internal slot, then
            //       a. Let s be value.[[StringData]].
            //       b. Assert: Type(s) is String.
            //       c. Return s.
            //    3. Throw a TypeError exception.
            make_rt_string(str_data.to_string())
        }

        // =====================================================================
        // String length property (not a method, but dispatched here)
        // §10.4.3.3 — length is a non-configurable, non-writable property
        // [spec]: https://tc39.es/ecma262/#sec-properties-of-string-instances-length
        // =====================================================================
        "length" => {
            // The length of a String value is the number of UTF-16 code units in it.
            let utf16_len = str_data.encode_utf16().count();
            JsValue::int(utf16_len as i32).raw_bits()
        }
        _ => JsValue::undefined().raw_bits(),
    }
}

/// Dispatch static `String` methods (`String.fromCharCode`, `String.fromCodePoint`,
/// `String.raw`).
///
/// Returns `Some(result)` if the method is recognized, `None` otherwise.
///
/// [spec]: https://tc39.es/ecma262/#sec-properties-of-the-string-constructor
pub(crate) fn dispatch_string_static_method(
    method: &str,
    argc: u32,
    argv: *const u64,
) -> Option<u64> {
    let args = read_argv(argc, argv);
    match method {
        // =====================================================================
        // String.fromCharCode ( ...codeUnits )
        // §22.1.2.1
        // [spec]: https://tc39.es/ecma262/#sec-string.fromcharcode
        // =====================================================================
        "fromCharCode" => {
            // 1. Let result be the empty String.
            let mut result = String::new();
            // 2. For each element next of codeUnits, do
            for arg in &args {
                //    a. Let nextCU be the code unit whose numeric value is ? ToUint16(next).
                let code = if let Some(n) = arg.as_int() {
                    n as u32
                } else if let Some(n) = arg.as_number() {
                    n as u32
                } else {
                    0
                };
                //    b. Set result to the string-concatenation of result and nextCU.
                if let Some(ch) = char::from_u32(code) {
                    result.push(ch);
                }
                // TODO: Step 2.a — should use ToUint16 (mod 65536), not truncation.
                // char::from_u32 rejects values > 0x10FFFF but doesn't wrap mod 65536.
            }
            // 3. Return result.
            Some(make_rt_string(result))
        }

        // =====================================================================
        // String.fromCodePoint ( ...codePoints )
        // §22.1.2.2
        // [spec]: https://tc39.es/ecma262/#sec-string.fromcodepoint
        // =====================================================================
        "fromCodePoint" => {
            // 1. Let result be the empty String.
            let mut result = String::new();
            // 2. For each element next of codePoints, do
            for arg in &args {
                //    a. Let nextCP be ? ToNumber(next).
                let n_f64 = if let Some(n) = arg.as_int() {
                    n as f64
                } else if let Some(n) = arg.as_number() {
                    n
                } else {
                    //    b. If nextCP is not an integral Number, throw a RangeError exception.
                    let msg = rt_api::make_rt_string("RangeError: Invalid code point".to_string());
                    let err =
                        rt_api::__esc_rt_create_error(exceptions::error_tag::RANGE_ERROR, msg);
                    rt_api::__esc_rt_throw(err);
                    return Some(JsValue::undefined().raw_bits());
                };
                //    b. If nextCP is not an integral Number, throw a RangeError exception.
                if n_f64.fract() != 0.0 || n_f64.is_nan() || n_f64.is_infinite() {
                    let msg = rt_api::make_rt_string("RangeError: Invalid code point".to_string());
                    let err =
                        rt_api::__esc_rt_create_error(exceptions::error_tag::RANGE_ERROR, msg);
                    rt_api::__esc_rt_throw(err);
                    return Some(JsValue::undefined().raw_bits());
                }
                //    c. If R(nextCP) < 0 or R(nextCP) > 0x10FFFF, throw a RangeError exception.
                let cp = n_f64 as i64;
                if !(0..=0x10FFFF).contains(&cp) {
                    let msg = rt_api::make_rt_string("RangeError: Invalid code point".to_string());
                    let err =
                        rt_api::__esc_rt_create_error(exceptions::error_tag::RANGE_ERROR, msg);
                    rt_api::__esc_rt_throw(err);
                    return Some(JsValue::undefined().raw_bits());
                }
                //    d. Set result to the string-concatenation of result and UTF16EncodeCodePoint(R(nextCP)).
                match char::from_u32(cp as u32) {
                    Some(ch) => result.push(ch),
                    None => {
                        let msg =
                            rt_api::make_rt_string("RangeError: Invalid code point".to_string());
                        let err =
                            rt_api::__esc_rt_create_error(exceptions::error_tag::RANGE_ERROR, msg);
                        rt_api::__esc_rt_throw(err);
                        return Some(JsValue::undefined().raw_bits());
                    }
                }
            }
            // 3. Assert: If codePoints is empty, then result is the empty String.
            // 4. Return result.
            Some(make_rt_string(result))
        }

        // =====================================================================
        // String.raw ( template, ...substitutions )
        // §22.1.2.4
        // [spec]: https://tc39.es/ecma262/#sec-string.raw
        // =====================================================================
        "raw" => {
            // 1. Let substitutionCount be the number of elements in substitutions.
            // 2. Let cooked be ? ToObject(template).
            // 3. Let literals be ? ToObject(? Get(cooked, "raw")).
            // 4. Let literalCount be ? LengthOfArrayLike(literals).
            // 5. If literalCount <= 0, return the empty String.
            if args.is_empty() {
                return Some(make_rt_string(String::new()));
            }
            // Try to read the `raw` property from the template object
            let template_bits = args[0].raw_bits();
            let raw_key = make_rt_string("raw".to_string());
            let raw_arr_bits = crate::rt_api::__esc_rt_get_prop(template_bits, raw_key);
            let raw_arr = JsValue::from_raw_bits(raw_arr_bits);
            // Get length of raw array
            let len_key = make_rt_string("length".to_string());
            let len_bits = crate::rt_api::__esc_rt_get_prop(raw_arr_bits, len_key);
            let len = JsValue::from_raw_bits(len_bits)
                .as_int()
                .unwrap_or(0)
                .max(0) as usize;
            if len == 0 {
                return Some(make_rt_string(String::new()));
            }
            // 6. Let R be the empty String.
            let mut result = String::new();
            // 7. Let nextIndex be 0.
            // 8. Repeat,
            for i in 0..len {
                //    a. Let nextLiteralVal be ? Get(literals, ! ToString(𝔽(nextIndex))).
                //    b. Let nextLiteral be ? ToString(nextLiteralVal).
                let idx = JsValue::int(i as i32).raw_bits();
                let raw_elem = crate::rt_api::__esc_rt_get_elem(raw_arr.raw_bits(), idx);
                //    c. Set R to the string-concatenation of R and nextLiteral.
                if let Some(s) = extract_key_string(raw_elem) {
                    result.push_str(&s);
                }
                //    d. If nextIndex + 1 = literalCount, return R.
                //    e. If nextIndex < substitutionCount, then
                //       i. Let nextSubVal be substitutions[nextIndex].
                //       ii. Let nextSub be ? ToString(nextSubVal).
                //       iii. Set R to the string-concatenation of R and nextSub.
                if i + 1 < len
                    && let Some(sub) = args.get(i + 1)
                {
                    if let Some(s) = extract_key_string(sub.raw_bits()) {
                        result.push_str(&s);
                    } else if let Some(n) = sub.as_int() {
                        result.push_str(&n.to_string());
                    } else if let Some(n) = sub.as_number() {
                        result.push_str(&n.to_string());
                    }
                }
                //    f. Set nextIndex to nextIndex + 1.
            }
            Some(make_rt_string(result))
        }
        _ => None,
    }
}
