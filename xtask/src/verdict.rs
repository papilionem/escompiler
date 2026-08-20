//! The verdict vocabulary.
//!
//! This module exists because the previous harness collapsed every outcome into
//! "ok" or a number. Two consequences it could not express, and both mattered:
//!
//!   * `1 / 0` died with SIGILL. The old runner reported `exit -1`
//!     (`unwrap_or(-1)`), which is indistinguishable from a program that chose to
//!     exit with an error. A signal is not an exit status.
//!   * A run that executed nothing printed the same thing as a run that executed
//!     everything and found no failures.
//!
//! **There is deliberately no `Skipped` variant.** Every reason a previous
//! harness had for skipping — no runtime archive, no `esc` binary, no `node`, a
//! missing fixture — is a failure here, because each one produced a green run
//! that had checked nothing.

use std::fmt;

/// How a process terminated, preserving the distinction the old runner lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitClass {
    /// Exited 0.
    Ok,
    /// Exited 2 — reserved for a deliberate, declared refusal.
    Refused,
    /// Exited non-zero and not 2: a genuine error.
    Error(i32),
    /// Killed by a signal. `1 / 0` produced SIGILL (132 = 128 + 4) and `9 % 0`
    /// SIGFPE (136 = 128 + 8); a vocabulary without this class calls both
    /// "exit -1" and hides that the compiler emitted a trapping instruction.
    Signal(i32),
}

impl ExitClass {
    /// Classify a raw exit code as observed by the shell (128 + n for signals).
    pub fn from_raw(code: Option<i32>, signal: Option<i32>) -> Self {
        if let Some(sig) = signal {
            return ExitClass::Signal(sig);
        }
        match code {
            Some(0) => ExitClass::Ok,
            Some(2) => ExitClass::Refused,
            Some(n) if (129..=192).contains(&n) => ExitClass::Signal(n - 128),
            Some(n) => ExitClass::Error(n),
            None => ExitClass::Error(-1),
        }
    }

    /// True when this class is acceptable for a MATCH entry.
    pub fn is_clean(self) -> bool {
        matches!(self, ExitClass::Ok)
    }
}

impl fmt::Display for ExitClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExitClass::Ok => write!(f, "ok"),
            ExitClass::Refused => write!(f, "refused(2)"),
            ExitClass::Error(n) => write!(f, "error({n})"),
            ExitClass::Signal(n) => write!(f, "signal({})", signal_name(*n)),
        }
    }
}

fn signal_name(n: i32) -> String {
    match n {
        4 => "SIGILL".into(),
        6 => "SIGABRT".into(),
        8 => "SIGFPE".into(),
        11 => "SIGSEGV".into(),
        _ => format!("{n}"),
    }
}

/// Whether stderr was empty, and if not, roughly what it looked like.
///
/// Compared as a CLASS, never byte-for-byte. Amendment A1: byte-comparing stderr
/// is unsatisfiable while stack frames are deferred to v0.12, and a criterion
/// nobody can satisfy is one that gets disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StderrClass {
    Empty,
    NonEmpty,
}

impl StderrClass {
    pub fn of(s: &str) -> Self {
        if s.trim().is_empty() {
            StderrClass::Empty
        } else {
            StderrClass::NonEmpty
        }
    }
}

impl fmt::Display for StderrClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StderrClass::Empty => write!(f, "empty"),
            StderrClass::NonEmpty => write!(f, "non-empty"),
        }
    }
}

/// The outcome of checking one manifest entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    /// Checked, and wrong. Carries why, for the report.
    Fail(String),
}

impl Verdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, Verdict::Pass)
    }
    pub fn fail(why: impl Into<String>) -> Self {
        Verdict::Fail(why.into())
    }
}
