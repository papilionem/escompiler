//! Error types for the LLVM code generation backend.

use thiserror::Error;

/// Errors that can occur during LLVM code generation.
#[derive(Debug, Error)]
pub enum LlvmCodegenError {
    /// An error from LLVM module verification or compilation.
    #[error("llvm module error: {0}")]
    Module(String),

    /// A reference to an IR value that was not previously defined.
    #[error("undefined value: v{0}")]
    UndefinedValue(u32),

    /// An IR type that cannot be mapped to an LLVM type.
    #[error("unsupported type: {0}")]
    UnsupportedType(String),

    /// The module has no entry function set.
    #[error("no entry function in module")]
    NoEntryFunction,

    /// Target machine creation or configuration error.
    #[error("target error: {0}")]
    Target(String),

    /// An unsupported opcode was encountered during lowering.
    #[error("unsupported opcode: {0}")]
    UnsupportedOpcode(String),

    /// LLVM failed to write the object file.
    #[error("object file write error: {0}")]
    ObjectWrite(String),
}
