//! JavaScript RegExp implementation backed by fancy-regex.
//!
//! Wraps [`fancy_regex::Regex`] with JS-compatible flag handling,
//! `lastIndex` tracking, and `exec`/`test`/`matchAll` methods.
//!
//! # Key types
//!
//! - [`RegExpFlags`] — parsed flag set (`g`, `i`, `m`, `s`, `u`, `y`)
//! - [`JsRegExp`] — compiled regex with mutable `lastIndex` state
//! - [`RegExpMatch`] — result of a single match (full match + capture groups)
//! - [`RegExpError`] — error type for invalid patterns or flags

pub mod flags;
pub mod matcher;

pub use flags::RegExpFlags;
pub use matcher::{JsRegExp, RegExpError, RegExpMatch};

#[cfg(test)]
mod tests;
