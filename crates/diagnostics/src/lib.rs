//! # diagnostics — Compiler Diagnostics and Error Formatting
//!
//! Collects and formats diagnostics (errors, warnings, hints) emitted during
//! compilation. Provides structured [`Diagnostic`] objects with severity, source
//! spans, help text, and labels, plus a [`DiagnosticEmitter`] for accumulating
//! them during a compilation pass.
//!
//! ## Key Types
//!
//! - [`Severity`] — error, warning, info, or hint
//! - [`Diagnostic`] — a single diagnostic message with optional span and help
//! - [`DiagnosticEmitter`] — collects diagnostics and provides aggregate queries
//! - [`format_error`] — formats a [`CompileError`] for display

use common::CompileError;

/// Severity level of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// An error that prevents compilation.
    Error,
    /// A warning that does not prevent compilation.
    Warning,
    /// An informational message.
    Info,
    /// A hint for the user.
    Hint,
}

/// Structured diagnostic codes for compiler messages.
///
/// Each code identifies a specific class of diagnostic, making it possible to
/// filter, suppress, or search for specific issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticCode {
    /// ESC-W700: FFI is enabled — safety guarantees are bypassed.
    ///
    /// Emitted as a warning when `--allow-ffi` is passed or
    /// `permissions.allowFfi` is set in `esc.json`.
    FfiEnabled,
    /// ESC-E700: FFI usage detected without permission.
    ///
    /// Emitted as an error when the source uses FFI features but
    /// `--allow-ffi` was not passed.
    FfiNotAllowed,
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FfiEnabled => write!(f, "ESC-W700"),
            Self::FfiNotAllowed => write!(f, "ESC-E700"),
        }
    }
}

/// A single diagnostic message produced during compilation.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// The severity level (error, warning, info, hint).
    pub severity: Severity,
    /// The human-readable diagnostic message.
    pub message: String,
    /// An optional source span pointing to the relevant code.
    pub span: Option<common::SourceSpan>,
    /// An optional help message with suggestions for fixing the issue.
    pub help: Option<String>,
    /// Additional labels providing context for the diagnostic.
    pub labels: Vec<String>,
    /// An optional structured diagnostic code (e.g. ESC-W700).
    pub code: Option<DiagnosticCode>,
}

impl Diagnostic {
    /// Create an error-level diagnostic.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            span: None,
            help: None,
            labels: Vec::new(),
            code: None,
        }
    }

    /// Create a warning-level diagnostic.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            span: None,
            help: None,
            labels: Vec::new(),
            code: None,
        }
    }

    /// Create an info-level diagnostic.
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            message: message.into(),
            span: None,
            help: None,
            labels: Vec::new(),
            code: None,
        }
    }

    /// Create a hint-level diagnostic.
    pub fn hint(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Hint,
            message: message.into(),
            span: None,
            help: None,
            labels: Vec::new(),
            code: None,
        }
    }

    /// Attach a source span to this diagnostic.
    pub fn with_span(mut self, span: common::SourceSpan) -> Self {
        self.span = Some(span);
        self
    }

    /// Attach a help message to this diagnostic.
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Add a label to this diagnostic for additional context.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.labels.push(label.into());
        self
    }

    /// Attach a structured diagnostic code to this diagnostic.
    pub fn with_code(mut self, code: DiagnosticCode) -> Self {
        self.code = Some(code);
        self
    }

    /// Create the ESC-W700 warning diagnostic for FFI being enabled.
    pub fn ffi_enabled_warning() -> Self {
        Self::warning("FFI is enabled: native code may bypass compiler safety guarantees")
            .with_code(DiagnosticCode::FfiEnabled)
            .with_help("Remove --allow-ffi or set permissions.allowFfi to false to disable FFI")
    }

    /// Create the ESC-E700 error diagnostic for FFI usage without permission.
    pub fn ffi_not_allowed_error() -> Self {
        Self::error(
            "FFI usage requires explicit permission",
        )
        .with_code(DiagnosticCode::FfiNotAllowed)
        .with_help("Pass --allow-ffi on the command line or set permissions.allowFfi to true in esc.json")
    }
}

/// Collects diagnostics emitted during compilation.
pub struct DiagnosticEmitter {
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticEmitter {
    /// Create a new empty diagnostic emitter.
    pub fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
        }
    }

    /// Emit a diagnostic.
    pub fn emit(&mut self, diag: Diagnostic) {
        self.diagnostics.push(diag);
    }

    /// Returns true if any emitted diagnostic is an error.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Returns the number of error-level diagnostics.
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }

    /// Returns the number of warning-level diagnostics.
    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count()
    }

    /// Returns a slice of all emitted diagnostics.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Drain all diagnostics, leaving the emitter empty.
    pub fn drain(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// Consume the emitter and return all collected diagnostics.
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}

impl Default for DiagnosticEmitter {
    fn default() -> Self {
        Self::new()
    }
}

/// Format a [`CompileError`] into a human-readable, multi-line diagnostic string.
///
/// Includes the error kind, message, and — when a span is present — a location
/// hint with file ID and byte range.
pub fn format_error(err: &CompileError) -> String {
    use std::fmt::Write;

    let mut out = String::new();

    // Classify the error kind and extract the optional span.
    let (kind, span) = match err {
        CompileError::Parse { span, .. } => ("error[parse]", Some(span)),
        CompileError::Type { span, .. } => ("error[type]", Some(span)),
        CompileError::Escape { span, .. } => ("error[escape]", Some(span)),
        CompileError::Codegen { span, .. } => ("error[codegen]", Some(span)),
        CompileError::Runtime { span, .. } => ("error[runtime]", Some(span)),
        CompileError::Ir { .. } => ("error[ir]", None),
        CompileError::Module { span, .. } => ("error[module]", Some(span)),
        CompileError::Internal { .. } => ("error[internal]", None),
    };

    // First line: kind + display message.
    let _ = writeln!(out, "{kind}: {err}");

    // Second line: source location hint when a span is available.
    if let Some(span) = span {
        let _ = writeln!(
            out,
            "  --> file({}) {}..{}",
            span.file_id.0, span.start, span.end
        );
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use common::{FileId, SourceSpan};

    #[test]
    fn diagnostic_info_constructor() {
        let d = Diagnostic::info("informational");
        assert_eq!(d.severity, Severity::Info);
        assert_eq!(d.message, "informational");
    }

    #[test]
    fn diagnostic_hint_constructor() {
        let d = Diagnostic::hint("did you mean...");
        assert_eq!(d.severity, Severity::Hint);
        assert_eq!(d.message, "did you mean...");
    }

    #[test]
    fn diagnostic_with_label() {
        let d = Diagnostic::error("bad")
            .with_label("first label")
            .with_label("second label");
        assert_eq!(d.labels.len(), 2);
        assert_eq!(d.labels[0], "first label");
        assert_eq!(d.labels[1], "second label");
    }

    #[test]
    fn emitter_warning_count() {
        let mut em = DiagnosticEmitter::new();
        em.emit(Diagnostic::warning("w1"));
        em.emit(Diagnostic::warning("w2"));
        em.emit(Diagnostic::error("e1"));
        assert_eq!(em.warning_count(), 2);
        assert_eq!(em.error_count(), 1);
    }

    #[test]
    fn emitter_drain_leaves_empty() {
        let mut em = DiagnosticEmitter::new();
        em.emit(Diagnostic::error("e1"));
        em.emit(Diagnostic::info("i1"));
        let drained = em.drain();
        assert_eq!(drained.len(), 2);
        assert!(em.diagnostics().is_empty());
        assert!(!em.has_errors());
    }

    #[test]
    fn emitter_into_diagnostics() {
        let mut em = DiagnosticEmitter::new();
        em.emit(Diagnostic::hint("h1"));
        em.emit(Diagnostic::warning("w1"));
        let all = em.into_diagnostics();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].severity, Severity::Hint);
    }

    #[test]
    fn format_error_with_span() {
        let err = CompileError::Parse {
            message: "unexpected token".into(),
            span: SourceSpan::new(FileId(0), 10, 15),
        };
        let formatted = format_error(&err);
        assert!(formatted.contains("error[parse]"));
        assert!(formatted.contains("unexpected token"));
        assert!(formatted.contains("file(0)"));
        assert!(formatted.contains("10..15"));
    }

    #[test]
    fn format_error_without_span() {
        let err = CompileError::Internal {
            message: "ice".into(),
        };
        let formatted = format_error(&err);
        assert!(formatted.contains("error[internal]"));
        assert!(formatted.contains("ice"));
        assert!(!formatted.contains("-->"));
    }

    #[test]
    fn format_error_ir_variant() {
        let err = CompileError::Ir {
            message: "bad block".into(),
        };
        let formatted = format_error(&err);
        assert!(formatted.contains("error[ir]"));
        assert!(formatted.contains("bad block"));
    }

    #[test]
    fn format_error_module_variant() {
        let err = CompileError::Module {
            message: "not found".into(),
            span: SourceSpan::new(FileId(5), 100, 200),
        };
        let formatted = format_error(&err);
        assert!(formatted.contains("error[module]"));
        assert!(formatted.contains("not found"));
        assert!(formatted.contains("file(5)"));
        assert!(formatted.contains("100..200"));
    }

    // -- DiagnosticCode tests -----------------------------------------------

    #[test]
    fn test_diagnostic_code_ffi_enabled_display() {
        assert_eq!(DiagnosticCode::FfiEnabled.to_string(), "ESC-W700");
    }

    #[test]
    fn test_diagnostic_code_ffi_not_allowed_display() {
        assert_eq!(DiagnosticCode::FfiNotAllowed.to_string(), "ESC-E700");
    }

    #[test]
    fn test_diagnostic_code_equality() {
        assert_eq!(DiagnosticCode::FfiEnabled, DiagnosticCode::FfiEnabled);
        assert_ne!(DiagnosticCode::FfiEnabled, DiagnosticCode::FfiNotAllowed);
    }

    #[test]
    fn test_diagnostic_with_code() {
        let d = Diagnostic::error("test error").with_code(DiagnosticCode::FfiNotAllowed);
        assert_eq!(d.code, Some(DiagnosticCode::FfiNotAllowed));
        assert_eq!(d.severity, Severity::Error);
    }

    #[test]
    fn test_ffi_enabled_warning_diagnostic() {
        let d = Diagnostic::ffi_enabled_warning();
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.code, Some(DiagnosticCode::FfiEnabled));
        assert!(d.message.contains("FFI"));
        assert!(d.help.is_some());
    }

    #[test]
    fn test_ffi_not_allowed_error_diagnostic() {
        let d = Diagnostic::ffi_not_allowed_error();
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.code, Some(DiagnosticCode::FfiNotAllowed));
        assert!(d.message.contains("FFI"));
        assert!(d.help.is_some());
        assert!(
            d.help.as_ref().is_some_and(|h| h.contains("--allow-ffi")),
            "help should mention --allow-ffi"
        );
    }

    #[test]
    fn test_diagnostic_code_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(DiagnosticCode::FfiEnabled);
        set.insert(DiagnosticCode::FfiEnabled);
        set.insert(DiagnosticCode::FfiNotAllowed);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_diagnostic_default_code_is_none() {
        let d = Diagnostic::error("no code");
        assert!(d.code.is_none());
    }
}
