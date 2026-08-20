//! Runtime builtin function registration and dispatch.
//!
//! Provides helpers for formatting `JsValue` instances used by
//! console builtins and `toString` coercion.

use nanbox::JsValue;

use crate::display;

/// Format a `JsValue` for display (used by console builtins and `toString`).
pub fn display_jsvalue(val: JsValue) -> String {
    display::display_value(val)
}
