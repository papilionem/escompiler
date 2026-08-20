//! Console built-in methods (`console.log`, `console.error`, `console.warn`, `console.debug`).
//!
//! These are standalone functions used by the stdlib registry. Output is routed
//! through the host ABI (`__esc_host_fd_write`) rather than using `println!`
//! directly, so that I/O flows through the host abstraction layer.
//!
//! For the host-level console abstraction (trait-based), see [`host::ConsoleHost`].

use nanbox::JsValue;

/// Console log -- format and print all arguments to stdout via host fd_write.
pub fn console_log(args: &[JsValue]) {
    let parts: Vec<String> = args.iter().map(format_arg).collect();
    let mut output = parts.join(" ");
    output.push('\n');
    write_to_host_fd(1, &output);
}

/// Console error -- format and print all arguments to stderr via host fd_write.
pub fn console_error(args: &[JsValue]) {
    let parts: Vec<String> = args.iter().map(format_arg).collect();
    let mut output = parts.join(" ");
    output.push('\n');
    write_to_host_fd(2, &output);
}

/// Console warn -- alias for `console.error` (writes to stderr).
pub fn console_warn(args: &[JsValue]) {
    console_error(args);
}

/// Console debug -- alias for `console.log` (writes to stdout).
pub fn console_debug(args: &[JsValue]) {
    console_log(args);
}

/// Write a string to a file descriptor via the host ABI.
fn write_to_host_fd(fd: i32, s: &str) {
    let bytes = s.as_bytes();
    // SAFETY: bytes is a valid slice derived from a Rust &str.
    unsafe {
        host::abi::__esc_host_fd_write(fd, bytes.as_ptr(), bytes.len() as u32);
    }
}

/// Format a single [`JsValue`] argument for console output.
fn format_arg(val: &JsValue) -> String {
    if val.is_undefined() {
        "undefined".to_string()
    } else if val.is_null() {
        "null".to_string()
    } else if let Some(b) = val.as_bool() {
        b.to_string()
    } else if let Some(n) = val.as_int() {
        n.to_string()
    } else if let Some(n) = val.as_number() {
        if n.is_nan() {
            "NaN".to_string()
        } else if n.is_infinite() {
            if n > 0.0 {
                "Infinity".to_string()
            } else {
                "-Infinity".to_string()
            }
        } else if n == n.trunc() && n.abs() < 1e15 {
            format!("{}", n as i64)
        } else {
            format!("{n}")
        }
    } else if val.is_string() {
        "[string]".to_string()
    } else if val.is_object() {
        "[object Object]".to_string()
    } else {
        "undefined".to_string()
    }
}
