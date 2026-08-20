//! JavaScript Error type constructors (`Error`, `TypeError`, `RangeError`, etc.).
//!
//! Provides the [`JsErrorKind`] enum representing the standard JS error hierarchy,
//! constructor functions, and a simple [`JsError`] representation with `message`,
//! `name`, and `stack` properties stored behind an object pointer.
//!
//! Also includes [`AggregateError`](aggregate_error) — an `Error` subclass
//! that wraps multiple errors, used by `Promise.any()` when all promises reject.

use nanbox::JsValue;

/// Error kind enumeration matching JavaScript's built-in error types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsErrorKind {
    /// Generic `Error`.
    Error,
    /// `TypeError` — value is not of the expected type.
    TypeError,
    /// `RangeError` — numeric value is out of range.
    RangeError,
    /// `ReferenceError` — reference to an undeclared variable.
    ReferenceError,
    /// `SyntaxError` — parsing error.
    SyntaxError,
    /// `URIError` — malformed URI.
    URIError,
    /// `EvalError` — error related to `eval()`.
    EvalError,
    /// `AggregateError` — wraps multiple errors (e.g. from `Promise.any()`).
    AggregateError,
}

impl JsErrorKind {
    /// Returns the JavaScript name of this error kind (e.g. `"TypeError"`).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::TypeError => "TypeError",
            Self::RangeError => "RangeError",
            Self::ReferenceError => "ReferenceError",
            Self::SyntaxError => "SyntaxError",
            Self::URIError => "URIError",
            Self::EvalError => "EvalError",
            Self::AggregateError => "AggregateError",
        }
    }
}

/// Discriminant tag for ErrorInner — validates type before deref (ESC-20).
const ERROR_INNER_TAG: u64 = 0x4553435F4552525F; // "ESC_ERR_"

/// Internal error representation stored behind an object pointer.
///
/// The `tag` field is a type discriminant validated by `extract_error` to
/// prevent type-confusion: `val.as_object()` returns a raw pointer for ANY
/// object, so without a tag check, `Error.prototype.toString.call({})` would
/// reinterpret an arbitrary JsObject as ErrorInner (UB, ESC-20 / DG-3).
#[repr(C)]
struct ErrorInner {
    /// Type discriminant — must equal ERROR_INNER_TAG.
    tag: u64,
    kind: JsErrorKind,
    message: String,
}

/// Internal representation for `AggregateError`.
#[repr(C)]
struct AggregateErrorInner {
    /// Type discriminant — must equal ERROR_INNER_TAG.
    tag: u64,
    kind: JsErrorKind,
    message: String,
    /// The `.errors` property — a JsValue representing an array of errors.
    errors: JsValue,
}

fn make_error(kind: JsErrorKind, message: String) -> JsValue {
    let inner = Box::new(ErrorInner {
        tag: ERROR_INNER_TAG,
        kind,
        message,
    });
    let raw_ptr = Box::into_raw(inner) as *const ();
    JsValue::object(raw_ptr)
}
/// Extract error data from an object JsValue.
///
/// Validates the type discriminant before deref — non-Error objects
/// return `None` instead of reading foreign memory (ESC-20 / DG-3).
unsafe fn extract_error(val: &JsValue) -> Option<&ErrorInner> {
    let ptr = val.as_object()? as *const ErrorInner;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: discriminant validates this is a real ErrorInner before deref
    let inner = unsafe { &*ptr };
    if inner.tag != ERROR_INNER_TAG {
        return None;
    }
    Some(inner)
}

/// Extract `AggregateError` data from an object JsValue.
///
/// Validates the type discriminant before deref (ESC-20 / DG-3).
unsafe fn extract_aggregate_error(val: &JsValue) -> Option<&AggregateErrorInner> {
    let ptr = val.as_object()? as *const AggregateErrorInner;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: discriminant validates this is a real AggregateErrorInner
    let inner = unsafe { &*ptr };
    if inner.tag != ERROR_INNER_TAG {
        return None;
    }
    Some(inner)
}

/// Create a new string JsValue from a Rust String.
fn make_string(s: String) -> JsValue {
    let rt_str = Box::new(runtime::string_ops::RtString::new(s));
    let raw_ptr = Box::into_raw(rt_str) as *const ();
    JsValue::string(raw_ptr)
}

/// Create a new string JsValue from a Rust String (public API).
///
/// Exposed for use by sibling modules (e.g., `promise`) that need to create
/// string values without duplicating the `RtString` allocation logic.
pub fn make_error_string(s: String) -> JsValue {
    make_string(s)
}

/// Extract string data from a JsValue that is a string.
fn extract_string(val: &JsValue) -> Option<String> {
    if let Some(ptr) = val.as_string() {
        if ptr.is_null() {
            return Some(String::new());
        }
        let rt_str = unsafe {
            // SAFETY: ptr was created by make_string or string_from_data
            &*(ptr as *const runtime::string_ops::RtString)
        };
        Some(rt_str.as_str().to_string())
    } else {
        None
    }
}

/// Construct a generic `Error` with the given message.
///
/// Args: `[message]`
pub fn error(args: &[JsValue]) -> JsValue {
    let msg = args.first().and_then(extract_string).unwrap_or_default();
    make_error(JsErrorKind::Error, msg)
}

/// Construct a `TypeError` with the given message.
///
/// Args: `[message]`
pub fn type_error(args: &[JsValue]) -> JsValue {
    let msg = args.first().and_then(extract_string).unwrap_or_default();
    make_error(JsErrorKind::TypeError, msg)
}

/// Construct a `RangeError` with the given message.
///
/// Args: `[message]`
pub fn range_error(args: &[JsValue]) -> JsValue {
    let msg = args.first().and_then(extract_string).unwrap_or_default();
    make_error(JsErrorKind::RangeError, msg)
}

/// Construct a `ReferenceError` with the given message.
///
/// Args: `[message]`
pub fn reference_error(args: &[JsValue]) -> JsValue {
    let msg = args.first().and_then(extract_string).unwrap_or_default();
    make_error(JsErrorKind::ReferenceError, msg)
}

/// Construct a `SyntaxError` with the given message.
///
/// Args: `[message]`
pub fn syntax_error(args: &[JsValue]) -> JsValue {
    let msg = args.first().and_then(extract_string).unwrap_or_default();
    make_error(JsErrorKind::SyntaxError, msg)
}

/// Construct a `URIError` with the given message.
///
/// Args: `[message]`
pub fn uri_error(args: &[JsValue]) -> JsValue {
    let msg = args.first().and_then(extract_string).unwrap_or_default();
    make_error(JsErrorKind::URIError, msg)
}

/// Construct an `EvalError` with the given message.
///
/// Args: `[message]`
pub fn eval_error(args: &[JsValue]) -> JsValue {
    let msg = args.first().and_then(extract_string).unwrap_or_default();
    make_error(JsErrorKind::EvalError, msg)
}

/// `Error.prototype.toString()` — returns `"ErrorName: message"`.
///
/// Args: `[this]`
pub fn error_to_string(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let inner = unsafe {
        // SAFETY: this was created by make_error
        extract_error(&this)
    };
    let Some(inner) = inner else {
        return make_string("Error".to_string());
    };
    if inner.message.is_empty() {
        make_string(inner.kind.name().to_string())
    } else {
        make_string(format!("{}: {}", inner.kind.name(), inner.message))
    }
}

/// Get the `message` property of an error object.
///
/// Args: `[this]`
pub fn error_message(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let inner = unsafe {
        // SAFETY: this was created by make_error
        extract_error(&this)
    };
    let Some(inner) = inner else {
        return make_string(String::new());
    };
    make_string(inner.message.clone())
}

/// Get the `name` property of an error object.
///
/// Args: `[this]`
pub fn error_name(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);
    let inner = unsafe {
        // SAFETY: this was created by make_error
        extract_error(&this)
    };
    let Some(inner) = inner else {
        return make_string("Error".to_string());
    };
    make_string(inner.kind.name().to_string())
}

/// Get the error kind from an error JsValue.
///
/// Returns `None` if the value is not an error object.
pub fn get_error_kind(val: &JsValue) -> Option<JsErrorKind> {
    let inner = unsafe {
        // SAFETY: called on values created by this module
        extract_error(val)
    };
    inner.map(|e| e.kind)
}

// =========================================================================
// AggregateError
// =========================================================================

/// Construct an `AggregateError` with the given errors array and message.
///
/// Args: `[errors_array, message]`
///
/// - `errors_array` — a JsValue representing an array of individual errors.
/// - `message` — a JsValue string with the error message.
///
/// Returns an object JsValue backed by [`AggregateErrorInner`].
pub fn aggregate_error(args: &[JsValue]) -> JsValue {
    let errors = args.first().copied().unwrap_or_else(JsValue::undefined);
    let msg = args.get(1).and_then(extract_string).unwrap_or_default();
    let inner = Box::new(AggregateErrorInner {
        tag: ERROR_INNER_TAG,
        kind: JsErrorKind::AggregateError,
        message: msg,
        errors,
    });
    let raw_ptr = Box::into_raw(inner) as *const ();
    JsValue::object(raw_ptr)
}

/// Get the `.errors` property of an `AggregateError`.
///
/// Returns the errors array JsValue, or `None` if the value is not an
/// `AggregateError`.
pub fn get_aggregate_errors(val: &JsValue) -> Option<JsValue> {
    let inner = unsafe {
        // SAFETY: called on values created by aggregate_error
        extract_aggregate_error(val)
    };
    inner.map(|e| e.errors)
}

/// Get the error kind from an `AggregateError` JsValue.
///
/// Returns [`JsErrorKind::AggregateError`] if the value is a valid
/// `AggregateError`, `None` otherwise.
pub fn get_aggregate_error_kind(val: &JsValue) -> Option<JsErrorKind> {
    let inner = unsafe {
        // SAFETY: called on values created by aggregate_error
        extract_aggregate_error(val)
    };
    inner.map(|e| e.kind)
}

/// Get the `.message` property of an `AggregateError`.
///
/// Returns the message string as a JsValue, or an empty string if the
/// value is not an `AggregateError`.
pub fn get_aggregate_error_message(val: &JsValue) -> JsValue {
    let inner = unsafe {
        // SAFETY: called on values created by aggregate_error
        extract_aggregate_error(val)
    };
    let Some(inner) = inner else {
        return make_string(String::new());
    };
    make_string(inner.message.clone())
}
