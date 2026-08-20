//! Shared types for host primitive operations.
//!
//! Contains data structures returned by host syscall primitives,
//! such as file metadata ([`HostStat`]) and process spawn results
//! ([`SpawnResult`]).

/// File metadata returned by [`fd_stat`](super::HostPrimitives::fd_stat).
///
/// Contains the essential fields needed for JavaScript's `fs.stat` and
/// related APIs.
#[derive(Debug, Clone)]
pub struct HostStat {
    /// File size in bytes.
    pub size: u64,

    /// Whether this entry is a regular file.
    pub is_file: bool,

    /// Whether this entry is a directory.
    pub is_dir: bool,

    /// Last modification time in milliseconds since the Unix epoch.
    pub modified_ms: f64,
}

/// Result of a synchronous process spawn via
/// [`spawn_sync`](super::HostPrimitives::spawn_sync).
///
/// Contains the exit code and captured stdout/stderr output.
#[derive(Debug, Clone)]
pub struct SpawnResult {
    /// The exit code of the spawned process.
    pub exit_code: i32,

    /// Captured standard output bytes.
    pub stdout: Vec<u8>,

    /// Captured standard error bytes.
    pub stderr: Vec<u8>,
}
