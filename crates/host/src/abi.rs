//! ABI layer — `extern "C"` wrapper functions for host primitives.
//!
//! Provides C-callable entry points that delegate to a thread-local
//! [`Host`](crate::Host) instance. These are the symbols that compiled
//! JavaScript binaries link against at runtime.
//!
//! All functions follow the `__esc_host_` naming convention to avoid
//! symbol collisions with user code.
//!
//! Permission checks are performed before gated operations. When a
//! permission is denied, the function returns -1 (or 0 for boolean
//! returns) and prints the ESC-E701 error to stderr.

use std::cell::RefCell;
use std::slice;

use crate::default::DefaultHost;
use crate::permissions::{self, PermissionKind};
use crate::primitives::HostPrimitives;

thread_local! {
    /// Thread-local host instance used by ABI wrapper functions.
    ///
    /// Defaults to [`DefaultHost`] which provides real implementations
    /// of all 16 MUST HAVE primitives.
    static HOST: RefCell<DefaultHost> = RefCell::new(DefaultHost::new());
}

/// Helper to convert a raw pointer + length to a `&[u8]`, returning `None`
/// if the pointer is null.
///
/// # Safety
///
/// The caller must ensure that if `ptr` is non-null, it points to at least
/// `len` valid bytes.
unsafe fn raw_to_slice<'a>(ptr: *const u8, len: u32) -> Option<&'a [u8]> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees ptr points to len valid bytes when non-null
    Some(unsafe { slice::from_raw_parts(ptr, len as usize) })
}

/// Helper to convert a raw pointer + length to a `&mut [u8]`, returning
/// `None` if the pointer is null.
///
/// # Safety
///
/// The caller must ensure that if `ptr` is non-null, it points to at least
/// `len` valid, writable bytes with no aliasing.
unsafe fn raw_to_slice_mut<'a>(ptr: *mut u8, len: u32) -> Option<&'a mut [u8]> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees ptr points to len valid writable bytes
    Some(unsafe { slice::from_raw_parts_mut(ptr, len as usize) })
}

/// Check a permission and print the ESC-E701 error to stderr on denial.
///
/// Returns `true` if the permission is granted, `false` if denied.
fn check_and_report(kind: PermissionKind, resource: &str) -> bool {
    if let Err(e) = permissions::check_permission(kind, resource) {
        eprintln!("{e}");
        return false;
    }
    true
}

/// O_RDONLY flag constant (POSIX).
const O_RDONLY: u32 = 0;
/// O_WRONLY flag constant (POSIX).
const O_WRONLY: u32 = 1;

// =========================================================================
// Runtime permission check entry point
// =========================================================================

/// Runtime permission check callable from compiled code.
///
/// `kind` maps to:
/// - 0 = Read
/// - 1 = Write
/// - 2 = Net
/// - 3 = Env
/// - 4 = Run
///
/// Returns 1 if permission is granted, 0 if denied. On denial, the
/// ESC-E701 error message is printed to stderr.
///
/// # Safety
///
/// `resource` must point to `resource_len` valid bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_rt_check_permission(
    kind: u32,
    resource: *const u8,
    resource_len: u32,
) -> i32 {
    let perm_kind = match kind {
        0 => PermissionKind::Read,
        1 => PermissionKind::Write,
        2 => PermissionKind::Net,
        3 => PermissionKind::Env,
        4 => PermissionKind::Run,
        _ => return 0,
    };

    // SAFETY: caller guarantees resource validity per function contract
    let resource_str = if resource.is_null() || resource_len == 0 {
        ""
    } else {
        let Some(bytes) = (unsafe { raw_to_slice(resource, resource_len) }) else {
            return 0;
        };
        match std::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    if check_and_report(perm_kind, resource_str) {
        1
    } else {
        0
    }
}

/// Initialise the runtime permission table from compiled-in configuration.
///
/// Called once at program startup before any gated operations.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_init_permissions(
    allow_read: i32,
    allow_write: i32,
    allow_net: i32,
    allow_env: i32,
    allow_run: i32,
) {
    use crate::permissions::{PermissionValue, PermissionsConfig};

    let to_perm = |flag: i32| -> PermissionValue {
        if flag > 0 {
            PermissionValue::Granted
        } else {
            PermissionValue::Denied
        }
    };

    let config = PermissionsConfig {
        allow_read: to_perm(allow_read),
        allow_write: to_perm(allow_write),
        allow_net: to_perm(allow_net),
        allow_env: to_perm(allow_env),
        allow_run: to_perm(allow_run),
    };

    permissions::init_permissions(config);
}

// =========================================================================
// MUST HAVE primitives (16)
// =========================================================================

/// Open a file by path and return a file descriptor.
///
/// Checks read or write permission based on `flags` before opening.
/// Returns the fd on success, or -1 on error (null pointer, invalid UTF-8,
/// I/O failure, or permission denied).
///
/// # Safety
///
/// `path` must point to `path_len` valid bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_fd_open(
    path: *const u8,
    path_len: u32,
    flags: u32,
    mode: u32,
) -> i32 {
    // SAFETY: caller guarantees path validity per function contract
    let Some(path_bytes) = (unsafe { raw_to_slice(path, path_len) }) else {
        return -1;
    };

    // Check permissions based on open flags
    if let Ok(path_str) = std::str::from_utf8(path_bytes) {
        let is_write = (flags & O_WRONLY) != 0 || (flags & 2) != 0; // O_WRONLY or O_RDWR
        let is_read = flags == O_RDONLY || (flags & 2) != 0; // O_RDONLY or O_RDWR

        if is_read && !check_and_report(PermissionKind::Read, path_str) {
            return -1;
        }
        if is_write && !check_and_report(PermissionKind::Write, path_str) {
            return -1;
        }
    }

    HOST.with(|h| h.borrow().fd_open(path_bytes, flags, mode).unwrap_or(-1))
}

/// Read bytes from a file descriptor.
///
/// Returns the number of bytes read, or -1 on error.
///
/// # Safety
///
/// `buf` must point to `buf_len` valid writable bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_fd_read(fd: i32, buf: *mut u8, buf_len: u32) -> i32 {
    // SAFETY: caller guarantees buf validity per function contract
    let Some(buf_slice) = (unsafe { raw_to_slice_mut(buf, buf_len) }) else {
        return -1;
    };

    HOST.with(|h| {
        h.borrow()
            .fd_read(fd, buf_slice)
            .map(|n| n as i32)
            .unwrap_or(-1)
    })
}

/// Write bytes to a file descriptor.
///
/// Checks write permission for non-stdout/stderr file descriptors.
/// Returns the number of bytes written, or -1 on error.
///
/// # Safety
///
/// `buf` must point to `buf_len` valid bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_fd_write(fd: i32, buf: *const u8, buf_len: u32) -> i32 {
    // SAFETY: caller guarantees buf validity per function contract
    let Some(buf_slice) = (unsafe { raw_to_slice(buf, buf_len) }) else {
        return -1;
    };

    // stdout (1) and stderr (2) are always allowed
    if fd != 1 && fd != 2 && !check_and_report(PermissionKind::Write, &format!("fd:{fd}")) {
        return -1;
    }

    HOST.with(|h| {
        h.borrow()
            .fd_write(fd, buf_slice)
            .map(|n| n as i32)
            .unwrap_or(-1)
    })
}

/// Close a file descriptor.
///
/// Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_host_fd_close(fd: i32) -> i32 {
    HOST.with(|h| {
        if h.borrow().fd_close(fd).is_ok() {
            0
        } else {
            -1
        }
    })
}

/// Get the size of a file descriptor's underlying file.
///
/// Returns the file size in bytes, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_host_fd_stat_size(fd: i32) -> i64 {
    HOST.with(|h| h.borrow().fd_stat(fd).map(|s| s.size as i64).unwrap_or(-1))
}

/// Seek to a position in a file descriptor.
///
/// Returns the new absolute position, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_host_fd_seek(fd: i32, offset: i64, whence: u32) -> i64 {
    HOST.with(|h| h.borrow().fd_seek(fd, offset, whence).unwrap_or(-1))
}

/// Terminate the process with the given exit code.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_host_exit(code: i32) -> ! {
    HOST.with(|h| h.borrow().exit(code))
}

/// Return the number of command-line arguments.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_host_args_count() -> u32 {
    HOST.with(|h| h.borrow().args_count())
}

/// Get a command-line argument by index.
///
/// Writes the argument string to `buf` (up to `buf_len` bytes).
/// Returns the actual byte length of the argument, or -1 on error.
/// If the return value exceeds `buf_len`, the string was truncated.
///
/// # Safety
///
/// `buf` must point to `buf_len` valid writable bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_args_get(index: u32, buf: *mut u8, buf_len: u32) -> i32 {
    HOST.with(|h| {
        let Ok(arg) = h.borrow().args_get(index) else {
            return -1;
        };
        let arg_bytes = arg.as_bytes();

        if !buf.is_null() && buf_len > 0 {
            let copy_len = arg_bytes.len().min(buf_len as usize);
            // SAFETY: caller guarantees buf validity per function contract
            unsafe {
                std::ptr::copy_nonoverlapping(arg_bytes.as_ptr(), buf, copy_len);
            }
        }

        arg_bytes.len() as i32
    })
}

/// Look up an environment variable by key.
///
/// Checks env permission before accessing the variable.
/// Writes the value to `buf` (up to `buf_len` bytes).
/// Returns the actual byte length of the value, or -1 if the variable
/// is not set or an error occurs.
///
/// # Safety
///
/// - `key` must point to `key_len` valid bytes if non-null.
/// - `buf` must point to `buf_len` valid writable bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_env_get(
    key: *const u8,
    key_len: u32,
    buf: *mut u8,
    buf_len: u32,
) -> i32 {
    // SAFETY: caller guarantees key validity per function contract
    let Some(key_bytes) = (unsafe { raw_to_slice(key, key_len) }) else {
        return -1;
    };
    let Ok(key_str) = std::str::from_utf8(key_bytes) else {
        return -1;
    };

    // Check env permission
    if !check_and_report(PermissionKind::Env, key_str) {
        return -1;
    }

    HOST.with(|h| {
        let Ok(Some(val)) = h.borrow().env_get(key_str) else {
            return -1;
        };
        let val_bytes = val.as_bytes();

        if !buf.is_null() && buf_len > 0 {
            let copy_len = val_bytes.len().min(buf_len as usize);
            // SAFETY: caller guarantees buf validity per function contract
            unsafe {
                std::ptr::copy_nonoverlapping(val_bytes.as_ptr(), buf, copy_len);
            }
        }

        val_bytes.len() as i32
    })
}

/// Get the current working directory.
///
/// Writes the path to `buf` (up to `buf_len` bytes).
/// Returns the actual byte length, or -1 on error.
///
/// # Safety
///
/// `buf` must point to `buf_len` valid writable bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_cwd(buf: *mut u8, buf_len: u32) -> i32 {
    HOST.with(|h| {
        let Ok(cwd) = h.borrow().cwd() else {
            return -1;
        };
        let cwd_bytes = cwd.as_bytes();

        if !buf.is_null() && buf_len > 0 {
            let copy_len = cwd_bytes.len().min(buf_len as usize);
            // SAFETY: caller guarantees buf validity per function contract
            unsafe {
                std::ptr::copy_nonoverlapping(cwd_bytes.as_ptr(), buf, copy_len);
            }
        }

        cwd_bytes.len() as i32
    })
}

/// Test whether a file descriptor refers to a terminal.
///
/// Returns 1 if it is a TTY, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_host_isatty(fd: i32) -> i32 {
    HOST.with(|h| if h.borrow().isatty(fd) { 1 } else { 0 })
}

/// Return the current wall-clock time in milliseconds since the Unix epoch.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_host_now_ms() -> f64 {
    HOST.with(|h| h.borrow().now_ms())
}

/// Return a high-resolution monotonic timestamp in nanoseconds.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_host_hrtime_ns() -> u64 {
    HOST.with(|h| h.borrow().hrtime_ns())
}

/// Fill a buffer with cryptographically secure random bytes.
///
/// # Safety
///
/// `buf` must point to `buf_len` valid writable bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_random_bytes(buf: *mut u8, buf_len: u32) {
    // SAFETY: caller guarantees buf validity per function contract
    let Some(buf_slice) = (unsafe { raw_to_slice_mut(buf, buf_len) }) else {
        return;
    };

    HOST.with(|h| h.borrow().random_bytes(buf_slice));
}

// =========================================================================
// SHOULD HAVE primitives (8)
// =========================================================================

/// Set an environment variable.
///
/// Checks env permission before setting.
/// Returns 0 on success, -1 on error.
///
/// # Safety
///
/// - `key` must point to `key_len` valid bytes if non-null.
/// - `val` must point to `val_len` valid bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_env_set(
    key: *const u8,
    key_len: u32,
    val: *const u8,
    val_len: u32,
) -> i32 {
    // SAFETY: caller guarantees key/val validity per function contract
    let Some(key_bytes) = (unsafe { raw_to_slice(key, key_len) }) else {
        return -1;
    };
    let Ok(key_str) = std::str::from_utf8(key_bytes) else {
        return -1;
    };
    let Some(val_bytes) = (unsafe { raw_to_slice(val, val_len) }) else {
        return -1;
    };
    let Ok(val_str) = std::str::from_utf8(val_bytes) else {
        return -1;
    };

    // Check env permission
    if !check_and_report(PermissionKind::Env, key_str) {
        return -1;
    }

    HOST.with(|h| {
        if h.borrow().env_set(key_str, val_str).is_ok() {
            0
        } else {
            -1
        }
    })
}

/// Change the current working directory.
///
/// Returns 0 on success, -1 on error.
///
/// # Safety
///
/// `path` must point to `path_len` valid bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_chdir(path: *const u8, path_len: u32) -> i32 {
    // SAFETY: caller guarantees path validity per function contract
    let Some(path_bytes) = (unsafe { raw_to_slice(path, path_len) }) else {
        return -1;
    };
    let Ok(path_str) = std::str::from_utf8(path_bytes) else {
        return -1;
    };

    HOST.with(|h| {
        if h.borrow().chdir(path_str).is_ok() {
            0
        } else {
            -1
        }
    })
}

/// Create a directory (and all parent directories) at the given path.
///
/// Checks write permission before creating.
/// Returns 0 on success, -1 on error.
///
/// # Safety
///
/// `path` must point to `path_len` valid bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_fs_mkdir(path: *const u8, path_len: u32, mode: u32) -> i32 {
    // SAFETY: caller guarantees path validity per function contract
    let Some(path_bytes) = (unsafe { raw_to_slice(path, path_len) }) else {
        return -1;
    };
    let Ok(path_str) = std::str::from_utf8(path_bytes) else {
        return -1;
    };

    // Check write permission for directory creation
    if !check_and_report(PermissionKind::Write, path_str) {
        return -1;
    }

    HOST.with(|h| {
        if h.borrow().fs_mkdir(path_str, mode).is_ok() {
            0
        } else {
            -1
        }
    })
}

/// List entries in a directory.
///
/// Checks read permission before listing.
/// Writes a null-separated list of entry names to `buf` (up to `buf_len` bytes).
/// Returns the total byte length of the result (including null separators),
/// or -1 on error. If the return value exceeds `buf_len`, the output was
/// truncated.
///
/// # Safety
///
/// - `path` must point to `path_len` valid bytes if non-null.
/// - `buf` must point to `buf_len` valid writable bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_fs_readdir(
    path: *const u8,
    path_len: u32,
    buf: *mut u8,
    buf_len: u32,
) -> i32 {
    // SAFETY: caller guarantees path/buf validity per function contract
    let Some(path_bytes) = (unsafe { raw_to_slice(path, path_len) }) else {
        return -1;
    };
    let Ok(path_str) = std::str::from_utf8(path_bytes) else {
        return -1;
    };

    // Check read permission for directory listing
    if !check_and_report(PermissionKind::Read, path_str) {
        return -1;
    }

    HOST.with(|h| {
        let Ok(entries) = h.borrow().fs_readdir(path_str) else {
            return -1;
        };

        // Build null-separated string
        let joined: Vec<u8> = entries
            .iter()
            .enumerate()
            .flat_map(|(i, name)| {
                let bytes = name.as_bytes();
                if i + 1 < entries.len() {
                    let mut v = bytes.to_vec();
                    v.push(0);
                    v
                } else {
                    bytes.to_vec()
                }
            })
            .collect();

        let total_len = joined.len();

        if !buf.is_null() && buf_len > 0 {
            let copy_len = total_len.min(buf_len as usize);
            // SAFETY: caller guarantees buf validity per function contract
            unsafe {
                std::ptr::copy_nonoverlapping(joined.as_ptr(), buf, copy_len);
            }
        }

        total_len as i32
    })
}

/// Remove a file at the given path.
///
/// Checks write permission before removing.
/// Returns 0 on success, -1 on error.
///
/// # Safety
///
/// `path` must point to `path_len` valid bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_fs_unlink(path: *const u8, path_len: u32) -> i32 {
    // SAFETY: caller guarantees path validity per function contract
    let Some(path_bytes) = (unsafe { raw_to_slice(path, path_len) }) else {
        return -1;
    };
    let Ok(path_str) = std::str::from_utf8(path_bytes) else {
        return -1;
    };

    // Check write permission for file deletion
    if !check_and_report(PermissionKind::Write, path_str) {
        return -1;
    }

    HOST.with(|h| {
        if h.borrow().fs_unlink(path_str).is_ok() {
            0
        } else {
            -1
        }
    })
}

/// Rename or move a file or directory.
///
/// Checks write permission for both source and destination.
/// Returns 0 on success, -1 on error.
///
/// # Safety
///
/// - `old_path` must point to `old_len` valid bytes if non-null.
/// - `new_path` must point to `new_len` valid bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_fs_rename(
    old_path: *const u8,
    old_len: u32,
    new_path: *const u8,
    new_len: u32,
) -> i32 {
    // SAFETY: caller guarantees path validity per function contract
    let Some(old_bytes) = (unsafe { raw_to_slice(old_path, old_len) }) else {
        return -1;
    };
    let Ok(old_str) = std::str::from_utf8(old_bytes) else {
        return -1;
    };
    let Some(new_bytes) = (unsafe { raw_to_slice(new_path, new_len) }) else {
        return -1;
    };
    let Ok(new_str) = std::str::from_utf8(new_bytes) else {
        return -1;
    };

    // Check write permission for both paths
    if !check_and_report(PermissionKind::Write, old_str) {
        return -1;
    }
    if !check_and_report(PermissionKind::Write, new_str) {
        return -1;
    }

    HOST.with(|h| {
        if h.borrow().fs_rename(old_str, new_str).is_ok() {
            0
        } else {
            -1
        }
    })
}

/// Sleep for the given number of milliseconds.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_host_sleep_ms(ms: u64) {
    HOST.with(|h| h.borrow().sleep_ms(ms));
}

/// Check whether a path exists.
///
/// Checks read permission before checking existence.
/// Returns 1 if the path exists, 0 otherwise.
///
/// # Safety
///
/// `path` must point to `path_len` valid bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_fs_exists(path: *const u8, path_len: u32) -> i32 {
    // SAFETY: caller guarantees path validity per function contract
    let Some(path_bytes) = (unsafe { raw_to_slice(path, path_len) }) else {
        return 0;
    };
    let Ok(path_str) = std::str::from_utf8(path_bytes) else {
        return 0;
    };

    // Check read permission for existence check
    if !check_and_report(PermissionKind::Read, path_str) {
        return 0;
    }

    HOST.with(|h| if h.borrow().fs_exists(path_str) { 1 } else { 0 })
}

/// Spawn a child process synchronously.
///
/// Checks run permission before spawning.
/// Returns the exit code, or -1 on error.
///
/// # Safety
///
/// - `cmd` must point to `cmd_len` valid bytes if non-null.
/// - `args_buf` must point to `args_len` valid bytes if non-null
///   (null-separated argument strings).
/// - `stdout_buf` and `stderr_buf` must point to their respective
///   `*_buf_len` valid writable bytes if non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_host_spawn_sync(
    cmd: *const u8,
    cmd_len: u32,
    args_buf: *const u8,
    args_len: u32,
) -> i32 {
    // SAFETY: caller guarantees cmd validity per function contract
    let Some(cmd_bytes) = (unsafe { raw_to_slice(cmd, cmd_len) }) else {
        return -1;
    };
    let Ok(cmd_str) = std::str::from_utf8(cmd_bytes) else {
        return -1;
    };

    // Check run permission
    if !check_and_report(PermissionKind::Run, cmd_str) {
        return -1;
    }

    // Parse null-separated args
    let args: Vec<&str> = if args_buf.is_null() || args_len == 0 {
        Vec::new()
    } else {
        // SAFETY: caller guarantees args_buf validity per function contract
        let Some(args_bytes) = (unsafe { raw_to_slice(args_buf, args_len) }) else {
            return -1;
        };
        let mut result = Vec::new();
        for chunk in args_bytes.split(|&b| b == 0) {
            if let Ok(s) = std::str::from_utf8(chunk)
                && !s.is_empty()
            {
                result.push(s);
            }
        }
        result
    };

    HOST.with(|h| {
        h.borrow()
            .spawn_sync(cmd_str, &args)
            .map(|r| r.exit_code)
            .unwrap_or(-1)
    })
}
