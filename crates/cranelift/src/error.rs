//! Error types for the Cranelift code generation backend.

use thiserror::Error;

/// Errors that can occur during Cranelift code generation.
#[derive(Debug, Error)]
pub enum CodegenError {
    /// An error from the Cranelift module layer (e.g., defining or declaring
    /// functions/data).
    #[error("cranelift module error: {0}")]
    Module(String),

    /// An error from the Cranelift codegen layer (e.g., verifier, register
    /// allocation).
    #[error("cranelift codegen error: {0}")]
    Codegen(String),

    /// A reference to an IR value that was not previously defined.
    #[error("undefined value: v{0}")]
    UndefinedValue(u32),

    /// An IR type that cannot be mapped to a Cranelift type.
    #[error("unsupported type: {0}")]
    UnsupportedType(String),

    /// The module has no entry function set.
    #[error("no entry function in module")]
    NoEntryFunction,

    /// ISA lookup or creation failed.
    #[error("ISA error: {0}")]
    Isa(String),
}

impl From<cranelift_module::ModuleError> for CodegenError {
    fn from(e: cranelift_module::ModuleError) -> Self {
        CodegenError::Module(e.to_string())
    }
}

impl From<cranelift_codegen::CodegenError> for CodegenError {
    fn from(e: cranelift_codegen::CodegenError) -> Self {
        CodegenError::Codegen(e.to_string())
    }
}
