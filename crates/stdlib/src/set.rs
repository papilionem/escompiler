//! Set built-in methods.
//!
//! Provides a `Set` implementation using an ordered list of unique values
//! stored behind an object pointer. Values are compared by their raw
//! NaN-boxed bit representation.

use nanbox::JsValue;

/// Internal set storage — an ordered list of unique values.
#[repr(C)]
struct SetInner {
    values: Vec<JsValue>,
}

/// Create a new empty Set as a JsValue.
fn make_set() -> JsValue {
    let inner = Box::new(SetInner { values: Vec::new() });
    let raw_ptr = Box::into_raw(inner) as *const ();
    JsValue::object(raw_ptr)
}

/// Extract a mutable reference to the set inner from an object JsValue.
///
/// # Safety
/// Caller must ensure the pointer was created by `make_set`
/// and that no other references to the set exist.
#[allow(clippy::mut_from_ref)]
unsafe fn extract_set_mut(val: &JsValue) -> Option<&mut SetInner> {
    let ptr = val.as_object()? as *mut SetInner;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees ptr was created by make_set
    Some(unsafe { &mut *ptr })
}

/// Extract a shared reference to the set inner.
///
/// # Safety
/// Caller must ensure the pointer was created by `make_set`.
unsafe fn extract_set(val: &JsValue) -> Option<&SetInner> {
    let ptr = val.as_object()? as *const SetInner;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees ptr was created by make_set
    Some(unsafe { &*ptr })
}

/// Check if a value exists in the set by raw bit comparison.
fn contains(values: &[JsValue], val: &JsValue) -> bool {
    let bits = val.raw_bits();
    values.iter().any(|v| v.raw_bits() == bits)
}

/// `new Set()` — create a new empty Set.
pub fn set_new(_args: &[JsValue]) -> JsValue {
    make_set()
}

/// `Set.prototype.add(value)` — add a value to the set.
///
/// Args: `[this, value]`. Returns the set.
pub fn add(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let value = args.get(1).copied().unwrap_or_else(JsValue::undefined);

    let set = unsafe {
        // SAFETY: this was created by make_set
        extract_set_mut(&this)
    };
    let Some(set) = set else {
        return this;
    };

    if !contains(&set.values, &value) {
        set.values.push(value);
    }
    this
}

/// `Set.prototype.has(value)` — check if a value exists.
///
/// Args: `[this, value]`.
pub fn has(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let value = args.get(1).copied().unwrap_or_else(JsValue::undefined);

    let set = unsafe {
        // SAFETY: this was created by make_set
        extract_set(&this)
    };
    let Some(set) = set else {
        return JsValue::bool(false);
    };

    JsValue::bool(contains(&set.values, &value))
}

/// `Set.prototype.delete(value)` — remove a value.
///
/// Args: `[this, value]`. Returns `true` if the value was found and removed.
pub fn delete(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let value = args.get(1).copied().unwrap_or_else(JsValue::undefined);

    let set = unsafe {
        // SAFETY: this was created by make_set
        extract_set_mut(&this)
    };
    let Some(set) = set else {
        return JsValue::bool(false);
    };

    let bits = value.raw_bits();
    if let Some(idx) = set.values.iter().position(|v| v.raw_bits() == bits) {
        set.values.remove(idx);
        JsValue::bool(true)
    } else {
        JsValue::bool(false)
    }
}

/// `Set.prototype.clear()` — remove all values.
///
/// Args: `[this]`.
pub fn clear(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);

    let set = unsafe {
        // SAFETY: this was created by make_set
        extract_set_mut(&this)
    };
    if let Some(set) = set {
        set.values.clear();
    }
    JsValue::undefined()
}

/// `Set.prototype.size` — return the number of values.
///
/// Args: `[this]`.
pub fn size(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);

    let set = unsafe {
        // SAFETY: this was created by make_set
        extract_set(&this)
    };
    let Some(set) = set else {
        return JsValue::int(0);
    };

    JsValue::int(set.values.len() as i32)
}

/// `Set.prototype.forEach(callback)` — call function for each value.
///
/// Structural placeholder — actual callback invocation requires runtime.
/// Returns `undefined`.
pub fn for_each(_args: &[JsValue]) -> JsValue {
    JsValue::undefined()
}

/// `Set.prototype.keys()` — return an iterator over values (same as `values()`).
///
/// Structural placeholder — returns the count.
pub fn keys(args: &[JsValue]) -> JsValue {
    size(args)
}

/// `Set.prototype.values()` — return an iterator over values.
///
/// Structural placeholder — returns the count.
pub fn values(args: &[JsValue]) -> JsValue {
    size(args)
}

/// `Set.prototype.entries()` — return an iterator over `[value, value]` pairs.
///
/// Structural placeholder — returns the count.
pub fn entries(args: &[JsValue]) -> JsValue {
    size(args)
}
