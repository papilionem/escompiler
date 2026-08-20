//! Boolean built-in methods.
//!
//! Provides the `Boolean()` conversion function that follows standard JS
//! truthiness rules.

use nanbox::JsValue;

/// The `Boolean()` conversion function.
///
/// Converts any JavaScript value to a boolean using the standard JS rules:
/// - `null`, `undefined`, `false`, `0`, `NaN`, `""` -> `false`
/// - Everything else -> `true`
pub fn boolean_call(args: &[JsValue]) -> JsValue {
    let val = args.first().copied().unwrap_or_else(JsValue::undefined);
    JsValue::bool(!val.is_falsy())
}
