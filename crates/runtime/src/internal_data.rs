//! Internal data types for the unified JsObject.
//!
//! Each JS object has a kind ([`InternalKind`]) that determines its exotic
//! behavior (array length auto-update, function callability, proxy traps, etc.)
//! and optional internal data ([`InternalData`]) for kind-specific state.
//!
//! ## Key Types
//!
//! - [`InternalKind`] -- discriminant for exotic behavior
//! - [`InternalData`] -- kind-specific internal state
//! - [`ElementsStorage`] -- how indexed elements are stored
//! - [`ObjFlags`] -- packed object constraint/capability flags
//! - [`UnifiedObject`] -- the unified JS object representation

use std::collections::HashMap;

use nanbox::JsValue;
use shapes::ShapeId;

use crate::property::{DefinePropertyOptions, OwnPropertyDescriptor, PropertyError};

/// Sparse-array threshold: when `length > SPARSE_LENGTH_THRESHOLD` and
/// `used_elements < length / SPARSE_DENSITY_DIVISOR`, transition to Dictionary.
const SPARSE_LENGTH_THRESHOLD: u32 = 1024;

/// Density divisor for sparse detection (use Dictionary when < 25% full).
const SPARSE_DENSITY_DIVISOR: u32 = 4;

// =========================================================================
// InternalKind
// =========================================================================

/// Discriminant for the exotic behavior of a unified JsObject.
///
/// Determines which internal methods (e.g., `[[Call]]`, `[[DefineOwnProperty]]`)
/// have non-default behavior. The `repr(u8)` layout ensures compact storage
/// and fast comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InternalKind {
    /// Ordinary object -- no exotic behavior.
    Ordinary = 0,
    /// Array exotic -- auto-length, sparse holes, `Array.isArray()` returns true.
    Array = 1,
    /// Function -- callable, has `[[Call]]` internal method.
    Function = 2,
    /// Closure -- compiled function with captured environment.
    Closure = 3,
    /// Error -- has .message, .name, .stack properties.
    ErrorObj = 4,
    /// Proxy -- traps all internal methods to handler.
    Proxy = 5,
    /// Promise -- async state machine.
    Promise = 6,
    /// Iterator -- iteration protocol.
    Iterator = 7,
    /// IteratorResult -- {value, done} pair.
    IterResult = 8,
    /// Generator -- yield/next protocol.
    Generator = 9,
    /// Map -- ordered key-value collection.
    MapObj = 10,
    /// Set -- ordered value collection.
    SetObj = 11,
    /// RegExp -- regular expression.
    RegExpObj = 12,
    /// Date -- date/time (reserved).
    DateObj = 13,
    /// WeakMap -- weak key-value collection.
    WeakMapObj = 14,
    /// WeakSet -- weak value collection.
    WeakSetObj = 15,
    /// WeakRef -- weak reference.
    WeakRefObj = 16,
    /// Symbol -- symbol value wrapper.
    SymbolObj = 17,
    /// NativeFunc -- Rust-implemented callable.
    NativeFunc = 18,
    /// AsyncGenerator -- async generator protocol (wraps a sync generator).
    AsyncGenerator = 19,
    /// AsyncIterator -- async iterator helper wrapper (async map, filter, etc.).
    AsyncIterator = 20,
    /// Boolean wrapper object -- wraps a boolean primitive (e.g., `new Boolean(true)`).
    BooleanObj = 21,
    /// Number wrapper object -- wraps a number primitive (e.g., `new Number(42)`).
    NumberObj = 22,
    /// String wrapper object -- wraps a string primitive (e.g., `new String("hello")`).
    StringObj = 23,
}

// =========================================================================
// InternalData
// =========================================================================

/// Kind-specific internal data for a unified JsObject.
///
/// Each variant stores the state that the corresponding [`InternalKind`]
/// needs beyond the base object properties and elements.
#[derive(Debug)]
pub enum InternalData {
    /// No internal data (ordinary objects).
    None,

    /// Array: tracks the logical length (may differ from elements.len()).
    Array {
        /// The `.length` property value.
        length: u32,
        /// Whether the `.length` property is writable (default: true).
        /// Per ES spec §9.4.2, the `length` property has [[Writable]] which
        /// can be set to false via `Object.defineProperty`.
        length_writable: bool,
    },

    /// Function: code pointer + environment + metadata.
    Function {
        /// Index into the dispatch table (function pointer).
        code_idx: u32,
        /// Captured environment (NaN-boxed pointer), or 0 if none.
        env: u64,
        /// Function name (NaN-boxed string).
        name: u64,
        /// Number of formal parameters.
        param_count: u32,
        /// Whether this is an arrow function (no own `this`, no `arguments`).
        is_arrow: bool,
        /// Whether this is a generator function.
        is_generator: bool,
        /// Whether this function was defined in strict mode.
        is_strict: bool,
    },

    /// Error: message and stack trace.
    Error {
        /// Error type tag (0=Error, 1=TypeError, 2=RangeError, etc.).
        error_tag: u32,
        /// The error message (NaN-boxed string).
        message: u64,
        /// The raw message without prefix (NaN-boxed string).
        raw_message: u64,
        /// Stack trace (NaN-boxed string).
        stack: u64,
    },

    /// Proxy: target + handler + revocation state.
    Proxy {
        /// The proxy target (NaN-boxed object).
        target: u64,
        /// The handler object (NaN-boxed object).
        handler: u64,
        /// Whether this proxy has been revoked.
        revoked: bool,
    },

    /// Promise: async state machine backed by a full [`JsPromise`](crate::promise::JsPromise).
    Promise {
        /// The underlying promise state machine.
        inner: Box<crate::promise::JsPromise>,
    },

    /// Iterator: full iteration state backed by [`JsIterator`](crate::iterator::JsIterator).
    IteratorState {
        /// The underlying iterator state machine.
        inner: Box<crate::iterator::JsIterator>,
    },

    /// IteratorResult: {value, done} pair.
    IterResult {
        /// The value field.
        value: u64,
        /// The done field (NaN-boxed boolean).
        done: u64,
    },

    /// Generator: state machine protocol.
    Generator {
        /// The state object holding state_index, resume_mode, sent_value, params, and live var slots.
        state_obj: u64,
        /// Index of the compiled resume function in the module's function table.
        resume_func_idx: u32,
    },

    /// Map: ordered key-value pairs.
    Map {
        /// Key-value entries in insertion order.
        entries: Vec<(JsValue, JsValue)>,
    },

    /// Set: ordered values.
    Set {
        /// Values in insertion order.
        values: Vec<JsValue>,
    },

    /// RegExp: compiled pattern.
    RegExp {
        /// The compiled regex (boxed to keep enum small).
        inner: Box<dyn std::any::Any>,
    },

    /// WeakRef: weak target reference.
    WeakRef {
        /// The target (strong ref for now; weak when GC matures).
        target: u64,
    },

    /// Date: milliseconds since Unix epoch (like JS `Date.getTime()`).
    Date {
        /// Milliseconds since 1970-01-01T00:00:00Z. NaN for invalid dates.
        timestamp: f64,
    },

    /// Symbol: unique symbol identifier.
    Symbol {
        /// The unique symbol ID (monotonically increasing).
        id: u64,
    },

    /// NativeFunc: Rust-implemented callable.
    NativeFunc {
        /// The native function pointer (takes `this` as u64, returns u64).
        func: fn(u64) -> u64,
        /// Context data.
        context: u64,
    },

    /// AsyncGenerator: wraps a sync generator with an async request queue.
    AsyncGenerator {
        /// Current state of the async generator state machine.
        state: crate::async_generator::AsyncGeneratorState,
        /// Queue of pending requests (`.next()`, `.throw()`, `.return()` calls).
        queue: std::collections::VecDeque<crate::async_generator::AsyncGeneratorRequest>,
        /// The underlying sync generator object (NaN-boxed).
        generator: u64,
    },

    /// AsyncIterator: async iterator helper wrapping a source async iterator.
    AsyncIterator {
        /// The async iterator helper state.
        inner: Box<crate::async_iterator_helpers::AsyncIteratorState>,
    },

    /// Boolean wrapper: stores the wrapped boolean value.
    BooleanWrapper {
        /// The wrapped boolean value (NaN-boxed boolean).
        value: u64,
    },

    /// Number wrapper: stores the wrapped number value.
    NumberWrapper {
        /// The wrapped number value (NaN-boxed number).
        value: u64,
    },

    /// String wrapper: stores the wrapped string value.
    StringWrapper {
        /// The wrapped string value (NaN-boxed string).
        value: u64,
    },
}

// =========================================================================
// ElementsStorage
// =========================================================================

/// How indexed elements are stored in a unified JsObject.
#[derive(Debug)]
pub enum ElementsStorage {
    /// No indexed elements.
    None,
    /// Dense, contiguous elements (typical arrays).
    Dense(Vec<JsValue>),
    /// Dense with holes (deleted elements become `None` entries).
    Holey(Vec<Option<JsValue>>),
    /// Sparse dictionary storage -- used when the array is very sparse
    /// (e.g., `arr[1_000_000] = 1`). Only populated indices are stored.
    Dictionary(HashMap<u32, JsValue>),
}

/// Discriminant for the elements storage kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ElementKind {
    /// No indexed elements.
    None = 0,
    /// Dense contiguous elements.
    Dense = 1,
    /// Dense with holes.
    Holey = 2,
    /// Sparse dictionary storage.
    Dictionary = 3,
}

// =========================================================================
// ObjFlags
// =========================================================================

/// Object flags packed into a single byte.
///
/// Tracks immutability constraints (`frozen`, `sealed`, `non_extensible`)
/// and capabilities (`callable`, `constructable`) for a unified JsObject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjFlags(u8);

impl ObjFlags {
    /// Bit mask for the frozen flag.
    pub const FROZEN: u8 = 0x01;
    /// Bit mask for the sealed flag.
    pub const SEALED: u8 = 0x02;
    /// Bit mask for the non-extensible flag.
    pub const NON_EXTENSIBLE: u8 = 0x04;
    /// Bit mask for the callable flag.
    pub const CALLABLE: u8 = 0x08;
    /// Bit mask for the constructable flag.
    pub const CONSTRUCTABLE: u8 = 0x10;

    /// Creates a new `ObjFlags` with all flags cleared (extensible, not callable).
    pub fn new() -> Self {
        Self(0)
    }

    /// Returns `true` if the object is frozen.
    pub fn is_frozen(&self) -> bool {
        self.0 & Self::FROZEN != 0
    }

    /// Returns `true` if the object is sealed.
    pub fn is_sealed(&self) -> bool {
        self.0 & Self::SEALED != 0
    }

    /// Returns `true` if the object is extensible (can have new properties added).
    pub fn is_extensible(&self) -> bool {
        self.0 & Self::NON_EXTENSIBLE == 0
    }

    /// Returns `true` if the object is callable (has `[[Call]]`).
    pub fn is_callable(&self) -> bool {
        self.0 & Self::CALLABLE != 0
    }

    /// Returns `true` if the object is constructable (has `[[Construct]]`).
    pub fn is_constructable(&self) -> bool {
        self.0 & Self::CONSTRUCTABLE != 0
    }

    /// Freezes the object, which also seals it and prevents extensions.
    pub fn set_frozen(&mut self) {
        self.0 |= Self::FROZEN | Self::SEALED | Self::NON_EXTENSIBLE;
    }

    /// Seals the object, which also prevents extensions.
    pub fn set_sealed(&mut self) {
        self.0 |= Self::SEALED | Self::NON_EXTENSIBLE;
    }

    /// Prevents extensions on the object.
    pub fn prevent_extensions(&mut self) {
        self.0 |= Self::NON_EXTENSIBLE;
    }

    /// Marks the object as callable.
    pub fn set_callable(&mut self) {
        self.0 |= Self::CALLABLE;
    }

    /// Marks the object as constructable.
    pub fn set_constructable(&mut self) {
        self.0 |= Self::CONSTRUCTABLE;
    }

    /// Returns the raw byte value.
    pub fn raw(&self) -> u8 {
        self.0
    }
}

impl Default for ObjFlags {
    fn default() -> Self {
        Self::new()
    }
}

// =========================================================================
// UnifiedObject
// =========================================================================

/// Unified JavaScript object representation.
///
/// All JS object types (plain objects, arrays, functions, closures, errors,
/// proxies, promises, etc.) share this single struct. The `kind` field
/// determines exotic behavior, and `internal` holds kind-specific data.
///
/// # Memory Layout
///
/// Target: ~64 bytes on 64-bit platforms (fits in one cache line).
/// - `flags` (1) + `kind` (1) + `element_kind` (1) + `_pad` (1) = 4 bytes
/// - `shape_id` (4 bytes)
/// - `slots`: `Vec<JsValue>` (24 bytes)
/// - `elements`: `ElementsStorage` (24 bytes -- enum with Vec)
/// - `internal`: `Option<Box<InternalData>>` (8 bytes)
/// - Total: ~64 bytes (may vary with alignment)
pub struct UnifiedObject {
    /// Packed flags: frozen, sealed, non-extensible, callable, constructable.
    pub flags: ObjFlags,
    /// The exotic behavior kind.
    pub kind: InternalKind,
    /// Elements storage kind (cached for fast dispatch).
    pub element_kind: ElementKind,
    /// Padding for alignment.
    _pad: u8,
    /// Shape reference (includes property layout + prototype in future).
    pub shape_id: ShapeId,
    /// Named property slots (indexed by shape's property descriptors).
    pub slots: Vec<JsValue>,
    /// Indexed element storage.
    pub elements: ElementsStorage,
    /// Kind-specific internal data (`None` for ordinary objects).
    pub internal: Option<Box<InternalData>>,
}

impl std::fmt::Debug for UnifiedObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedObject")
            .field("flags", &self.flags)
            .field("kind", &self.kind)
            .field("element_kind", &self.element_kind)
            .field("shape_id", &self.shape_id)
            .field("slots_len", &self.slots.len())
            .field("has_internal", &self.internal.is_some())
            .finish()
    }
}

impl UnifiedObject {
    // =====================================================================
    // Constructors
    // =====================================================================

    /// Create a new ordinary object with no properties or elements.
    pub fn ordinary(shape_id: ShapeId) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::Ordinary,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: None,
        }
    }

    /// Create a new array object with dense elements.
    pub fn array(shape_id: ShapeId, elements: Vec<JsValue>) -> Self {
        let length = elements.len() as u32;
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::Array,
            element_kind: ElementKind::Dense,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::Dense(elements),
            internal: Some(Box::new(InternalData::Array {
                length,
                length_writable: true,
            })),
        }
    }

    /// Create a new function object.
    pub fn function(
        shape_id: ShapeId,
        code_idx: u32,
        env: u64,
        name: u64,
        param_count: u32,
        is_arrow: bool,
    ) -> Self {
        let mut flags = ObjFlags::new();
        flags.set_callable();
        Self {
            flags,
            kind: InternalKind::Function,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Function {
                code_idx,
                env,
                name,
                param_count,
                is_arrow,
                is_generator: false,
                is_strict: false,
            })),
        }
    }

    /// Create a new closure object (compiled function with captured environment).
    ///
    /// `closure_flags` encodes:
    /// - Bit 0: `is_arrow` (skip .prototype creation, lexical this)
    /// - Bit 1: `is_strict` (sloppy this substitution check)
    /// - Bit 2: `is_generator` (generator function)
    pub fn closure(shape_id: ShapeId, code_idx: u32, env: u64, closure_flags: u32) -> Self {
        let mut flags = ObjFlags::new();
        flags.set_callable();
        let is_arrow = closure_flags & 1 != 0;
        let is_strict = closure_flags & 2 != 0;
        let is_generator = closure_flags & 4 != 0;
        Self {
            flags,
            kind: InternalKind::Closure,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Function {
                code_idx,
                env,
                name: 0,
                param_count: 0,
                is_arrow,
                is_generator,
                is_strict,
            })),
        }
    }

    /// Create a new error object.
    pub fn error(
        shape_id: ShapeId,
        error_tag: u32,
        message: u64,
        raw_message: u64,
        stack: u64,
    ) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::ErrorObj,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Error {
                error_tag,
                message,
                raw_message,
                stack,
            })),
        }
    }

    /// Create a new proxy object.
    ///
    /// Proxy objects start with the empty shape since they intercept all
    /// property operations via handler traps.
    pub fn proxy(target: u64, handler: u64) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::Proxy,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id: ShapeId(0),
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Proxy {
                target,
                handler,
                revoked: false,
            })),
        }
    }

    /// Create a new promise object backed by a real [`JsPromise`](crate::promise::JsPromise).
    pub fn promise(shape_id: ShapeId) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::Promise,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Promise {
                inner: Box::new(crate::promise::JsPromise::new()),
            })),
        }
    }

    /// Create a new iterator object from a [`JsIterator`](crate::iterator::JsIterator).
    pub fn iterator(shape_id: ShapeId, iter: crate::iterator::JsIterator) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::Iterator,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::IteratorState {
                inner: Box::new(iter),
            })),
        }
    }

    /// Create a new iterator result object (`{value, done}`).
    pub fn iter_result(value: u64, done: u64) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::IterResult,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id: ShapeId(0),
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::IterResult { value, done })),
        }
    }

    /// Create a new generator object backed by the state machine protocol.
    ///
    /// `state_obj` is a NaN-boxed reference to the state object holding
    /// state_index, resume_mode, sent_value, params, and live variable slots.
    /// `resume_func_idx` is the index of the compiled resume function.
    pub fn generator(shape_id: ShapeId, state_obj: u64, resume_func_idx: u32) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::Generator,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Generator {
                state_obj,
                resume_func_idx,
            })),
        }
    }

    /// Create a new async generator object wrapping a sync generator.
    ///
    /// `generator` is a NaN-boxed reference to the underlying sync generator
    /// that will be driven by the async generator protocol.
    pub fn async_generator(shape_id: ShapeId, generator: u64) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::AsyncGenerator,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::AsyncGenerator {
                state: crate::async_generator::AsyncGeneratorState::SuspendedStart,
                queue: std::collections::VecDeque::new(),
                generator,
            })),
        }
    }

    /// Create a new async iterator helper object.
    ///
    /// Wraps an [`AsyncIteratorState`](crate::async_iterator_helpers::AsyncIteratorState)
    /// that drives a source async iterator through a lazy transformation.
    pub fn async_iterator(
        shape_id: ShapeId,
        state: crate::async_iterator_helpers::AsyncIteratorState,
    ) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::AsyncIterator,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::AsyncIterator {
                inner: Box::new(state),
            })),
        }
    }

    /// Create a new Map object.
    pub fn map(shape_id: ShapeId) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::MapObj,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Map {
                entries: Vec::new(),
            })),
        }
    }

    /// Create a new Set object.
    pub fn set(shape_id: ShapeId) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::SetObj,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Set { values: Vec::new() })),
        }
    }

    /// Create a new WeakMap object.
    ///
    /// Reuses [`InternalData::Map`] for storage since WeakMap has the same
    /// key-value pair structure (true weak semantics deferred to GC maturity).
    pub fn weak_map(shape_id: ShapeId) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::WeakMapObj,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Map {
                entries: Vec::new(),
            })),
        }
    }

    /// Create a new WeakSet object.
    ///
    /// Reuses [`InternalData::Set`] for storage since WeakSet has the same
    /// value collection structure (true weak semantics deferred to GC maturity).
    pub fn weak_set(shape_id: ShapeId) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::WeakSetObj,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Set { values: Vec::new() })),
        }
    }

    /// Create a new WeakRef object.
    ///
    /// Stores the target as a strong reference for now; true weak reference
    /// semantics will be implemented when the GC matures.
    pub fn weak_ref(shape_id: ShapeId, target: u64) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::WeakRefObj,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::WeakRef { target })),
        }
    }

    /// Create a new Symbol wrapper object.
    ///
    /// Stores the unique symbol ID in [`InternalData::Symbol`].
    pub fn symbol(shape_id: ShapeId, id: u64) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::SymbolObj,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Symbol { id })),
        }
    }

    /// Create a new RegExp object.
    ///
    /// Stores the compiled regexp data in [`InternalData::RegExp`] as a
    /// type-erased `Box<dyn Any>`.
    pub fn regexp(shape_id: ShapeId, data: Box<dyn std::any::Any>) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::RegExpObj,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::RegExp { inner: data })),
        }
    }

    /// Create a new Date object from a millisecond timestamp.
    ///
    /// `timestamp` is milliseconds since 1970-01-01T00:00:00Z.
    /// Use `f64::NAN` for an invalid date.
    pub fn date(shape_id: ShapeId, timestamp: f64) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::DateObj,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::Date { timestamp })),
        }
    }

    /// Create a new native function object.
    pub fn native_func(func: fn(u64) -> u64, context: u64) -> Self {
        let mut flags = ObjFlags::new();
        flags.set_callable();
        Self {
            flags,
            kind: InternalKind::NativeFunc,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id: ShapeId(0),
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::NativeFunc { func, context })),
        }
    }

    /// Create a new Boolean wrapper object.
    ///
    /// Wraps a boolean primitive value in an object (equivalent to `new Boolean(val)`).
    /// The wrapped value is stored as NaN-boxed bits in [`InternalData::BooleanWrapper`].
    pub fn boolean_wrapper(shape_id: ShapeId, val: u64) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::BooleanObj,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::BooleanWrapper { value: val })),
        }
    }

    /// Create a new Number wrapper object.
    ///
    /// Wraps a number primitive value in an object (equivalent to `new Number(val)`).
    /// The wrapped value is stored as NaN-boxed bits in [`InternalData::NumberWrapper`].
    pub fn number_wrapper(shape_id: ShapeId, val: u64) -> Self {
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::NumberObj,
            element_kind: ElementKind::None,
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: ElementsStorage::None,
            internal: Some(Box::new(InternalData::NumberWrapper { value: val })),
        }
    }

    /// Create a new String wrapper object.
    ///
    /// Wraps a string primitive value in an object (equivalent to `new String(val)`).
    /// Populates indexed character elements and sets a `.length` slot.
    /// Each character is stored as a NaN-boxed single-character string element.
    pub fn string_wrapper(shape_id: ShapeId, val: u64) -> Self {
        let str_val = JsValue::from_raw_bits(val);
        let char_elements = Self::extract_string_chars(str_val);
        let char_count = char_elements.len();
        Self {
            flags: ObjFlags::new(),
            kind: InternalKind::StringObj,
            element_kind: if char_count > 0 {
                ElementKind::Dense
            } else {
                ElementKind::None
            },
            _pad: 0,
            shape_id,
            slots: Vec::new(),
            elements: if char_count > 0 {
                ElementsStorage::Dense(char_elements)
            } else {
                ElementsStorage::None
            },
            internal: Some(Box::new(InternalData::StringWrapper { value: val })),
        }
    }

    /// Extract individual characters from a NaN-boxed string value as NaN-boxed
    /// single-character strings.
    ///
    /// Returns an empty `Vec` if the value is not a string or is empty.
    fn extract_string_chars(val: JsValue) -> Vec<JsValue> {
        let Some(ptr) = val.as_string() else {
            return Vec::new();
        };
        if ptr.is_null() {
            return Vec::new();
        }
        // SAFETY: The string pointer was created by string_from_data or
        // equivalent via Box::into_raw on an RtString.
        let rt_str = unsafe { &*(ptr as *const crate::string_ops::RtString) };
        let s = rt_str.as_str();
        let mut chars = Vec::with_capacity(s.len());
        for ch in s.chars() {
            let ch_string = String::from(ch);
            let rt_ch = Box::new(crate::string_ops::RtString::new(ch_string));
            let ch_ptr = Box::into_raw(rt_ch) as *const ();
            chars.push(JsValue::string(ch_ptr));
        }
        chars
    }

    // =====================================================================
    // Property slot access
    // =====================================================================

    /// Get the value at the given slot index, or `undefined` if out of bounds.
    pub fn get_slot(&self, index: u32) -> JsValue {
        self.slots
            .get(index as usize)
            .copied()
            .unwrap_or(JsValue::undefined())
    }

    /// Set the value at the given slot index.
    ///
    /// If the index is beyond the current slot count, the slots vector is
    /// extended with `undefined` values to accommodate.
    pub fn set_slot(&mut self, index: u32, value: JsValue) {
        let idx = index as usize;
        if idx >= self.slots.len() {
            self.slots.resize(idx + 1, JsValue::undefined());
        }
        self.slots[idx] = value;
    }

    /// Returns the number of named property slots.
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    // =====================================================================
    // Element access
    // =====================================================================

    /// Get the element at the given index, or `None` if not present.
    pub fn get_element(&self, index: u32) -> Option<JsValue> {
        match &self.elements {
            ElementsStorage::None => None,
            ElementsStorage::Dense(elems) => elems.get(index as usize).copied(),
            ElementsStorage::Holey(elems) => elems.get(index as usize).and_then(|opt| *opt),
            ElementsStorage::Dictionary(map) => map.get(&index).copied(),
        }
    }

    /// Set the element at the given index.
    ///
    /// If the elements storage is `None`, it is promoted to `Dense` or
    /// `Dictionary` depending on the index. If the index is beyond current
    /// capacity, the storage is extended (or transitioned to Dictionary
    /// if the resulting array would be too sparse).
    pub fn set_element(&mut self, index: u32, value: JsValue) {
        let idx = index as usize;

        // Check if we should use Dictionary mode for very sparse access
        if self.should_use_dictionary(index) {
            self.transition_to_dictionary();
        }

        match &mut self.elements {
            ElementsStorage::None => {
                // For large sparse indices, go directly to Dictionary
                if index >= SPARSE_LENGTH_THRESHOLD {
                    let mut map = HashMap::new();
                    map.insert(index, value);
                    self.elements = ElementsStorage::Dictionary(map);
                    self.element_kind = ElementKind::Dictionary;
                } else {
                    let mut elems = vec![JsValue::undefined(); idx + 1];
                    elems[idx] = value;
                    self.elements = ElementsStorage::Dense(elems);
                    self.element_kind = ElementKind::Dense;
                }
            }
            ElementsStorage::Dense(elems) => {
                if idx >= elems.len() {
                    elems.resize(idx + 1, JsValue::undefined());
                }
                elems[idx] = value;
            }
            ElementsStorage::Holey(elems) => {
                if idx >= elems.len() {
                    elems.resize(idx + 1, None);
                }
                elems[idx] = Some(value);
            }
            ElementsStorage::Dictionary(map) => {
                map.insert(index, value);
            }
        }
    }

    /// Check whether setting `index` should trigger a Dictionary transition.
    ///
    /// Returns `true` when the target index is far beyond the current storage
    /// capacity and storing it contiguously would waste memory.
    fn should_use_dictionary(&self, index: u32) -> bool {
        if index < SPARSE_LENGTH_THRESHOLD {
            return false;
        }
        let current_len = match &self.elements {
            ElementsStorage::None => 0u32,
            ElementsStorage::Dense(elems) => elems.len() as u32,
            ElementsStorage::Holey(elems) => elems.len() as u32,
            ElementsStorage::Dictionary(_) => return false, // already Dictionary
        };
        // If the new length would be > threshold and used < 25%, go Dictionary
        let new_len = index + 1;
        let used = match &self.elements {
            ElementsStorage::Dense(elems) => elems.len() as u32,
            ElementsStorage::Holey(elems) => {
                elems.iter().filter(|opt| opt.is_some()).count() as u32
            }
            _ => current_len,
        };
        // +1 for the element we're about to add
        let used_after = used + 1;
        new_len > SPARSE_LENGTH_THRESHOLD && used_after < new_len / SPARSE_DENSITY_DIVISOR
    }

    /// Transition the current elements storage to Dictionary mode.
    ///
    /// Copies all existing elements into a `HashMap<u32, JsValue>`.
    fn transition_to_dictionary(&mut self) {
        let map = match std::mem::replace(&mut self.elements, ElementsStorage::None) {
            ElementsStorage::None => HashMap::new(),
            ElementsStorage::Dense(elems) => {
                let mut map = HashMap::with_capacity(elems.len());
                for (i, val) in elems.into_iter().enumerate() {
                    if !val.is_undefined() {
                        map.insert(i as u32, val);
                    }
                }
                map
            }
            ElementsStorage::Holey(elems) => {
                let mut map = HashMap::new();
                for (i, opt) in elems.into_iter().enumerate() {
                    if let Some(val) = opt {
                        map.insert(i as u32, val);
                    }
                }
                map
            }
            ElementsStorage::Dictionary(map) => map,
        };
        self.elements = ElementsStorage::Dictionary(map);
        self.element_kind = ElementKind::Dictionary;
    }

    /// Returns the number of elements in the storage.
    ///
    /// For Dictionary storage, this returns the number of populated entries
    /// (not the logical length).
    pub fn elements_len(&self) -> usize {
        match &self.elements {
            ElementsStorage::None => 0,
            ElementsStorage::Dense(elems) => elems.len(),
            ElementsStorage::Holey(elems) => elems.len(),
            ElementsStorage::Dictionary(map) => map.len(),
        }
    }

    // =====================================================================
    // Array operations (for InternalKind::Array migration)
    // =====================================================================

    /// Push a value onto the end of this array's elements.
    ///
    /// Updates the internal `length` to match. Works with Dense, Holey, and
    /// Dictionary storage. No-op if storage is `None`.
    pub fn array_push(&mut self, val: JsValue) {
        // Read current length before borrowing elements mutably.
        let current_len = self.as_array_length().unwrap_or(0);
        match &mut self.elements {
            ElementsStorage::Dense(elems) => {
                elems.push(val);
                if let Some(InternalData::Array { length, .. }) = self.internal.as_deref_mut() {
                    *length = elems.len() as u32;
                }
            }
            ElementsStorage::Holey(elems) => {
                elems.push(Some(val));
                if let Some(InternalData::Array { length, .. }) = self.internal.as_deref_mut() {
                    *length = elems.len() as u32;
                }
            }
            ElementsStorage::Dictionary(map) => {
                map.insert(current_len, val);
                if let Some(InternalData::Array { length, .. }) = self.internal.as_deref_mut() {
                    *length = current_len + 1;
                }
            }
            ElementsStorage::None => {}
        }
    }

    /// Pop the last element from this array's elements.
    ///
    /// Updates the internal `length` to match. Returns `None` if empty.
    /// Works with Dense, Holey, and Dictionary storage.
    pub fn array_pop(&mut self) -> Option<JsValue> {
        // Read current length before borrowing elements mutably.
        let current_len = self.as_array_length().unwrap_or(0);
        match &mut self.elements {
            ElementsStorage::Dense(elems) => {
                let val = elems.pop();
                if let Some(InternalData::Array { length, .. }) = self.internal.as_deref_mut() {
                    *length = elems.len() as u32;
                }
                val
            }
            ElementsStorage::Holey(elems) => {
                let val = elems.pop().flatten();
                if let Some(InternalData::Array { length, .. }) = self.internal.as_deref_mut() {
                    *length = elems.len() as u32;
                }
                val
            }
            ElementsStorage::Dictionary(map) => {
                if current_len == 0 {
                    return None;
                }
                let last_idx = current_len - 1;
                let val = map.remove(&last_idx);
                if let Some(InternalData::Array { length, .. }) = self.internal.as_deref_mut() {
                    *length = last_idx;
                }
                val
            }
            ElementsStorage::None => None,
        }
    }

    /// Get the logical array length (from InternalData::Array).
    ///
    /// Returns 0 if this is not an array.
    pub fn array_len(&self) -> u32 {
        self.as_array_length().unwrap_or(0)
    }

    /// Get a read-only slice of the dense elements.
    ///
    /// Returns an empty slice if the storage is not `Dense`.
    /// For Holey or Dictionary storage, use [`array_elements_resolved`] instead.
    pub fn array_elements(&self) -> &[JsValue] {
        match &self.elements {
            ElementsStorage::Dense(elems) => elems,
            _ => &[],
        }
    }

    /// Collect all elements as a `Vec<JsValue>`, resolving holes to `undefined`.
    ///
    /// Works with all storage variants (Dense, Holey, Dictionary).
    pub fn array_elements_resolved(&self) -> Vec<JsValue> {
        let len = self.array_len() as usize;
        match &self.elements {
            ElementsStorage::None => Vec::new(),
            ElementsStorage::Dense(elems) => {
                let mut result = elems.clone();
                result.resize(len, JsValue::undefined());
                result
            }
            ElementsStorage::Holey(elems) => {
                let mut result = Vec::with_capacity(len);
                for i in 0..len {
                    result.push(
                        elems
                            .get(i)
                            .and_then(|opt| *opt)
                            .unwrap_or(JsValue::undefined()),
                    );
                }
                result
            }
            ElementsStorage::Dictionary(map) => {
                let mut result = vec![JsValue::undefined(); len];
                for (&idx, &val) in map {
                    if (idx as usize) < len {
                        result[idx as usize] = val;
                    }
                }
                result
            }
        }
    }

    /// Get a mutable reference to the dense elements vector.
    ///
    /// Returns `None` if the storage is not `Dense`.
    pub fn array_elements_mut(&mut self) -> Option<&mut Vec<JsValue>> {
        match &mut self.elements {
            ElementsStorage::Dense(elems) => Some(elems),
            _ => None,
        }
    }

    /// Set the logical array length, truncating elements if needed.
    ///
    /// Handles Dense, Holey, and Dictionary storage.
    pub fn array_set_length(&mut self, new_len: u32) {
        if let Some(InternalData::Array { length, .. }) = self.internal.as_deref_mut() {
            *length = new_len;
        }
        let new_len_usize = new_len as usize;
        match &mut self.elements {
            ElementsStorage::Dense(elems) => {
                if new_len_usize < elems.len() {
                    elems.truncate(new_len_usize);
                }
            }
            ElementsStorage::Holey(elems) => {
                if new_len_usize < elems.len() {
                    elems.truncate(new_len_usize);
                }
            }
            ElementsStorage::Dictionary(map) => {
                map.retain(|&idx, _| idx < new_len);
            }
            ElementsStorage::None => {}
        }
    }

    /// Sync the internal array length to match the elements count.
    ///
    /// Call this after directly mutating elements via `array_elements_mut()`.
    /// Works with Dense and Holey storage.
    pub fn array_sync_length(&mut self) {
        let len = match &self.elements {
            ElementsStorage::Dense(elems) => Some(elems.len() as u32),
            ElementsStorage::Holey(elems) => Some(elems.len() as u32),
            _ => None,
        };
        if let Some(new_len) = len
            && let Some(InternalData::Array { length, .. }) = self.internal.as_deref_mut()
        {
            *length = new_len;
        }
    }

    /// Delete an element from the array at the given index.
    ///
    /// For Dense storage, transitions to Holey. For Holey, sets the entry
    /// to `None`. For Dictionary, removes the key. Returns `true` if deleted.
    pub fn delete_element(&mut self, index: u32) -> bool {
        match &mut self.elements {
            ElementsStorage::None => false,
            ElementsStorage::Dense(_) => {
                // Transition Dense -> Holey
                let old = std::mem::replace(&mut self.elements, ElementsStorage::None);
                if let ElementsStorage::Dense(elems) = old {
                    let mut holey: Vec<Option<JsValue>> = elems.into_iter().map(Some).collect();
                    if (index as usize) < holey.len() {
                        holey[index as usize] = None;
                    }
                    self.elements = ElementsStorage::Holey(holey);
                    self.element_kind = ElementKind::Holey;
                }
                true
            }
            ElementsStorage::Holey(elems) => {
                if let Some(slot) = elems.get_mut(index as usize) {
                    *slot = None;
                    true
                } else {
                    false
                }
            }
            ElementsStorage::Dictionary(map) => {
                map.remove(&index);
                true
            }
        }
    }

    // =====================================================================
    // Internal data access
    // =====================================================================

    /// If this is an array, returns the logical length.
    pub fn as_array_length(&self) -> Option<u32> {
        match self.internal.as_deref() {
            Some(InternalData::Array { length, .. }) => Some(*length),
            _ => None,
        }
    }

    /// If this has function internal data, returns a reference to it.
    pub fn as_function_data(&self) -> Option<&InternalData> {
        match self.internal.as_deref() {
            Some(data @ InternalData::Function { .. }) => Some(data),
            _ => None,
        }
    }

    /// If this has error internal data, returns a reference to it.
    pub fn as_error_data(&self) -> Option<&InternalData> {
        match self.internal.as_deref() {
            Some(data @ InternalData::Error { .. }) => Some(data),
            _ => None,
        }
    }

    /// If this has proxy internal data, returns a reference to it.
    pub fn as_proxy_data(&self) -> Option<&InternalData> {
        match self.internal.as_deref() {
            Some(data @ InternalData::Proxy { .. }) => Some(data),
            _ => None,
        }
    }

    /// Returns a reference to the internal data, if any.
    pub fn internal_data(&self) -> Option<&InternalData> {
        self.internal.as_deref()
    }

    /// Returns a mutable reference to the internal data, if any.
    pub fn internal_data_mut(&mut self) -> Option<&mut InternalData> {
        self.internal.as_deref_mut()
    }

    // =====================================================================
    // Flags
    // =====================================================================

    /// Returns `true` if this object is callable (function, closure, or native func).
    pub fn is_callable(&self) -> bool {
        self.flags.is_callable()
    }

    /// Returns `true` if this object is frozen.
    pub fn is_frozen(&self) -> bool {
        self.flags.is_frozen()
    }

    /// Returns `true` if this object is sealed.
    pub fn is_sealed(&self) -> bool {
        self.flags.is_sealed()
    }

    /// Returns `true` if this object is extensible.
    pub fn is_extensible(&self) -> bool {
        self.flags.is_extensible()
    }

    /// Freeze this object (also seals and prevents extensions).
    pub fn freeze(&mut self) {
        self.flags.set_frozen();
    }

    /// Seal this object (also prevents extensions).
    pub fn seal(&mut self) {
        self.flags.set_sealed();
    }

    /// Prevent extensions on this object.
    pub fn prevent_extensions(&mut self) {
        self.flags.prevent_extensions();
    }

    // =====================================================================
    // Property access (bridge for migration from JsObject)
    // =====================================================================

    /// Get a property value by name using the shape table.
    ///
    /// For data properties, returns the stored value.
    /// For accessor properties, returns the getter function (or `undefined`
    /// if no getter). The caller is responsible for invoking the getter.
    ///
    /// Returns `None` if the property does not exist on this object.
    pub fn get_slot_by_name(
        &self,
        name: &str,
        shapes: &shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> Option<JsValue> {
        let atom = interner.intern(name);
        let desc = shapes.lookup(self.shape_id, atom)?;
        self.slots.get(desc.offset as usize).copied()
    }

    /// Get a property value by [`shapes::PropertyKey`].
    ///
    /// This is the general form that supports symbol and private keys
    /// in addition to string keys. For string keys, prefer
    /// [`get_slot_by_name`](Self::get_slot_by_name) which handles interning.
    pub fn get_slot_by_key(
        &self,
        key: &shapes::PropertyKey,
        shapes: &shapes::ShapeTable,
    ) -> Option<JsValue> {
        let desc = shapes.lookup_key(self.shape_id, key)?;
        self.slots.get(desc.offset as usize).copied()
    }

    /// Set a property value by [`shapes::PropertyKey`], transitioning the shape if needed.
    ///
    /// This is the general form that supports symbol and private keys.
    /// Returns `true` on success, `false` if the object is frozen/sealed/non-extensible
    /// or if the property is an accessor.
    pub fn set_slot_by_key(
        &mut self,
        key: shapes::PropertyKey,
        value: JsValue,
        shapes: &mut shapes::ShapeTable,
    ) -> bool {
        if self.flags.is_frozen() {
            return false;
        }
        if let Some(desc) = shapes.lookup_key(self.shape_id, &key) {
            if desc.is_accessor() {
                return false;
            }
            if !desc.writable {
                return false;
            }
            let offset = desc.offset as usize;
            if offset < self.slots.len() {
                self.slots[offset] = value;
            }
            return true;
        }
        // New property
        if !self.flags.is_extensible() {
            return false;
        }
        let new_shape = shapes.add_property_key(self.shape_id, key);
        self.shape_id = new_shape;
        self.slots.push(value);
        true
    }

    /// Check if an own property is an accessor property.
    ///
    /// Returns `true` if the property exists and has `PropertyKind::Accessor`.
    pub fn is_accessor_property(
        &self,
        name: &str,
        shapes: &shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> bool {
        let atom = interner.intern(name);
        shapes
            .lookup(self.shape_id, atom)
            .is_some_and(|desc| desc.is_accessor())
    }

    /// Get the getter function for an accessor property.
    ///
    /// Returns `None` if the property does not exist or is not an accessor.
    /// Returns `Some(getter)` where getter may be `undefined` if no getter was set.
    pub fn get_accessor_getter(
        &self,
        name: &str,
        shapes: &shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> Option<JsValue> {
        let atom = interner.intern(name);
        let desc = shapes.lookup(self.shape_id, atom)?;
        if !desc.is_accessor() {
            return None;
        }
        Some(
            self.slots
                .get(desc.offset as usize)
                .copied()
                .unwrap_or(JsValue::undefined()),
        )
    }

    /// Get the setter function for an accessor property.
    ///
    /// Returns `None` if the property does not exist or is not an accessor.
    /// Returns `Some(setter)` where setter may be `undefined` if no setter was set.
    pub fn get_accessor_setter(
        &self,
        name: &str,
        shapes: &shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> Option<JsValue> {
        let atom = interner.intern(name);
        let desc = shapes.lookup(self.shape_id, atom)?;
        if !desc.is_accessor() {
            return None;
        }
        Some(
            self.slots
                .get(desc.offset as usize + 1)
                .copied()
                .unwrap_or(JsValue::undefined()),
        )
    }

    /// Set a property value by name, transitioning the shape if needed.
    ///
    /// For accessor properties, this does NOT invoke the setter -- the caller
    /// must check `is_accessor_property` and handle setter invocation. This
    /// method returns `false` for accessor properties (indicating the caller
    /// should handle it), unless the property is being overwritten during a
    /// shape transition.
    ///
    /// Returns `true` on success, `false` if the object is frozen/sealed/non-extensible
    /// or if the property is an accessor (caller handles setter invocation).
    pub fn set_slot_by_name(
        &mut self,
        name: &str,
        value: JsValue,
        shapes: &mut shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> bool {
        if self.flags.is_frozen() {
            return false;
        }
        let atom = interner.intern(name);
        if let Some(desc) = shapes.lookup(self.shape_id, atom) {
            // Accessor properties: caller must handle setter invocation
            if desc.is_accessor() {
                return false;
            }
            if !desc.writable {
                return false;
            }
            let offset = desc.offset as usize;
            if offset < self.slots.len() {
                self.slots[offset] = value;
            }
            return true;
        }
        // New property
        if !self.flags.is_extensible() {
            return false;
        }
        let new_shape = shapes.add_property(self.shape_id, atom);
        self.shape_id = new_shape;
        self.slots.push(value);
        true
    }

    /// Set a named property slot with explicit descriptor flags.
    ///
    /// Like [`set_slot_by_name`], but uses `add_property_with_flags` to set
    /// custom `writable`, `enumerable`, and `configurable` flags on the shape
    /// descriptor. This is used for built-in prototype methods which need
    /// `{writable: true, enumerable: false, configurable: true}` per ES spec.
    ///
    /// If the property already exists, updates its value (respecting writable).
    /// If the property does not exist, adds it with the specified flags.
    /// Returns `true` on success, `false` if the object is frozen/sealed/non-extensible.
    //
    // Allow 8 args: mirrors `set_slot_by_name` signature plus three explicit
    // ES descriptor flags. Wrapping them in a struct would add ceremony with
    // no real benefit, since this is an internal method used by prototype
    // population only.
    #[allow(clippy::too_many_arguments)]
    pub fn set_slot_by_name_with_flags(
        &mut self,
        name: &str,
        value: JsValue,
        writable: bool,
        enumerable: bool,
        configurable: bool,
        shapes: &mut shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> bool {
        if self.flags.is_frozen() {
            return false;
        }
        let atom = interner.intern(name);
        if let Some(desc) = shapes.lookup(self.shape_id, atom) {
            // Accessor properties: caller must handle setter invocation
            if desc.is_accessor() {
                return false;
            }
            if !desc.writable {
                return false;
            }
            let offset = desc.offset as usize;
            if offset < self.slots.len() {
                self.slots[offset] = value;
            }
            return true;
        }
        // New property
        if !self.flags.is_extensible() {
            return false;
        }
        let new_shape =
            shapes.add_property_with_flags(self.shape_id, atom, writable, enumerable, configurable);
        self.shape_id = new_shape;
        self.slots.push(value);
        true
    }

    /// Check whether this object has an own property with the given name.
    pub fn has_own_property(
        &self,
        name: &str,
        shapes: &shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> bool {
        let atom = interner.intern(name);
        shapes.lookup(self.shape_id, atom).is_some()
    }

    /// Get the enumerable string property keys in ECMAScript spec order.
    ///
    /// Per the spec, integer indices come first in ascending numeric order,
    /// followed by string keys in insertion order. Symbol and Private keys
    /// are excluded (they are never enumerable by default).
    pub fn enumerable_keys(
        &self,
        shapes: &shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> Vec<String> {
        let Some(shape) = shapes.get(self.shape_id) else {
            return Vec::new();
        };
        let mut keys: Vec<(String, u32)> = shape
            .properties
            .iter()
            .filter(|(key, desc)| desc.enumerable && key.is_string())
            .filter_map(|(key, desc)| {
                key.as_string()
                    .map(|atom| (interner.resolve(*atom).to_string(), desc.offset))
            })
            .collect();
        crate::value_ops::sort_keys_spec_order(&mut keys);
        keys.into_iter().map(|(name, _)| name).collect()
    }

    /// Get all own string property keys in ECMAScript spec order.
    ///
    /// Per the spec, integer indices come first in ascending numeric order,
    /// followed by string keys in insertion order. Symbol and Private keys
    /// are excluded (use separate APIs for those).
    pub fn own_keys(
        &self,
        shapes: &shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> Vec<String> {
        let Some(shape) = shapes.get(self.shape_id) else {
            return Vec::new();
        };
        let mut keys: Vec<(String, u32)> = shape
            .properties
            .iter()
            .filter_map(|(key, desc)| {
                key.as_string()
                    .map(|atom| (interner.resolve(*atom).to_string(), desc.offset))
            })
            .collect();
        crate::value_ops::sort_keys_spec_order(&mut keys);
        keys.into_iter().map(|(name, _)| name).collect()
    }

    /// Set a property value by name with strict-mode error reporting.
    ///
    /// For accessor properties, returns `Err(PropertyError::NoSetter)` if
    /// the accessor has no setter. The caller is responsible for actually
    /// invoking the setter when `NoSetter` is NOT returned (the caller must
    /// check `is_accessor_property` before calling this).
    ///
    /// Returns `Ok(())` on success, or a `PropertyError` describing why the
    /// operation failed (frozen, sealed, not writable, etc.).
    pub fn set_property_strict(
        &mut self,
        name: &str,
        value: JsValue,
        shapes: &mut shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> Result<(), PropertyError> {
        if self.flags.is_frozen() {
            return Err(PropertyError::Frozen);
        }
        let atom = interner.intern(name);
        if let Some(desc) = shapes.lookup(self.shape_id, atom) {
            // Accessor properties: caller must handle setter invocation
            if desc.is_accessor() {
                // Check if setter exists (slot at offset+1)
                let setter = self
                    .slots
                    .get(desc.offset as usize + 1)
                    .copied()
                    .unwrap_or(JsValue::undefined());
                if setter.is_undefined() {
                    return Err(PropertyError::NoSetter);
                }
                // Setter exists; caller will invoke it
                return Ok(());
            }
            if !desc.writable {
                return Err(PropertyError::NotWritable);
            }
            let offset = desc.offset as usize;
            if offset < self.slots.len() {
                self.slots[offset] = value;
            }
            return Ok(());
        }
        // New property
        if !self.flags.is_extensible() {
            return Err(PropertyError::NotExtensible);
        }
        if self.flags.is_sealed() {
            return Err(PropertyError::Sealed);
        }
        let new_shape = shapes.add_property(self.shape_id, atom);
        self.shape_id = new_shape;
        self.slots.push(value);
        Ok(())
    }

    /// Delete a property by name (sloppy-mode semantics).
    ///
    /// Returns `true` if the property existed and was deleted, `false`
    /// if the object is frozen/sealed or the property is not configurable.
    ///
    /// Per ES2024 section 10.1.10 `[[Delete]]`, non-configurable properties
    /// cannot be deleted and the internal method returns `false`. In strict mode,
    /// the caller should throw a TypeError when this returns `false`.
    pub fn delete_slot_by_name(
        &mut self,
        name: &str,
        shapes: &shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> bool {
        if self.flags.is_frozen() || self.flags.is_sealed() {
            return false;
        }
        let atom = interner.intern(name);
        let Some(desc) = shapes.lookup(self.shape_id, atom) else {
            return false;
        };
        if !desc.configurable {
            return false;
        }
        // Set the slot to undefined (tombstone). Shape is not modified
        // because shape transitions don't support removal. The property
        // will still appear in shape lookups, so we clear the value.
        let offset = desc.offset as usize;
        if offset < self.slots.len() {
            self.slots[offset] = JsValue::undefined();
        }
        true
    }

    /// Define a property with explicit descriptor flags.
    ///
    /// Handles both data and accessor descriptors, including conversion between
    /// the two kinds. If the property already exists, updates its value/flags
    /// (subject to configurability). If the property does not exist, adds it
    /// with the specified descriptor.
    pub fn define_own_property(
        &mut self,
        name: &str,
        opts: &DefinePropertyOptions,
        shapes: &mut shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> Result<(), PropertyError> {
        // Mixed accessor+data descriptor is invalid
        if opts.is_invalid_mixed() {
            return Err(PropertyError::MixedDescriptor);
        }

        // Array `length`: virtual property handled via InternalData, not shapes.
        // Per ES spec §10.4.2.1 ArrayDefineOwnProperty, setting `.length` is complex:
        // it may truncate elements, and the writable flag can be tracked.
        // Array `.length` is non-configurable, so reject configurable: true and
        // accessor descriptors. Only value and writable changes are allowed.
        if self.kind == InternalKind::Array && name == "length" {
            // Cannot convert length to accessor
            if opts.is_accessor_descriptor() {
                return Err(PropertyError::NotConfigurable);
            }
            // Cannot make length configurable
            if opts.configurable == Some(true) {
                return Err(PropertyError::NotConfigurable);
            }
            // Cannot make length enumerable (it's non-enumerable)
            if opts.enumerable == Some(true) {
                return Err(PropertyError::NotConfigurable);
            }

            // Read current writable state
            let is_writable = match self.internal.as_deref() {
                Some(InternalData::Array {
                    length_writable, ..
                }) => *length_writable,
                _ => true,
            };

            // Per §10.4.2.1 step 3.a.i: if [[Writable]] is false, reject changing
            // writable from false to true.
            if let Some(new_writable) = opts.writable
                && !is_writable
                && new_writable
            {
                return Err(PropertyError::NotConfigurable);
            }

            // Per §10.4.2.1 step 3.b: if length is not writable and a new value
            // is provided, it's a TypeError.
            if let Some(val) = opts.value {
                if !is_writable {
                    // If same value, silently succeed; otherwise reject.
                    let cur_len = self.as_array_length().unwrap_or(0);
                    let new_len_f64 = crate::value_ops::to_number(val);
                    let new_len_u32 = new_len_f64 as u32;
                    if new_len_f64 != new_len_u32 as f64
                        || new_len_f64.is_nan()
                        || new_len_f64 < 0.0
                        || new_len_f64.is_infinite()
                    {
                        return Err(PropertyError::InvalidArrayLength);
                    }
                    if new_len_u32 != cur_len {
                        return Err(PropertyError::NotWritable);
                    }
                } else {
                    let new_len_f64 = crate::value_ops::to_number(val);
                    let new_len_u32 = new_len_f64 as u32;
                    // RangeError if not a valid array length
                    if new_len_f64 != new_len_u32 as f64
                        || new_len_f64.is_nan()
                        || new_len_f64 < 0.0
                        || new_len_f64.is_infinite()
                    {
                        return Err(PropertyError::InvalidArrayLength);
                    }
                    let cur_len = self.as_array_length().unwrap_or(0);
                    if new_len_u32 < cur_len {
                        // Shrinking: per §10.4.2.4 ArraySetLength steps 8-17.
                        // Check shape-based integer-indexed properties >= new_len.
                        // Collect (index, configurable) pairs from the shape.
                        let shape_idx_props: Vec<(u32, bool)> = shapes
                            .get(self.shape_id)
                            .map(|shape| {
                                shape
                                    .properties
                                    .iter()
                                    .filter_map(|(key, desc)| {
                                        let atom = key.as_string()?;
                                        let prop_name = interner.resolve(*atom);
                                        let idx = prop_name.parse::<u32>().ok()?;
                                        if idx >= new_len_u32 {
                                            Some((idx, desc.configurable))
                                        } else {
                                            None
                                        }
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();

                        // Find smallest non-configurable index >= new_len
                        let block_idx = shape_idx_props
                            .iter()
                            .filter(|(_, cfg)| !cfg)
                            .map(|(idx, _)| *idx)
                            .min();

                        if let Some(block_idx) = block_idx {
                            // Can't shrink past block_idx; set length to block_idx+1
                            let actual_new_len = block_idx + 1;
                            // Tombstone configurable indices in [new_len_u32, block_idx)
                            for (idx, cfg) in &shape_idx_props {
                                if *idx >= new_len_u32 && *idx < block_idx && *cfg {
                                    self.delete_slot_by_name(&idx.to_string(), shapes, interner);
                                }
                            }
                            self.array_set_length(actual_new_len);
                            // Apply writable flag change before reporting error
                            if let Some(false) = opts.writable
                                && let Some(InternalData::Array {
                                    length_writable, ..
                                }) = self.internal.as_deref_mut()
                            {
                                *length_writable = false;
                            }
                            return Err(PropertyError::NotConfigurable);
                        }

                        // All shape-based >= new_len are configurable: tombstone them
                        for (idx, _) in &shape_idx_props {
                            self.delete_slot_by_name(&idx.to_string(), shapes, interner);
                        }
                    }
                    self.array_set_length(new_len_u32);
                }
            }

            // Track the writable flag change (writable: false → false is valid; true → false is valid)
            if let Some(false) = opts.writable
                && let Some(InternalData::Array {
                    length_writable, ..
                }) = self.internal.as_deref_mut()
            {
                *length_writable = false;
            }

            return Ok(());
        }

        let atom = interner.intern(name);

        // For Array integer indices with no shape entry (property lives in dense storage),
        // the "current" descriptor is implicitly { writable: true, enumerable: true,
        // configurable: true, value: <element> }.  Per ES spec §10.4.2.1 step 4.c,
        // defineProperty must validate the change against these implicit flags and, if
        // valid, promote the element to a shape-based entry.
        //
        // Before delegating to the general shape-based logic below, we "materialise"
        // the implicit descriptor into the shape so that:
        //   – the existing-property path picks it up with the correct WEC flags, and
        //   – the dense element is cleared so the two representations don't conflict.
        if self.kind == InternalKind::Array
            && let Ok(idx) = name.parse::<u32>()
            && shapes.lookup(self.shape_id, atom).is_none()
            && let Some(dense_val) = self.get_element(idx)
        {
            // Materialise the dense element as a shape data property with the
            // implicit array-index flags: writable=true, enumerable=true,
            // configurable=true.
            let new_shape = shapes.add_property_with_flags(self.shape_id, atom, true, true, true);
            self.shape_id = new_shape;
            self.slots.push(dense_val);
            // Remove the dense element to prevent dual-representation confusion.
            self.delete_element(idx);
        }

        if let Some(existing) = shapes.lookup(self.shape_id, atom) {
            // Per ES spec §10.1.6.3 step 2: if Desc has no fields, ValidateAndApply
            // returns true without changing anything (the existing property is fine as-is).
            if opts.is_empty() {
                return Ok(());
            }

            let is_accessor_desc = opts.is_accessor_descriptor();
            let existing_is_accessor = existing.is_accessor();

            // Non-configurable property restrictions
            if !existing.configurable {
                let new_configurable = opts.configurable.unwrap_or(existing.configurable);
                let new_enumerable = opts.enumerable.unwrap_or(existing.enumerable);

                // Cannot make non-configurable property configurable
                if new_configurable {
                    return Err(PropertyError::NotConfigurable);
                }
                // Cannot change enumerable on non-configurable property
                if new_enumerable != existing.enumerable {
                    return Err(PropertyError::NotConfigurable);
                }
                // Cannot change kind on non-configurable property
                if is_accessor_desc && !existing_is_accessor {
                    return Err(PropertyError::NotConfigurable);
                }
                if !is_accessor_desc && existing_is_accessor && opts.is_data_descriptor() {
                    return Err(PropertyError::NotConfigurable);
                }
                // For non-configurable data: cannot make non-writable -> writable
                if !existing_is_accessor {
                    let new_writable = opts.writable.unwrap_or(existing.writable);
                    if !existing.writable && new_writable {
                        return Err(PropertyError::NotConfigurable);
                    }
                    if !existing.writable {
                        // Per ES spec sec-validateandapplypropertydescriptor step 8.a.iv:
                        // if a new value is specified and it differs from the current
                        // value (using SameValue), reject. If no new value or same
                        // value, silently succeed.
                        if let Some(new_val) = opts.value {
                            let old_val = self
                                .slots
                                .get(existing.offset as usize)
                                .copied()
                                .unwrap_or(JsValue::undefined());
                            if !crate::value_ops::same_value(new_val, old_val) {
                                return Err(PropertyError::NotWritable);
                            }
                        }
                        return Ok(());
                    }
                }
                // For non-configurable accessor: cannot change getter or setter
                // Per ES spec sec-validateandapplypropertydescriptor step 10.a:
                // If Desc.[[Get]] is present and SameValue(Desc.[[Get]], current.[[Get]])
                // is false, return false. Same for [[Set]].
                if existing_is_accessor {
                    let offset = existing.offset as usize;
                    if let Some(new_getter) = opts.getter {
                        let old_getter = self
                            .slots
                            .get(offset)
                            .copied()
                            .unwrap_or(JsValue::undefined());
                        if !crate::value_ops::same_value(new_getter, old_getter) {
                            return Err(PropertyError::NotConfigurable);
                        }
                    }
                    if let Some(new_setter) = opts.setter {
                        let old_setter = self
                            .slots
                            .get(offset + 1)
                            .copied()
                            .unwrap_or(JsValue::undefined());
                        if !crate::value_ops::same_value(new_setter, old_setter) {
                            return Err(PropertyError::NotConfigurable);
                        }
                    }
                    return Ok(());
                }
            }

            // Kind conversion: data -> accessor
            if is_accessor_desc && !existing_is_accessor {
                let e = opts.enumerable.unwrap_or(existing.enumerable);
                let c = opts.configurable.unwrap_or(existing.configurable);
                let getter = opts.getter.unwrap_or(JsValue::undefined());
                let setter = opts.setter.unwrap_or(JsValue::undefined());

                // Transition shape to accessor
                if let Some(new_shape) = shapes.update_property_kind(
                    self.shape_id,
                    atom,
                    shapes::PropertyKind::Accessor,
                    Some(e),
                    Some(c),
                ) {
                    // Get the new offset from the updated shape
                    let new_desc = shapes.lookup(new_shape, atom);
                    if let Some(nd) = new_desc {
                        let offset = nd.offset as usize;
                        // Ensure slots are large enough
                        if offset + 1 >= self.slots.len() {
                            self.slots.resize(offset + 2, JsValue::undefined());
                        }
                        self.slots[offset] = getter;
                        self.slots[offset + 1] = setter;
                    }
                    self.shape_id = new_shape;
                }
                return Ok(());
            }

            // Kind conversion: accessor -> data
            if !is_accessor_desc && existing_is_accessor && opts.is_data_descriptor() {
                let e = opts.enumerable.unwrap_or(existing.enumerable);
                let c = opts.configurable.unwrap_or(existing.configurable);
                let w = opts.writable.unwrap_or(false);
                let val = opts.value.unwrap_or(JsValue::undefined());

                if let Some(new_shape) = shapes.update_property_kind(
                    self.shape_id,
                    atom,
                    shapes::PropertyKind::Data,
                    Some(e),
                    Some(c),
                ) {
                    let new_desc = shapes.lookup(new_shape, atom);
                    if let Some(nd) = new_desc {
                        let offset = nd.offset as usize;
                        if offset >= self.slots.len() {
                            self.slots.resize(offset + 1, JsValue::undefined());
                        }
                        self.slots[offset] = val;
                    }
                    // Also update writable
                    if let Some(ws) =
                        shapes.update_property_flags(new_shape, atom, Some(w), None, None)
                    {
                        self.shape_id = ws;
                    } else {
                        self.shape_id = new_shape;
                    }
                }
                return Ok(());
            }

            // Same-kind update: accessor -> accessor
            if is_accessor_desc && existing_is_accessor {
                let e = opts.enumerable.unwrap_or(existing.enumerable);
                let c = opts.configurable.unwrap_or(existing.configurable);
                let offset = existing.offset as usize;

                // Update getter/setter slots
                if let Some(getter) = opts.getter
                    && offset < self.slots.len()
                {
                    self.slots[offset] = getter;
                }
                if let Some(setter) = opts.setter
                    && offset + 1 < self.slots.len()
                {
                    self.slots[offset + 1] = setter;
                }

                // Update flags
                if let Some(new_shape) =
                    shapes.update_property_flags(self.shape_id, atom, None, Some(e), Some(c))
                {
                    self.shape_id = new_shape;
                }
                return Ok(());
            }

            // Same-kind update: data -> data
            let offset = existing.offset as usize;
            let w = opts.writable.unwrap_or(existing.writable);
            let e = opts.enumerable.unwrap_or(existing.enumerable);
            let c = opts.configurable.unwrap_or(existing.configurable);

            if let Some(val) = opts.value
                && offset < self.slots.len()
            {
                self.slots[offset] = val;
            }

            if let Some(new_shape) =
                shapes.update_property_flags(self.shape_id, atom, Some(w), Some(e), Some(c))
            {
                self.shape_id = new_shape;
            }
            return Ok(());
        }

        // New property
        if !self.flags.is_extensible() {
            return Err(PropertyError::NotExtensible);
        }
        if self.flags.is_sealed() {
            return Err(PropertyError::Sealed);
        }

        // Per ES spec §10.4.2.1 step 4.b: if the length property is not writable,
        // reject defining a new array index property whose index >= length.
        // (Defining such a property would implicitly extend the array.)
        if self.kind == InternalKind::Array
            && let Ok(new_idx) = name.parse::<u32>()
            && new_idx < u32::MAX
        // not a valid array index if == u32::MAX
        {
            let len_writable = match self.internal.as_deref() {
                Some(InternalData::Array {
                    length_writable, ..
                }) => *length_writable,
                _ => true,
            };
            if !len_writable {
                let cur_len = self.as_array_length().unwrap_or(0);
                if new_idx >= cur_len {
                    return Err(PropertyError::NotWritable);
                }
            }
        }

        if opts.is_accessor_descriptor() {
            // New accessor property
            let e = opts.enumerable.unwrap_or(false);
            let c = opts.configurable.unwrap_or(false);
            let getter = opts.getter.unwrap_or(JsValue::undefined());
            let setter = opts.setter.unwrap_or(JsValue::undefined());

            let new_shape = shapes.add_property_as_accessor(self.shape_id, atom, e, c);
            self.shape_id = new_shape;
            self.slots.push(getter);
            self.slots.push(setter);
        } else {
            // New data property
            let w = opts.writable.unwrap_or(false);
            let e = opts.enumerable.unwrap_or(false);
            let c = opts.configurable.unwrap_or(false);
            let val = opts.value.unwrap_or(JsValue::undefined());

            let new_shape = shapes.add_property_with_flags(self.shape_id, atom, w, e, c);
            self.shape_id = new_shape;
            self.slots.push(val);
        }

        // Per ES spec §10.4.2.1 step 4.e.ii: when defining a new property with a
        // valid array index >= length, update the array length to index + 1.
        // A valid array index is in [0, 2^32-2] (i.e. < u32::MAX per ES spec §7.1.7.1).
        if self.kind == InternalKind::Array
            && let Ok(idx) = name.parse::<u32>()
            && idx < u32::MAX  // 4294967295 is not a valid array index
            && let Some(old_len) = self.as_array_length()
            && idx >= old_len
        {
            self.array_set_length(idx + 1);
        }

        Ok(())
    }

    /// Get the property descriptor for an own property.
    ///
    /// Returns `None` if the property does not exist on this object.
    /// Returns a data or accessor descriptor depending on the property kind.
    pub fn get_property_descriptor(
        &self,
        name: &str,
        shapes: &shapes::ShapeTable,
        interner: &interner::Interner,
    ) -> Option<OwnPropertyDescriptor> {
        // Array `length`: virtual property stored in InternalData, not in shape.
        // Per ES spec, array `.length` is { writable: <tracked>, enumerable: false, configurable: false }.
        if self.kind == InternalKind::Array
            && name == "length"
            && let Some(len) = self.as_array_length()
        {
            let len_writable = match self.internal.as_deref() {
                Some(InternalData::Array {
                    length_writable, ..
                }) => *length_writable,
                _ => true,
            };
            return Some(OwnPropertyDescriptor::Data {
                value: JsValue::number(len as f64),
                writable: len_writable,
                enumerable: false,
                configurable: false,
            });
        }

        // For Array integer indices, check the shape table FIRST.
        // Object.defineProperty(arr, "0", {...}) stores a shape entry that overrides
        // the dense element storage. Only fall back to dense elements if no shape entry
        // exists for this index. Per ES spec §15.4.5.1: once defineProperty has been
        // called on an array index, the property descriptor is authoritative.
        let atom = interner.intern(name);
        let shape_desc = shapes.lookup(self.shape_id, atom);

        // If no shape entry AND this is an Array integer index, fall back to dense elements.
        if shape_desc.is_none()
            && self.kind == InternalKind::Array
            && let Ok(idx) = name.parse::<u32>()
            && let Some(elem) = self.get_element(idx)
        {
            return Some(OwnPropertyDescriptor::Data {
                value: elem,
                writable: true,
                enumerable: true,
                configurable: true,
            });
        }

        // Use the shape descriptor (or return None if no property exists here).
        let desc = shape_desc?;

        if desc.is_accessor() {
            let getter = self
                .slots
                .get(desc.offset as usize)
                .copied()
                .unwrap_or(JsValue::undefined());
            let setter = self
                .slots
                .get(desc.offset as usize + 1)
                .copied()
                .unwrap_or(JsValue::undefined());
            Some(OwnPropertyDescriptor::Accessor {
                getter,
                setter,
                enumerable: desc.enumerable,
                configurable: desc.configurable,
            })
        } else {
            let value = self
                .slots
                .get(desc.offset as usize)
                .copied()
                .unwrap_or(JsValue::undefined());
            Some(OwnPropertyDescriptor::Data {
                value,
                writable: desc.writable,
                enumerable: desc.enumerable,
                configurable: desc.configurable,
            })
        }
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // InternalKind tests
    // -----------------------------------------------------------------

    #[test]
    fn test_internal_kind_repr_values() {
        assert_eq!(InternalKind::Ordinary as u8, 0);
        assert_eq!(InternalKind::Array as u8, 1);
        assert_eq!(InternalKind::Function as u8, 2);
        assert_eq!(InternalKind::Closure as u8, 3);
        assert_eq!(InternalKind::ErrorObj as u8, 4);
        assert_eq!(InternalKind::Proxy as u8, 5);
        assert_eq!(InternalKind::Promise as u8, 6);
        assert_eq!(InternalKind::Iterator as u8, 7);
        assert_eq!(InternalKind::IterResult as u8, 8);
        assert_eq!(InternalKind::Generator as u8, 9);
        assert_eq!(InternalKind::MapObj as u8, 10);
        assert_eq!(InternalKind::SetObj as u8, 11);
        assert_eq!(InternalKind::RegExpObj as u8, 12);
        assert_eq!(InternalKind::DateObj as u8, 13);
        assert_eq!(InternalKind::WeakMapObj as u8, 14);
        assert_eq!(InternalKind::WeakSetObj as u8, 15);
        assert_eq!(InternalKind::WeakRefObj as u8, 16);
        assert_eq!(InternalKind::SymbolObj as u8, 17);
        assert_eq!(InternalKind::NativeFunc as u8, 18);
    }

    #[test]
    fn test_internal_kind_equality() {
        assert_eq!(InternalKind::Ordinary, InternalKind::Ordinary);
        assert_ne!(InternalKind::Ordinary, InternalKind::Array);
        assert_ne!(InternalKind::Function, InternalKind::Closure);
    }

    #[test]
    fn test_internal_kind_copy() {
        let kind = InternalKind::Function;
        let kind2 = kind;
        assert_eq!(kind, kind2);
    }

    // -----------------------------------------------------------------
    // ElementKind tests
    // -----------------------------------------------------------------

    #[test]
    fn test_element_kind_repr_values() {
        assert_eq!(ElementKind::None as u8, 0);
        assert_eq!(ElementKind::Dense as u8, 1);
        assert_eq!(ElementKind::Holey as u8, 2);
    }

    // -----------------------------------------------------------------
    // ObjFlags tests
    // -----------------------------------------------------------------

    #[test]
    fn test_flags_default() {
        let flags = ObjFlags::new();
        assert!(!flags.is_frozen());
        assert!(!flags.is_sealed());
        assert!(flags.is_extensible());
        assert!(!flags.is_callable());
        assert!(!flags.is_constructable());
        assert_eq!(flags.raw(), 0);
    }

    #[test]
    fn test_flags_frozen() {
        let mut flags = ObjFlags::new();
        flags.set_frozen();
        assert!(flags.is_frozen());
        assert!(flags.is_sealed()); // frozen implies sealed
        assert!(!flags.is_extensible()); // frozen implies non-extensible
    }

    #[test]
    fn test_flags_sealed() {
        let mut flags = ObjFlags::new();
        flags.set_sealed();
        assert!(!flags.is_frozen());
        assert!(flags.is_sealed());
        assert!(!flags.is_extensible()); // sealed implies non-extensible
    }

    #[test]
    fn test_flags_prevent_extensions_only() {
        let mut flags = ObjFlags::new();
        flags.prevent_extensions();
        assert!(!flags.is_frozen());
        assert!(!flags.is_sealed());
        assert!(!flags.is_extensible());
    }

    #[test]
    fn test_flags_callable() {
        let mut flags = ObjFlags::new();
        assert!(!flags.is_callable());
        flags.set_callable();
        assert!(flags.is_callable());
    }

    #[test]
    fn test_flags_constructable() {
        let mut flags = ObjFlags::new();
        assert!(!flags.is_constructable());
        flags.set_constructable();
        assert!(flags.is_constructable());
    }

    #[test]
    fn test_flags_default_impl() {
        let flags = ObjFlags::default();
        assert_eq!(flags.raw(), 0);
    }

    // -----------------------------------------------------------------
    // UnifiedObject: ordinary
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_ordinary() {
        let obj = UnifiedObject::ordinary(ShapeId(0));
        assert_eq!(obj.kind, InternalKind::Ordinary);
        assert_eq!(obj.element_kind, ElementKind::None);
        assert_eq!(obj.shape_id, ShapeId(0));
        assert!(obj.slots.is_empty());
        assert!(obj.internal.is_none());
        assert!(!obj.is_callable());
        assert!(!obj.is_frozen());
        assert!(!obj.is_sealed());
        assert!(obj.is_extensible());
    }

    #[test]
    fn test_unified_object_ordinary_with_shape() {
        let obj = UnifiedObject::ordinary(ShapeId(42));
        assert_eq!(obj.shape_id, ShapeId(42));
    }

    // -----------------------------------------------------------------
    // UnifiedObject: array
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_array_empty() {
        let obj = UnifiedObject::array(ShapeId(1), Vec::new());
        assert_eq!(obj.kind, InternalKind::Array);
        assert_eq!(obj.element_kind, ElementKind::Dense);
        assert_eq!(obj.as_array_length(), Some(0));
        assert_eq!(obj.elements_len(), 0);
        assert!(!obj.is_callable());
    }

    #[test]
    fn test_unified_object_array_with_elements() {
        let elems = vec![JsValue::int(1), JsValue::int(2), JsValue::int(3)];
        let obj = UnifiedObject::array(ShapeId(1), elems);
        assert_eq!(obj.as_array_length(), Some(3));
        assert_eq!(obj.elements_len(), 3);
        assert_eq!(obj.get_element(0).map(|v| v.as_int()), Some(Some(1)));
        assert_eq!(obj.get_element(1).map(|v| v.as_int()), Some(Some(2)));
        assert_eq!(obj.get_element(2).map(|v| v.as_int()), Some(Some(3)));
        assert_eq!(obj.get_element(3), None);
    }

    // -----------------------------------------------------------------
    // UnifiedObject: function
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_function() {
        let obj = UnifiedObject::function(ShapeId(2), 10, 0, 0, 3, false);
        assert_eq!(obj.kind, InternalKind::Function);
        assert!(obj.is_callable());
        assert!(obj.as_function_data().is_some());

        if let Some(InternalData::Function {
            code_idx,
            param_count,
            is_arrow,
            is_generator,
            ..
        }) = obj.internal_data()
        {
            assert_eq!(*code_idx, 10);
            assert_eq!(*param_count, 3);
            assert!(!is_arrow);
            assert!(!is_generator);
        } else {
            panic!("expected Function internal data");
        }
    }

    #[test]
    fn test_unified_object_function_arrow() {
        let obj = UnifiedObject::function(ShapeId(2), 5, 0, 0, 0, true);
        if let Some(InternalData::Function { is_arrow, .. }) = obj.internal_data() {
            assert!(is_arrow);
        } else {
            panic!("expected Function internal data");
        }
    }

    // -----------------------------------------------------------------
    // UnifiedObject: closure
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_closure() {
        let obj = UnifiedObject::closure(ShapeId(2), 7, 0xDEAD, 0);
        assert_eq!(obj.kind, InternalKind::Closure);
        assert!(obj.is_callable());
        if let Some(InternalData::Function { code_idx, env, .. }) = obj.internal_data() {
            assert_eq!(*code_idx, 7);
            assert_eq!(*env, 0xDEAD);
        } else {
            panic!("expected Function internal data for closure");
        }
    }

    // -----------------------------------------------------------------
    // UnifiedObject: error
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_error() {
        let obj = UnifiedObject::error(ShapeId(3), 1, 100, 200, 300);
        assert_eq!(obj.kind, InternalKind::ErrorObj);
        assert!(!obj.is_callable());
        assert!(obj.as_error_data().is_some());

        if let Some(InternalData::Error {
            error_tag,
            message,
            raw_message,
            stack,
        }) = obj.internal_data()
        {
            assert_eq!(*error_tag, 1);
            assert_eq!(*message, 100);
            assert_eq!(*raw_message, 200);
            assert_eq!(*stack, 300);
        } else {
            panic!("expected Error internal data");
        }
    }

    // -----------------------------------------------------------------
    // UnifiedObject: proxy
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_proxy() {
        let obj = UnifiedObject::proxy(0xAAAA, 0xBBBB);
        assert_eq!(obj.kind, InternalKind::Proxy);
        assert!(obj.as_proxy_data().is_some());

        if let Some(InternalData::Proxy {
            target,
            handler,
            revoked,
        }) = obj.internal_data()
        {
            assert_eq!(*target, 0xAAAA);
            assert_eq!(*handler, 0xBBBB);
            assert!(!revoked);
        } else {
            panic!("expected Proxy internal data");
        }
    }

    // -----------------------------------------------------------------
    // UnifiedObject: promise
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_promise() {
        let obj = UnifiedObject::promise(ShapeId(0));
        assert_eq!(obj.kind, InternalKind::Promise);
        if let Some(InternalData::Promise { inner }) = obj.internal_data() {
            assert_eq!(inner.state, crate::promise::PromiseState::Pending);
        } else {
            panic!("expected Promise internal data");
        }
    }

    // -----------------------------------------------------------------
    // UnifiedObject: iterator
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_iterator() {
        use crate::iterator::{IteratorKind, JsIterator};
        let iter = JsIterator::new_array(0xCCCC);
        let obj = UnifiedObject::iterator(ShapeId(0), iter);
        assert_eq!(obj.kind, InternalKind::Iterator);
        if let Some(InternalData::IteratorState { inner }) = obj.internal_data() {
            assert_eq!(inner.kind, IteratorKind::Array);
            assert_eq!(inner.target, 0xCCCC);
            assert_eq!(inner.index, 0);
            assert!(!inner.done);
        } else {
            panic!("expected IteratorState internal data");
        }
    }

    // -----------------------------------------------------------------
    // UnifiedObject: iter_result
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_iter_result() {
        let val = JsValue::int(42).raw_bits();
        let done = JsValue::bool(false).raw_bits();
        let obj = UnifiedObject::iter_result(val, done);
        assert_eq!(obj.kind, InternalKind::IterResult);
        if let Some(InternalData::IterResult { value, done: d }) = obj.internal_data() {
            assert_eq!(*value, val);
            assert_eq!(*d, done);
        } else {
            panic!("expected IterResult internal data");
        }
    }

    // -----------------------------------------------------------------
    // UnifiedObject: generator
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_generator() {
        let state_bits = 0xDEAD_BEEF_u64;
        let resume_idx = 7_u32;
        let obj = UnifiedObject::generator(ShapeId(0), state_bits, resume_idx);
        assert_eq!(obj.kind, InternalKind::Generator);
        if let Some(InternalData::Generator {
            state_obj,
            resume_func_idx,
        }) = obj.internal_data()
        {
            assert_eq!(*state_obj, state_bits);
            assert_eq!(*resume_func_idx, resume_idx);
        } else {
            panic!("expected Generator internal data");
        }
    }

    // -----------------------------------------------------------------
    // UnifiedObject: map
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_map() {
        let obj = UnifiedObject::map(ShapeId(0));
        assert_eq!(obj.kind, InternalKind::MapObj);
        if let Some(InternalData::Map { entries }) = obj.internal_data() {
            assert!(entries.is_empty());
        } else {
            panic!("expected Map internal data");
        }
    }

    // -----------------------------------------------------------------
    // UnifiedObject: set
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_set() {
        let obj = UnifiedObject::set(ShapeId(0));
        assert_eq!(obj.kind, InternalKind::SetObj);
        if let Some(InternalData::Set { values }) = obj.internal_data() {
            assert!(values.is_empty());
        } else {
            panic!("expected Set internal data");
        }
    }

    // -----------------------------------------------------------------
    // UnifiedObject: native_func
    // -----------------------------------------------------------------

    fn dummy_native(x: u64) -> u64 {
        x + 1
    }

    #[test]
    fn test_unified_object_native_func() {
        let obj = UnifiedObject::native_func(dummy_native, 0xFF);
        assert_eq!(obj.kind, InternalKind::NativeFunc);
        assert!(obj.is_callable());
        if let Some(InternalData::NativeFunc { func, context }) = obj.internal_data() {
            assert_eq!(func(10), 11);
            assert_eq!(*context, 0xFF);
        } else {
            panic!("expected NativeFunc internal data");
        }
    }

    // -----------------------------------------------------------------
    // Slot access
    // -----------------------------------------------------------------

    #[test]
    fn test_slot_get_set() {
        let mut obj = UnifiedObject::ordinary(ShapeId(0));
        obj.set_slot(0, JsValue::int(100));
        obj.set_slot(1, JsValue::int(200));
        assert_eq!(obj.get_slot(0).as_int(), Some(100));
        assert_eq!(obj.get_slot(1).as_int(), Some(200));
        assert_eq!(obj.slot_count(), 2);
    }

    #[test]
    fn test_slot_out_of_bounds_returns_undefined() {
        let obj = UnifiedObject::ordinary(ShapeId(0));
        let val = obj.get_slot(999);
        assert!(val.is_undefined());
    }

    #[test]
    fn test_slot_set_sparse_fills_with_undefined() {
        let mut obj = UnifiedObject::ordinary(ShapeId(0));
        obj.set_slot(5, JsValue::int(42));
        assert_eq!(obj.slot_count(), 6);
        // Slots 0-4 should be undefined
        assert!(obj.get_slot(0).is_undefined());
        assert!(obj.get_slot(4).is_undefined());
        assert_eq!(obj.get_slot(5).as_int(), Some(42));
    }

    // -----------------------------------------------------------------
    // Element access
    // -----------------------------------------------------------------

    #[test]
    fn test_element_get_set_dense() {
        let elems = vec![JsValue::int(10), JsValue::int(20)];
        let mut obj = UnifiedObject::array(ShapeId(1), elems);
        assert_eq!(obj.get_element(0).map(|v| v.as_int()), Some(Some(10)));
        assert_eq!(obj.get_element(1).map(|v| v.as_int()), Some(Some(20)));

        obj.set_element(1, JsValue::int(99));
        assert_eq!(obj.get_element(1).map(|v| v.as_int()), Some(Some(99)));
    }

    #[test]
    fn test_element_get_none_storage() {
        let obj = UnifiedObject::ordinary(ShapeId(0));
        assert_eq!(obj.get_element(0), None);
        assert_eq!(obj.elements_len(), 0);
    }

    #[test]
    fn test_element_set_on_none_promotes_to_dense() {
        let mut obj = UnifiedObject::ordinary(ShapeId(0));
        obj.set_element(0, JsValue::int(42));
        assert_eq!(obj.element_kind, ElementKind::Dense);
        assert_eq!(obj.get_element(0).map(|v| v.as_int()), Some(Some(42)));
        assert_eq!(obj.elements_len(), 1);
    }

    #[test]
    fn test_element_set_sparse_extends_dense() {
        let mut obj = UnifiedObject::array(ShapeId(1), vec![JsValue::int(1)]);
        obj.set_element(5, JsValue::int(99));
        assert_eq!(obj.elements_len(), 6);
        assert_eq!(obj.get_element(5).map(|v| v.as_int()), Some(Some(99)));
        // Intermediate elements should be undefined
        assert!(
            obj.get_element(3)
                .map(|v| v.is_undefined())
                .unwrap_or(false)
        );
    }

    #[test]
    fn test_element_holey_storage() {
        let mut obj = UnifiedObject::ordinary(ShapeId(0));
        obj.elements =
            ElementsStorage::Holey(vec![Some(JsValue::int(1)), None, Some(JsValue::int(3))]);
        obj.element_kind = ElementKind::Holey;

        assert_eq!(obj.get_element(0).map(|v| v.as_int()), Some(Some(1)));
        assert_eq!(obj.get_element(1), None); // hole
        assert_eq!(obj.get_element(2).map(|v| v.as_int()), Some(Some(3)));
        assert_eq!(obj.elements_len(), 3);
    }

    // -----------------------------------------------------------------
    // Element kind matches storage
    // -----------------------------------------------------------------

    #[test]
    fn test_element_kind_matches_storage() {
        let obj_none = UnifiedObject::ordinary(ShapeId(0));
        assert_eq!(obj_none.element_kind, ElementKind::None);

        let obj_dense = UnifiedObject::array(ShapeId(1), vec![JsValue::int(1)]);
        assert_eq!(obj_dense.element_kind, ElementKind::Dense);
    }

    // -----------------------------------------------------------------
    // Internal data none for ordinary
    // -----------------------------------------------------------------

    #[test]
    fn test_internal_data_none_for_ordinary() {
        let obj = UnifiedObject::ordinary(ShapeId(0));
        assert!(obj.internal_data().is_none());
        assert!(obj.as_array_length().is_none());
        assert!(obj.as_function_data().is_none());
        assert!(obj.as_error_data().is_none());
        assert!(obj.as_proxy_data().is_none());
    }

    // -----------------------------------------------------------------
    // Internal data array length
    // -----------------------------------------------------------------

    #[test]
    fn test_internal_data_array_length_tracking() {
        let elems = vec![JsValue::int(1), JsValue::int(2)];
        let obj = UnifiedObject::array(ShapeId(1), elems);
        assert_eq!(obj.as_array_length(), Some(2));
    }

    // -----------------------------------------------------------------
    // Flags on unified objects
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_freeze() {
        let mut obj = UnifiedObject::ordinary(ShapeId(0));
        assert!(!obj.is_frozen());
        assert!(obj.is_extensible());
        obj.freeze();
        assert!(obj.is_frozen());
        assert!(obj.is_sealed());
        assert!(!obj.is_extensible());
    }

    #[test]
    fn test_unified_object_seal() {
        let mut obj = UnifiedObject::ordinary(ShapeId(0));
        obj.seal();
        assert!(!obj.is_frozen());
        assert!(obj.is_sealed());
        assert!(!obj.is_extensible());
    }

    #[test]
    fn test_unified_object_prevent_extensions() {
        let mut obj = UnifiedObject::ordinary(ShapeId(0));
        obj.prevent_extensions();
        assert!(!obj.is_frozen());
        assert!(!obj.is_sealed());
        assert!(!obj.is_extensible());
    }

    #[test]
    fn test_unified_object_callable_flags() {
        let ordinary = UnifiedObject::ordinary(ShapeId(0));
        assert!(!ordinary.is_callable());

        let func = UnifiedObject::function(ShapeId(0), 0, 0, 0, 0, false);
        assert!(func.is_callable());

        let closure = UnifiedObject::closure(ShapeId(0), 0, 0, 0);
        assert!(closure.is_callable());

        let native = UnifiedObject::native_func(dummy_native, 0);
        assert!(native.is_callable());
    }

    // -----------------------------------------------------------------
    // Size check (informational, not a hard requirement yet)
    // -----------------------------------------------------------------

    #[test]
    fn test_unified_object_size_reasonable() {
        let size = std::mem::size_of::<UnifiedObject>();
        // We expect roughly 64-80 bytes. The exact size depends on alignment
        // and platform. We just verify it's not absurdly large.
        assert!(
            size <= 128,
            "UnifiedObject is {size} bytes, expected <= 128"
        );
    }

    #[test]
    fn test_unified_object_debug_format() {
        let obj = UnifiedObject::ordinary(ShapeId(0));
        let debug = format!("{:?}", obj);
        assert!(debug.contains("UnifiedObject"));
        assert!(debug.contains("Ordinary"));
    }

    // -----------------------------------------------------------------
    // Mutable internal data access
    // -----------------------------------------------------------------

    #[test]
    fn test_internal_data_mut_access() {
        let mut obj = UnifiedObject::array(ShapeId(1), vec![JsValue::int(1)]);
        if let Some(InternalData::Array { length, .. }) = obj.internal_data_mut() {
            *length = 10;
        }
        assert_eq!(obj.as_array_length(), Some(10));
    }

    // -----------------------------------------------------------------
    // Elements storage push (via set_element on dense)
    // -----------------------------------------------------------------

    #[test]
    fn test_elements_push_via_set_element() {
        let mut obj = UnifiedObject::array(ShapeId(1), Vec::new());
        obj.set_element(0, JsValue::int(10));
        obj.set_element(1, JsValue::int(20));
        obj.set_element(2, JsValue::int(30));
        assert_eq!(obj.elements_len(), 3);
        assert_eq!(obj.get_element(0).map(|v| v.as_int()), Some(Some(10)));
        assert_eq!(obj.get_element(2).map(|v| v.as_int()), Some(Some(30)));
    }

    // -----------------------------------------------------------------
    // Holey set_element extends correctly
    // -----------------------------------------------------------------

    #[test]
    fn test_holey_set_element_extends() {
        let mut obj = UnifiedObject::ordinary(ShapeId(0));
        obj.elements = ElementsStorage::Holey(vec![Some(JsValue::int(1))]);
        obj.element_kind = ElementKind::Holey;

        obj.set_element(3, JsValue::int(99));
        assert_eq!(obj.elements_len(), 4);
        // Index 1 and 2 should be holes (None)
        assert_eq!(obj.get_element(1), None);
        assert_eq!(obj.get_element(2), None);
        assert_eq!(obj.get_element(3).map(|v| v.as_int()), Some(Some(99)));
    }

    // -----------------------------------------------------------------
    // Array exotic behavior: auto-length on index set
    // -----------------------------------------------------------------

    #[test]
    fn test_auto_length_on_index_set() {
        let mut arr = UnifiedObject::array(ShapeId(0), Vec::new());
        assert_eq!(arr.array_len(), 0);

        arr.set_element(5, JsValue::int(1));
        // After set_element(5, ...) length should still be 0 in InternalData
        // because set_element doesn't update array length — the caller does.
        // But auto-length is tested via the combined set_element + length update
        // in array_set_length tests below.
        // For direct array construction, length is set at creation time.
        // The actual auto-length behavior is in rt_api/property.rs set_prop.
        assert_eq!(arr.elements_len(), 6);
    }

    #[test]
    fn test_auto_length_fills_gaps_with_undefined() {
        let mut arr = UnifiedObject::array(ShapeId(0), Vec::new());
        arr.set_element(2, JsValue::int(42));

        // Elements at index 0 and 1 should be undefined (gaps)
        assert_eq!(arr.get_element(0).map(|v| v.is_undefined()), Some(true));
        assert_eq!(arr.get_element(1).map(|v| v.is_undefined()), Some(true));
        assert_eq!(arr.get_element(2).map(|v| v.as_int()), Some(Some(42)));
    }

    // -----------------------------------------------------------------
    // Array length truncation
    // -----------------------------------------------------------------

    #[test]
    fn test_array_set_length_truncates_dense() {
        let elems = vec![
            JsValue::int(1),
            JsValue::int(2),
            JsValue::int(3),
            JsValue::int(4),
            JsValue::int(5),
        ];
        let mut arr = UnifiedObject::array(ShapeId(0), elems);
        assert_eq!(arr.array_len(), 5);

        arr.array_set_length(2);
        assert_eq!(arr.array_len(), 2);
        assert_eq!(arr.get_element(0).map(|v| v.as_int()), Some(Some(1)));
        assert_eq!(arr.get_element(1).map(|v| v.as_int()), Some(Some(2)));
        // Elements at indices 2, 3, 4 should be gone
        assert_eq!(arr.get_element(2), None);
    }

    #[test]
    fn test_array_set_length_truncates_holey() {
        let mut arr = UnifiedObject::array(ShapeId(0), vec![JsValue::int(1), JsValue::int(2)]);
        // Transition to Holey by deleting element 0
        arr.delete_element(0);
        assert_eq!(arr.element_kind, ElementKind::Holey);

        // Now add more elements
        arr.set_element(2, JsValue::int(3));
        arr.set_element(3, JsValue::int(4));

        // Truncate to length 2
        arr.array_set_length(2);
        assert_eq!(arr.array_len(), 2);
        assert_eq!(arr.get_element(2), None);
        assert_eq!(arr.get_element(3), None);
    }

    #[test]
    fn test_array_set_length_truncates_dictionary() {
        let mut arr = UnifiedObject::array(ShapeId(0), Vec::new());
        // Force dictionary mode by setting a very large index
        arr.set_element(100_000, JsValue::int(99));
        assert_eq!(arr.element_kind, ElementKind::Dictionary);

        // Set length to smaller value should remove entries beyond
        if let Some(InternalData::Array { length, .. }) = arr.internal_data_mut() {
            *length = 100_001;
        }
        arr.array_set_length(50);
        assert_eq!(arr.array_len(), 50);
        // The element at 100_000 should be gone
        assert_eq!(arr.get_element(100_000), None);
    }

    #[test]
    fn test_array_set_length_to_zero() {
        let elems = vec![JsValue::int(1), JsValue::int(2), JsValue::int(3)];
        let mut arr = UnifiedObject::array(ShapeId(0), elems);
        arr.array_set_length(0);
        assert_eq!(arr.array_len(), 0);
        assert_eq!(arr.elements_len(), 0);
    }

    // -----------------------------------------------------------------
    // Dense → Holey transition on delete
    // -----------------------------------------------------------------

    #[test]
    fn test_delete_element_dense_to_holey() {
        let elems = vec![JsValue::int(10), JsValue::int(20), JsValue::int(30)];
        let mut arr = UnifiedObject::array(ShapeId(0), elems);
        assert_eq!(arr.element_kind, ElementKind::Dense);

        let deleted = arr.delete_element(1);
        assert!(deleted);
        assert_eq!(arr.element_kind, ElementKind::Holey);

        // Element at index 1 should be a hole (None)
        assert_eq!(arr.get_element(1), None);
        // Other elements should be intact
        assert_eq!(arr.get_element(0).map(|v| v.as_int()), Some(Some(10)));
        assert_eq!(arr.get_element(2).map(|v| v.as_int()), Some(Some(30)));
    }

    #[test]
    fn test_holey_element_access_returns_none_for_holes() {
        let mut arr = UnifiedObject::array(ShapeId(0), vec![JsValue::int(1), JsValue::int(2)]);
        arr.delete_element(0);
        // Hole at index 0 should return None (caller maps to undefined)
        assert_eq!(arr.get_element(0), None);
        // Non-hole at index 1 should return the value
        assert_eq!(arr.get_element(1).map(|v| v.as_int()), Some(Some(2)));
    }

    #[test]
    fn test_delete_element_from_holey() {
        let mut arr = UnifiedObject::array(
            ShapeId(0),
            vec![JsValue::int(1), JsValue::int(2), JsValue::int(3)],
        );
        arr.delete_element(0); // Dense -> Holey
        arr.delete_element(2); // Delete another in Holey
        assert_eq!(arr.get_element(0), None);
        assert_eq!(arr.get_element(1).map(|v| v.as_int()), Some(Some(2)));
        assert_eq!(arr.get_element(2), None);
    }

    #[test]
    fn test_delete_element_out_of_bounds() {
        let elems = vec![JsValue::int(10)];
        let mut arr = UnifiedObject::array(ShapeId(0), elems);
        // Delete element beyond length — should still transition to Holey
        // but the delete itself doesn't find anything
        arr.delete_element(0);
        assert_eq!(arr.element_kind, ElementKind::Holey);

        // Deleting from empty-ish area in Holey
        let result = arr.delete_element(100);
        assert!(!result);
    }

    #[test]
    fn test_delete_element_from_none_storage() {
        let mut obj = UnifiedObject::ordinary(ShapeId(0));
        let result = obj.delete_element(0);
        assert!(!result);
    }

    // -----------------------------------------------------------------
    // Dictionary mode (sparse arrays)
    // -----------------------------------------------------------------

    #[test]
    fn test_sparse_array_uses_dictionary() {
        let mut arr = UnifiedObject::array(ShapeId(0), Vec::new());
        // Setting a very large index should trigger Dictionary mode
        arr.set_element(1_000_000, JsValue::int(42));
        assert_eq!(arr.element_kind, ElementKind::Dictionary);
        // Should NOT allocate 1M elements
        assert_eq!(arr.elements_len(), 1); // only 1 entry in the HashMap
        assert_eq!(
            arr.get_element(1_000_000).map(|v| v.as_int()),
            Some(Some(42))
        );
    }

    #[test]
    fn test_dictionary_element_access() {
        let mut arr = UnifiedObject::array(ShapeId(0), Vec::new());
        arr.set_element(1_000_000, JsValue::int(1));
        arr.set_element(2_000_000, JsValue::int(2));
        assert_eq!(arr.element_kind, ElementKind::Dictionary);
        assert_eq!(
            arr.get_element(1_000_000).map(|v| v.as_int()),
            Some(Some(1))
        );
        assert_eq!(
            arr.get_element(2_000_000).map(|v| v.as_int()),
            Some(Some(2))
        );
        assert_eq!(arr.get_element(500), None); // not set
    }

    #[test]
    fn test_dictionary_push() {
        let mut arr = UnifiedObject::array(ShapeId(0), Vec::new());
        arr.set_element(1_000_000, JsValue::int(1));
        if let Some(InternalData::Array { length, .. }) = arr.internal_data_mut() {
            *length = 1_000_001;
        }
        arr.array_push(JsValue::int(99));
        assert_eq!(arr.array_len(), 1_000_002);
        assert_eq!(
            arr.get_element(1_000_001).map(|v| v.as_int()),
            Some(Some(99))
        );
    }

    #[test]
    fn test_dictionary_pop() {
        let mut arr = UnifiedObject::array(ShapeId(0), Vec::new());
        arr.set_element(1_000_000, JsValue::int(42));
        if let Some(InternalData::Array { length, .. }) = arr.internal_data_mut() {
            *length = 1_000_001;
        }
        let val = arr.array_pop();
        assert_eq!(val.map(|v| v.as_int()), Some(Some(42)));
        assert_eq!(arr.array_len(), 1_000_000);
    }

    #[test]
    fn test_dictionary_delete_element() {
        let mut arr = UnifiedObject::array(ShapeId(0), Vec::new());
        arr.set_element(1_000_000, JsValue::int(42));
        assert_eq!(arr.element_kind, ElementKind::Dictionary);
        let deleted = arr.delete_element(1_000_000);
        assert!(deleted);
        assert_eq!(arr.get_element(1_000_000), None);
    }

    #[test]
    fn test_element_kind_dictionary_repr() {
        assert_eq!(ElementKind::Dictionary as u8, 3);
    }

    // -----------------------------------------------------------------
    // array_elements_resolved works across all storage types
    // -----------------------------------------------------------------

    #[test]
    fn test_array_elements_resolved_dense() {
        let elems = vec![JsValue::int(1), JsValue::int(2), JsValue::int(3)];
        let arr = UnifiedObject::array(ShapeId(0), elems);
        let resolved = arr.array_elements_resolved();
        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].as_int(), Some(1));
        assert_eq!(resolved[2].as_int(), Some(3));
    }

    #[test]
    fn test_array_elements_resolved_holey() {
        let mut arr = UnifiedObject::array(
            ShapeId(0),
            vec![JsValue::int(1), JsValue::int(2), JsValue::int(3)],
        );
        arr.delete_element(1); // Create hole at index 1
        let resolved = arr.array_elements_resolved();
        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].as_int(), Some(1));
        assert!(resolved[1].is_undefined()); // hole resolved to undefined
        assert_eq!(resolved[2].as_int(), Some(3));
    }

    #[test]
    fn test_array_elements_resolved_dictionary() {
        let mut arr = UnifiedObject::array(ShapeId(0), Vec::new());
        arr.set_element(2, JsValue::int(42));
        // Force dictionary mode
        let mut map = HashMap::new();
        map.insert(0u32, JsValue::int(10));
        map.insert(2u32, JsValue::int(30));
        arr.elements = ElementsStorage::Dictionary(map);
        arr.element_kind = ElementKind::Dictionary;
        if let Some(InternalData::Array { length, .. }) = arr.internal_data_mut() {
            *length = 3;
        }
        let resolved = arr.array_elements_resolved();
        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].as_int(), Some(10));
        assert!(resolved[1].is_undefined()); // not in map
        assert_eq!(resolved[2].as_int(), Some(30));
    }

    // -----------------------------------------------------------------
    // Push and pop on Holey arrays
    // -----------------------------------------------------------------

    #[test]
    fn test_holey_array_push() {
        let mut arr = UnifiedObject::array(ShapeId(0), vec![JsValue::int(1), JsValue::int(2)]);
        arr.delete_element(0); // Dense -> Holey
        assert_eq!(arr.element_kind, ElementKind::Holey);
        arr.array_push(JsValue::int(3));
        assert_eq!(arr.array_len(), 3);
        assert_eq!(arr.get_element(2).map(|v| v.as_int()), Some(Some(3)));
    }

    #[test]
    fn test_holey_array_pop() {
        let mut arr = UnifiedObject::array(ShapeId(0), vec![JsValue::int(1), JsValue::int(2)]);
        arr.delete_element(0); // Dense -> Holey
        let val = arr.array_pop();
        assert_eq!(val.map(|v| v.as_int()), Some(Some(2)));
        assert_eq!(arr.array_len(), 1);
    }

    #[test]
    fn test_holey_array_pop_hole() {
        let mut arr = UnifiedObject::array(ShapeId(0), vec![JsValue::int(1), JsValue::int(2)]);
        arr.delete_element(1); // Delete last element, creating hole
        let val = arr.array_pop();
        // Pop on holey array with hole at the end returns None
        assert!(val.is_none());
        assert_eq!(arr.array_len(), 1);
    }

    // -----------------------------------------------------------------
    // Dense → Dictionary transition threshold
    // -----------------------------------------------------------------

    #[test]
    fn test_small_index_stays_dense() {
        let mut arr = UnifiedObject::array(ShapeId(0), Vec::new());
        // Setting index 100 should NOT trigger Dictionary
        arr.set_element(100, JsValue::int(42));
        assert_eq!(arr.element_kind, ElementKind::Dense);
        assert_eq!(arr.elements_len(), 101);
    }

    #[test]
    fn test_transition_to_dictionary_preserves_existing_elements() {
        let elems = vec![JsValue::int(10), JsValue::int(20)];
        let mut arr = UnifiedObject::array(ShapeId(0), elems);
        // Setting a very large index should transition and preserve existing elements
        arr.set_element(1_000_000, JsValue::int(99));
        assert_eq!(arr.element_kind, ElementKind::Dictionary);
        assert_eq!(arr.get_element(0).map(|v| v.as_int()), Some(Some(10)));
        assert_eq!(arr.get_element(1).map(|v| v.as_int()), Some(Some(20)));
        assert_eq!(
            arr.get_element(1_000_000).map(|v| v.as_int()),
            Some(Some(99))
        );
    }

    // -----------------------------------------------------------------
    // Array sync length works with Holey
    // -----------------------------------------------------------------

    #[test]
    fn test_array_sync_length_holey() {
        let mut arr = UnifiedObject::array(
            ShapeId(0),
            vec![JsValue::int(1), JsValue::int(2), JsValue::int(3)],
        );
        arr.delete_element(1); // Dense -> Holey
        // Manually set length out of sync
        if let Some(InternalData::Array { length, .. }) = arr.internal_data_mut() {
            *length = 99;
        }
        arr.array_sync_length();
        assert_eq!(arr.array_len(), 3); // synced back to actual holey vec len
    }
}
