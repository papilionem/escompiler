//! Function built-in methods.
//!
//! Provides `Function.prototype.bind()`, `Function.prototype.call()`, and
//! `Function.prototype.apply()`. These are structural placeholders that return
//! the function or `undefined` since actual function invocation requires the
//! runtime call mechanism.

use nanbox::JsValue;

/// `Function.prototype.bind(thisArg, ...args)` — return a bound function.
///
/// Structural placeholder — returns the original function value. Full bind
/// support (creating a new function closure with captured `this` and partial
/// args) requires runtime function object support.
pub fn bind(args: &[JsValue]) -> JsValue {
    // In a full implementation, this would create a new BoundFunction object.
    // For now, return the function itself (first arg is `this` function).
    args.first().copied().unwrap_or_else(JsValue::undefined)
}

/// `Function.prototype.call(thisArg, ...args)` — invoke with explicit this.
///
/// Structural placeholder — returns `undefined`. Actual function invocation
/// requires the runtime dispatch mechanism.
pub fn call(_args: &[JsValue]) -> JsValue {
    JsValue::undefined()
}

/// `Function.prototype.apply(thisArg, argsArray)` — invoke with array of args.
///
/// Structural placeholder — returns `undefined`. Actual function invocation
/// requires the runtime dispatch mechanism.
pub fn apply(_args: &[JsValue]) -> JsValue {
    JsValue::undefined()
}
