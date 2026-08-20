//! Linker error types.

use thiserror::Error;

/// Errors that can occur during the linking phase.
#[derive(Debug, Error)]
pub enum LinkerError {
    /// No system linker (cc, gcc, clang) was found on PATH.
    #[error("no system linker found (tried: cc, gcc, clang)")]
    NoLinkerFound,

    /// The system linker process exited with a non-zero status.
    #[error("linker failed with exit code {code}: {stderr}")]
    LinkFailed {
        /// The exit code returned by the linker process.
        code: i32,
        /// Captured stderr output from the linker.
        stderr: String,
    },

    /// No object files were provided to the linker.
    #[error("no object files provided")]
    NoObjects,

    /// An I/O error occurred during linking.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// No output path was specified in the linker configuration.
    #[error("output path not specified")]
    NoOutputPath,
}
