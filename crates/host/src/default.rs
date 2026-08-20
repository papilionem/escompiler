//! Default host — full implementation of all 16 MUST HAVE host primitives.
//!
//! The [`DefaultHost`] provides console output (via [`StdoutConsole`]) and
//! real implementations of all [`HostPrimitives`] methods using `std::fs`,
//! `std::process`, `std::env`, and `std::time`. Also implements the 8
//! SHOULD HAVE primitives.
//!
//! File descriptors are managed via an internal table. Fds 0, 1, 2 are
//! reserved for stdin, stdout, and stderr and handled specially.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::Host;
use crate::console::{ConsoleHost, StdoutConsole};
use crate::error::HostError;
use crate::primitives::HostPrimitives;
use crate::types::{HostStat, SpawnResult};
use nanbox::JsValue;

/// POSIX-compatible open flag: read only.
pub const O_RDONLY: u32 = 0;
/// POSIX-compatible open flag: write only.
pub const O_WRONLY: u32 = 1;
/// POSIX-compatible open flag: read and write.
pub const O_RDWR: u32 = 2;
/// POSIX-compatible open flag: create file if it does not exist.
pub const O_CREAT: u32 = 0x40;
/// POSIX-compatible open flag: truncate file to zero length.
pub const O_TRUNC: u32 = 0x200;
/// POSIX-compatible open flag: append to file.
pub const O_APPEND: u32 = 0x400;

/// Seek from the beginning of the file (POSIX SEEK_SET).
const SEEK_SET: u32 = 0;
/// Seek from the current position (POSIX SEEK_CUR).
const SEEK_CUR: u32 = 1;
/// Seek from the end of the file (POSIX SEEK_END).
const SEEK_END: u32 = 2;

/// Internal file descriptor table that maps integer fds to open file handles.
///
/// Fds 0, 1, 2 are reserved for stdin/stdout/stderr and are not stored in
/// this table — they are handled specially via `std::io::stdin()` /
/// `std::io::stdout()` / `std::io::stderr()`.
struct FdTable {
    files: HashMap<i32, File>,
    next_fd: i32,
}

impl FdTable {
    /// Create a new fd table with the next available fd set to 3.
    fn new() -> Self {
        Self {
            files: HashMap::new(),
            next_fd: 3,
        }
    }

    /// Insert a file and return the assigned fd.
    fn insert(&mut self, file: File) -> i32 {
        let fd = self.next_fd;
        self.next_fd += 1;
        self.files.insert(fd, file);
        fd
    }

    /// Remove and return the file for a given fd.
    fn remove(&mut self, fd: i32) -> Option<File> {
        self.files.remove(&fd)
    }

    /// Get a mutable reference to the file for a given fd.
    fn get_mut(&mut self, fd: i32) -> Option<&mut File> {
        self.files.get_mut(&fd)
    }

    /// Get a reference to the file for a given fd.
    fn get(&self, fd: i32) -> Option<&File> {
        self.files.get(&fd)
    }
}

/// A host with console support and real syscall-level primitive implementations.
///
/// Uses `std::fs`, `std::process`, `std::env`, and `std::time` to implement
/// the 16 MUST HAVE and 8 SHOULD HAVE host primitives. File descriptors are
/// managed via an internal table protected by a [`Mutex`] for thread safety.
pub struct DefaultHost {
    console: StdoutConsole,
    fd_table: Mutex<FdTable>,
    start_instant: Instant,
}

impl DefaultHost {
    /// Create a new default host with an empty file descriptor table.
    ///
    /// Fds 0, 1, 2 are implicitly available for stdin/stdout/stderr.
    pub fn new() -> Self {
        Self {
            console: StdoutConsole,
            fd_table: Mutex::new(FdTable::new()),
            start_instant: Instant::now(),
        }
    }
}

impl Default for DefaultHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleHost for DefaultHost {
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

impl Host for DefaultHost {
    fn name(&self) -> &str {
        "default"
    }

    fn description(&self) -> &str {
        "Default host (console + real primitives)"
    }
}

/// Convert `std::fs::Metadata` to a `HostStat`.
fn metadata_to_stat(meta: &std::fs::Metadata) -> HostStat {
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);

    HostStat {
        size: meta.len(),
        is_file: meta.is_file(),
        is_dir: meta.is_dir(),
        modified_ms,
    }
}

impl HostPrimitives for DefaultHost {
    fn fd_open(&self, path: &[u8], flags: u32, _mode: u32) -> Result<i32, HostError> {
        let path_str = std::str::from_utf8(path)
            .map_err(|e| HostError::InvalidArgument(format!("invalid UTF-8 path: {e}")))?;

        let access = flags & 0x3; // bottom 2 bits = access mode
        let mut opts = OpenOptions::new();
        match access {
            0 => {
                opts.read(true);
            }
            1 => {
                opts.write(true);
            }
            2 => {
                opts.read(true).write(true);
            }
            _ => {
                return Err(HostError::InvalidArgument(format!(
                    "invalid access mode: {access}"
                )));
            }
        }

        if flags & O_CREAT != 0 {
            opts.create(true);
        }
        if flags & O_TRUNC != 0 {
            opts.truncate(true);
        }
        if flags & O_APPEND != 0 {
            opts.append(true);
        }

        let file = opts.open(path_str)?;

        let mut table = self
            .fd_table
            .lock()
            .map_err(|e| HostError::Io(std::io::Error::other(format!("lock poisoned: {e}"))))?;
        let fd = table.insert(file);
        Ok(fd)
    }

    fn fd_read(&self, fd: i32, buf: &mut [u8]) -> Result<usize, HostError> {
        // Handle stdin specially
        if fd == 0 {
            let n = std::io::stdin().read(buf)?;
            return Ok(n);
        }

        let mut table = self
            .fd_table
            .lock()
            .map_err(|e| HostError::Io(std::io::Error::other(format!("lock poisoned: {e}"))))?;
        let file = table
            .get_mut(fd)
            .ok_or_else(|| HostError::InvalidArgument(format!("bad fd: {fd}")))?;
        let n = file.read(buf)?;
        Ok(n)
    }

    fn fd_write(&self, fd: i32, buf: &[u8]) -> Result<usize, HostError> {
        // Handle stdout/stderr specially
        match fd {
            1 => {
                let n = std::io::stdout().write(buf)?;
                std::io::stdout().flush()?;
                return Ok(n);
            }
            2 => {
                let n = std::io::stderr().write(buf)?;
                std::io::stderr().flush()?;
                return Ok(n);
            }
            _ => {}
        }

        let mut table = self
            .fd_table
            .lock()
            .map_err(|e| HostError::Io(std::io::Error::other(format!("lock poisoned: {e}"))))?;
        let file = table
            .get_mut(fd)
            .ok_or_else(|| HostError::InvalidArgument(format!("bad fd: {fd}")))?;
        let n = file.write(buf)?;
        Ok(n)
    }

    fn fd_close(&self, fd: i32) -> Result<(), HostError> {
        // Do not close stdin/stdout/stderr
        if fd < 3 {
            return Err(HostError::InvalidArgument(format!(
                "cannot close standard fd: {fd}"
            )));
        }

        let mut table = self
            .fd_table
            .lock()
            .map_err(|e| HostError::Io(std::io::Error::other(format!("lock poisoned: {e}"))))?;
        table
            .remove(fd)
            .ok_or_else(|| HostError::InvalidArgument(format!("bad fd: {fd}")))?;
        // File is dropped here, which closes the underlying OS handle
        Ok(())
    }

    fn fd_stat(&self, fd: i32) -> Result<HostStat, HostError> {
        // Handle stdin/stdout/stderr — they have no meaningful file metadata
        if fd < 3 {
            return Ok(HostStat {
                size: 0,
                is_file: false,
                is_dir: false,
                modified_ms: 0.0,
            });
        }

        let table = self
            .fd_table
            .lock()
            .map_err(|e| HostError::Io(std::io::Error::other(format!("lock poisoned: {e}"))))?;
        let file = table
            .get(fd)
            .ok_or_else(|| HostError::InvalidArgument(format!("bad fd: {fd}")))?;
        let meta = file.metadata()?;
        Ok(metadata_to_stat(&meta))
    }

    fn fd_seek(&self, fd: i32, offset: i64, whence: u32) -> Result<i64, HostError> {
        if fd < 3 {
            return Err(HostError::InvalidArgument(format!(
                "cannot seek on standard fd: {fd}"
            )));
        }

        let seek_from = match whence {
            SEEK_SET => SeekFrom::Start(offset as u64),
            SEEK_CUR => SeekFrom::Current(offset),
            SEEK_END => SeekFrom::End(offset),
            _ => {
                return Err(HostError::InvalidArgument(format!(
                    "invalid whence: {whence}"
                )));
            }
        };

        let mut table = self
            .fd_table
            .lock()
            .map_err(|e| HostError::Io(std::io::Error::other(format!("lock poisoned: {e}"))))?;
        let file = table
            .get_mut(fd)
            .ok_or_else(|| HostError::InvalidArgument(format!("bad fd: {fd}")))?;
        let pos = file.seek(seek_from)?;
        Ok(pos as i64)
    }

    fn exit(&self, code: i32) -> ! {
        std::process::exit(code)
    }

    fn args_count(&self) -> u32 {
        std::env::args().count() as u32
    }

    fn args_get(&self, index: u32) -> Result<String, HostError> {
        std::env::args()
            .nth(index as usize)
            .ok_or_else(|| HostError::InvalidArgument(format!("arg index out of range: {index}")))
    }

    fn env_get(&self, key: &str) -> Result<Option<String>, HostError> {
        match std::env::var(key) {
            Ok(val) => Ok(Some(val)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(HostError::InvalidArgument(format!(
                "env var {key} is not valid UTF-8"
            ))),
        }
    }

    fn spawn_sync(&self, cmd: &str, args: &[&str]) -> Result<SpawnResult, HostError> {
        let output = std::process::Command::new(cmd).args(args).output()?;

        Ok(SpawnResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn cwd(&self) -> Result<String, HostError> {
        let cwd = std::env::current_dir()?;
        cwd.to_str()
            .map(|s| s.to_string())
            .ok_or_else(|| HostError::InvalidArgument("cwd is not valid UTF-8".to_string()))
    }

    fn isatty(&self, fd: i32) -> bool {
        // SAFETY: `libc::isatty` is a simple query function that is safe
        // to call with any fd value. It returns 0 for non-terminal fds
        // and 1 for terminal fds, with no side effects.
        unsafe { libc::isatty(fd) != 0 }
    }

    fn now_ms(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0)
    }

    fn hrtime_ns(&self) -> u64 {
        self.start_instant.elapsed().as_nanos() as u64
    }

    fn random_bytes(&self, buf: &mut [u8]) {
        // getrandom::fill can fail on some platforms, but on Linux/macOS
        // it should always succeed for reasonable buffer sizes. We silently
        // ignore errors to match the void return type of the trait.
        let _ = getrandom::getrandom(buf);
    }

    // === SHOULD HAVE (8) ===

    fn env_set(&self, key: &str, val: &str) -> Result<(), HostError> {
        // SAFETY: `set_var` is not thread-safe in general, but compiled
        // JavaScript programs are single-threaded. The host primitives
        // are only called from the main thread during program execution.
        unsafe { std::env::set_var(key, val) };
        Ok(())
    }

    fn chdir(&self, path: &str) -> Result<(), HostError> {
        std::env::set_current_dir(path)?;
        Ok(())
    }

    fn fs_mkdir(&self, path: &str, _mode: u32) -> Result<(), HostError> {
        std::fs::create_dir_all(path)?;
        Ok(())
    }

    fn fs_readdir(&self, path: &str) -> Result<Vec<String>, HostError> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .to_str()
                .ok_or_else(|| {
                    HostError::InvalidArgument("directory entry is not valid UTF-8".to_string())
                })?
                .to_string();
            entries.push(name);
        }
        Ok(entries)
    }

    fn fs_unlink(&self, path: &str) -> Result<(), HostError> {
        std::fs::remove_file(path)?;
        Ok(())
    }

    fn fs_rename(&self, old: &str, new: &str) -> Result<(), HostError> {
        std::fs::rename(old, new)?;
        Ok(())
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    fn fs_exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }
}
