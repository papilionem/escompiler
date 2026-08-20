//! JSON method dispatch.
//!
//! Contains `dispatch_json_method` for `JSON.parse` and `JSON.stringify`,
//! plus the recursive-descent parser and serializer helpers.

use nanbox::JsValue;

use crate::tagged_obj::{ObjTag, deref_tagged, read_obj_tag};

use super::{
    __esc_rt_create_object, __esc_rt_delete_prop, __esc_rt_get_prop, __esc_rt_object_keys,
    __esc_rt_set_prop, create_array_from_elements, create_empty_array, extract_key_string,
    make_rt_string, read_argv,
};

/// Configuration for JSON.stringify, carrying replacer and space through recursion.
struct StringifyConfig {
    /// Indent string per level (empty = no indentation).
    indent_str: String,
    /// Replacer array: if Some, only these keys are included.
    replacer_keys: Option<Vec<String>>,
    /// Replacer function: if Some, called as `replacer(key, value)` for each property.
    replacer_fn: Option<u64>,
}

/// Dispatch a JSON method (`JSON.parse`, `JSON.stringify`).
///
/// Implements the top-level entry points for the `JSON` global object.
///
/// - `JSON.parse(text [, reviver])` — [spec]: https://tc39.es/ecma262/#sec-json.parse (§25.5.1)
/// - `JSON.stringify(value [, replacer [, space]])` — [spec]: https://tc39.es/ecma262/#sec-json.stringify (§25.5.2)
///
/// Returns `Some(result)` if the method is recognized, `None` otherwise.
pub(crate) fn dispatch_json_method(method: &str, argc: u32, argv: *const u64) -> Option<u64> {
    let args = read_argv(argc, argv);
    match method {
        // ----- JSON.parse ( text [ , reviver ] ) — §25.5.1 -----
        "parse" => {
            // 1. Let jsonString be ? ToString(text).
            let text = args
                .first()
                .and_then(|v| extract_key_string(v.raw_bits()))
                .unwrap_or_default();
            // 2. Parse StringToCodePoints(jsonString) as a JSON text (ECMA-404).
            //    Throw a SyntaxError if it is not a valid JSON text.
            let trimmed = text.trim();
            let bytes = trimmed.as_bytes();
            match json_parse_value(bytes, 0) {
                Some((result, _)) => {
                    // 3. Let scriptString be the string-concatenation of "(", jsonString, and ");".
                    // 4. Let script be ParseText(StringToCodePoints(scriptString), Script).
                    // NOTE: Steps 3-4 describe the spec's "parse as script" approach;
                    //       this implementation uses a direct recursive-descent JSON parser instead.

                    // 5. Let completion be Completion(Evaluation of script).
                    //    (result already holds the parsed value)

                    // 6. NOTE: The syntax of a valid JSON text is a subset of ECMAScript.
                    // 7. Let unfiltered be completion.[[Value]].

                    // 8. If IsCallable(reviver) is true, then
                    let reviver = args.get(1).copied().unwrap_or(JsValue::undefined());
                    if is_callable(reviver.raw_bits()) {
                        // 8a. Let root be OrdinaryObjectCreate(%Object.prototype%).
                        let holder = __esc_rt_create_object();
                        // 8b. Let rootName be the empty String.
                        let empty_key = make_rt_string(String::new());
                        // 8c. Perform ! CreateDataPropertyOrThrow(root, rootName, unfiltered).
                        __esc_rt_set_prop(holder, empty_key, result);
                        // 8d. Return ? InternalizeJSONProperty(root, rootName, reviver).
                        let revived = internalize_json_property(holder, "", reviver.raw_bits());
                        Some(revived)
                    } else {
                        // 9. Else,
                        //    a. Return unfiltered.
                        Some(result)
                    }
                }
                // Parse failure — spec says throw SyntaxError.
                None => {
                    // 2. Parse StringToCodePoints(jsonString) as a JSON text (ECMA-404).
                    //    Throw a SyntaxError if it is not a valid JSON text.
                    let msg =
                        make_rt_string("SyntaxError: Unexpected end of JSON input".to_string());
                    let err = super::__esc_rt_create_error(
                        crate::exceptions::error_tag::SYNTAX_ERROR,
                        msg,
                    );
                    super::__esc_rt_throw(err);
                    Some(JsValue::undefined().raw_bits())
                }
            }
        }
        // ----- JSON.stringify ( value [ , replacer [ , space ] ] ) — §25.5.2 -----
        "stringify" => {
            // 1. Let stack be a new empty List.
            //    (We use `seen: HashSet<u64>` for cycle detection below.)

            // 2. Let indent be the empty String.
            //    (Built into `config.indent_str` at depth 0.)

            // 3. Let PropertyList and ReplacerFunction be undefined.
            let val = args.first().copied().unwrap_or(JsValue::undefined());

            // 4. If Type(replacer) is Object, then ...
            //    a. If IsCallable(replacer) is true, let ReplacerFunction be replacer.
            //    b. Else, if replacer is an Array, build PropertyList.
            let replacer_arg = args.get(1).copied().unwrap_or(JsValue::undefined());
            let replacer_fn = if !replacer_arg.is_undefined()
                && !replacer_arg.is_null()
                && replacer_arg.is_object()
                && is_callable(replacer_arg.raw_bits())
            {
                Some(replacer_arg.raw_bits())
            } else {
                None
            };
            let replacer_keys = if replacer_fn.is_none() {
                parse_replacer(replacer_arg)
            } else {
                None
            };

            // 5-8. Process the space argument (number → spaces, string → truncated to 10).
            let indent_str = parse_space_arg(args.get(2).copied().unwrap_or(JsValue::undefined()));

            // 9. Let gap be indent (the resolved indent string).
            let config = StringifyConfig {
                indent_str,
                replacer_keys,
                replacer_fn,
            };

            // 10. Let wrapper be OrdinaryObjectCreate(%Object.prototype%).
            // 11. Perform ! CreateDataPropertyOrThrow(wrapper, "", value).
            // 12. Let state be Record { ... }.
            // 13. Return ? SerializeJSONProperty(state, "", wrapper).
            let mut seen = std::collections::HashSet::new();
            match json_stringify_value_ext(val, &config, 0, &mut seen) {
                Some(s) => Some(make_rt_string(s)),
                None => Some(JsValue::undefined().raw_bits()),
            }
        }
        _ => None,
    }
}

/// Parse the `replacer` argument for `JSON.stringify` (§25.5.2 steps 4a-4b).
///
/// `JSON.stringify ( value [ , replacer [ , space ] ] )`
///
/// [spec]: https://tc39.es/ecma262/#sec-json.stringify
///
/// Returns `Some(keys)` for array replacer (key whitelist), `None` otherwise.
/// Function replacer is recognized but deferred (requires calling back into compiled code).
fn parse_replacer(replacer: JsValue) -> Option<Vec<String>> {
    // 4. If Type(replacer) is Object, then
    if replacer.is_undefined() || replacer.is_null() {
        return None;
    }
    // 4a. If IsCallable(replacer) is true, let ReplacerFunction be replacer.
    // TODO: Step 4a — function replacer support (requires __esc_rt_call_indirect callback)
    if is_callable(replacer.raw_bits()) {
        return None;
    }
    // 4b. Else,
    //   i. Let isArray be ? IsArray(replacer).
    //   ii. If isArray is true, then
    let tag = read_obj_tag(replacer.raw_bits());
    if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<crate::internal_data::UnifiedObject>(replacer.raw_bits())
        };
        if let Some(u) = uni
            && u.kind == crate::internal_data::InternalKind::Array
        {
            //     1. Let len be ? LengthOfArrayLike(replacer).
            //     2. Let k be 0.
            //     3. Repeat, while k < len,
            //        a. Let v be ? Get(replacer, ! ToString(F(k))).
            //        b. If Type(v) is String or Number, add ToString(v) to PropertyList
            //           (if not already present).
            //        c. Set k to k + 1.
            // NOTE: We simplify by collecting all string-coercible elements.
            // TODO: Step 4b.ii.3 — handle Number elements via ToString coercion,
            //       deduplicate keys, handle String wrapper objects.
            let elements = u.array_elements_resolved();
            let keys: Vec<String> = elements
                .iter()
                .filter_map(|v| extract_key_string(v.raw_bits()))
                .collect();
            return Some(keys);
        }
    }
    None
}

/// Parse the `space` argument for `JSON.stringify` (§25.5.2 steps 5-8).
///
/// `JSON.stringify ( value [ , replacer [ , space ] ] )`
///
/// [spec]: https://tc39.es/ecma262/#sec-json.stringify
///
/// Returns the indent string (empty = no indentation).
fn parse_space_arg(space: JsValue) -> String {
    // 5. If Type(space) is Object, then
    //    a. If space has a [[NumberData]] internal slot, let space be ? ToNumber(space).
    //    b. Else if space has a [[StringData]] internal slot, let space be ? ToString(space).
    // TODO: Step 5 — unwrap Number/String wrapper objects.

    // 6. If Type(space) is Number, then
    if let Some(n) = space.as_int() {
        //    a. Let spaceMV be ! ToInteger(space).
        //    b. Set spaceMV to min(spaceMV, 10).
        //    c. If spaceMV < 1, let gap be the empty String.
        //    d. Else, let gap be the String value consisting of spaceMV code units of U+0020.
        let n = n.clamp(0, 10) as usize;
        if n > 0 {
            return " ".repeat(n);
        }
    } else if let Some(n) = space.as_number() {
        // 6 (continued). Same logic for f64 number values.
        let n = (n as i32).clamp(0, 10) as usize;
        if n > 0 {
            return " ".repeat(n);
        }
    } else if let Some(s) = extract_key_string(space.raw_bits()) {
        // 7. Else if Type(space) is String, then
        //    a. If the length of space >= 10, let gap be the substring of space from 0 to 10.
        //    b. Else, let gap be space.
        let truncated: String = s.chars().take(10).collect();
        if !truncated.is_empty() {
            return truncated;
        }
    }
    // 8. Else, let gap be the empty String.
    String::new()
}

/// Check if a value is callable (function, closure, or native function).
///
/// Implements the abstract operation `IsCallable(argument)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-iscallable
///
/// Returns `true` if the value has a `[[Call]]` internal method.
fn is_callable(bits: u64) -> bool {
    // 1. If argument is not an Object, return false.
    let tag = read_obj_tag(bits);
    if tag != Some(ObjTag::Unified as u8) {
        return false;
    }
    // 2. If argument has a [[Call]] internal method, return true.
    // 3. Return false.
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<crate::internal_data::UnifiedObject>(bits)
    };
    uni.is_some_and(|u| {
        matches!(
            u.kind,
            crate::internal_data::InternalKind::Function
                | crate::internal_data::InternalKind::Closure
                | crate::internal_data::InternalKind::NativeFunc
        )
    })
}

// TODO(v0.8): Reviver function support for JSON.parse requires calling back
// into compiled code via __esc_rt_call_indirect. Deferred because the calling
// convention needs careful handling in the AOT context.

// -------------------------------------------------------------------------
// JSON parse helpers — recursive descent parser producing real objects/arrays
// -------------------------------------------------------------------------
// These functions implement the JSON parsing grammar from ECMA-404 / §25.5.1.
// The spec describes parsing via "parse as a Script" (steps 3-5), but this
// implementation uses a direct recursive-descent parser for efficiency in
// the AOT context.

/// Parse a JSON value from bytes, returning the NaN-boxed result and next offset.
///
/// Implements the top-level JSON *value* production from ECMA-404 §5:
///
/// > *value* : **null** | **true** | **false** | *string* | *number* | *object* | *array*
///
/// [spec]: https://www.ecma-international.org/publications-and-standards/standards/ecma-404/
fn json_parse_value(bytes: &[u8], start: usize) -> Option<(u64, usize)> {
    let pos = json_skip_ws(bytes, start);
    if pos >= bytes.len() {
        return None;
    }
    match bytes[pos] {
        b'n' => json_parse_null(bytes, pos),
        b't' => json_parse_true(bytes, pos),
        b'f' => json_parse_false(bytes, pos),
        b'"' => json_parse_string(bytes, pos),
        b'[' => json_parse_array(bytes, pos),
        b'{' => json_parse_object(bytes, pos),
        b'-' | b'0'..=b'9' => json_parse_number(bytes, pos),
        _ => None,
    }
}

/// Skip JSON whitespace characters (U+0009, U+000A, U+000D, U+0020).
///
/// Per ECMA-404 §2, insignificant whitespace consists of:
/// - U+0009 (CHARACTER TABULATION)
/// - U+000A (LINE FEED)
/// - U+000D (CARRIAGE RETURN)
/// - U+0020 (SPACE)
fn json_skip_ws(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    pos
}

/// Parse the JSON literal `null`.
///
/// ECMA-404 §8: The literal name tokens `true`, `false`, and `null`.
fn json_parse_null(bytes: &[u8], pos: usize) -> Option<(u64, usize)> {
    if bytes.get(pos..pos + 4)? == b"null" {
        Some((JsValue::null().raw_bits(), pos + 4))
    } else {
        None
    }
}

/// Parse the JSON literal `true`.
///
/// ECMA-404 §8: The literal name tokens `true`, `false`, and `null`.
fn json_parse_true(bytes: &[u8], pos: usize) -> Option<(u64, usize)> {
    if bytes.get(pos..pos + 4)? == b"true" {
        Some((JsValue::bool(true).raw_bits(), pos + 4))
    } else {
        None
    }
}

/// Parse the JSON literal `false`.
///
/// ECMA-404 §8: The literal name tokens `true`, `false`, and `null`.
fn json_parse_false(bytes: &[u8], pos: usize) -> Option<(u64, usize)> {
    if bytes.get(pos..pos + 5)? == b"false" {
        Some((JsValue::bool(false).raw_bits(), pos + 5))
    } else {
        None
    }
}

/// Parse a JSON number.
///
/// Implements the JSON *number* production from ECMA-404 §6:
///
/// > *number* : *integer* *fraction*? *exponent*?
/// >
/// > *integer* : *digit* | *onenine* *digits* | **-** *digit* | **-** *onenine* *digits*
/// >
/// > *fraction* : **.** *digits*
/// >
/// > *exponent* : (**e** | **E**) *sign*? *digits*
fn json_parse_number(bytes: &[u8], pos: usize) -> Option<(u64, usize)> {
    let mut end = pos;
    // integer: optional leading minus
    if end < bytes.len() && bytes[end] == b'-' {
        end += 1;
    }
    // integer: digit(s)
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    // fraction: '.' digit(s)
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
    }
    // exponent: ('e' | 'E') sign? digit(s)
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
    Some((JsValue::number(n).raw_bits(), end))
}

/// Parse a JSON string (including quotes), returning NaN-boxed string bits.
///
/// Implements the JSON *string* production from ECMA-404 §7. Delegates to
/// `json_parse_string_raw` for the actual character-level parsing.
fn json_parse_string(bytes: &[u8], pos: usize) -> Option<(u64, usize)> {
    let (s, end) = json_parse_string_raw(bytes, pos)?;
    Some((make_rt_string(s), end))
}

/// Parse a JSON string, returning the raw Rust `String` content and offset after
/// the closing quote.
///
/// Implements the JSON *string* production from ECMA-404 §7:
///
/// > *string* : **"** *characters*? **"**
/// >
/// > *character* : any-Unicode-character-except-"-or-\\-or-control-character
/// >             | **\\** *escape*
/// >
/// > *escape* : **"** | **\\** | **/** | **b** | **f** | **n** | **r** | **t**
/// >          | **u** *hex* *hex* *hex* *hex*
fn json_parse_string_raw(bytes: &[u8], pos: usize) -> Option<(String, usize)> {
    // Opening '"'
    if bytes.get(pos)? != &b'"' {
        return None;
    }
    let mut result = String::new();
    let mut i = pos + 1;
    while i < bytes.len() {
        match bytes[i] {
            // Closing '"'
            b'"' => return Some((result, i + 1)),
            // Escape sequence
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
                        // \uHHHH escape
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
            // Regular character
            ch => {
                result.push(ch as char);
            }
        }
        i += 1;
    }
    None
}

/// Parse a JSON array, creating a real `JsArray` via `TaggedObj`.
///
/// Implements the JSON *array* production from ECMA-404 §5:
///
/// > *array* : **[** *ws* **]** | **[** *elements* **]**
/// >
/// > *elements* : *element* | *element* **,** *elements*
fn json_parse_array(bytes: &[u8], pos: usize) -> Option<(u64, usize)> {
    // Opening '['
    if bytes.get(pos)? != &b'[' {
        return None;
    }
    let mut i = json_skip_ws(bytes, pos + 1);
    // Empty array: ']'
    if i < bytes.len() && bytes[i] == b']' {
        return Some((create_empty_array(), i + 1));
    }

    // Non-empty array: parse comma-separated values
    let mut elements = Vec::new();
    loop {
        let (val_bits, next) = json_parse_value(bytes, i)?;
        elements.push(JsValue::from_raw_bits(val_bits));
        i = json_skip_ws(bytes, next);
        if i >= bytes.len() {
            return None;
        }
        // Closing ']'
        if bytes[i] == b']' {
            return Some((create_array_from_elements(elements), i + 1));
        }
        // Comma separator
        if bytes[i] != b',' {
            return None;
        }
        i = json_skip_ws(bytes, i + 1);
    }
}

/// Parse a JSON object, creating a real `JsObject` via `__esc_rt_create_object`
/// and `__esc_rt_set_prop`.
///
/// Implements the JSON *object* production from ECMA-404 §4:
///
/// > *object* : **{** *ws* **}** | **{** *members* **}**
/// >
/// > *members* : *member* | *member* **,** *members*
/// >
/// > *member* : *ws* *string* *ws* **:** *element*
fn json_parse_object(bytes: &[u8], pos: usize) -> Option<(u64, usize)> {
    // Opening '{'
    if bytes.get(pos)? != &b'{' {
        return None;
    }
    let mut i = json_skip_ws(bytes, pos + 1);
    let obj = __esc_rt_create_object();

    // Empty object: '}'
    if i < bytes.len() && bytes[i] == b'}' {
        return Some((obj, i + 1));
    }

    // Non-empty object: parse comma-separated key:value members
    loop {
        // Parse key (must be a string)
        let (key_str, key_end) = json_parse_string_raw(bytes, i)?;
        i = json_skip_ws(bytes, key_end);
        // Colon separator
        if i >= bytes.len() || bytes[i] != b':' {
            return None;
        }
        i = json_skip_ws(bytes, i + 1);

        // Parse value
        let (val_bits, val_end) = json_parse_value(bytes, i)?;

        // Set property on object
        let key_bits = make_rt_string(key_str);
        __esc_rt_set_prop(obj, key_bits, val_bits);

        i = json_skip_ws(bytes, val_end);
        if i >= bytes.len() {
            return None;
        }
        // Closing '}'
        if bytes[i] == b'}' {
            return Some((obj, i + 1));
        }
        // Comma separator
        if bytes[i] != b',' {
            return None;
        }
        i = json_skip_ws(bytes, i + 1);
    }
}

// -------------------------------------------------------------------------
// JSON stringify helpers — serializer reading real object/array properties
// -------------------------------------------------------------------------

/// Recursively stringify a `JsValue` to JSON with full config support.
///
/// Top-level entry point for `SerializeJSONProperty` (§25.5.2.5).
/// Supports replacer arrays, string/numeric space indent.
///
/// [spec]: https://tc39.es/ecma262/#sec-serializejsonproperty
fn json_stringify_value_ext(
    val: JsValue,
    config: &StringifyConfig,
    depth: usize,
    seen: &mut std::collections::HashSet<u64>,
) -> Option<String> {
    json_stringify_value_ext_with_key(val, "", config, depth, seen)
}

/// `SerializeJSONProperty ( state, key, holder )` — §25.5.2.5
///
/// Serializes a single JSON property value, handling toJSON, primitives,
/// arrays, and objects.
///
/// [spec]: https://tc39.es/ecma262/#sec-serializejsonproperty
fn json_stringify_value_ext_with_key(
    val: JsValue,
    key: &str,
    config: &StringifyConfig,
    depth: usize,
    seen: &mut std::collections::HashSet<u64>,
) -> Option<String> {
    // 1. Let value be ? Get(holder, key).
    //    (Already passed in as `val`.)

    // 2. If Type(value) is Object or BigInt, then
    //    a. Let toJSON be ? GetV(value, "toJSON").
    //    b. If IsCallable(toJSON) is true, then
    //       i. Set value to ? Call(toJSON, value, « key »).
    let val = apply_to_json(val, key);

    // 3. If ReplacerFunction is not undefined, then
    //    a. Set value to ? Call(ReplacerFunction, holder, « key, value »).
    let val = if let Some(replacer_fn) = config.replacer_fn {
        let key_arg = make_rt_string(key.to_string());
        let call_args = [key_arg, val.raw_bits()];
        let result = unsafe {
            // SAFETY: call_args is a valid 2-element array on the stack.
            super::dispatch_core::__esc_rt_call_indirect(replacer_fn, 2, call_args.as_ptr())
        };
        JsValue::from_raw_bits(result)
    } else {
        val
    };

    // 4. If Type(value) is Object, then
    //    a. If value has a [[NumberData]] internal slot, set value to ? ToNumber(value).
    //    b. If value has a [[StringData]] internal slot, set value to ? ToString(value).
    //    c. If value has a [[BooleanData]] internal slot, set value to value.[[BooleanData]].
    //    d. If value has a [[BigIntData]] internal slot, set value to value.[[BigIntData]].
    // TODO: Step 4 — unwrap Number/String/Boolean wrapper objects.

    // 5. If value is null, return "null".
    if val.is_undefined() {
        // 10. Return undefined (signal the caller to omit the property).
        return None;
    }
    if val.is_null() {
        return Some("null".to_string());
    }
    // 6. If value is true, return "true".
    // 7. If value is false, return "false".
    if let Some(b) = val.as_bool() {
        return Some(if b { "true" } else { "false" }.to_string());
    }
    // 8. If Type(value) is String, return QuoteJSONString(value).
    // (Checked after numbers below to match the code's existing order.)

    // 9. If Type(value) is Number, then
    //    a. If value is finite, return ! ToString(value).
    //    b. Return "null".
    if let Some(n) = val.as_int() {
        return Some(n.to_string());
    }
    if let Some(n) = val.as_number() {
        // 9a. If value is finite, return ! ToString(value).
        if n.is_nan() || n.is_infinite() {
            // 9b. Return "null".
            return Some("null".to_string());
        }
        if n == n.trunc() && n.abs() < 1e15 {
            return Some(format!("{}", n as i64));
        }
        return Some(format!("{n}"));
    }
    // 8. If Type(value) is String, return QuoteJSONString(value).
    if let Some(s) = extract_key_string(val.raw_bits()) {
        return Some(json_escape_string(&s));
    }
    // 9c. If Type(value) is BigInt, throw a TypeError exception.
    // TODO: Step 9c — BigInt TypeError.

    // 10. If Type(value) is Object and IsCallable(value) is false, then
    if !val.is_object() {
        return None;
    }

    let has_indent = !config.indent_str.is_empty();
    let bits = val.raw_bits();
    // Circular reference detection (spec uses `stack` in the state record).
    if !seen.insert(bits) {
        return None; // circular reference
    }

    let tag = read_obj_tag(bits);

    // 10a. Let isArray be ? IsArray(value).
    let is_array = tag == Some(ObjTag::Unified as u8) && {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<crate::internal_data::UnifiedObject>(bits)
        };
        uni.is_some_and(|u| u.kind == crate::internal_data::InternalKind::Array)
    };
    // 10b. If isArray is true, return ? SerializeJSONArray(state, value).
    if is_array {
        let elements: Vec<JsValue> = {
            let uni = unsafe {
                // SAFETY: tag check confirms this is a unified object.
                deref_tagged::<crate::internal_data::UnifiedObject>(bits)
            };
            uni.map_or_else(Vec::new, |u| u.array_elements_resolved())
        };

        // --- SerializeJSONArray ( state, value ) — §25.5.2.7 ---
        // 1. If state.[[Stack]] contains value, throw a TypeError (circular).
        //    (Handled by `seen` HashSet above.)
        // 2. Append value to state.[[Stack]].
        //    (Done by `seen.insert(bits)` above.)
        // 3. Let stepback be state.[[Indent]].
        // 4. Set state.[[Indent]] to state.[[Indent]] + state.[[Gap]].
        let newline = if has_indent { "\n" } else { "" };
        let child_prefix = if has_indent {
            config.indent_str.repeat(depth + 1)
        } else {
            String::new()
        };
        let close_prefix = if has_indent {
            config.indent_str.repeat(depth)
        } else {
            String::new()
        };
        let sep = if has_indent { ", " } else { "," };

        // 5. Let partial be a new empty List.
        let mut parts = Vec::new();
        // 6. Let len be ? LengthOfArrayLike(value).
        // 7. Let index be 0.
        // 8. Repeat, while index < len,
        for (idx, elem) in elements.iter().enumerate() {
            let idx_key = idx.to_string();
            //    a. Let strP be ? SerializeJSONProperty(state, ! ToString(F(index)), value).
            match json_stringify_value_ext_with_key(*elem, &idx_key, config, depth + 1, seen) {
                Some(s) => {
                    //    b. If strP is undefined, append "null" to partial.
                    //       (This branch: strP is not undefined.)
                    if has_indent {
                        parts.push(format!("{child_prefix}{s}"));
                    } else {
                        parts.push(s);
                    }
                }
                None => {
                    //    b. If strP is undefined, append "null" to partial.
                    if has_indent {
                        parts.push(format!("{child_prefix}null"));
                    } else {
                        parts.push("null".to_string());
                    }
                }
            }
            //    c. Set index to index + 1.
        }
        // 9. Remove the last element of state.[[Stack]].
        seen.remove(&bits);
        // 10. Set state.[[Indent]] to stepback.
        // 11. If partial is empty, return "[]".
        if parts.is_empty() {
            return Some("[]".to_string());
        }
        // 12. If state.[[Gap]] is the empty String, then
        //     a. Let final be the String-concatenation of "[", comma-joined partial, "]".
        // 13. Else,
        //     a. Let separator be the string-concatenation of ",", LF, state.[[Indent]].
        //     b. Let final be "[", LF, indent, separator-joined partial, LF, stepback, "]".
        // 14. Return final.
        if has_indent {
            return Some(format!(
                "[{newline}{}{newline}{close_prefix}]",
                parts.join(&format!(",{newline}"))
            ));
        }
        return Some(format!("[{}]", parts.join(sep)));
    }

    // 10c. Return ? SerializeJSONObject(state, value).
    if tag == Some(ObjTag::Unified as u8) {
        // --- SerializeJSONObject ( state, value ) — §25.5.2.6 ---
        // 1. If state.[[Stack]] contains value, throw a TypeError (circular).
        //    (Handled by `seen` HashSet above.)
        // 2. Append value to state.[[Stack]].
        //    (Done by `seen.insert(bits)` above.)
        // 3. Let stepback be state.[[Indent]].
        // 4. Set state.[[Indent]] to state.[[Indent]] + state.[[Gap]].
        // 5. If state.[[PropertyList]] is not undefined, let K be state.[[PropertyList]].
        // 6. Else, let K be ? EnumerableOwnProperties(value, key).
        let keys_bits = __esc_rt_object_keys(bits);
        let key_elements: Vec<JsValue> = {
            let keys_tag = read_obj_tag(keys_bits);
            if keys_tag == Some(ObjTag::Unified as u8) {
                let uni = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged::<crate::internal_data::UnifiedObject>(keys_bits)
                };
                uni.map_or_else(Vec::new, |u| u.array_elements_resolved())
            } else {
                Vec::new()
            }
        };
        let sep = if has_indent { ", " } else { "," };
        let newline = if has_indent { "\n" } else { "" };
        let child_prefix = if has_indent {
            config.indent_str.repeat(depth + 1)
        } else {
            String::new()
        };
        let close_prefix = if has_indent {
            config.indent_str.repeat(depth)
        } else {
            String::new()
        };

        // 7. Let partial be a new empty List.
        let mut parts = Vec::new();
        // 8. For each element P of K, do
        for key_val in &key_elements {
            let key_name = extract_key_string(key_val.raw_bits()).unwrap_or_default();
            // Replacer array filter (PropertyList): skip keys not in whitelist
            // (corresponds to step 5: K = state.[[PropertyList]] filtering)
            if let Some(ref allowed_keys) = config.replacer_keys
                && !allowed_keys.contains(&key_name)
            {
                continue;
            }
            //    a. Let strP be ? SerializeJSONProperty(state, P, value).
            let prop_val = __esc_rt_get_prop(bits, key_val.raw_bits());
            let prop = JsValue::from_raw_bits(prop_val);
            if let Some(val_str) =
                json_stringify_value_ext_with_key(prop, &key_name, config, depth + 1, seen)
            {
                //    b. If strP is not undefined, then
                //       i. Let member be QuoteJSONString(P).
                let escaped_key = json_escape_string(&key_name);
                //       ii. Set member to the string-concatenation of member and ":".
                //       iii. If state.[[Gap]] is not empty, append " " after ":".
                //       iv. Set member to member + strP.
                //       v. Append member to partial.
                if has_indent {
                    parts.push(format!("{child_prefix}{escaped_key}: {val_str}"));
                } else {
                    parts.push(format!("{escaped_key}:{val_str}"));
                }
            }
        }
        // 9. Remove the last element of state.[[Stack]].
        seen.remove(&bits);
        // 10. Set state.[[Indent]] to stepback.
        // 11. If partial is empty, return "{}".
        if parts.is_empty() {
            return Some("{}".to_string());
        }
        // 12. If state.[[Gap]] is the empty String, then
        //     a. Let final be "{" + comma-joined partial + "}".
        // 13. Else,
        //     a. Let separator be ",", LF, state.[[Indent]].
        //     b. Let final be "{", LF, indent, separator-joined, LF, stepback, "}".
        // 14. Return final.
        if has_indent {
            return Some(format!(
                "{{{newline}{}{newline}{close_prefix}}}",
                parts.join(&format!(",{newline}"))
            ));
        }
        return Some(format!("{{{}}}", parts.join(sep)));
    }

    seen.remove(&bits);
    Some("{}".to_string())
}

/// `QuoteJSONString ( value )` — §25.5.2.3
///
/// Wraps a string in double quotes and escapes special characters.
///
/// [spec]: https://tc39.es/ecma262/#sec-quotejsonstring
///
/// The algorithm is:
/// 1. Let product be the String value consisting solely of the code unit U+0022 (QUOTATION MARK).
/// 2. For each code point C of StringToCodePoints(value), do
///    a. If C is listed in the "Code Point" column of the following table, append the
///    corresponding escape sequence.
///    b. Else if C has a numeric value less than 0x0020, append \\uHHHH.
///    c. Else, append the UTF-16 encoding of C.
/// 3. Append U+0022 (QUOTATION MARK) to product.
/// 4. Return product.
fn json_escape_string(s: &str) -> String {
    // 1. Let product be "\"".
    let mut result = String::with_capacity(s.len() + 2);
    result.push('"');
    // 2. For each code point C of StringToCodePoints(value), do
    for ch in s.chars() {
        match ch {
            // 2a. Escape sequences from the table:
            //     U+0022 → \\", U+005C → \\\\, U+000A → \\n, U+000D → \\r,
            //     U+0009 → \\t, U+0008 → \\b, U+000C → \\f
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\u{0008}' => result.push_str("\\b"),
            '\u{000C}' => result.push_str("\\f"),
            // 2b. If C < U+0020, append \\uHHHH.
            c if c < '\u{0020}' => {
                result.push_str(&format!("\\u{:04x}", c as u32));
            }
            // 2c. Else, append the UTF-16 encoding of C.
            c => result.push(c),
        }
    }
    // 3. Append U+0022 (QUOTATION MARK).
    result.push('"');
    // 4. Return product.
    result
}

// -------------------------------------------------------------------------
// JSON.parse reviver support (ES spec §25.5.1.1 InternalizeJSONProperty)
// -------------------------------------------------------------------------

/// `InternalizeJSONProperty ( holder, name, reviver )` — §25.5.1.1
///
/// Recursively walks the parsed JSON value, calling the reviver function
/// for each property. If the reviver returns `undefined`, the property is
/// deleted. Otherwise it is replaced with the reviver's return value.
///
/// [spec]: https://tc39.es/ecma262/#sec-internalizejsonproperty
fn internalize_json_property(holder: u64, name: &str, reviver: u64) -> u64 {
    // 1. Let val be ? Get(holder, name).
    let key_bits = make_rt_string(name.to_string());
    let val = __esc_rt_get_prop(holder, key_bits);
    let val_js = JsValue::from_raw_bits(val);

    // 2. If Type(val) is Object, then
    if val_js.is_object() {
        let tag = read_obj_tag(val);
        if tag == Some(ObjTag::Unified as u8) {
            let uni = unsafe {
                // SAFETY: tag check confirms this is a unified object.
                deref_tagged::<crate::internal_data::UnifiedObject>(val)
            };
            if let Some(u) = uni {
                // 2a. Let isArray be ? IsArray(val).
                if u.kind == crate::internal_data::InternalKind::Array {
                    // 2b. If isArray is true, then
                    //   i. Let I be 0.
                    //   ii. Let len be ? LengthOfArrayLike(val).
                    let len = u.array_len() as usize;
                    //   iii. Repeat, while I < len,
                    for i in 0..len {
                        let idx_str = i.to_string();
                        //     1. Let prop be ? InternalizeJSONProperty(val, ! ToString(F(I)), reviver).
                        let new_elem = internalize_json_property(val, &idx_str, reviver);
                        let new_elem_val = JsValue::from_raw_bits(new_elem);
                        let idx_key = make_rt_string(idx_str);
                        //     2. If prop is undefined, then
                        if new_elem_val.is_undefined() {
                            //        a. Perform ? val.[[Delete]](! ToString(F(I))).
                            __esc_rt_delete_prop(val, idx_key);
                        } else {
                            //     3. Else,
                            //        a. Perform ? CreateDataProperty(val, ! ToString(F(I)), prop).
                            __esc_rt_set_prop(val, idx_key, new_elem);
                        }
                        //     4. Set I to I + 1.
                    }
                } else {
                    // 2c. Else (val is a plain object),
                    //   i. Let keys be ? EnumerableOwnProperties(val, key).
                    let keys_bits = __esc_rt_object_keys(val);
                    let key_elements: Vec<JsValue> = {
                        let keys_tag = read_obj_tag(keys_bits);
                        if keys_tag == Some(ObjTag::Unified as u8) {
                            let keys_uni = unsafe {
                                // SAFETY: tag check confirms this is a unified object.
                                deref_tagged::<crate::internal_data::UnifiedObject>(keys_bits)
                            };
                            keys_uni.map_or_else(Vec::new, |ku| ku.array_elements_resolved())
                        } else {
                            Vec::new()
                        }
                    };
                    //   ii. For each String P of keys, do
                    for k in &key_elements {
                        let k_name = extract_key_string(k.raw_bits()).unwrap_or_default();
                        //     1. Let newElement be ? InternalizeJSONProperty(val, P, reviver).
                        let new_val = internalize_json_property(val, &k_name, reviver);
                        let new_val_js = JsValue::from_raw_bits(new_val);
                        let k_key = make_rt_string(k_name);
                        //     2. If newElement is undefined, then
                        if new_val_js.is_undefined() {
                            //        a. Perform ? val.[[Delete]](P).
                            __esc_rt_delete_prop(val, k_key);
                        } else {
                            //     3. Else,
                            //        a. Perform ? CreateDataProperty(val, P, newElement).
                            __esc_rt_set_prop(val, k_key, new_val);
                        }
                    }
                }
            }
        }
    }

    // 3. Return ? Call(reviver, holder, « name, val »).
    let name_arg = make_rt_string(name.to_string());
    let current_val = __esc_rt_get_prop(holder, make_rt_string(name.to_string()));
    let call_args = [name_arg, current_val];
    // Set CURRENT_THIS to `holder` for the reviver call
    super::CURRENT_THIS.with(|cell| cell.set(holder));
    unsafe {
        // SAFETY: call_args is a valid pointer to 2 u64 values, reviver is
        // checked callable above via is_callable.
        super::__esc_rt_call_indirect(reviver, 2, call_args.as_ptr())
    }
}

// -------------------------------------------------------------------------
// JSON.stringify toJSON support (ES spec §25.5.2.5 SerializeJSONProperty)
// -------------------------------------------------------------------------

/// Apply `toJSON` if the value has a callable `toJSON` method.
///
/// Per `SerializeJSONProperty` (§25.5.2.5) step 2:
/// > If Type(value) is Object or BigInt, then
/// >   a. Let toJSON be ? GetV(value, "toJSON").
/// >   b. If IsCallable(toJSON) is true, then
/// >      i. Set value to ? Call(toJSON, value, « key »).
///
/// [spec]: https://tc39.es/ecma262/#sec-serializejsonproperty
///
/// Returns the original value if no callable `toJSON` is found.
fn apply_to_json(val: JsValue, key: &str) -> JsValue {
    // 2. If Type(value) is Object or BigInt, then
    if !val.is_object() {
        return val;
    }
    //    a. Let toJSON be ? GetV(value, "toJSON").
    let to_json_key = make_rt_string("toJSON".to_string());
    let to_json_bits = __esc_rt_get_prop(val.raw_bits(), to_json_key);
    let to_json_val = JsValue::from_raw_bits(to_json_bits);
    //    b. If IsCallable(toJSON) is true, then
    if to_json_val.is_undefined() || !is_callable(to_json_bits) {
        return val;
    }
    //       i. Set value to ? Call(toJSON, value, « key »).
    let key_arg = make_rt_string(key.to_string());
    super::CURRENT_THIS.with(|cell| cell.set(val.raw_bits()));
    let result = unsafe {
        // SAFETY: key_arg is a valid u64 value on the stack, to_json_bits is
        // checked callable above via is_callable.
        super::__esc_rt_call_indirect(to_json_bits, 1, &key_arg)
    };
    JsValue::from_raw_bits(result)
}
