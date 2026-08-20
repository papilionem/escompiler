//! Host primitives trait defining the minimum syscall-level operations.
//!
//! The [`HostPrimitives`] trait declares 24 methods covering I/O, process
//! management, system queries, time, and randomness. These are the ONLY
//! operations implemented in Rust — everything else belongs in the JS/TS
//! standard library layer.

use crate::error::HostError;
use crate::types::{HostStat, SpawnResult};

/// Trait defining the minimum syscall-level primitives that JS/TS cannot
/// implement natively.
///
/// These are the ONLY things implemented in Rust. Everything higher-level
/// (e.g., `fs.readFile`, `path.join`, `Buffer`) belongs in the JS/TS
/// standard library layer compiled by the AOT compiler.
///
/// The 24 methods are grouped into six categories:
///
/// - **I/O (6):** File descriptor operations (`fd_open`, `fd_read`, `fd_write`,
///   `fd_close`, `fd_stat`, `fd_seek`)
/// - **Process (5):** Process lifecycle and environment (`exit`, `args_count`,
///   `args_get`, `env_get`, `spawn_sync`)
/// - **System (2):** OS queries (`cwd`, `isatty`)
/// - **Time (2):** Clock access (`now_ms`, `hrtime_ns`)
/// - **Random (1):** Cryptographic randomness (`random_bytes`)
/// - **Should-have (8):** Additional filesystem and environment operations
///   (`env_set`, `chdir`, `fs_mkdir`, `fs_readdir`, `fs_unlink`, `fs_rename`,
///   `sleep_ms`, `fs_exists`)
pub trait HostPrimitives {
    // === I/O (6) -- MUST HAVE ===

    /// Open a file by path and return a file descriptor.
    ///
    /// `flags` and `mode` follow POSIX semantics (O_RDONLY, O_WRONLY, etc.).
    fn fd_open(&self, path: &[u8], flags: u32, mode: u32) -> Result<i32, HostError>;

    /// Read bytes from a file descriptor into `buf`.
    ///
    /// Returns the number of bytes actually read, which may be less than
    /// `buf.len()` (partial reads are allowed).
    fn fd_read(&self, fd: i32, buf: &mut [u8]) -> Result<usize, HostError>;

    /// Write bytes from `buf` to a file descriptor.
    ///
    /// Returns the number of bytes actually written.
    fn fd_write(&self, fd: i32, buf: &[u8]) -> Result<usize, HostError>;

    /// Close a file descriptor.
    fn fd_close(&self, fd: i32) -> Result<(), HostError>;

    /// Retrieve metadata for a file descriptor.
    fn fd_stat(&self, fd: i32) -> Result<HostStat, HostError>;

    /// Seek to a position in a file descriptor.
    ///
    /// `whence` follows POSIX semantics: 0 = SEEK_SET, 1 = SEEK_CUR,
    /// 2 = SEEK_END. Returns the new absolute position.
    fn fd_seek(&self, fd: i32, offset: i64, whence: u32) -> Result<i64, HostError>;

    // === Process (5) -- MUST HAVE ===

    /// Terminate the process with the given exit code.
    fn exit(&self, code: i32) -> !;

    /// Return the number of command-line arguments.
    fn args_count(&self) -> u32;

    /// Return the command-line argument at `index`.
    ///
    /// Returns `HostError::InvalidArgument` if `index` is out of range.
    fn args_get(&self, index: u32) -> Result<String, HostError>;

    /// Look up an environment variable by key.
    ///
    /// Returns `Ok(None)` if the variable is not set (as opposed to
    /// `HostError::NotFound`, which is reserved for filesystem entities).
    fn env_get(&self, key: &str) -> Result<Option<String>, HostError>;

    /// Spawn a child process synchronously and wait for it to complete.
    ///
    /// Returns the exit code and captured stdout/stderr.
    fn spawn_sync(&self, cmd: &str, args: &[&str]) -> Result<SpawnResult, HostError>;

    // === System (2) -- MUST HAVE ===

    /// Return the current working directory as a string.
    fn cwd(&self) -> Result<String, HostError>;

    /// Test whether a file descriptor refers to a terminal (TTY).
    fn isatty(&self, fd: i32) -> bool;

    // === Time (2) -- MUST HAVE ===

    /// Return the current wall-clock time in milliseconds since the Unix epoch.
    ///
    /// This corresponds to JavaScript's `Date.now()`.
    fn now_ms(&self) -> f64;

    /// Return a high-resolution monotonic timestamp in nanoseconds.
    ///
    /// This corresponds to Node.js's `process.hrtime.bigint()`.
    fn hrtime_ns(&self) -> u64;

    // === Random (1) -- MUST HAVE ===

    /// Fill `buf` with cryptographically secure random bytes.
    fn random_bytes(&self, buf: &mut [u8]);

    // === SHOULD HAVE (8) ===

    /// Set an environment variable.
    fn env_set(&self, key: &str, val: &str) -> Result<(), HostError>;

    /// Change the current working directory.
    fn chdir(&self, path: &str) -> Result<(), HostError>;

    /// Create a directory at `path` with the given permission mode.
    fn fs_mkdir(&self, path: &str, mode: u32) -> Result<(), HostError>;

    /// List the entries in a directory.
    fn fs_readdir(&self, path: &str) -> Result<Vec<String>, HostError>;

    /// Remove a file.
    fn fs_unlink(&self, path: &str) -> Result<(), HostError>;

    /// Rename/move a file or directory.
    fn fs_rename(&self, old: &str, new: &str) -> Result<(), HostError>;

    /// Sleep for `ms` milliseconds.
    fn sleep_ms(&self, ms: u64);

    /// Check whether a path exists.
    fn fs_exists(&self, path: &str) -> bool;
}
