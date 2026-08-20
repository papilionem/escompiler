//! Driver error types aggregating errors from all compilation phases.

use thiserror::Error;

/// Errors that can occur during the compilation pipeline.
#[derive(Debug, Error)]
pub enum DriverError {
    /// An I/O error occurred (reading files, writing objects, etc.).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The parser encountered a syntax error.
    #[error("parse error: {0}")]
    Parse(String),

    /// One or more errors occurred during AST-to-IR lowering.
    #[error("lowering errors: {}", .0.join("; "))]
    Lowering(Vec<String>),

    /// One or more IR verification errors were detected.
    #[error("verification errors: {}", .0.join("; "))]
    Verification(Vec<String>),

    /// Code generation (Cranelift/LLVM) failed.
    #[error("codegen error: {0}")]
    Codegen(String),

    /// The system linker failed.
    #[error("linker error: {0}")]
    Linker(#[from] linker::LinkerError),

    /// No input files were provided.
    #[error("no input files")]
    NoInput,

    /// The specified input file was not found on disk.
    #[error("input file not found: {0}")]
    FileNotFound(String),

    /// FFI usage detected but `--allow-ffi` was not passed (ESC-E700).
    #[error("ESC-E700: FFI usage requires --allow-ffi flag or permissions.allowFfi in esc.json")]
    FfiNotAllowed,

    /// `eval()` or `Function()` usage detected while `--no-eval` is active (ESC-E400).
    #[error(
        "ESC-E400: eval() and Function() are disabled (--no-eval). Remove dynamic code execution or enable with --allow-eval"
    )]
    EvalDisabled,

    /// `eval()` or `Function()` usage detected while `--no-jit` is active (ESC-E401).
    #[error(
        "ESC-E401: JIT compilation is disabled (--no-jit). Code using eval()/Function() requires JIT. Remove dynamic code or enable with --allow-jit"
    )]
    JitDisabled,
    /// LLVM backend is not available — the `llvm` cargo feature was not enabled.
    /// Use `cargo install esc --features llvm` or recompile with `--features llvm`.
    #[error(
        "ESC-E601: --release requires the LLVM backend, which is not enabled. \
         Rebuild with `--features llvm` or use the default (Cranelift) backend."
    )]
    LlvmNotAvailable,

    /// The compiler deliberately declined to compile the program, with a reason.
    ///
    /// Distinct from every other variant: those mean *the compiler could not do
    /// its job*, this means *it will not, and says exactly why*. Exits **2**, not
    /// 1, so a caller can tell "this feature does not exist yet" apart from "your
    /// program is broken".
    #[error("{}", .0.join("\n"))]
    Refused(Vec<String>),
}

impl DriverError {
    /// The process exit status this error should produce.
    ///
    /// `2` for a deliberate refusal, `1` for everything else. The sealed v0.9
    /// rung requires refusal and failure-to-compile to stay distinguishable,
    /// since both otherwise exit 1 and a corpus entry cannot assert which
    /// happened.
    pub fn exit_code(&self) -> i32 {
        match self {
            DriverError::Refused(_) => 2,
            _ => 1,
        }
    }
}

impl From<DriverError> for Vec<common::CompileError> {
    fn from(err: DriverError) -> Self {
        vec![common::CompileError::Internal {
            message: err.to_string(),
        }]
    }
}
