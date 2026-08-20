//! RegExp method dispatch.
//!
//! Contains `dispatch_regexp_method` for routing method calls on RegExp objects.
//! Supports `test`, `exec`, `toString`, and well-known symbol methods
//! (`Symbol.match`, `Symbol.search`, `Symbol.replace`, `Symbol.split`).

use nanbox::JsValue;

use crate::internal_data::{InternalData, UnifiedObject};
use crate::tagged_obj::{ObjTag, deref_tagged, deref_tagged_mut, read_obj_tag};

use super::{
    __esc_rt_array_push, __esc_rt_create_array, __esc_rt_regexp_exec, __esc_rt_regexp_test,
    extract_key_string, make_rt_string, read_argv,
};

/// Throw a TypeError with the given message and return `undefined` bits.
///
/// Used by RegExp methods when the receiver is not a valid RegExp.
fn throw_regexp_type_error(msg: &str) -> u64 {
    let msg_bits = make_rt_string(format!("TypeError: {msg}"));
    let err = super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg_bits);
    super::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

// =========================================================================
// RegExp dispatch
// =========================================================================

/// Dispatch a method call on a RegExp object.
///
/// Routes to the appropriate handler based on the method name string.
/// Supports `"test"`, `"exec"`, `"toString"`, and well-known symbol methods
/// `"Symbol.match"`, `"Symbol.search"`, `"Symbol.replace"`, and `"Symbol.split"`.
///
/// This is an internal dispatch function with no direct spec equivalent.
pub(crate) fn dispatch_regexp_method(obj: u64, method: &str, argc: u32, argv: *const u64) -> u64 {
    let args = read_argv(argc, argv);
    let first_arg = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());

    // For exec and symbol methods, validate receiver is an Object.
    // Per spec, these methods require `this` to be an Object (typically a RegExp).
    match method {
        "exec" | "Symbol.match" | "Symbol.search" | "Symbol.replace" | "Symbol.split" => {
            let obj_val = JsValue::from_raw_bits(obj);
            if !obj_val.is_object() {
                return throw_regexp_type_error(&format!(
                    "RegExp.prototype.{method} called on non-object"
                ));
            }
        }
        _ => {}
    }

    match method {
        "test" => __esc_rt_regexp_test(obj, first_arg),
        "exec" => __esc_rt_regexp_exec(obj, first_arg),
        "toString" => regexp_to_string(obj),
        "Symbol.match" => regexp_symbol_match(obj, first_arg),
        "Symbol.search" => regexp_symbol_search(obj, first_arg),
        "Symbol.replace" => {
            let second_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            regexp_symbol_replace(obj, first_arg, second_arg)
        }
        "Symbol.split" => {
            let second_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            regexp_symbol_split(obj, first_arg, second_arg)
        }
        _ => JsValue::undefined().raw_bits(),
    }
}

// =========================================================================
// toString
// =========================================================================

/// `RegExp.prototype.toString ( )`
///
/// Returns the string `"/" + source + "/" + flags`.
///
/// [spec]: https://tc39.es/ecma262/#sec-regexp.prototype.tostring
fn regexp_to_string(obj: u64) -> u64 {
    // 1. Let R be the this value.
    // 2. If R is not an Object, throw a TypeError exception.
    let obj_val = JsValue::from_raw_bits(obj);
    if !obj_val.is_object() {
        return throw_regexp_type_error("RegExp.prototype.toString called on non-object");
    }
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        return make_rt_string("/undefined/undefined".to_string());
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(obj)
    };
    if let Some(u) = uni
        && let Some(InternalData::RegExp { inner }) = u.internal_data()
        && let Some(re) = inner.downcast_ref::<crate::regexp_bridge::JsRegExpData>()
    {
        // 3. Let pattern be ? ToString(? Get(R, "source")).
        // 4. Let flags be ? ToString(? Get(R, "flags")).
        // (We read directly from the internal data rather than using Get.)
        // 5. Let result be the string-concatenation of "/", pattern, "/", and flags.
        let s = format!("/{}/{}", re.inner.pattern, re.flags_string());
        // 6. Return result.
        return make_rt_string(s);
    }
    // Per spec step 3-4, generic objects read "source" and "flags" as properties.
    // For objects without those properties, Get returns undefined, so toString is "undefined".
    make_rt_string("/undefined/undefined".to_string())
}

// =========================================================================
// Symbol.match
// =========================================================================

/// `RegExp.prototype [ @@match ] ( string )`
///
/// For non-global regexp, performs a single exec and returns the result array.
/// For global regexp, collects all match strings into an array.
///
/// [spec]: https://tc39.es/ecma262/#sec-regexp.prototype-@@match
fn regexp_symbol_match(obj: u64, input: u64) -> u64 {
    // 1. Let rx be the this value.
    // 2. If rx is not an Object, throw a TypeError exception.
    // (Implicit — we check the tag below.)

    // 3. Let S be ? ToString(string).
    let input_str = extract_key_string(input).unwrap_or_default();

    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        // TODO: Step 2 — should throw TypeError
        return JsValue::null().raw_bits();
    }

    // 4. Let flags be ? ToString(? Get(rx, "flags")).
    // 5. If flags does not contain "g", then
    let is_global = {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(obj)
        };
        if let Some(u) = uni
            && let Some(InternalData::RegExp { inner }) = u.internal_data()
            && let Some(re) = inner.downcast_ref::<crate::regexp_bridge::JsRegExpData>()
        {
            re.inner.flags.global
        } else {
            return JsValue::null().raw_bits();
        }
    };

    if !is_global {
        // 5. If flags does not contain "g", then
        //   a. Return ? RegExpExec(rx, S).
        return __esc_rt_regexp_exec(obj, input);
    }

    // 6. Else,
    //   a. If flags contains "u" or "v", let fullUnicode be true.
    //      Else, let fullUnicode be false.
    // TODO: Step 6a — fullUnicode flag handling

    //   b. Perform ? Set(rx, "lastIndex", +0F, true).
    let uni = unsafe {
        // SAFETY: tag check above confirmed this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return JsValue::null().raw_bits();
    };
    let Some(InternalData::RegExp { inner }) = u.internal_data_mut() else {
        return JsValue::null().raw_bits();
    };
    let Some(re) = inner.downcast_mut::<crate::regexp_bridge::JsRegExpData>() else {
        return JsValue::null().raw_bits();
    };

    re.inner.last_index = 0;

    //   c. Let A be ! ArrayCreate(0).
    //   d. Let n be 0.
    //   e. Repeat,
    //     i. Let result be ? RegExpExec(rx, S).
    //     ii. If result is null, then
    //       1. If n = 0, return null.
    //       2. Return A.
    //     iii. Else,
    //       1. Let matchStr be ? ToString(? Get(result, "0")).
    //       2. Perform ! CreateDataPropertyOrThrow(A, ! ToString(F(n)), matchStr).
    //       3. If matchStr is the empty String, then
    //         a. Let thisIndex be AdvanceStringIndex(S, ...).
    //         b. Perform ? Set(rx, "lastIndex", ..., true).
    //       4. Set n to n + 1.
    let matches = re.inner.match_all(&input_str);
    if matches.is_empty() {
        //   ii. If result is null, then
        //     1. If n = 0, return null.
        return JsValue::null().raw_bits();
    }

    let arr = __esc_rt_create_array(0);
    for m in &matches {
        let s = make_rt_string(m.full_match.clone());
        __esc_rt_array_push(arr, s);
    }
    //   ii. 2. Return A.
    arr
}

// =========================================================================
// Symbol.search
// =========================================================================

/// `RegExp.prototype [ @@search ] ( string )`
///
/// Executes the regexp against the string and returns the index of the
/// first match, or -1 if no match is found.
///
/// [spec]: https://tc39.es/ecma262/#sec-regexp.prototype-@@search
fn regexp_symbol_search(obj: u64, input: u64) -> u64 {
    // 1. Let rx be the this value.
    // 2. If rx is not an Object, throw a TypeError exception.
    // (Implicit — we check the tag below.)

    // 3. Let S be ? ToString(string).
    let input_str = extract_key_string(input).unwrap_or_default();

    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        // TODO: Step 2 — should throw TypeError
        return JsValue::number(-1.0).raw_bits();
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return JsValue::number(-1.0).raw_bits();
    };
    let Some(InternalData::RegExp { inner }) = u.internal_data_mut() else {
        return JsValue::number(-1.0).raw_bits();
    };
    let Some(re) = inner.downcast_mut::<crate::regexp_bridge::JsRegExpData>() else {
        return JsValue::number(-1.0).raw_bits();
    };

    // 4. Let previousLastIndex be ? Get(rx, "lastIndex").
    let saved_last_index = re.inner.last_index;
    // 5. If SameValue(previousLastIndex, +0F) is false, then
    //   a. Perform ? Set(rx, "lastIndex", +0F, true).
    re.inner.last_index = 0;
    // 6. Let result be ? RegExpExec(rx, S).
    let result = re.inner.exec(&input_str);
    // 7. Let currentLastIndex be ? Get(rx, "lastIndex").
    // 8. If SameValue(currentLastIndex, previousLastIndex) is false, then
    //   a. Perform ? Set(rx, "lastIndex", previousLastIndex, true).
    re.inner.last_index = saved_last_index;

    match result {
        // 9. If result is null, return -1F.
        None => JsValue::number(-1.0).raw_bits(),
        // 10. Return ? Get(result, "index").
        Some(m) => JsValue::number(m.index as f64).raw_bits(),
    }
}

// =========================================================================
// Symbol.replace
// =========================================================================

/// `RegExp.prototype [ @@replace ] ( string, replaceValue )`
///
/// Replaces matches with the replacement string. For global regexp, replaces
/// all matches. Supports `$&` (matched text), `` $` `` (before match),
/// `$'` (after match), and `$1`-`$9` capture group references.
///
/// [spec]: https://tc39.es/ecma262/#sec-regexp.prototype-@@replace
fn regexp_symbol_replace(obj: u64, input: u64, replacement: u64) -> u64 {
    // 1. Let rx be the this value.
    // 2. If rx is not an Object, throw a TypeError exception.
    // (Implicit — we check the tag below.)

    // 3. Let S be ? ToString(string).
    let input_str = extract_key_string(input).unwrap_or_default();
    // 5. Let functionalReplace be IsCallable(replaceValue).
    let functional_replace = super::dispatch_core::is_callable(replacement);
    let repl_str = if functional_replace {
        String::new()
    } else {
        extract_key_string(replacement).unwrap_or_default()
    };

    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        // TODO: Step 2 — should throw TypeError
        return make_rt_string(input_str);
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return make_rt_string(input_str);
    };
    let Some(InternalData::RegExp { inner }) = u.internal_data_mut() else {
        return make_rt_string(input_str);
    };
    let Some(re) = inner.downcast_mut::<crate::regexp_bridge::JsRegExpData>() else {
        return make_rt_string(input_str);
    };

    // 4. Let lengthS be the length of S.
    // 5. Let functionalReplace be IsCallable(replaceValue).
    // TODO: Step 5 — functional replace not supported, only string replacement

    // 6. If functionalReplace is false, then
    //   a. Let replacement be ? ToString(replaceValue).
    // (Already done above via extract_key_string.)

    // 7. Let flags be ? ToString(? Get(rx, "flags")).
    // 8. If flags contains "g", then
    //   a. Let global be true.
    //   b. If flags contains "u" or "v", let fullUnicode be true.
    //      Else, let fullUnicode be false.
    // 9. Else, let global be false.
    let is_global = re.inner.flags.global;
    // TODO: Step 8b — fullUnicode flag handling

    // 10. If global is true, then
    //   a. Perform ? Set(rx, "lastIndex", +0F, true).
    re.inner.last_index = 0;

    // 11. Let results be a new empty List.
    // 12. Let done be false.
    // 13. Repeat, while done is false,
    //   a. Let result be ? RegExpExec(rx, S).
    //   b. If result is null, then set done to true.
    //   c. Else,
    //     i. Append result to results.
    //     ii. If global is false, set done to true.
    //     iii. Else,
    //       1. Let matchStr be ? ToString(? Get(result, "0")).
    //       2. If matchStr is "", then advance lastIndex.
    let mut result = String::new();
    let mut last_end = 0usize;

    while let Some(m) = re.inner.exec(&input_str) {
        // 14. Let accumulatedResult be the empty String.
        // 15. Let nextSourcePosition be 0.
        // 16. For each element result of results, do
        //   a. Let nCaptures be ? LengthOfArrayLike(result) - 1.
        //   b. Let matched be ? ToString(? Get(result, "0")).
        //   c. Let matchLength be the length of matched.
        //   d. Let position be ? ToIntegerOrInfinity(? Get(result, "index")).
        //   e. Set position to the result of clamping position between 0 and lengthS.

        // Append text before the match
        if m.index > last_end {
            result.push_str(&input_str[last_end..m.index]);
        }

        //   f-j. Build replacement string using captures and substitution template.
        //   k. Let replacement be ? GetSubstitution or Call(replaceValue, ...).
        let replaced = if functional_replace {
            // Call replaceValue(matched, p1...pN, position, string)
            let mut call_args: Vec<u64> = Vec::new();
            call_args.push(make_rt_string(m.full_match.clone()));
            for group in &m.groups {
                match group {
                    Some(g) => call_args.push(make_rt_string(g.clone())),
                    None => call_args.push(JsValue::undefined().raw_bits()),
                }
            }
            call_args.push(JsValue::int(m.index as i32).raw_bits());
            call_args.push(make_rt_string(input_str.clone()));
            let res = unsafe {
                super::dispatch_core::__esc_rt_call_indirect(
                    replacement,
                    call_args.len() as i32,
                    call_args.as_ptr(),
                )
            };
            extract_key_string(res)
                .unwrap_or_else(|| crate::display::display_value(JsValue::from_raw_bits(res)))
        } else {
            // TODO: named captures ($<name>) not yet supported
            apply_replacement_pattern(&repl_str, &m.full_match, &m.groups, &input_str, m.index)
        };
        //   l. If position >= nextSourcePosition, then
        //     i. Set accumulatedResult to the string-concatenation of
        //        accumulatedResult, the substring of S from nextSourcePosition to
        //        position, and replacement.
        //     ii. Set nextSourcePosition to position + matchLength.
        result.push_str(&replaced);

        last_end = m.index + m.full_match.len();

        // Guard against zero-length match infinite loop
        // (Spec step 13.c.iii.2: If matchStr is "", advance string index)
        if m.full_match.is_empty() {
            if last_end < input_str.len() {
                result.push_str(&input_str[last_end..last_end + 1]);
                last_end += 1;
                re.inner.last_index = last_end;
            } else {
                break;
            }
        }

        if !is_global {
            break;
        }
    }

    // 17. If nextSourcePosition >= lengthS, return accumulatedResult.
    // 18. Return the string-concatenation of accumulatedResult and the substring of
    //     S from nextSourcePosition.
    if last_end < input_str.len() {
        result.push_str(&input_str[last_end..]);
    }

    make_rt_string(result)
}

/// `GetSubstitution ( matched, str, position, captures, namedCaptures, replacementTemplate )`
///
/// Applies replacement pattern substitution for `$$`, `$&`, `` $` ``, `$'`,
/// and `$1`-`$9` / `$10`-`$99` capture group references.
///
/// [spec]: https://tc39.es/ecma262/#sec-getsubstitution
fn apply_replacement_pattern(
    pattern: &str,
    matched: &str,
    groups: &[Option<String>],
    input: &str,
    match_index: usize,
) -> String {
    // 1. Let stringLength be the length of str.
    // 2. Let templateRemainder be replacementTemplate.
    // 3. Let result be the empty String.
    let mut result = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    // 4. Repeat, while templateRemainder is not the empty String,
    while i < chars.len() {
        //   a. NOTE: The following steps isolate ref (a prefix of templateRemainder)
        //      and the replacement for ref (refReplacement).
        //   b-c. If templateRemainder starts with "$$", then
        //        ref is "$$" and refReplacement is "$".
        //   d-e. If starts with "$`", ref is "$`", refReplacement is
        //        the substring of str from 0 to position.
        //   f-g. If starts with "$&", ref is "$&", refReplacement is matched.
        //   h-i. If starts with "$'", ref is "$'", refReplacement is
        //        the substring of str from position + matchLength.
        //   j-k. If starts with "$<" ... (named captures).
        //   l-m. If starts with "$N" or "$NN" (capture group references).
        //   n. Else, ref is the first code unit, refReplacement is ref.
        if chars[i] == '$' && i + 1 < chars.len() {
            match chars[i + 1] {
                // b-c. "$$" -> "$"
                '$' => {
                    result.push('$');
                    i += 2;
                }
                // f-g. "$&" -> matched substring
                '&' => {
                    result.push_str(matched);
                    i += 2;
                }
                // d-e. "$`" -> portion of string before the match
                '`' => {
                    result.push_str(&input[..match_index]);
                    i += 2;
                }
                // h-i. "$'" -> portion of string after the match
                '\'' => {
                    let after = match_index + matched.len();
                    if after < input.len() {
                        result.push_str(&input[after..]);
                    }
                    i += 2;
                }
                // l-m. "$N" or "$NN" -> capture group reference
                c if c.is_ascii_digit() && c != '0' => {
                    let digit = (c as u32 - '0' as u32) as usize;
                    // Check for two-digit reference ($10-$99)
                    let group_idx = if i + 2 < chars.len() && chars[i + 2].is_ascii_digit() {
                        let two_digit = digit * 10 + (chars[i + 2] as u32 - '0' as u32) as usize;
                        if two_digit <= groups.len() && two_digit > 0 {
                            i += 3;
                            two_digit
                        } else {
                            i += 2;
                            digit
                        }
                    } else {
                        i += 2;
                        digit
                    };
                    if group_idx > 0 && group_idx <= groups.len() {
                        if let Some(ref g) = groups[group_idx - 1] {
                            result.push_str(g);
                        }
                        // (If the capture is undefined, refReplacement is the empty String.)
                    } else {
                        // No such group — output the literal $N
                        result.push('$');
                        result.push(c);
                        if group_idx != digit {
                            // We consumed an extra digit
                            result.push(chars[i - 1]);
                        }
                    }
                }
                // TODO: Step j-k — "$<name>" named capture groups not implemented
                // n. Otherwise, ref is "$" and refReplacement is "$".
                _ => {
                    result.push('$');
                    i += 1;
                }
            }
        } else {
            // n. ref is the single code unit, refReplacement is ref.
            result.push(chars[i]);
            i += 1;
        }
    }
    //   o. Set result to the string-concatenation of result and refReplacement.
    //   p. Set templateRemainder to ...
    // 5. Return result.
    result
}

// =========================================================================
// Symbol.split
// =========================================================================

/// `RegExp.prototype [ @@split ] ( string, limit )`
///
/// Splits the input string by the regex pattern, returning an array of
/// substrings. If `limit` is provided, the result is truncated.
///
/// [spec]: https://tc39.es/ecma262/#sec-regexp.prototype-@@split
fn regexp_symbol_split(obj: u64, input: u64, limit: u64) -> u64 {
    // 1. Let rx be the this value.
    // 2. If rx is not an Object, throw a TypeError exception.
    // (Implicit — we check the tag below.)

    // TODO: Step 3 — Let C be ? SpeciesConstructor(rx, %RegExp%).
    // TODO: Step 4 — Let flags be ? ToString(? Get(rx, "flags")).
    // TODO: Step 5-6 — Create splitter via Construct(C, rx, newFlags) with sticky flag.

    // 7. Let S be ? ToString(string).
    let input_str = extract_key_string(input).unwrap_or_default();

    // 8. Let A be ! ArrayCreate(0).
    // 9. Let lengthA be 0.

    // 10. If limit is undefined, let lim be 2^32 - 1; else let lim be
    //     ? ToUint32(limit).
    let limit_val = JsValue::from_raw_bits(limit);
    let max_parts = if limit_val.is_undefined() {
        u32::MAX
    } else if let Some(n) = limit_val.as_number() {
        if n < 0.0 || n.is_nan() {
            u32::MAX
        } else {
            n as u32
        }
    } else if let Some(n) = limit_val.as_int() {
        if n < 0 { u32::MAX } else { n as u32 }
    } else {
        u32::MAX
    };

    let arr = __esc_rt_create_array(0);

    // 11. If lim is 0, return A.
    if max_parts == 0 {
        return arr;
    }

    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        // Not a valid regexp — return array with the whole string
        let s = make_rt_string(input_str);
        __esc_rt_array_push(arr, s);
        return arr;
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        let s = make_rt_string(input_str);
        __esc_rt_array_push(arr, s);
        return arr;
    };
    let Some(InternalData::RegExp { inner }) = u.internal_data_mut() else {
        let s = make_rt_string(input_str);
        __esc_rt_array_push(arr, s);
        return arr;
    };
    let Some(re) = inner.downcast_mut::<crate::regexp_bridge::JsRegExpData>() else {
        let s = make_rt_string(input_str);
        __esc_rt_array_push(arr, s);
        return arr;
    };

    // 12. Let size be the length of S.
    // 13. If size is 0, then
    if input_str.is_empty() {
        //   a. Let z be ? RegExpExec(splitter, S).
        re.inner.last_index = 0;
        let m = re.inner.exec("");
        //   b. If z is not null, return A.
        if m.is_some() {
            return arr;
        }
        //   c. Perform ! CreateDataPropertyOrThrow(A, "0", S).
        let s = make_rt_string(String::new());
        __esc_rt_array_push(arr, s);
        //   d. Return A.
        return arr;
    }

    // 14. Let p be 0.
    // 15. Let q be p.
    re.inner.last_index = 0;
    let mut parts: u32 = 0;
    let mut last_end = 0usize;

    // Save global flag state — we need to iterate all matches
    // (Spec uses a splitter with sticky flag; we emulate by setting global.)
    let was_global = re.inner.flags.global;
    re.inner.flags.global = true;
    re.inner.last_index = 0;

    // 16. Repeat, while q < size,
    while let Some(m) = re.inner.exec(&input_str) {
        // Skip zero-length matches at the start of the string that don't advance
        // (Spec step 16.c.iii: If z is null or e = p, advance q.)
        if m.index == last_end && m.full_match.is_empty() {
            if re.inner.last_index <= input_str.len() {
                continue;
            }
            break;
        }

        // 16.c.iv. Else,
        //   1. Let T be the substring of S from p to q.
        //   2. Perform ! CreateDataPropertyOrThrow(A, ! ToString(F(lengthA)), T).
        //   3. Set lengthA to lengthA + 1.
        let segment = &input_str[last_end..m.index];
        let seg_bits = make_rt_string(segment.to_string());
        __esc_rt_array_push(arr, seg_bits);
        parts += 1;
        //   4. If lengthA = lim, return A.
        if parts >= max_parts {
            re.inner.flags.global = was_global;
            return arr;
        }

        //   5. Set p to e.
        //   6. Let numberOfCaptures be ? LengthOfArrayLike(z) - 1.
        //   7. Set numberOfCaptures to max(numberOfCaptures, 0).
        //   8. Let i be 1.
        //   9. Repeat, while i <= numberOfCaptures,
        for group in &m.groups {
            //   a. Let nextCapture be ? Get(z, ! ToString(F(i))).
            //   b. Perform ! CreateDataPropertyOrThrow(A, ! ToString(F(lengthA)), nextCapture).
            //   c. Set i to i + 1.
            //   d. Set lengthA to lengthA + 1.
            let g_bits = match group {
                Some(g) => make_rt_string(g.clone()),
                None => JsValue::undefined().raw_bits(),
            };
            __esc_rt_array_push(arr, g_bits);
            parts += 1;
            //   e. If lengthA = lim, return A.
            if parts >= max_parts {
                re.inner.flags.global = was_global;
                return arr;
            }
        }

        //   10. Set q to p.
        last_end = m.index + m.full_match.len();

        // Guard against zero-length match infinite loop
        // (Spec step 16.c.iii: advance q when match is empty.)
        if m.full_match.is_empty() {
            if last_end < input_str.len() {
                last_end += 1;
                re.inner.last_index = last_end;
            } else {
                break;
            }
        }
    }

    // 17. Let T be the substring of S from p to size.
    // 18. Perform ! CreateDataPropertyOrThrow(A, ! ToString(F(lengthA)), T).
    if parts < max_parts {
        let remaining = &input_str[last_end..];
        let rem_bits = make_rt_string(remaining.to_string());
        __esc_rt_array_push(arr, rem_bits);
    }

    // Restore original global flag
    re.inner.flags.global = was_global;

    // 19. Return A.
    arr
}
