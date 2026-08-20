//! Host environment abstraction for the ESCompiler JavaScript runtime.
//!
//! Provides trait hierarchies that define what APIs are available to JavaScript
//! code at runtime. Different host implementations provide different levels of
//! capability (bare console-only, default with stub primitives, full with real I/O).
//!
//! The base [`Host`] trait requires [`ConsoleHost`] and [`HostPrimitives`].
//! Concrete implementations include:
//! - [`BareHost`] — minimal, console-only, all primitives return `NotSupported`
//! - [`DefaultHost`] — console + real implementations of all 16 MUST HAVE primitives

pub mod abi;
pub mod bare;
pub mod console;
pub mod default;
pub mod error;
pub mod permissions;
pub mod primitives;
pub mod types;

#[cfg(test)]
mod tests;

pub use bare::BareHost;
pub use console::{ConsoleHost, StdoutConsole};
pub use default::DefaultHost;
pub use error::HostError;
pub use permissions::{
    PermissionError, PermissionKind, PermissionValue, PermissionsConfig, check_permission,
    current_permissions, init_permissions,
};
pub use primitives::HostPrimitives;
pub use types::{HostStat, SpawnResult};

use nanbox::JsValue;

/// The base host trait that all host environments must implement.
///
/// A host provides the bridge between compiled JavaScript code and the
/// operating system / embedding environment. Every host must support
/// console output ([`ConsoleHost`]) and syscall-level primitives
/// ([`HostPrimitives`]).
pub trait Host: ConsoleHost + HostPrimitives + Send + Sync {
    /// Returns the name of this host implementation.
    fn name(&self) -> &str;

    /// Returns the host's description string (shown in error messages).
    fn description(&self) -> &str {
        self.name()
    }
}

/// Format a [`JsValue`] for display purposes (used by console implementations).
///
/// This is a simplified formatter — the full implementation will live in
/// `runtime::display`. This version handles the common cases needed
/// for console output.
pub fn format_value(val: &JsValue) -> String {
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
        "[string]".to_string() // TODO: Phase D — dereference string pointer
    } else if val.is_object() {
        "[object Object]".to_string()
    } else if val.is_symbol() {
        "Symbol()".to_string()
    } else {
        "undefined".to_string()
    }
}
