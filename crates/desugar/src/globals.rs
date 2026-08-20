//! Well-known global identifiers recognized during desugaring.
//!
//! When the lowerer encounters member expressions like `console.log(...)`,
//! it can emit specialized IR (e.g., `Op::CallRuntime`) instead of generic
//! property access + call, enabling the backend to generate direct calls
//! to runtime helpers.

/// Well-known global object: `console`.
pub const CONSOLE: &str = "console";
/// Well-known global object: `Math`.
pub const MATH: &str = "Math";
/// Well-known global object: `JSON`.
pub const JSON: &str = "JSON";
/// Well-known global object: `Number`.
pub const NUMBER: &str = "Number";
/// Well-known global object: `Object`.
pub const OBJECT: &str = "Object";
/// Well-known global object: `Array`.
pub const ARRAY: &str = "Array";
/// Well-known global object: `String`.
pub const STRING_GLOBAL: &str = "String";
/// Well-known global object: `process`.
pub const PROCESS: &str = "process";

/// Console method runtime name: `console.log`.
pub const CONSOLE_LOG: &str = "console.log";
/// Console method runtime name: `console.error`.
pub const CONSOLE_ERROR: &str = "console.error";
/// Console method runtime name: `console.warn`.
pub const CONSOLE_WARN: &str = "console.warn";
/// Console method runtime name: `console.debug`.
pub const CONSOLE_DEBUG: &str = "console.debug";

/// Built-in constructor names that should be passed as string identifiers
/// to `__esc_rt_call_new` rather than resolving to `undefined`.
const BUILTIN_CONSTRUCTORS: &[&str] = &[
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "WeakRef",
    "RegExp",
    "Proxy",
    "Promise",
    "Error",
    "TypeError",
    "RangeError",
    "ReferenceError",
    "SyntaxError",
    "URIError",
    "EvalError",
    "Symbol",
    "Date",
    "Int8Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "Int16Array",
    "Uint16Array",
    "Int32Array",
    "Uint32Array",
    "Float32Array",
    "Float64Array",
    "BigInt64Array",
    "BigUint64Array",
    "ArrayBuffer",
    "SharedArrayBuffer",
    "DataView",
];

/// Globals that JavaScript defines and this compiler does **not** implement.
///
/// Referencing one of these compiles cleanly today and then dies at run time with
/// **zero bytes on both streams** — the artifact exits 1 having printed nothing.
/// That is the single most common way this compiler violates rung 1's thesis that
/// *exit 0 means it worked*, so these are refused at compile time instead.
///
/// # Membership rule
///
/// A name belongs here iff **the pinned Node has it and this compiler fails on
/// it**. Both halves matter:
///
///  * `XMLHttpRequest` is deliberately absent — Node does not define it either, so
///    a `ReferenceError` is the *correct* answer and refusing would diverge from
///    the oracle.
///  * `ArrayBuffer`, `Uint8Array` and `DataView` are deliberately absent — they
///    resolve and bind fine (`var f = ArrayBuffer` exits 0). They are unusable in
///    other ways, which is a different defect with a different ticket. Being
///    listed in `BUILTIN_CONSTRUCTORS` means "emit LoadGlobal", not "implemented".
///
/// Every entry below was measured by compiling and running `var f = <name>;` and
/// comparing against `node -e "typeof <name>"`. Nothing here is assumed.
const UNIMPLEMENTED_GLOBALS: &[(&str, &str)] = &[
    // Timers and microtask scheduling — the host event loop does not exist yet.
    ("setTimeout", "timers"),
    ("setInterval", "timers"),
    ("clearTimeout", "timers"),
    ("clearInterval", "timers"),
    ("setImmediate", "timers"),
    ("clearImmediate", "timers"),
    ("queueMicrotask", "timers"),
    // Network — compiled programs cannot reach host I/O at all.
    ("fetch", "network"),
    ("WebSocket", "network"),
    ("Headers", "network"),
    ("Request", "network"),
    ("Response", "network"),
    ("FormData", "network"),
    // Streams.
    ("ReadableStream", "streams"),
    ("WritableStream", "streams"),
    ("TransformStream", "streams"),
    ("TextDecoderStream", "streams"),
    // Text encoding.
    ("TextEncoder", "text-encoding"),
    ("TextDecoder", "text-encoding"),
    // URL.
    ("URL", "url"),
    ("URLSearchParams", "url"),
    // Events and cancellation.
    ("Event", "events"),
    ("EventTarget", "events"),
    ("AbortController", "events"),
    ("AbortSignal", "events"),
    ("MessageChannel", "events"),
    // Binary data helpers that genuinely do not resolve.
    ("Blob", "binary"),
    ("File", "binary"),
    ("Buffer", "binary"),
    ("Atomics", "binary"),
    // Numerics.
    ("BigInt", "bigint"),
    // Internationalisation.
    ("Intl", "intl"),
    // Miscellaneous host services.
    ("structuredClone", "host"),
    ("crypto", "host"),
    ("performance", "host"),
    ("FinalizationRegistry", "gc"),
];

/// If `name` is a JavaScript global this compiler does not implement, return the
/// feature area it belongs to.
///
/// Used to turn a silent runtime death into a compile-time refusal. See
/// [`UNIMPLEMENTED_GLOBALS`] for the membership rule.
pub fn unimplemented_global(name: &str) -> Option<&'static str> {
    UNIMPLEMENTED_GLOBALS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, area)| *area)
}

/// Check if a name is a well-known built-in global (constructor or namespace).
///
/// These identifiers should be emitted as string constants rather than
/// `undefined` so the runtime can dispatch on them. Includes constructors,
/// namespaces, and global functions like `parseInt`.
pub fn is_builtin_global(name: &str) -> bool {
    BUILTIN_CONSTRUCTORS.contains(&name)
        || matches!(
            name,
            "console"
                | "Math"
                | "JSON"
                | "Number"
                | "Object"
                | "Array"
                | "String"
                | "Boolean"
                | "Function"
                | "Reflect"
                | "globalThis"
                | "process"
                | "parseInt"
                | "parseFloat"
                | "isNaN"
                | "isFinite"
                | "encodeURI"
                | "encodeURIComponent"
                | "decodeURI"
                | "decodeURIComponent"
        )
}

/// Check if a string name represents a built-in callable (constructor or function).
///
/// Returns `true` for names like `Object`, `parseInt`, `Array`, etc. that implement
/// `[[Call]]` and should return `"function"` from the `typeof` operator.
/// Returns `false` for namespaces like `Math`, `JSON`, `Reflect` that are plain objects.
pub fn is_builtin_callable(name: &str) -> bool {
    BUILTIN_CONSTRUCTORS.contains(&name)
        || matches!(
            name,
            "Object"
                | "Array"
                | "String"
                | "Number"
                | "Boolean"
                | "Function"
                | "parseInt"
                | "parseFloat"
                | "isNaN"
                | "isFinite"
                | "encodeURI"
                | "encodeURIComponent"
                | "decodeURI"
                | "decodeURIComponent"
        )
}

/// Check if a string name represents a built-in namespace object (not callable).
///
/// Namespace objects like `Math`, `JSON`, `Reflect` implement no `[[Call]]` and
/// should return `"object"` from the `typeof` operator.
pub fn is_builtin_namespace(name: &str) -> bool {
    matches!(
        name,
        "Math" | "JSON" | "Reflect" | "globalThis" | "console" | "process"
    )
}

/// Check if a dotted name is a well-known console method.
pub fn is_console_method(obj_name: &str, method_name: &str) -> bool {
    obj_name == CONSOLE
        && matches!(
            method_name,
            "log" | "error" | "warn" | "debug" | "info" | "trace"
        )
}

/// Get the runtime call name for a console method (e.g., `"log"` -> `"__esc_rt_console_log"`).
pub fn console_runtime_name(method_name: &str) -> Option<&'static str> {
    match method_name {
        "log" | "info" | "trace" => Some("__esc_rt_console_log"),
        "error" => Some("__esc_rt_console_error"),
        "warn" => Some("__esc_rt_console_warn"),
        "debug" => Some("__esc_rt_console_log"),
        _ => None,
    }
}
