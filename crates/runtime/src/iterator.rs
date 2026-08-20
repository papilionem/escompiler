//! Iterator protocol support for the runtime ABI.
//!
//! Provides an `ArrayIterator` that implements the JS iterator protocol
//! (`next()` returning `{value, done}`), used by `for-of` loops and
//! spread syntax.
//!
//! Also supports ES2025 Iterator Helpers via [`HelperKind`], which wrap an
//! underlying iterator and apply lazy transformations (map, filter, take,
//! drop, flatMap).

use nanbox::JsValue;

/// The kind of iterator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IteratorKind {
    /// Iterates over array elements by index.
    Array,
    /// Iterates over object property keys (for `for...in`).
    ObjectKeys,
    /// Iterates over string characters.
    StringChars,
    /// Custom iterator with a JS `.next()` method (from `[Symbol.iterator]()`).
    Custom,
    /// Iterates over Map entries as `[key, value]` pairs.
    MapEntries,
    /// Iterates over Set values.
    SetValues,
    /// Iterates over a generator object's yielded values.
    Generator,
    /// ES2025 Iterator Helper — wraps another iterator with a transformation.
    Helper,
    /// Iterates over array `[index, value]` pairs (for `Array.prototype.entries()`).
    ArrayEntries,
    /// Iterates over array indices (for `Array.prototype.keys()`).
    ArrayKeys,
    /// Iterates over array values (for `Array.prototype.values()`).
    ArrayValues,
}

/// The specific kind of ES2025 Iterator Helper transformation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperKind {
    /// `Iterator.prototype.map(fn)` — transforms each value.
    Map,
    /// `Iterator.prototype.filter(fn)` — skips non-matching values.
    Filter,
    /// `Iterator.prototype.take(n)` — limits to first n values.
    Take,
    /// `Iterator.prototype.drop(n)` — skips first n values.
    Drop,
    /// `Iterator.prototype.flatMap(fn)` — maps then flattens one level.
    FlatMap,
}

/// State for an ES2025 Iterator Helper wrapper.
///
/// Holds the underlying iterator, the callback function (if any),
/// the helper kind, and any extra state (counter for take/drop,
/// inner iterator for flatMap).
#[derive(Debug)]
pub struct HelperState {
    /// The underlying iterator object (NaN-boxed).
    pub underlying: u64,
    /// The callback function (NaN-boxed), or 0 if none (take/drop).
    pub callback: u64,
    /// Which helper transformation to apply.
    pub helper_kind: HelperKind,
    /// Counter for take/drop (remaining count).
    pub counter: u32,
    /// Whether the initial drop phase is complete.
    pub drop_done: bool,
    /// Inner iterator for flatMap (NaN-boxed), or 0 if none.
    pub inner_iter: u64,
}

/// An iterator over a JS array's elements or object property keys.
///
/// Stores a pointer to the target (NaN-boxed) and a current index.
/// Each call to `next()` advances the index and returns the element/key,
/// or signals completion with `done = true`.
#[derive(Debug)]
pub struct JsIterator {
    /// The kind of iterator.
    pub kind: IteratorKind,
    /// The target object being iterated (NaN-boxed).
    pub target: u64,
    /// Current iteration index.
    pub index: u32,
    /// Whether iteration is complete.
    pub done: bool,
    /// Cached property keys for ObjectKeys iteration.
    pub keys: Vec<String>,
    /// State for ES2025 Iterator Helper (only used when `kind == Helper`).
    pub helper: Option<Box<HelperState>>,
}

impl JsIterator {
    /// Creates a new array iterator for the given NaN-boxed array.
    pub fn new_array(target: u64) -> Self {
        Self {
            kind: IteratorKind::Array,
            target,
            index: 0,
            done: false,
            keys: Vec::new(),
            helper: None,
        }
    }

    /// Creates a new object-keys iterator for `for...in`.
    pub fn new_object_keys(target: u64, keys: Vec<String>) -> Self {
        Self {
            kind: IteratorKind::ObjectKeys,
            target,
            index: 0,
            done: false,
            keys,
            helper: None,
        }
    }

    /// Creates a new string character iterator for `for...of` on strings.
    pub fn new_string_chars(target: u64, chars: Vec<String>) -> Self {
        Self {
            kind: IteratorKind::StringChars,
            target,
            index: 0,
            done: false,
            keys: chars, // reuse keys field for char strings
            helper: None,
        }
    }

    /// Creates a custom iterator wrapping a JS object with a `.next()` method.
    ///
    /// `iterator_obj` is the NaN-boxed object returned by `[Symbol.iterator]()`.
    pub fn new_custom(iterator_obj: u64) -> Self {
        Self {
            kind: IteratorKind::Custom,
            target: iterator_obj,
            index: 0,
            done: false,
            keys: Vec::new(),
            helper: None,
        }
    }

    /// Creates a Map entries iterator that yields `[key, value]` pairs.
    pub fn new_map_entries(target: u64) -> Self {
        Self {
            kind: IteratorKind::MapEntries,
            target,
            index: 0,
            done: false,
            keys: Vec::new(),
            helper: None,
        }
    }

    /// Creates a Set values iterator that yields each set element.
    pub fn new_set_values(target: u64) -> Self {
        Self {
            kind: IteratorKind::SetValues,
            target,
            index: 0,
            done: false,
            keys: Vec::new(),
            helper: None,
        }
    }

    /// Creates a generator iterator that delegates to the generator's `.next()` protocol.
    pub fn new_generator(target: u64) -> Self {
        Self {
            kind: IteratorKind::Generator,
            target,
            index: 0,
            done: false,
            keys: Vec::new(),
            helper: None,
        }
    }

    /// Creates an ES2025 Iterator Helper wrapping an underlying iterator.
    ///
    /// The helper applies a lazy transformation (map, filter, take, drop, flatMap)
    /// to the values produced by the underlying iterator.
    pub fn new_helper(underlying: u64, callback: u64, helper_kind: HelperKind, count: u32) -> Self {
        Self {
            kind: IteratorKind::Helper,
            target: 0,
            index: 0,
            done: false,
            keys: Vec::new(),
            helper: Some(Box::new(HelperState {
                underlying,
                callback,
                helper_kind,
                counter: count,
                drop_done: false,
                inner_iter: 0,
            })),
        }
    }

    /// Creates a new array entries iterator yielding `[index, value]` pairs.
    pub fn new_array_entries(target: u64) -> Self {
        Self {
            kind: IteratorKind::ArrayEntries,
            target,
            index: 0,
            done: false,
            keys: Vec::new(),
            helper: None,
        }
    }

    /// Creates a new array keys iterator yielding indices.
    pub fn new_array_keys(target: u64) -> Self {
        Self {
            kind: IteratorKind::ArrayKeys,
            target,
            index: 0,
            done: false,
            keys: Vec::new(),
            helper: None,
        }
    }

    /// Creates a new array values iterator yielding element values.
    pub fn new_array_values(target: u64) -> Self {
        Self {
            kind: IteratorKind::ArrayValues,
            target,
            index: 0,
            done: false,
            keys: Vec::new(),
            helper: None,
        }
    }
}

/// An iterator result `{ value, done }`, stored as two NaN-boxed values.
///
/// Implements the IteratorResult interface from the ECMAScript specification.
///
/// `CreateIterResultObject ( value, done )`
///
/// [spec]: https://tc39.es/ecma262/#sec-createiterresultobject
pub struct IteratorResult {
    /// The `value` field of the result.
    pub value: u64,
    /// The `done` field of the result (NaN-boxed boolean).
    pub done: u64,
}

impl IteratorResult {
    /// `CreateIterResultObject ( value, done )` with `done = false`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-createiterresultobject
    pub fn with_value(value: u64) -> Self {
        // 1. Let obj be OrdinaryObjectCreate(%Object.prototype%).
        // 2. Perform ! CreateDataPropertyOrThrow(obj, "value", value).
        // 3. Perform ! CreateDataPropertyOrThrow(obj, "done", done).
        // NOTE: We store the fields directly instead of creating a full JS object.
        Self {
            value,
            done: JsValue::bool(false).raw_bits(),
        }
        // 4. Return obj.
    }

    /// `CreateIterResultObject ( value, done )` with `value = undefined, done = true`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-createiterresultobject
    pub fn done() -> Self {
        // 1. Let obj be OrdinaryObjectCreate(%Object.prototype%).
        // 2. Perform ! CreateDataPropertyOrThrow(obj, "value", undefined).
        // 3. Perform ! CreateDataPropertyOrThrow(obj, "done", true).
        // NOTE: We store the fields directly instead of creating a full JS object.
        Self {
            value: JsValue::undefined().raw_bits(),
            done: JsValue::bool(true).raw_bits(),
        }
        // 4. Return obj.
    }
}
