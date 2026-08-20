//! Core RegExp matcher wrapping fancy-regex.
//!
//! [`JsRegExp`] compiles a JS regex pattern with flags and provides
//! `test`, `exec`, and `match_all` methods that mirror the JS spec.

use fancy_regex::Regex;
use thiserror::Error;

use crate::flags::RegExpFlags;

/// Errors produced by RegExp compilation or flag parsing.
#[derive(Debug, Error)]
pub enum RegExpError {
    /// The regex pattern failed to compile.
    #[error("invalid RegExp pattern `{pattern}`: {reason}")]
    InvalidPattern {
        /// The source pattern string.
        pattern: String,
        /// Human-readable reason from the regex engine.
        reason: String,
    },
    /// The flags string contains unknown or duplicate flags.
    #[error("invalid RegExp flags: `{flags}`")]
    InvalidFlags {
        /// The offending flags string.
        flags: String,
    },
}

/// Result of a single regex match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegExpMatch {
    /// The full matched text.
    pub full_match: String,
    /// Capture groups (index 0 is group 1, etc.). `None` for unmatched groups.
    pub groups: Vec<Option<String>>,
    /// Byte offset of the match start in the input.
    pub index: usize,
}

/// A compiled JS RegExp with mutable `lastIndex` tracking.
pub struct JsRegExp {
    /// The original source pattern.
    pub pattern: String,
    /// The compiled regex.
    regex: Regex,
    /// Parsed flags.
    pub flags: RegExpFlags,
    /// The current lastIndex (used by global/sticky modes).
    pub last_index: usize,
}

impl JsRegExp {
    /// Compile a new `JsRegExp` from a pattern and flags string.
    pub fn new(pattern: &str, flags_str: &str) -> Result<Self, RegExpError> {
        let flags = RegExpFlags::parse(flags_str)?;
        let modified = format!("{}{}", flags.to_inline_modifiers(), pattern);
        let regex = Regex::new(&modified).map_err(|e| RegExpError::InvalidPattern {
            pattern: pattern.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Self {
            pattern: pattern.to_string(),
            regex,
            flags,
            last_index: 0,
        })
    }

    /// Test whether the pattern matches `input`.
    ///
    /// For global/sticky regexps, matching starts at `lastIndex` and advances it.
    pub fn test(&mut self, input: &str) -> bool {
        self.exec(input).is_some()
    }

    /// Execute the pattern against `input`, returning the first match (if any).
    ///
    /// For global/sticky regexps, matching starts at `lastIndex` and updates it.
    /// Non-global/non-sticky regexps always search from the start.
    pub fn exec(&mut self, input: &str) -> Option<RegExpMatch> {
        let start = if self.flags.global || self.flags.sticky {
            self.last_index
        } else {
            0
        };

        if start > input.len() {
            if self.flags.global || self.flags.sticky {
                self.last_index = 0;
            }
            return None;
        }

        let slice = &input[start..];
        let captures = match self.regex.captures(slice).ok().flatten() {
            Some(c) => c,
            None => {
                if self.flags.global || self.flags.sticky {
                    self.last_index = 0;
                }
                return None;
            }
        };
        let Some(m) = captures.get(0) else {
            if self.flags.global || self.flags.sticky {
                self.last_index = 0;
            }
            return None;
        };

        // Sticky requires the match to start at position 0 of the slice.
        if self.flags.sticky && m.start() != 0 {
            self.last_index = 0;
            return None;
        }

        let groups: Vec<Option<String>> = (1..captures.len())
            .map(|i| captures.get(i).map(|g| g.as_str().to_string()))
            .collect();

        let match_index = start + m.start();
        if self.flags.global || self.flags.sticky {
            self.last_index = start + m.end();
        }

        Some(RegExpMatch {
            full_match: m.as_str().to_string(),
            groups,
            index: match_index,
        })
    }

    /// Return all matches in `input` (global-flag behaviour).
    ///
    /// Resets `lastIndex` to 0, then collects every successive match.
    pub fn match_all(&mut self, input: &str) -> Vec<RegExpMatch> {
        self.last_index = 0;
        let mut results = Vec::new();
        while let Some(m) = self.exec(input) {
            // Guard against zero-length matches causing infinite loops.
            if m.full_match.is_empty() {
                self.last_index += 1;
            }
            results.push(m);
            if !self.flags.global {
                break;
            }
        }
        results
    }

    /// Reset `lastIndex` to 0.
    pub fn reset(&mut self) {
        self.last_index = 0;
    }
}

impl std::fmt::Debug for JsRegExp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JsRegExp(/{}/{})", self.pattern, self.flags_string())
    }
}

impl JsRegExp {
    /// Reconstruct the flags string from the parsed flags.
    fn flags_string(&self) -> String {
        let mut s = String::new();
        if self.flags.global {
            s.push('g');
        }
        if self.flags.ignore_case {
            s.push('i');
        }
        if self.flags.multiline {
            s.push('m');
        }
        if self.flags.dot_all {
            s.push('s');
        }
        if self.flags.unicode {
            s.push('u');
        }
        if self.flags.sticky {
            s.push('y');
        }
        s
    }
}
