//! Global object registry for built-in constructors and namespaces.
//!
//! Implements the global object's built-in properties as defined in
//! ES2024 §19 "The Global Object". Specifically:
//!
//! - §19.1 Value properties (`globalThis`, `NaN`, `Infinity`, `undefined`)
//! - §19.2 Function properties (`isNaN`, `isFinite`, `parseInt`, `parseFloat`,
//!   `eval`, `decodeURI`, etc.) — dispatched elsewhere
//! - §19.3 Constructor properties (`Array`, `Object`, `Map`, `Set`, etc.)
//! - §19.4 Other properties (`Math`, `JSON`, `Reflect`)
//!
//! Provides singleton global objects (e.g., `Array`, `Object`, `Math`, `JSON`)
//! that are cached in a thread-local registry. This guarantees identity semantics:
//! `Array === Array` evaluates to `true` because every access returns the same
//! NaN-boxed pointer.
//!
//! The entry point for compiled code is [`__esc_rt_get_global`], which extracts
//! a name string from a NaN-boxed value and returns the corresponding global
//! object. Constructor globals are `NativeFunc` objects (callable); namespace
//! globals are ordinary objects.
//!
//! [spec]: <https://tc39.es/ecma262/#sec-global-object>

use std::cell::RefCell;
use std::collections::HashMap;

use nanbox::JsValue;

use crate::internal_data::UnifiedObject;
use crate::tagged_obj::{ObjTag, TaggedObj};

use super::{OBJECT_PROPS, make_rt_string};

// =========================================================================
// Thread-local global object cache
// =========================================================================

thread_local! {
    /// Cache of global objects by name. Each entry maps a built-in name
    /// (e.g., `"Array"`, `"Math"`) to the NaN-boxed bits of its singleton
    /// object. Populated lazily on first access.
    static GLOBAL_OBJECTS: RefCell<HashMap<&'static str, u64>> = RefCell::new(HashMap::new());
}

// =========================================================================
// Constructor argument lengths (ES spec)
// =========================================================================

/// Returns the `length` property for a built-in constructor per the ES spec.
///
/// Each built-in constructor has a `"length"` own data property whose value
/// is the number of required arguments. Per §20.x.2 for each constructor,
/// the `length` property has attributes `{ [[Writable]]: false,
/// [[Enumerable]]: false, [[Configurable]]: true }`.
///
/// [spec]: https://tc39.es/ecma262/#sec-built-in-function-objects
///
/// Values per constructor:
/// - `Array(len)` §22.1.1 — length 1
/// - `Object(value)` §20.1.1 — length 1
/// - `String(value)` §22.1.1 — length 1
/// - `Number(value)` §21.1.1 — length 1
/// - `Boolean(value)` §20.3.1 — length 1
/// - `Function(…args, body)` §20.2.1 — length 1
/// - `RegExp(pattern, flags)` §22.2.1 — length 2
/// - `Date(y,m,d,h,min,s,ms)` §21.4.2 — length 7
/// - `Promise(executor)` §27.2.3 — length 1
/// - `Error(message)` §20.5.1 — length 1
/// - `TypeError/RangeError/…(message)` §20.5.5.x — length 1
/// - `Symbol(description)` §20.4.1 — length 0
/// - `Proxy(target, handler)` §28.2.1 — length 2
/// - `Map()` §24.1.1 — length 0
/// - `Set()` §24.2.1 — length 0
/// - `WeakMap()` §24.3.1 — length 0
/// - `WeakSet()` §24.4.1 — length 0
/// - `WeakRef(target)` §26.1.1 — length 1
fn constructor_length(name: &str) -> u32 {
    match name {
        // §22.1.1 Array ( ...values ) — length 1
        "Array" => 1,
        // §20.1.1 Object ( [ value ] ) — length 1
        "Object" => 1,
        // §22.1.1 String ( value ) — length 1
        "String" => 1,
        // §21.1.1 Number ( value ) — length 1
        "Number" => 1,
        // §20.3.1 Boolean ( value ) — length 1
        "Boolean" => 1,
        // §20.2.1 Function ( ...parameterArgs, bodyArg ) — length 1
        "Function" => 1,
        // §22.2.1 RegExp ( pattern, flags ) — length 2
        "RegExp" => 2,
        // §21.4.2 Date ( year, month [, date [, hours [, minutes [, seconds [, ms ] ] ] ] ] ) — length 7
        "Date" => 7,
        // §27.2.3 Promise ( executor ) — length 1
        "Promise" => 1,
        // §20.5.1 Error ( message [, options ] ) — length 1
        "Error" => 1,
        // §20.5.5.1 TypeError ( message [, options ] ) — length 1
        "TypeError" => 1,
        // §20.5.5.4 RangeError ( message [, options ] ) — length 1
        "RangeError" => 1,
        // §20.5.5.5 ReferenceError ( message [, options ] ) — length 1
        "ReferenceError" => 1,
        // §20.5.5.3 SyntaxError ( message [, options ] ) — length 1
        "SyntaxError" => 1,
        // §20.5.5.6 URIError ( message [, options ] ) — length 1
        "URIError" => 1,
        // §20.5.5.2 EvalError ( message [, options ] ) — length 1
        "EvalError" => 1,
        // §20.4.1 Symbol ( [ description ] ) — length 0
        "Symbol" => 0,
        // §28.2.1 Proxy ( target, handler ) — length 2
        "Proxy" => 2,
        // §24.1.1 Map ( [ iterable ] ) — length 0
        "Map" => 0,
        // §24.2.1 Set ( [ iterable ] ) — length 0
        "Set" => 0,
        // §24.3.1 WeakMap ( [ iterable ] ) — length 0
        "WeakMap" => 0,
        // §24.4.1 WeakSet ( [ iterable ] ) — length 0
        "WeakSet" => 0,
        // §26.1.1 WeakRef ( target ) — length 1
        "WeakRef" => 1,
        _ => 0,
    }
}

// =========================================================================
// Constructor trampolines
// =========================================================================
//
// Each constructor gets a dedicated trampoline function that reads the
// current call's argc/argv from thread-locals and delegates to the
// existing `call_builtin_constructor`.

/// Trampoline for calling a built-in constructor by name.
///
/// Reads `CURRENT_ARGC` / `CURRENT_ARGV` from thread-locals (set by the
/// dispatch layer before invoking a NativeFunc) and forwards to
/// `call_builtin_constructor`.
///
/// Each trampoline is the `[[Call]]` internal method (§10.3.1) for the
/// corresponding built-in constructor function object. When the compiled code
/// invokes e.g. `Array(1)` or `new Array(1)`, the dispatch layer sets the
/// thread-local argc/argv and calls the trampoline, which delegates to the
/// constructor implementation.
macro_rules! constructor_trampoline {
    ($fn_name:ident, $ctor_name:expr) => {
        fn $fn_name(_context: u64) -> u64 {
            let argc = super::CURRENT_ARGC.with(|c| c.get());
            let argv = super::CURRENT_ARGV.with(|c| c.get());
            // SAFETY: argc/argv are set by the dispatch layer before calling
            // into a NativeFunc. argv points to argc valid u64 values.
            unsafe { super::call_builtin_constructor($ctor_name, argc, argv) }
        }
    };
}

constructor_trampoline!(array_constructor_trampoline, "Array");
constructor_trampoline!(object_constructor_trampoline, "Object");
constructor_trampoline!(string_constructor_trampoline, "String");
constructor_trampoline!(number_constructor_trampoline, "Number");
constructor_trampoline!(boolean_constructor_trampoline, "Boolean");
constructor_trampoline!(function_constructor_trampoline, "Function");
constructor_trampoline!(regexp_constructor_trampoline, "RegExp");
constructor_trampoline!(date_constructor_trampoline, "Date");
constructor_trampoline!(promise_constructor_trampoline, "Promise");
constructor_trampoline!(error_constructor_trampoline, "Error");
constructor_trampoline!(type_error_constructor_trampoline, "TypeError");
constructor_trampoline!(range_error_constructor_trampoline, "RangeError");
constructor_trampoline!(reference_error_constructor_trampoline, "ReferenceError");
constructor_trampoline!(syntax_error_constructor_trampoline, "SyntaxError");
constructor_trampoline!(uri_error_constructor_trampoline, "URIError");
constructor_trampoline!(eval_error_constructor_trampoline, "EvalError");
constructor_trampoline!(proxy_constructor_trampoline, "Proxy");
constructor_trampoline!(map_constructor_trampoline, "Map");
constructor_trampoline!(set_constructor_trampoline, "Set");
constructor_trampoline!(weak_map_constructor_trampoline, "WeakMap");
constructor_trampoline!(weak_set_constructor_trampoline, "WeakSet");
constructor_trampoline!(weak_ref_constructor_trampoline, "WeakRef");

/// Trampoline for `Symbol()` — uses `call_builtin_function` since Symbol
/// is not constructable (calling `new Symbol()` is a TypeError).
///
/// Per §20.4.1.1, `Symbol` is not intended to be used with the `new` operator.
/// It is a function that returns a new unique Symbol value. This trampoline
/// routes through `call_builtin_function` (not `call_builtin_constructor`)
/// to reflect that distinction.
///
/// [spec]: https://tc39.es/ecma262/#sec-symbol-description
fn symbol_constructor_trampoline(_context: u64) -> u64 {
    let argc = super::CURRENT_ARGC.with(|c| c.get());
    let argv = super::CURRENT_ARGV.with(|c| c.get());
    // SAFETY: argc/argv are set by the dispatch layer before calling
    // into a NativeFunc. argv points to argc valid u64 values.
    unsafe { super::call_builtin_function("Symbol", argc, argv) }
}

// =========================================================================
// Plain callable function trampolines (§19.2 Function Properties)
// =========================================================================
//
// These are non-constructable built-in functions that appear as callable
// properties on the global object: parseInt, parseFloat, isNaN, isFinite,
// encodeURI, decodeURI, encodeURIComponent, decodeURIComponent.
//
// Each gets a dedicated trampoline (fn(u64) -> u64) that reads argc/argv
// from thread-locals and delegates to `call_builtin_function`.

/// Trampoline macro for plain callable global functions.
///
/// Unlike constructor trampolines, these call `call_builtin_function` and
/// are marked with `__non_ctor__` so `new parseInt()` throws TypeError.
macro_rules! callable_fn_trampoline {
    ($fn_name:ident, $builtin_name:expr) => {
        fn $fn_name(_context: u64) -> u64 {
            let argc = super::CURRENT_ARGC.with(|c| c.get());
            let argv = super::CURRENT_ARGV.with(|c| c.get());
            // SAFETY: argc/argv are set by the dispatch layer before calling
            // into a NativeFunc. argv points to argc valid u64 values.
            unsafe { super::call_builtin_function($builtin_name, argc, argv) }
        }
    };
}

callable_fn_trampoline!(parse_int_trampoline, "parseInt");
callable_fn_trampoline!(parse_float_trampoline, "parseFloat");
callable_fn_trampoline!(is_nan_trampoline, "isNaN");
callable_fn_trampoline!(is_finite_trampoline, "isFinite");
callable_fn_trampoline!(encode_uri_trampoline, "encodeURI");
callable_fn_trampoline!(decode_uri_trampoline, "decodeURI");
callable_fn_trampoline!(encode_uri_component_trampoline, "encodeURIComponent");
callable_fn_trampoline!(decode_uri_component_trampoline, "decodeURIComponent");

/// Returns the trampoline and length for a plain callable global function.
///
/// Returns `Some((trampoline, length))` for recognized function names,
/// `None` otherwise.
#[allow(clippy::type_complexity)]
fn callable_function_for(name: &str) -> Option<(fn(u64) -> u64, u32)> {
    match name {
        // §19.2.5 parseInt ( string, radix ) — length 2
        "parseInt" => Some((parse_int_trampoline, 2)),
        // §19.2.4 parseFloat ( string ) — length 1
        "parseFloat" => Some((parse_float_trampoline, 1)),
        // §19.2.2 isNaN ( number ) — length 1
        "isNaN" => Some((is_nan_trampoline, 1)),
        // §19.2.3 isFinite ( number ) — length 1
        "isFinite" => Some((is_finite_trampoline, 1)),
        // §19.2.6 encodeURI ( uri ) — length 1
        "encodeURI" => Some((encode_uri_trampoline, 1)),
        // §19.2.7 decodeURI ( encodedURI ) — length 1
        "decodeURI" => Some((decode_uri_trampoline, 1)),
        // §19.2.8 encodeURIComponent ( uriComponent ) — length 1
        "encodeURIComponent" => Some((encode_uri_component_trampoline, 1)),
        // §19.2.9 decodeURIComponent ( encodedURIComponent ) — length 1
        "decodeURIComponent" => Some((decode_uri_component_trampoline, 1)),
        _ => None,
    }
}

/// Returns the trampoline function pointer for a given constructor name.
///
/// This is an internal dispatch table that maps each built-in constructor
/// name to its trampoline. Returns `None` for names that are not built-in
/// constructors (e.g., namespace objects like `"Math"`).
fn constructor_trampoline_for(name: &str) -> Option<fn(u64) -> u64> {
    match name {
        "Array" => Some(array_constructor_trampoline),
        "Object" => Some(object_constructor_trampoline),
        "String" => Some(string_constructor_trampoline),
        "Number" => Some(number_constructor_trampoline),
        "Boolean" => Some(boolean_constructor_trampoline),
        "Function" => Some(function_constructor_trampoline),
        "RegExp" => Some(regexp_constructor_trampoline),
        "Date" => Some(date_constructor_trampoline),
        "Promise" => Some(promise_constructor_trampoline),
        "Error" => Some(error_constructor_trampoline),
        "TypeError" => Some(type_error_constructor_trampoline),
        "RangeError" => Some(range_error_constructor_trampoline),
        "ReferenceError" => Some(reference_error_constructor_trampoline),
        "SyntaxError" => Some(syntax_error_constructor_trampoline),
        "URIError" => Some(uri_error_constructor_trampoline),
        "EvalError" => Some(eval_error_constructor_trampoline),
        "Symbol" => Some(symbol_constructor_trampoline),
        "Proxy" => Some(proxy_constructor_trampoline),
        "Map" => Some(map_constructor_trampoline),
        "Set" => Some(set_constructor_trampoline),
        "WeakMap" => Some(weak_map_constructor_trampoline),
        "WeakSet" => Some(weak_set_constructor_trampoline),
        "WeakRef" => Some(weak_ref_constructor_trampoline),
        _ => None,
    }
}

// =========================================================================
// Namespace names
// =========================================================================

/// Returns `true` if the given name is a built-in namespace object
/// (not a constructor). Namespace objects are ordinary objects whose
/// `typeof` is `"object"`, not `"function"`.
///
/// Per the ES2024 specification:
/// - `Math` is the Math object (§21.3) — not a function, not callable
/// - `JSON` is the JSON object (§25.5) — not a function, not callable
/// - `Reflect` is the Reflect object (§28.1) — not a function, not callable
/// - `globalThis` is the global `this` value (§19.1)
fn is_namespace(name: &str) -> bool {
    matches!(name, "Math" | "JSON" | "Reflect" | "globalThis")
}

// =========================================================================
// Global object creation and lookup
// =========================================================================

/// `SetDefaultGlobalBindings ( realmRec )` (partial)
///
/// Get or create a singleton global object for the given built-in name.
/// This implements the lazy-initialization portion of §19.3
/// `SetDefaultGlobalBindings`, which defines the standard built-in
/// properties of the global object.
///
/// Constructors (e.g., `Array`, `Object`, `Map`) are created as `NativeFunc`
/// objects with the callable flag set, so `typeof Array` returns `"function"`.
/// Namespaces (e.g., `Math`, `JSON`, `Reflect`) are created as ordinary
/// objects, so `typeof Math` returns `"object"`.
///
/// The returned value is cached in the thread-local `GLOBAL_OBJECTS` map,
/// guaranteeing identity: `Array === Array` is `true`.
///
/// Returns `JsValue::undefined()` bits if the name is not a recognized built-in.
///
/// [spec]: https://tc39.es/ecma262/#sec-setdefaultglobalbindings
///
/// Note: The spec installs all globals eagerly during realm initialization.
/// We use lazy singletons instead — each global is created on first access —
/// but the observable behavior is the same.
pub fn get_global_object(name: &str) -> u64 {
    // Fast path: check cache (already initialized on a prior access)
    let cached = GLOBAL_OBJECTS.with(|globals| {
        let map = globals.borrow();
        map.get(name).copied()
    });
    if let Some(bits) = cached {
        return bits;
    }

    // §19.1 Value Properties of the Global Object:
    // globalThis — the global object itself (§19.1.1).
    if name == "globalThis" {
        let bits = super::__esc_rt_get_global_this();
        // Cache with a 'static key — "globalThis" is a string literal
        GLOBAL_OBJECTS.with(|globals| {
            globals.borrow_mut().insert("globalThis", bits);
        });
        return bits;
    }

    // §19.3 Step 3: For each property defined in §19.1–§19.4, create the
    // property on the global object. We split into namespace objects
    // (ordinary, non-callable) and constructor function objects.

    // Namespace objects: Math (§21.3), JSON (§25.5), Reflect (§28.1)
    if is_namespace(name) {
        let bits = create_namespace_object(name);
        return bits;
    }

    // Constructor function objects: Array (§22.1.1), Object (§20.1.1), etc.
    // Each is a built-in function object (§10.3) with [[Construct]].
    if let Some(trampoline) = constructor_trampoline_for(name) {
        let bits = create_constructor_object(name, trampoline);
        return bits;
    }

    // §19.2 Plain callable function objects: parseInt, parseFloat, isNaN, etc.
    // These are callable but NOT constructable (new parseInt() throws TypeError).
    if let Some((trampoline, length)) = callable_function_for(name) {
        let bits = create_callable_function_object(name, trampoline, length);
        return bits;
    }

    // Unknown global — return undefined
    JsValue::undefined().raw_bits()
}

/// `CreateBuiltinFunction` + property setup for a constructor global.
///
/// Creates a built-in constructor function object per §10.3.3
/// `CreateBuiltinFunction` and installs its `"name"` and `"length"` own
/// properties per §10.3.3 steps 8–9.
///
/// [spec]: https://tc39.es/ecma262/#sec-createbuiltinfunction
/// [spec-length]: https://tc39.es/ecma262/#sec-built-in-function-objects
///
/// The `.prototype` property is lazily created via
/// `get_or_create_builtin_prototype` from property.rs. Per each constructor's
/// §20.x.2 "Properties of the Xxx Constructor" section, the `prototype`
/// property is defined with attributes `{ [[Writable]]: false,
/// [[Enumerable]]: false, [[Configurable]]: false }`.
fn create_constructor_object(name: &str, trampoline: fn(u64) -> u64) -> u64 {
    // §10.3.3 Step 1–7: Create a new built-in function object.
    let obj = UnifiedObject::native_func(trampoline, 0);
    let bits = TaggedObj::boxed(ObjTag::Unified, obj);

    // §10.3.3 Step 8: Perform SetFunctionName(func, name).
    let name_bits = make_rt_string(name.to_string());
    // §10.3.3 Step 9: Perform SetFunctionLength(func, length).
    // The "length" property has attributes { [[Writable]]: false,
    // [[Enumerable]]: false, [[Configurable]]: true } per §20.x.2.
    let length_bits = JsValue::number(constructor_length(name) as f64).raw_bits();

    OBJECT_PROPS.with(|props| {
        let mut props = props.borrow_mut();
        let map = props.entry(bits).or_default();
        // "name" — §20.x.2.x: has { [[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: true }
        map.insert("name".to_string(), name_bits);
        // "length" — §20.x.2.x: has { [[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: true }
        map.insert("length".to_string(), length_bits);
    });
    // TODO: Step — Set "prototype" property eagerly with correct descriptor attributes.
    // Currently the prototype is lazily created in property.rs on first access.

    // Cache with a 'static key. We match the name to a string literal so
    // the HashMap key is `&'static str` and does not allocate.
    if let Some(static_name) = to_static_name(name) {
        GLOBAL_OBJECTS.with(|globals| {
            globals.borrow_mut().insert(static_name, bits);
        });
    }

    bits
}

/// Create a plain callable (non-constructable) built-in function object.
///
/// Used for §19.2 global function properties (`parseInt`, `parseFloat`,
/// `isNaN`, `isFinite`, `encodeURI`, `decodeURI`, `encodeURIComponent`,
/// `decodeURIComponent`). These are `NativeFunc` objects (callable) that
/// are NOT constructable — `new parseInt()` must throw a TypeError.
///
/// The `__non_ctor__` marker in `OBJECT_PROPS` signals the dispatch layer
/// that this function object does not implement `[[Construct]]`.
///
/// [spec]: https://tc39.es/ecma262/#sec-createbuiltinfunction
fn create_callable_function_object(name: &str, trampoline: fn(u64) -> u64, length: u32) -> u64 {
    // §10.3.3 Step 1–7: Create a new built-in function object.
    let obj = UnifiedObject::native_func(trampoline, 0);
    let bits = TaggedObj::boxed(ObjTag::Unified, obj);

    // §10.3.3 Step 8: Perform SetFunctionName(func, name).
    let name_bits = make_rt_string(name.to_string());
    // §10.3.3 Step 9: Perform SetFunctionLength(func, length).
    let length_bits = JsValue::number(length as f64).raw_bits();

    OBJECT_PROPS.with(|props| {
        let mut props = props.borrow_mut();
        let map = props.entry(bits).or_default();
        map.insert("name".to_string(), name_bits);
        map.insert("length".to_string(), length_bits);
        // Mark as non-constructable — `new parseInt()` must throw TypeError.
        map.insert("__non_ctor__".to_string(), JsValue::bool(true).raw_bits());
    });

    // Cache with a 'static key.
    if let Some(static_name) = to_static_name(name) {
        GLOBAL_OBJECTS.with(|globals| {
            globals.borrow_mut().insert(static_name, bits);
        });
    }

    bits
}

/// Create a namespace global object (ordinary, not callable) and cache it.
///
/// Namespace objects are ordinary objects (§10.1) that are NOT callable.
/// Their static methods are dispatched by name in
/// `dispatch_global_namespace_method`.
///
/// Per the spec, each namespace is defined as a property of the global
/// object in §19.3 `SetDefaultGlobalBindings`:
/// - The `Math` object (§21.3): `{ [[Writable]]: true, [[Enumerable]]: false, [[Configurable]]: true }`
/// - The `JSON` object (§25.5): `{ [[Writable]]: true, [[Enumerable]]: false, [[Configurable]]: true }`
/// - The `Reflect` object (§28.1): `{ [[Writable]]: true, [[Enumerable]]: false, [[Configurable]]: true }`
///
/// [spec-math]: https://tc39.es/ecma262/#sec-math-object
/// [spec-json]: https://tc39.es/ecma262/#sec-json-object
/// [spec-reflect]: https://tc39.es/ecma262/#sec-reflect-object
fn create_namespace_object(name: &str) -> u64 {
    // Create an ordinary object (§10.1 Ordinary Object Internal Methods).
    // The object has [[Prototype]] of %Object.prototype% — required so that
    // user assignments like `Object.prototype.enumerable = true` are visible
    // through Math/JSON/Reflect during ToPropertyDescriptor reads (§8.10.5)
    // and ordinary property lookup (§10.1.8).
    //
    // NB: do NOT use __esc_rt_object_create here — it stores an enumerable
    // own `__proto__` data property (legacy mechanism), which leaks into
    // for-in enumeration (JSON must have zero enumerable properties).
    let obj = UnifiedObject::ordinary(shapes::ShapeTable::EMPTY_SHAPE);
    let bits = TaggedObj::boxed(ObjTag::Unified, obj);
    let proto = crate::rt_api::property::get_or_create_builtin_prototype("Object");
    crate::rt_api::property::register_prototype_on_object(bits, proto);

    // Store the namespace name so get_prop can dispatch property access
    // to the correct builtin methods (e.g., Math.exp, JSON.parse).
    OBJECT_PROPS.with(|props| {
        let mut props = props.borrow_mut();
        let map = props.entry(bits).or_default();
        map.insert(
            "__namespace__".to_string(),
            make_rt_string(name.to_string()),
        );
    });

    // Cache with a 'static key
    if let Some(static_name) = to_static_name(name) {
        GLOBAL_OBJECTS.with(|globals| {
            globals.borrow_mut().insert(static_name, bits);
        });
    }

    bits
}

/// Maps a runtime name string to a `&'static str` for use as a HashMap key.
///
/// This is a pure internal helper with no direct spec equivalent. It exists
/// to avoid allocating `String` keys in the cache `HashMap` — every
/// recognized global name maps to a string literal with `'static` lifetime.
///
/// Returns `None` for unrecognized names (which won't be cached).
fn to_static_name(name: &str) -> Option<&'static str> {
    match name {
        "Array" => Some("Array"),
        "Object" => Some("Object"),
        "String" => Some("String"),
        "Number" => Some("Number"),
        "Boolean" => Some("Boolean"),
        "Function" => Some("Function"),
        "RegExp" => Some("RegExp"),
        "Date" => Some("Date"),
        "Promise" => Some("Promise"),
        "Error" => Some("Error"),
        "TypeError" => Some("TypeError"),
        "RangeError" => Some("RangeError"),
        "ReferenceError" => Some("ReferenceError"),
        "SyntaxError" => Some("SyntaxError"),
        "URIError" => Some("URIError"),
        "EvalError" => Some("EvalError"),
        "Symbol" => Some("Symbol"),
        "Proxy" => Some("Proxy"),
        "Map" => Some("Map"),
        "Set" => Some("Set"),
        "WeakMap" => Some("WeakMap"),
        "WeakSet" => Some("WeakSet"),
        "WeakRef" => Some("WeakRef"),
        "Math" => Some("Math"),
        "JSON" => Some("JSON"),
        "Reflect" => Some("Reflect"),
        "globalThis" => Some("globalThis"),
        // §19.2 plain callable functions
        "parseInt" => Some("parseInt"),
        "parseFloat" => Some("parseFloat"),
        "isNaN" => Some("isNaN"),
        "isFinite" => Some("isFinite"),
        "encodeURI" => Some("encodeURI"),
        "decodeURI" => Some("decodeURI"),
        "encodeURIComponent" => Some("encodeURIComponent"),
        "decodeURIComponent" => Some("decodeURIComponent"),
        _ => None,
    }
}

// =========================================================================
// C ABI entry point
// =========================================================================

/// Runtime entry point for the `LoadGlobal` IR opcode.
///
/// Implements the runtime half of `ResolveBinding` (§9.1.2.1) for global
/// built-in names. The compiler emits a `LoadGlobal` opcode when it
/// statically determines that a free variable refers to a built-in global
/// (e.g., `Array`, `Math`, `JSON`). At runtime, this function resolves the
/// name to the singleton global object.
///
/// Compiled code calls this function with a NaN-boxed string containing the
/// global name. Returns the singleton global object for that name, or
/// `undefined` for unrecognized names.
///
/// The returned object is cached so that identity checks like
/// `Array === Array` work correctly.
///
/// [spec]: https://tc39.es/ecma262/#sec-resolvebinding
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_get_global(name_bits: u64) -> u64 {
    // 1. Extract the name string from the NaN-boxed value.
    let val = JsValue::from_raw_bits(name_bits);
    if val.is_string() {
        // 2. Look up the name in the global object registry.
        let name = crate::string_ops::get_string_data(val);
        return get_global_object(&name);
    }
    // Non-string argument — return undefined (should not happen in normal
    // compiled code, but is a safe fallback).
    JsValue::undefined().raw_bits()
}
