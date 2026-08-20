//! Console host trait and implementations.
//!
//! Defines the [`ConsoleHost`] trait that all host environments must implement
//! to provide `console.log`, `console.error`, `console.warn`, and `console.debug`.

use std::io::Write;

use nanbox::JsValue;

/// Trait for hosts that support console output.
///
/// Every host environment must be able to handle console output at minimum.
/// The methods correspond to the standard JavaScript `console` API.
pub trait ConsoleHost {
    /// Output values to the console (like `console.log`).
    fn log(&self, args: &[JsValue]);

    /// Output error values (like `console.error`).
    fn error(&self, args: &[JsValue]);

    /// Output warning values (like `console.warn`).
    fn warn(&self, args: &[JsValue]);

    /// Output debug values (like `console.debug`).
    fn debug(&self, args: &[JsValue]);
}

/// Console implementation that writes to stdout/stderr.
///
/// `console.log` and `console.debug` write to stdout via `std::io::stdout()`.
/// `console.error` and `console.warn` write to stderr via `std::io::stderr()`.
///
/// This implementation is used by [`DefaultHost`](crate::default::DefaultHost)
/// and writes directly to the OS streams (not through the ABI layer, since the
/// ABI wrappers themselves delegate to this host).
pub struct StdoutConsole;

impl ConsoleHost for StdoutConsole {
    fn log(&self, args: &[JsValue]) {
        let formatted: Vec<String> = args.iter().map(crate::format_value).collect();
        let mut output = formatted.join(" ");
        output.push('\n');
        let _ = std::io::stdout().write_all(output.as_bytes());
        let _ = std::io::stdout().flush();
    }

    fn error(&self, args: &[JsValue]) {
        let formatted: Vec<String> = args.iter().map(crate::format_value).collect();
        let mut output = formatted.join(" ");
        output.push('\n');
        let _ = std::io::stderr().write_all(output.as_bytes());
        let _ = std::io::stderr().flush();
    }

    fn warn(&self, args: &[JsValue]) {
        self.error(args); // warn goes to stderr
    }

    fn debug(&self, args: &[JsValue]) {
        self.log(args); // debug goes to stdout
    }
}
