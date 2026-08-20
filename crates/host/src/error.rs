//! Host error types for syscall-level primitive operations.
//!
//! [`HostError`] represents failures that can occur when JavaScript code
//! interacts with the underlying operating system through host primitives.

/// Error type for host primitive operations.
///
/// Covers the standard failure modes for syscall-level operations:
/// I/O errors, missing features, permission issues, and invalid arguments.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// An I/O error occurred during the operation.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The operation is not supported by this host implementation.
    #[error("not supported: {0}")]
    NotSupported(String),

    /// The operation was denied due to insufficient permissions.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// The requested resource was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// An invalid argument was provided to the operation.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}
