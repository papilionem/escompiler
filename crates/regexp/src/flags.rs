//! RegExp flag parsing and representation.
//!
//! Converts a JS flags string (e.g. `"gi"`, `"gms"`) into a [`RegExpFlags`]
//! struct and translates the flags into fancy-regex inline modifiers.

use crate::matcher::RegExpError;

/// Parsed set of JS RegExp flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RegExpFlags {
    /// `g` — global: find all matches rather than stopping after the first.
    pub global: bool,
    /// `i` — ignore case.
    pub ignore_case: bool,
    /// `m` — multiline: `^` and `$` match per-line.
    pub multiline: bool,
    /// `s` — dotAll: `.` matches `\n`.
    pub dot_all: bool,
    /// `u` — unicode.
    pub unicode: bool,
    /// `y` — sticky: anchored at `lastIndex`.
    pub sticky: bool,
}

impl RegExpFlags {
    /// Parse a JS flags string into [`RegExpFlags`].
    ///
    /// Returns an error on unknown or duplicate flags.
    pub fn parse(flags_str: &str) -> Result<Self, RegExpError> {
        let mut f = RegExpFlags::default();
        for ch in flags_str.chars() {
            match ch {
                'g' if !f.global => f.global = true,
                'i' if !f.ignore_case => f.ignore_case = true,
                'm' if !f.multiline => f.multiline = true,
                's' if !f.dot_all => f.dot_all = true,
                'u' if !f.unicode => f.unicode = true,
                'y' if !f.sticky => f.sticky = true,
                _ => {
                    return Err(RegExpError::InvalidFlags {
                        flags: flags_str.to_string(),
                    });
                }
            }
        }
        Ok(f)
    }

    /// Build a fancy-regex inline-modifier prefix for these flags.
    ///
    /// Returns a string like `"(?ims)"` that is prepended to the pattern.
    pub fn to_inline_modifiers(&self) -> String {
        let mut mods = String::new();
        if self.ignore_case {
            mods.push('i');
        }
        if self.multiline {
            mods.push('m');
        }
        if self.dot_all {
            mods.push('s');
        }
        if mods.is_empty() {
            String::new()
        } else {
            format!("(?{mods})")
        }
    }
}
