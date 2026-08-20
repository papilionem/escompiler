//! Runtime permission system for controlling access to system resources.
//!
//! Implements a Deno-style permission model where compiled binaries can be
//! restricted from accessing files, network, environment variables, and
//! subprocesses. Permissions are set at compile time and baked into the
//! binary as a global permission table.
//!
//! # Key types
//!
//! - [`PermissionKind`] — the category of permission (read, write, net, env, run)
//! - [`PermissionValue`] — whether a permission is granted, denied, or restricted to specific resources
//! - [`PermissionsConfig`] — the full permission configuration for a compiled binary
//!
//! # Default behaviour
//!
//! Without any `--allow-*` flags, ALL permissions are GRANTED by default.
//! This differs from Deno — we default to permissive for backwards
//! compatibility. Users opt INTO restrictions by specifying at least one
//! `--allow-*` flag (which implies all others are denied unless also specified).

use std::cell::RefCell;
use std::path::Path;

/// The category of permission being checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionKind {
    /// File read access.
    Read,
    /// File write access.
    Write,
    /// Network access.
    Net,
    /// Environment variable access.
    Env,
    /// Subprocess execution.
    Run,
}

impl PermissionKind {
    /// Returns the CLI flag name for this permission kind.
    pub fn flag_name(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Net => "net",
            Self::Env => "env",
            Self::Run => "run",
        }
    }
}

impl std::fmt::Display for PermissionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.flag_name())
    }
}

/// The value of a permission: granted, denied, or restricted to specific resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionValue {
    /// Full access granted (no restrictions).
    Granted,
    /// Access denied entirely.
    Denied,
    /// Access restricted to the listed resources (paths, hosts, vars, programs).
    Restricted(Vec<String>),
}

impl PermissionValue {
    /// Returns `true` if this permission allows access to `resource`.
    ///
    /// - [`Granted`](PermissionValue::Granted) always returns `true`.
    /// - [`Denied`](PermissionValue::Denied) always returns `false`.
    /// - [`Restricted`](PermissionValue::Restricted) returns `true` only if
    ///   `resource` starts with one of the allowed values (for paths) or
    ///   matches exactly (for env vars, hosts, programs).
    pub fn allows(&self, resource: &str, kind: PermissionKind) -> bool {
        match self {
            Self::Granted => true,
            Self::Denied => false,
            Self::Restricted(allowed) => match kind {
                PermissionKind::Read | PermissionKind::Write => {
                    // For file paths, check if the resource is under an allowed path
                    let resource_path = Path::new(resource);
                    allowed.iter().any(|allowed_path| {
                        let ap = Path::new(allowed_path);
                        resource_path.starts_with(ap)
                    })
                }
                PermissionKind::Net | PermissionKind::Env | PermissionKind::Run => {
                    // For net/env/run, check exact match
                    allowed.iter().any(|a| a == resource)
                }
            },
        }
    }
}

/// Full permission configuration for a compiled binary.
///
/// By default, all permissions are [`Granted`](PermissionValue::Granted).
/// When the user specifies at least one `--allow-*` flag, the "restricted
/// mode" is activated: all unspecified permissions become
/// [`Denied`](PermissionValue::Denied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionsConfig {
    /// File read permission.
    pub allow_read: PermissionValue,
    /// File write permission.
    pub allow_write: PermissionValue,
    /// Network access permission.
    pub allow_net: PermissionValue,
    /// Environment variable access permission.
    pub allow_env: PermissionValue,
    /// Subprocess execution permission.
    pub allow_run: PermissionValue,
}

impl Default for PermissionsConfig {
    /// Returns the default configuration: all permissions granted.
    fn default() -> Self {
        Self {
            allow_read: PermissionValue::Granted,
            allow_write: PermissionValue::Granted,
            allow_net: PermissionValue::Granted,
            allow_env: PermissionValue::Granted,
            allow_run: PermissionValue::Granted,
        }
    }
}

impl PermissionsConfig {
    /// Creates a new permissions config with all permissions granted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a permissions config with all permissions denied.
    ///
    /// This is the starting point when the user specifies at least one
    /// `--allow-*` flag — unspecified permissions default to denied.
    pub fn all_denied() -> Self {
        Self {
            allow_read: PermissionValue::Denied,
            allow_write: PermissionValue::Denied,
            allow_net: PermissionValue::Denied,
            allow_env: PermissionValue::Denied,
            allow_run: PermissionValue::Denied,
        }
    }

    /// Returns the permission value for the given kind.
    pub fn get(&self, kind: PermissionKind) -> &PermissionValue {
        match kind {
            PermissionKind::Read => &self.allow_read,
            PermissionKind::Write => &self.allow_write,
            PermissionKind::Net => &self.allow_net,
            PermissionKind::Env => &self.allow_env,
            PermissionKind::Run => &self.allow_run,
        }
    }

    /// Checks whether the given operation is allowed.
    ///
    /// Returns `Ok(())` if the permission is granted, or `Err` with an
    /// ESC-E701 error message if denied.
    pub fn check(&self, kind: PermissionKind, resource: &str) -> Result<(), PermissionError> {
        let perm = self.get(kind);
        if perm.allows(resource, kind) {
            Ok(())
        } else {
            Err(PermissionError::Denied {
                operation: resource.to_string(),
                kind,
            })
        }
    }
}

/// Error returned when a permission check fails (ESC-E701).
#[derive(Debug, thiserror::Error)]
pub enum PermissionError {
    /// The operation was denied because the required permission was not granted.
    #[error("ESC-E701: Permission denied: {operation} requires --allow-{kind}")]
    Denied {
        /// Description of the operation that was denied.
        operation: String,
        /// The permission kind that is required.
        kind: PermissionKind,
    },
}

// =========================================================================
// Thread-local global permission state
// =========================================================================

thread_local! {
    /// Thread-local permission configuration.
    ///
    /// This is initialised once at program startup (from the baked-in
    /// permission table) and cannot be escalated at runtime.
    static PERMISSIONS: RefCell<PermissionsConfig> = RefCell::new(PermissionsConfig::new());
}

/// Initialise the global permission state.
///
/// Called once at program startup to set the permissions that were
/// determined at compile time. Once set, permissions cannot be escalated.
pub fn init_permissions(config: PermissionsConfig) {
    PERMISSIONS.with(|p| {
        *p.borrow_mut() = config;
    });
}

/// Check whether the given operation is allowed by the current permissions.
///
/// This is the main entry point called by host ABI functions before
/// performing gated operations. Returns `Ok(())` if allowed, or
/// `Err(PermissionError)` if denied.
///
/// # ESC-E701
///
/// When a permission is denied, returns an error with the message:
/// `"ESC-E701: Permission denied: {operation} requires --allow-{kind}"`
pub fn check_permission(kind: PermissionKind, resource: &str) -> Result<(), PermissionError> {
    PERMISSIONS.with(|p| p.borrow().check(kind, resource))
}

/// Returns a clone of the current global permission configuration.
///
/// Useful for testing and debugging.
pub fn current_permissions() -> PermissionsConfig {
    PERMISSIONS.with(|p| p.borrow().clone())
}
