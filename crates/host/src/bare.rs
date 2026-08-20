//! Bare host — console output only, no timers, no filesystem, no network.
//!
//! The [`BareHost`] is the simplest possible host environment, providing only
//! console output via stdout/stderr. All [`HostPrimitives`] methods return
//! [`HostError::NotSupported`]. Useful for testing, embedded environments,
//! or situations where no I/O beyond stdout/stderr is needed.

use crate::Host;
use crate::console::{ConsoleHost, StdoutConsole};
use crate::error::HostError;
use crate::primitives::HostPrimitives;
use crate::types::{HostStat, SpawnResult};
use nanbox::JsValue;

/// A minimal host that only provides console output.
///
/// All [`HostPrimitives`] methods return [`HostError::NotSupported`].
/// Useful for testing, embedded environments, or situations where
/// no I/O beyond stdout/stderr is needed.
pub struct BareHost {
    console: StdoutConsole,
}

impl BareHost {
    /// Create a new bare host.
    pub fn new() -> Self {
        Self {
            console: StdoutConsole,
        }
    }
}

impl Default for BareHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleHost for BareHost {
    fn log(&self, args: &[JsValue]) {
        self.console.log(args);
    }

    fn error(&self, args: &[JsValue]) {
        self.console.error(args);
    }

    fn warn(&self, args: &[JsValue]) {
        self.console.warn(args);
    }

    fn debug(&self, args: &[JsValue]) {
        self.console.debug(args);
    }
}

impl Host for BareHost {
    fn name(&self) -> &str {
        "bare"
    }

    fn description(&self) -> &str {
        "Bare host (console only)"
    }
}

impl HostPrimitives for BareHost {
    fn fd_open(&self, _path: &[u8], _flags: u32, _mode: u32) -> Result<i32, HostError> {
        Err(HostError::NotSupported("fd_open".to_string()))
    }

    fn fd_read(&self, _fd: i32, _buf: &mut [u8]) -> Result<usize, HostError> {
        Err(HostError::NotSupported("fd_read".to_string()))
    }

    fn fd_write(&self, _fd: i32, _buf: &[u8]) -> Result<usize, HostError> {
        Err(HostError::NotSupported("fd_write".to_string()))
    }

    fn fd_close(&self, _fd: i32) -> Result<(), HostError> {
        Err(HostError::NotSupported("fd_close".to_string()))
    }

    fn fd_stat(&self, _fd: i32) -> Result<HostStat, HostError> {
        Err(HostError::NotSupported("fd_stat".to_string()))
    }

    fn fd_seek(&self, _fd: i32, _offset: i64, _whence: u32) -> Result<i64, HostError> {
        Err(HostError::NotSupported("fd_seek".to_string()))
    }

    fn exit(&self, code: i32) -> ! {
        std::process::exit(code)
    }

    fn args_count(&self) -> u32 {
        0
    }

    fn args_get(&self, _index: u32) -> Result<String, HostError> {
        Err(HostError::NotSupported("args_get".to_string()))
    }

    fn env_get(&self, _key: &str) -> Result<Option<String>, HostError> {
        Err(HostError::NotSupported("env_get".to_string()))
    }

    fn spawn_sync(&self, _cmd: &str, _args: &[&str]) -> Result<SpawnResult, HostError> {
        Err(HostError::NotSupported("spawn_sync".to_string()))
    }

    fn cwd(&self) -> Result<String, HostError> {
        Err(HostError::NotSupported("cwd".to_string()))
    }

    fn isatty(&self, _fd: i32) -> bool {
        false
    }

    fn now_ms(&self) -> f64 {
        0.0
    }

    fn hrtime_ns(&self) -> u64 {
        0
    }

    fn random_bytes(&self, _buf: &mut [u8]) {
        // Bare host does not support randomness
    }

    fn env_set(&self, _key: &str, _val: &str) -> Result<(), HostError> {
        Err(HostError::NotSupported("env_set".to_string()))
    }

    fn chdir(&self, _path: &str) -> Result<(), HostError> {
        Err(HostError::NotSupported("chdir".to_string()))
    }

    fn fs_mkdir(&self, _path: &str, _mode: u32) -> Result<(), HostError> {
        Err(HostError::NotSupported("fs_mkdir".to_string()))
    }

    fn fs_readdir(&self, _path: &str) -> Result<Vec<String>, HostError> {
        Err(HostError::NotSupported("fs_readdir".to_string()))
    }

    fn fs_unlink(&self, _path: &str) -> Result<(), HostError> {
        Err(HostError::NotSupported("fs_unlink".to_string()))
    }

    fn fs_rename(&self, _old: &str, _new: &str) -> Result<(), HostError> {
        Err(HostError::NotSupported("fs_rename".to_string()))
    }

    fn sleep_ms(&self, _ms: u64) {
        // Bare host does not support sleeping
    }

    fn fs_exists(&self, _path: &str) -> bool {
        false
    }
}
