//! The `process` global object for the ESCompiler JavaScript runtime.
//!
//! Provides Node.js-compatible `process` properties and methods:
//! - `process.argv` — command-line arguments array
//! - `process.env` — snapshot of environment variables
//! - `process.platform` — compile-time platform string
//! - `process.arch` — compile-time architecture string
//! - `process.pid` — process ID
//! - `process.version` — ESCompiler version string
//! - `process.exit(code)` — terminate the process
//! - `process.cwd()` — current working directory
//! - `process.hrtime()` — high-resolution time as `[seconds, nanoseconds]`
//!
//! Also provides `__filename` and `__dirname` global placeholders (empty strings
//! for now; real values require module path info from the pipeline).

/// Return the platform string using Node.js conventions.
///
/// Maps Rust `cfg(target_os)` to Node.js values:
/// - `"linux"` for Linux
/// - `"darwin"` for macOS
/// - `"win32"` for Windows
/// - `"freebsd"` for FreeBSD
/// - `"openbsd"` for OpenBSD
pub fn platform() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else if cfg!(target_os = "openbsd") {
        "openbsd"
    } else {
        "unknown"
    }
}

/// Return the architecture string using Node.js conventions.
///
/// Maps Rust `cfg(target_arch)` to Node.js values:
/// - `"x64"` for x86_64
/// - `"arm64"` for aarch64
/// - `"ia32"` for x86
/// - `"arm"` for arm
pub fn arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "ia32"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else {
        "unknown"
    }
}

/// Return the ESCompiler version string (e.g., `"v0.5.0-dev"`).
pub fn version() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

/// Return the current process ID.
pub fn pid() -> u32 {
    std::process::id()
}

/// Return the current working directory via the host ABI.
///
/// Falls back to an empty string if the host call fails.
pub fn cwd() -> String {
    let mut buf = vec![0u8; 4096];
    // SAFETY: buf is a valid mutable slice of known length.
    let len = unsafe { host::abi::__esc_host_cwd(buf.as_mut_ptr(), buf.len() as u32) };
    if len < 0 {
        return String::new();
    }
    let actual_len = len as usize;
    if actual_len > buf.len() {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..actual_len]).into_owned()
}

/// Return the command-line arguments as a vector of strings via the host ABI.
pub fn argv() -> Vec<String> {
    let count = host::abi::__esc_host_args_count();
    let mut args = Vec::with_capacity(count as usize);
    for i in 0..count {
        let mut buf = vec![0u8; 4096];
        // SAFETY: buf is a valid mutable slice of known length.
        let len = unsafe { host::abi::__esc_host_args_get(i, buf.as_mut_ptr(), buf.len() as u32) };
        if len >= 0 {
            let actual_len = (len as usize).min(buf.len());
            args.push(String::from_utf8_lossy(&buf[..actual_len]).into_owned());
        }
    }
    args
}

/// Return the high-resolution time as `(seconds, nanoseconds)`.
///
/// Uses `__esc_host_hrtime_ns()` and splits into seconds and remaining nanoseconds.
pub fn hrtime() -> (u64, u64) {
    let ns = host::abi::__esc_host_hrtime_ns();
    let seconds = ns / 1_000_000_000;
    let remaining_ns = ns % 1_000_000_000;
    (seconds, remaining_ns)
}

/// Exit the process with the given code via the host ABI.
///
/// This function never returns.
pub fn exit(code: i32) -> ! {
    host::abi::__esc_host_exit(code)
}

/// Return a snapshot of environment variables as key-value pairs.
///
/// Queries a set of well-known env vars plus any available from `std::env::vars()`.
pub fn env_snapshot() -> Vec<(String, String)> {
    std::env::vars().collect()
}
