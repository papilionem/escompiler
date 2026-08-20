//! Array built-in methods.
//!
//! Provides `Array.isArray()`, `Array.from()`, `Array.of()`, and prototype methods
//! operating on a simple `RtArray` representation: a heap-allocated `Vec<JsValue>`
//! behind an object pointer.

use nanbox::JsValue;

/// Runtime array layout — a heap-allocated `Vec<JsValue>` behind an object pointer.
///
/// This mirrors the layout used by the runtime for dense arrays.
#[repr(C)]
struct RtArray {
    elements: Vec<JsValue>,
}

/// Create a new array JsValue from a Vec of elements.
fn make_array(elements: Vec<JsValue>) -> JsValue {
    let arr = Box::new(RtArray { elements });
    let raw_ptr = Box::into_raw(arr) as *const ();
    JsValue::object(raw_ptr)
}

/// Extract mutable reference to the array elements from an object JsValue.
///
/// Returns `None` if the value is not an object or the pointer is null.
///
/// # Safety
/// The caller must ensure the object pointer was created by `make_array`
/// and that no other references to the array exist.
#[allow(clippy::mut_from_ref)]
unsafe fn extract_array_mut(val: &JsValue) -> Option<&mut RtArray> {
    let ptr = val.as_object()? as *mut RtArray;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees ptr was created by make_array (Box::into_raw)
    Some(unsafe { &mut *ptr })
}

/// Extract a shared reference to the array elements from an object JsValue.
///
/// Returns `None` if the value is not an object or the pointer is null.
///
/// # Safety
/// The caller must ensure the object pointer was created by `make_array`.
unsafe fn extract_array(val: &JsValue) -> Option<&RtArray> {
    let ptr = val.as_object()? as *const RtArray;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees ptr was created by make_array (Box::into_raw)
    Some(unsafe { &*ptr })
}

/// `Array.isArray(value)` — simplified check.
///
/// Currently returns `true` for any object value. A future version will check
/// for the internal array slot.
pub fn is_array(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    // TODO: Phase D — check if object has array internal slot
    JsValue::bool(val.is_object())
}

/// `Array.of(...args)` — create an array from arguments.
///
/// Returns a new array containing all passed arguments.
pub fn of(args: &[JsValue]) -> JsValue {
    make_array(args.to_vec())
}

/// `Array.from(iterable)` — create an array from an iterable-like source.
///
/// Currently supports creating from another array (copies elements).
/// For non-array inputs, wraps the single value in a one-element array.
pub fn from(args: &[JsValue]) -> JsValue {
    let source = args.first().copied().unwrap_or_else(JsValue::undefined);
    if source.is_undefined() || source.is_null() {
        return make_array(Vec::new());
    }
    // If source is an array object, copy its elements
    if source.is_object() {
        let arr = unsafe {
            // SAFETY: we only access if is_object() — may not be an RtArray,
            // but in the stdlib context arrays are the primary object type
            extract_array(&source)
        };
        if let Some(a) = arr {
            return make_array(a.elements.clone());
        }
    }
    // Wrap single value
    make_array(vec![source])
}

// === Mutators ===

/// `Array.prototype.push(...items)` — append elements and return new length.
///
/// Args: `[this, ...items]`
pub fn push(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array_mut(&this)
    };
    let Some(arr) = arr else {
        return JsValue::int(0);
    };
    for item in args.iter().skip(1) {
        arr.elements.push(*item);
    }
    JsValue::int(arr.elements.len() as i32)
}

/// `Array.prototype.pop()` — remove and return the last element.
///
/// Args: `[this]`
pub fn pop(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array_mut(&this)
    };
    let Some(arr) = arr else {
        return JsValue::undefined();
    };
    arr.elements.pop().unwrap_or_else(JsValue::undefined)
}

/// `Array.prototype.shift()` — remove and return the first element.
///
/// Args: `[this]`
pub fn shift(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array_mut(&this)
    };
    let Some(arr) = arr else {
        return JsValue::undefined();
    };
    if arr.elements.is_empty() {
        JsValue::undefined()
    } else {
        arr.elements.remove(0)
    }
}

/// `Array.prototype.unshift(...items)` — prepend elements, return new length.
///
/// Args: `[this, ...items]`
pub fn unshift(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array_mut(&this)
    };
    let Some(arr) = arr else {
        return JsValue::int(0);
    };
    let items: Vec<JsValue> = args.iter().skip(1).copied().collect();
    for (i, item) in items.into_iter().enumerate() {
        arr.elements.insert(i, item);
    }
    JsValue::int(arr.elements.len() as i32)
}

/// `Array.prototype.reverse()` — reverse elements in place, return the array.
///
/// Args: `[this]`
pub fn reverse(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array_mut(&this)
    };
    let Some(arr) = arr else {
        return this;
    };
    arr.elements.reverse();
    this
}

/// `Array.prototype.sort()` — sort elements in place using string comparison.
///
/// Args: `[this]`. Default sort uses lexicographic string comparison per the
/// ECMAScript specification. Full comparator callback support requires runtime
/// call support.
pub fn sort(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array_mut(&this)
    };
    let Some(arr) = arr else {
        return this;
    };
    arr.elements.sort_by(|a, b| {
        let sa = stdlib_format_value(*a);
        let sb = stdlib_format_value(*b);
        sa.cmp(&sb)
    });
    this
}

/// Format a JsValue as a string for sort comparison (ES spec default).
fn stdlib_format_value(v: JsValue) -> String {
    if v.is_undefined() {
        return String::new();
    }
    if let Some(n) = v.as_int() {
        return n.to_string();
    }
    if let Some(n) = v.as_number() {
        if n == n.trunc() && n.abs() < 1e15 {
            return format!("{}", n as i64);
        }
        return format!("{n}");
    }
    if let Some(b) = v.as_bool() {
        return b.to_string();
    }
    String::new()
}

/// `Array.prototype.splice(start, deleteCount, ...items)` — remove/insert elements.
///
/// Args: `[this, start, deleteCount, ...items]`.
/// Returns a new array of deleted elements.
pub fn splice(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array_mut(&this)
    };
    let Some(arr) = arr else {
        return make_array(Vec::new());
    };
    let len = arr.elements.len() as i32;

    let raw_start = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);

    let start = if raw_start < 0 {
        (len + raw_start).max(0) as usize
    } else {
        raw_start.min(len) as usize
    };

    let delete_count = args
        .get(2)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .map(|d| d.max(0) as usize)
        .unwrap_or((len as usize).saturating_sub(start));

    let actual_delete = delete_count.min(arr.elements.len().saturating_sub(start));

    let deleted: Vec<JsValue> = arr.elements.drain(start..start + actual_delete).collect();

    let new_items: Vec<JsValue> = args.iter().skip(3).copied().collect();
    for (i, item) in new_items.into_iter().enumerate() {
        arr.elements.insert(start + i, item);
    }

    make_array(deleted)
}

/// `Array.prototype.fill(value, start, end)` — fill elements with a value.
///
/// Args: `[this, value, start?, end?]`
pub fn fill(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array_mut(&this)
    };
    let Some(arr) = arr else {
        return this;
    };
    let value = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let len = arr.elements.len() as i32;

    let raw_start = args
        .get(2)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);
    let raw_end = args
        .get(3)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(len);

    let start = if raw_start < 0 {
        (len + raw_start).max(0) as usize
    } else {
        raw_start.min(len) as usize
    };
    let end = if raw_end < 0 {
        (len + raw_end).max(0) as usize
    } else {
        raw_end.min(len) as usize
    };

    for i in start..end {
        arr.elements[i] = value;
    }
    this
}

/// `Array.prototype.copyWithin(target, start, end)` — copy a section within the array.
///
/// Args: `[this, target, start, end?]`
pub fn copy_within(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array_mut(&this)
    };
    let Some(arr) = arr else {
        return this;
    };
    let len = arr.elements.len() as i32;

    let raw_target = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);
    let raw_start = args
        .get(2)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);
    let raw_end = args
        .get(3)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(len);

    let target = if raw_target < 0 {
        (len + raw_target).max(0) as usize
    } else {
        raw_target.min(len) as usize
    };
    let start = if raw_start < 0 {
        (len + raw_start).max(0) as usize
    } else {
        raw_start.min(len) as usize
    };
    let end = if raw_end < 0 {
        (len + raw_end).max(0) as usize
    } else {
        raw_end.min(len) as usize
    };

    if start >= end || target >= arr.elements.len() {
        return this;
    }

    // Copy elements to a temp buffer to handle overlapping regions
    let to_copy: Vec<JsValue> = arr.elements[start..end].to_vec();
    let count = to_copy.len().min(arr.elements.len().saturating_sub(target));
    for (i, val) in to_copy.into_iter().take(count).enumerate() {
        arr.elements[target + i] = val;
    }
    this
}

// === Accessors ===

/// `Array.prototype.indexOf(searchElement, fromIndex?)` — find first index of element.
///
/// Args: `[this, searchElement, fromIndex?]`
pub fn index_of(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array(&this)
    };
    let Some(arr) = arr else {
        return JsValue::int(-1);
    };
    let search = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let from = args
        .get(2)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);

    let len = arr.elements.len() as i32;
    let start = if from < 0 {
        (len + from).max(0) as usize
    } else {
        from.min(len) as usize
    };

    for (i, elem) in arr.elements[start..].iter().enumerate() {
        if elem.raw_bits() == search.raw_bits() {
            return JsValue::int((start + i) as i32);
        }
    }
    JsValue::int(-1)
}

/// `Array.prototype.includes(searchElement, fromIndex?)` — check if element exists.
///
/// Args: `[this, searchElement, fromIndex?]`
pub fn includes(args: &[JsValue]) -> JsValue {
    let result = index_of(args);
    if let Some(idx) = result.as_int() {
        JsValue::bool(idx >= 0)
    } else {
        JsValue::bool(false)
    }
}

/// `Array.prototype.join(separator?)` — join elements into a string.
///
/// Args: `[this, separator?]`
pub fn join(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array(&this)
    };
    let Some(arr) = arr else {
        return make_join_string(String::new());
    };

    let sep = args
        .get(1)
        .and_then(extract_join_string)
        .unwrap_or_else(|| ",".to_string());

    let parts: Vec<String> = arr
        .elements
        .iter()
        .map(|v| {
            if v.is_undefined() || v.is_null() {
                String::new()
            } else if let Some(n) = v.as_int() {
                n.to_string()
            } else if let Some(n) = v.as_number() {
                if n == n.trunc() && n.abs() < 1e15 {
                    format!("{}", n as i64)
                } else {
                    format!("{n}")
                }
            } else if let Some(b) = v.as_bool() {
                b.to_string()
            } else {
                String::new()
            }
        })
        .collect();

    make_join_string(parts.join(&sep))
}

/// Create a string JsValue for join output.
fn make_join_string(s: String) -> JsValue {
    let rt_str = Box::new(runtime::string_ops::RtString::new(s));
    let raw_ptr = Box::into_raw(rt_str) as *const ();
    JsValue::string(raw_ptr)
}

/// Extract string data from a JsValue for separator.
fn extract_join_string(val: &JsValue) -> Option<String> {
    if let Some(ptr) = val.as_string() {
        if ptr.is_null() {
            return Some(String::new());
        }
        let rt_str = unsafe {
            // SAFETY: ptr was created by string_from_data or make_string
            &*(ptr as *const runtime::string_ops::RtString)
        };
        Some(rt_str.as_str().to_string())
    } else {
        None
    }
}

/// `Array.prototype.slice(start?, end?)` — extract a section of the array.
///
/// Args: `[this, start?, end?]`
pub fn slice(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array(&this)
    };
    let Some(arr) = arr else {
        return make_array(Vec::new());
    };
    let len = arr.elements.len() as i32;

    let raw_start = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(0);
    let raw_end = args
        .get(2)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(len);

    let start = if raw_start < 0 {
        (len + raw_start).max(0) as usize
    } else {
        raw_start.min(len) as usize
    };
    let end = if raw_end < 0 {
        (len + raw_end).max(0) as usize
    } else {
        raw_end.min(len) as usize
    };

    if start >= end {
        return make_array(Vec::new());
    }
    make_array(arr.elements[start..end].to_vec())
}

/// `Array.prototype.concat(...arrays)` — merge arrays.
///
/// Args: `[this, ...arrays]`
pub fn concat(args: &[JsValue]) -> JsValue {
    let mut result = Vec::new();
    for arg in args {
        if arg.is_object() {
            let arr = unsafe {
                // SAFETY: object created by make_array
                extract_array(arg)
            };
            if let Some(a) = arr {
                result.extend_from_slice(&a.elements);
                continue;
            }
        }
        result.push(*arg);
    }
    make_array(result)
}

/// `Array.prototype.flat(depth?)` — flatten nested arrays.
///
/// Args: `[this, depth?]`. Default depth is 1.
pub fn flat(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array(&this)
    };
    let Some(arr) = arr else {
        return make_array(Vec::new());
    };
    let depth = args
        .get(1)
        .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
        .unwrap_or(1)
        .max(0) as usize;

    let result = flatten_elements(&arr.elements, depth);
    make_array(result)
}

/// Recursively flatten array elements up to a given depth.
fn flatten_elements(elements: &[JsValue], depth: usize) -> Vec<JsValue> {
    let mut result = Vec::new();
    for elem in elements {
        if depth > 0 && elem.is_object() {
            let inner = unsafe { extract_array(elem) };
            if let Some(a) = inner {
                result.extend(flatten_elements(&a.elements, depth - 1));
                continue;
            }
        }
        result.push(*elem);
    }
    result
}

/// `Array.prototype.flatMap(callback)` — map then flatten one level.
///
/// Args: `[this, callback]`. Callback dispatch is structural — actual invocation
/// requires runtime call support. For now, returns a flattened copy.
pub fn flat_map(args: &[JsValue]) -> JsValue {
    // flatMap is map + flat(1). Without callback invocation, behave as flat(1).
    flat(args)
}

// === Iterators (structural — callback dispatch placeholder) ===

/// `Array.prototype.forEach(callback)` — call function for each element.
///
/// Structural placeholder — actual callback invocation requires runtime.
/// Currently a no-op that returns undefined.
pub fn for_each(_args: &[JsValue]) -> JsValue {
    // Cannot invoke callback without runtime function call support
    JsValue::undefined()
}

/// `Array.prototype.map(callback)` — produce a new array from callback results.
///
/// Structural placeholder — returns a copy of the array. Actual callback
/// invocation requires runtime function call support.
pub fn map(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array(&this)
    };
    let Some(arr) = arr else {
        return make_array(Vec::new());
    };
    // Without callback invocation, return a copy
    make_array(arr.elements.clone())
}

/// `Array.prototype.filter(callback)` — produce a filtered array.
///
/// Structural placeholder — returns a copy of the array. Actual callback
/// invocation requires runtime function call support.
pub fn filter(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array(&this)
    };
    let Some(arr) = arr else {
        return make_array(Vec::new());
    };
    make_array(arr.elements.clone())
}

/// `Array.prototype.reduce(callback, initialValue?)` — reduce to a single value.
///
/// Structural placeholder — returns the initial value or the first element.
/// Actual callback invocation requires runtime function call support.
pub fn reduce(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array(&this)
    };
    let Some(arr) = arr else {
        return JsValue::undefined();
    };
    // Return initial value if provided, otherwise first element
    if let Some(init) = args.get(2) {
        return *init;
    }
    arr.elements
        .first()
        .copied()
        .unwrap_or_else(JsValue::undefined)
}

/// `Array.prototype.reduceRight(callback, initialValue?)` — reduce from right.
///
/// Structural placeholder — returns the initial value or the last element.
pub fn reduce_right(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array(&this)
    };
    let Some(arr) = arr else {
        return JsValue::undefined();
    };
    if let Some(init) = args.get(2) {
        return *init;
    }
    arr.elements
        .last()
        .copied()
        .unwrap_or_else(JsValue::undefined)
}

/// `Array.prototype.find(callback)` — find first matching element.
///
/// Structural placeholder — returns `undefined`. Actual callback
/// invocation requires runtime function call support.
pub fn find(_args: &[JsValue]) -> JsValue {
    JsValue::undefined()
}

/// `Array.prototype.findIndex(callback)` — find first matching index.
///
/// Structural placeholder — returns -1.
pub fn find_index(_args: &[JsValue]) -> JsValue {
    JsValue::int(-1)
}

/// `Array.prototype.findLast(callback)` — find last matching element.
///
/// Structural placeholder — returns `undefined`.
pub fn find_last(_args: &[JsValue]) -> JsValue {
    JsValue::undefined()
}

/// `Array.prototype.findLastIndex(callback)` — find last matching index.
///
/// Structural placeholder — returns -1.
pub fn find_last_index(_args: &[JsValue]) -> JsValue {
    JsValue::int(-1)
}

/// `Array.prototype.some(callback)` — test if any element matches.
///
/// Structural placeholder — returns `false`.
pub fn some(_args: &[JsValue]) -> JsValue {
    JsValue::bool(false)
}

/// `Array.prototype.every(callback)` — test if all elements match.
///
/// Structural placeholder — returns `true` (vacuously true for empty arrays).
pub fn every(_args: &[JsValue]) -> JsValue {
    JsValue::bool(true)
}

/// `Array.prototype.length` — return the length of the array.
///
/// Args: `[this]`
pub fn length(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let arr = unsafe {
        // SAFETY: this was created by make_array in stdlib usage
        extract_array(&this)
    };
    let Some(arr) = arr else {
        return JsValue::int(0);
    };
    JsValue::int(arr.elements.len() as i32)
}

#[cfg(test)]
pub(crate) use self::test_helpers::*;

#[cfg(test)]
mod test_helpers {
    use super::*;

    /// Extract elements from an array JsValue for testing assertions.
    pub fn test_extract_array(val: &JsValue) -> Option<Vec<JsValue>> {
        let arr = unsafe { extract_array(val) };
        arr.map(|a| a.elements.clone())
    }
}
