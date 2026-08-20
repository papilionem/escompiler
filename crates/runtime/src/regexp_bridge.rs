//! Bridge between `regexp` and the runtime's NaN-boxed value world.
//!
//! Wraps [`regexp::JsRegExp`] in a [`JsRegExpData`] struct suitable for
//! storing inside a `UnifiedObject` with `InternalKind::RegExpObj`.

use regexp::{JsRegExp, RegExpError};

/// Wrapper around [`JsRegExp`] suitable for storing in a `UnifiedObject`.
pub struct JsRegExpData {
    /// The underlying compiled regular expression.
    pub inner: JsRegExp,
}

impl JsRegExpData {
    /// Create a new RegExp wrapper from pattern and flags strings.
    pub fn new(pattern: &str, flags: &str) -> Result<Self, RegExpError> {
        Ok(Self {
            inner: JsRegExp::new(pattern, flags)?,
        })
    }

    /// Reconstruct the flags string from the parsed flags.
    pub fn flags_string(&self) -> String {
        let f = &self.inner.flags;
        let mut s = String::new();
        if f.global {
            s.push('g');
        }
        if f.ignore_case {
            s.push('i');
        }
        if f.multiline {
            s.push('m');
        }
        if f.dot_all {
            s.push('s');
        }
        if f.unicode {
            s.push('u');
        }
        if f.sticky {
            s.push('y');
        }
        s
    }
}
