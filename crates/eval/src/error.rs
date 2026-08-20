//! Error types for the eval JIT compiler.

use thiserror::Error;

/// Errors that can occur during eval JIT compilation and execution.
#[derive(Debug, Error)]
pub enum EvalError {
    /// A parse error occurred while parsing the eval source.
    #[error("parse error: {message}")]
    Parse {
        /// Description of the parse error.
        message: String,
    },

    /// An error occurred during IR lowering.
    #[error("lowering error: {message}")]
    Lowering {
        /// Description of the lowering error.
        message: String,
    },

    /// An error occurred during JIT compilation.
    #[error("JIT compilation error: {message}")]
    Jit {
        /// Description of the JIT error.
        message: String,
    },

    /// A runtime error occurred during eval execution.
    #[error("runtime error: {message}")]
    Runtime {
        /// Description of the runtime error.
        message: String,
    },
}
