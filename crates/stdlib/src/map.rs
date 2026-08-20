//! Map built-in methods.
//!
//! Provides a `Map` implementation using an insertion-ordered hash map
//! stored behind an object pointer. Keys are compared by their raw NaN-boxed
//! bit representation.

use nanbox::JsValue;

/// Internal map storage — an ordered list of key-value pairs.
///
/// Uses a `Vec` for insertion order and linear scan for lookups.
/// This is simple and correct; a hash-based approach can be added later
/// for performance on large maps.
#[repr(C)]
struct MapInner {
    entries: Vec<(JsValue, JsValue)>,
}

/// Create a new empty Map as a JsValue.
fn make_map() -> JsValue {
    let inner = Box::new(MapInner {
        entries: Vec::new(),
    });
    let raw_ptr = Box::into_raw(inner) as *const ();
    JsValue::object(raw_ptr)
}

/// Extract a mutable reference to the map inner from an object JsValue.
///
/// # Safety
/// Caller must ensure the pointer was created by `make_map`
/// and that no other references to the map exist.
#[allow(clippy::mut_from_ref)]
unsafe fn extract_map_mut(val: &JsValue) -> Option<&mut MapInner> {
    let ptr = val.as_object()? as *mut MapInner;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees ptr was created by make_map
    Some(unsafe { &mut *ptr })
}

/// Extract a shared reference to the map inner.
///
/// # Safety
/// Caller must ensure the pointer was created by `make_map`.
unsafe fn extract_map(val: &JsValue) -> Option<&MapInner> {
    let ptr = val.as_object()? as *const MapInner;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees ptr was created by make_map
    Some(unsafe { &*ptr })
}

/// Find the index of a key in the map entries by raw bit comparison.
fn find_key(entries: &[(JsValue, JsValue)], key: &JsValue) -> Option<usize> {
    let key_bits = key.raw_bits();
    entries.iter().position(|(k, _)| k.raw_bits() == key_bits)
}

/// `new Map()` — create a new empty Map.
pub fn map_new(_args: &[JsValue]) -> JsValue {
    make_map()
}

/// `Map.prototype.set(key, value)` — add or update a key-value pair.
///
/// Args: `[this, key, value]`. Returns the map.
pub fn set(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let key = args.get(1).copied().unwrap_or_else(JsValue::undefined);
    let value = args.get(2).copied().unwrap_or_else(JsValue::undefined);

    let map = unsafe {
        // SAFETY: this was created by make_map
        extract_map_mut(&this)
    };
    let Some(map) = map else {
        return this;
    };

    if let Some(idx) = find_key(&map.entries, &key) {
        map.entries[idx].1 = value;
    } else {
        map.entries.push((key, value));
    }
    this
}

/// `Map.prototype.get(key)` — get the value for a key.
///
/// Args: `[this, key]`. Returns the value or `undefined`.
pub fn get(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let key = args.get(1).copied().unwrap_or_else(JsValue::undefined);

    let map = unsafe {
        // SAFETY: this was created by make_map
        extract_map(&this)
    };
    let Some(map) = map else {
        return JsValue::undefined();
    };

    if let Some(idx) = find_key(&map.entries, &key) {
        map.entries[idx].1
    } else {
        JsValue::undefined()
    }
}

/// `Map.prototype.has(key)` — check if a key exists.
///
/// Args: `[this, key]`.
pub fn has(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let key = args.get(1).copied().unwrap_or_else(JsValue::undefined);

    let map = unsafe {
        // SAFETY: this was created by make_map
        extract_map(&this)
    };
    let Some(map) = map else {
        return JsValue::bool(false);
    };

    JsValue::bool(find_key(&map.entries, &key).is_some())
}

/// `Map.prototype.delete(key)` — remove a key-value pair.
///
/// Args: `[this, key]`. Returns `true` if the key was found and removed.
pub fn delete(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let key = args.get(1).copied().unwrap_or_else(JsValue::undefined);

    let map = unsafe {
        // SAFETY: this was created by make_map
        extract_map_mut(&this)
    };
    let Some(map) = map else {
        return JsValue::bool(false);
    };

    if let Some(idx) = find_key(&map.entries, &key) {
        map.entries.remove(idx);
        JsValue::bool(true)
    } else {
        JsValue::bool(false)
    }
}

/// `Map.prototype.clear()` — remove all entries.
///
/// Args: `[this]`.
pub fn clear(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);

    let map = unsafe {
        // SAFETY: this was created by make_map
        extract_map_mut(&this)
    };
    if let Some(map) = map {
        map.entries.clear();
    }
    JsValue::undefined()
}

/// `Map.prototype.size` — return the number of entries.
///
/// Args: `[this]`.
pub fn size(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);

    let map = unsafe {
        // SAFETY: this was created by make_map
        extract_map(&this)
    };
    let Some(map) = map else {
        return JsValue::int(0);
    };

    JsValue::int(map.entries.len() as i32)
}

/// `Map.prototype.forEach(callback)` — call function for each entry.
///
/// Structural placeholder — actual callback invocation requires runtime.
/// Returns `undefined`.
pub fn for_each(_args: &[JsValue]) -> JsValue {
    JsValue::undefined()
}

/// `Map.prototype.keys()` — return an iterator over keys.
///
/// Structural placeholder — returns the count of keys.
pub fn keys(args: &[JsValue]) -> JsValue {
    size(args)
}

/// `Map.prototype.values()` — return an iterator over values.
///
/// Structural placeholder — returns the count of values.
pub fn values(args: &[JsValue]) -> JsValue {
    size(args)
}

/// `Map.prototype.entries()` — return an iterator over `[key, value]` pairs.
///
/// Structural placeholder — returns the count of entries.
pub fn entries(args: &[JsValue]) -> JsValue {
    size(args)
}
