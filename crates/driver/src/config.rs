//! JSONC configuration file parser for `esc.json`.
//!
//! Provides discovery, parsing, and validation of the `esc.json` project
//! configuration file.  Supports JSON with Comments (JSONC) — both `//` line
//! comments and `/* */` block comments are stripped before parsing while
//! preserving any comment-like sequences that appear inside string literals.
//!
//! # Key types
//!
//! - [`EscConfig`] — top-level configuration structure
//! - [`CompilerOptions`] — compiler-related settings
//! - [`HostConfig`] — host module selection
//! - [`EvalConfig`] — eval behaviour settings
//! - [`ConfigError`] — errors that can occur during config loading
//!
//! # Key functions
//!
//! - [`find_config`] — walk parent directories to discover `esc.json`
//! - [`parse_config`] — read, strip comments, and deserialize `esc.json`
//! - [`strip_jsonc_comments`] — remove JSONC comments from a string

use std::path::{Path, PathBuf};

use crate::CompilerConfig;

// Re-export host permission types for convenience.
pub use host::{PermissionValue, PermissionsConfig};

/// Top-level configuration read from `esc.json`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscConfig {
    /// Compiler-related options (target edition, module system, output, etc.).
    pub compiler_options: Option<CompilerOptions>,
    /// Host module selection.
    pub host: Option<HostConfig>,
    /// Eval behaviour settings.
    pub eval: Option<EvalConfig>,
    /// Permission settings for controlling both compile-time features
    /// (FFI, eval, JIT) and runtime resource access (read, write, net, env, run).
    pub permissions: Option<PermissionsJsonConfig>,
}

/// Compiler-related configuration options.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompilerOptions {
    /// Target ECMAScript edition, e.g. `"es2025"`, `"esnext"`.
    pub target: Option<String>,
    /// Module system, e.g. `"esm"`.
    pub module: Option<String>,
    /// Output directory for compiled artifacts.
    pub out_dir: Option<String>,
    /// Whether to emit source maps.
    pub source_map: Option<bool>,
}

/// Host module configuration.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct HostConfig {
    /// List of host modules to enable, e.g. `["console", "fs", "process"]`.
    pub modules: Option<Vec<String>>,
}

/// Eval behaviour configuration.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct EvalConfig {
    /// Eval mode — v0.5 only supports `"indirect"`.
    pub mode: Option<String>,
}

/// Permission configuration as read from `esc.json`.
///
/// Combines compile-time permission flags (FFI, eval, JIT) with runtime
/// resource permissions (read, write, net, env, run).
///
/// Compile-time fields are simple booleans:
/// - `allowFfi` — whether FFI is permitted
/// - `allowEval` — whether eval/Function() is permitted
/// - `allowJit` — whether the JIT compiler may be included
///
/// Runtime resource fields can be:
/// - `true` (JSON boolean) — allow all access of that kind
/// - `false` (JSON boolean) — deny all access of that kind
/// - `["path1", "path2"]` (JSON array of strings) — allow only listed resources
///
/// When omitted, the default depends on whether restricted mode is active.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsJsonConfig {
    // --- Compile-time permission flags ---
    /// Whether FFI (Foreign Function Interface) is allowed.
    ///
    /// When `true`, the compiler permits extern declarations and native bindings.
    /// When `false` (default), any FFI usage results in a compile error (ESC-E700).
    pub allow_ffi: Option<bool>,
    /// Whether `eval()` and `new Function()` are allowed (default: `true`).
    ///
    /// Set to `false` to reject all dynamic code execution at compile time.
    pub allow_eval: Option<bool>,
    /// Whether JIT compilation (Cranelift) is allowed (default: `true`).
    ///
    /// Set to `false` to exclude the JIT compiler from the output binary.
    pub allow_jit: Option<bool>,

    // --- Runtime resource permissions ---
    /// File read permission: `true`, `false`, or list of allowed paths.
    pub allow_read: Option<PermissionJsonValue>,
    /// File write permission: `true`, `false`, or list of allowed paths.
    pub allow_write: Option<PermissionJsonValue>,
    /// Network permission: `true`, `false`, or list of allowed hosts.
    pub allow_net: Option<PermissionJsonValue>,
    /// Environment variable permission: `true`, `false`, or list of allowed vars.
    pub allow_env: Option<PermissionJsonValue>,
    /// Subprocess execution permission: `true`, `false`, or list of allowed programs.
    pub allow_run: Option<PermissionJsonValue>,
}

/// A single permission value from JSON: boolean or array of strings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
pub enum PermissionJsonValue {
    /// `true` = allow all, `false` = deny all.
    Bool(bool),
    /// Array of allowed resources (paths, hosts, vars, or programs).
    List(Vec<String>),
}

/// Errors that can occur while loading or parsing `esc.json`.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Failed to read the configuration file from disk.
    #[error("failed to read config: {0}")]
    ReadError(#[from] std::io::Error),
    /// The configuration file contains invalid JSON.
    #[error("invalid JSON in config: {0}")]
    ParseError(#[from] serde_json::Error),
    /// The configuration file contains an unknown field.
    #[error("unknown field in config: {field}")]
    UnknownField {
        /// Name of the unrecognised field.
        field: String,
    },
}

/// The conventional configuration file name.
const CONFIG_FILE_NAME: &str = "esc.json";

/// Walk parent directories starting from `start` looking for `esc.json`.
///
/// Returns the path to the first `esc.json` found, or `None` if no
/// configuration file exists in any ancestor directory.
pub fn find_config(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };

    loop {
        let candidate = dir.join(CONFIG_FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Read, strip JSONC comments, parse, and return the configuration from `path`.
///
/// # Errors
///
/// Returns [`ConfigError::ReadError`] if the file cannot be read, or
/// [`ConfigError::ParseError`] if the (comment-stripped) content is not valid
/// JSON.
pub fn parse_config(path: &Path) -> Result<EscConfig, ConfigError> {
    let raw = std::fs::read_to_string(path)?;
    let stripped = strip_jsonc_comments(&raw);
    let config: EscConfig = serde_json::from_str(&stripped)?;
    Ok(config)
}

/// Convert a [`PermissionJsonValue`] to a [`host::PermissionValue`].
pub fn json_perm_to_host_perm(val: &PermissionJsonValue) -> host::PermissionValue {
    match val {
        PermissionJsonValue::Bool(true) => host::PermissionValue::Granted,
        PermissionJsonValue::Bool(false) => host::PermissionValue::Denied,
        PermissionJsonValue::List(items) => host::PermissionValue::Restricted(items.clone()),
    }
}

/// Convert the runtime resource permissions from a [`PermissionsJsonConfig`]
/// into a [`host::PermissionsConfig`].
///
/// Fields that are not specified in the JSON config default to
/// [`host::PermissionValue::Granted`] (permissive default).
pub fn json_permissions_to_host(json: &PermissionsJsonConfig) -> host::PermissionsConfig {
    host::PermissionsConfig {
        allow_read: json
            .allow_read
            .as_ref()
            .map(json_perm_to_host_perm)
            .unwrap_or(host::PermissionValue::Granted),
        allow_write: json
            .allow_write
            .as_ref()
            .map(json_perm_to_host_perm)
            .unwrap_or(host::PermissionValue::Granted),
        allow_net: json
            .allow_net
            .as_ref()
            .map(json_perm_to_host_perm)
            .unwrap_or(host::PermissionValue::Granted),
        allow_env: json
            .allow_env
            .as_ref()
            .map(json_perm_to_host_perm)
            .unwrap_or(host::PermissionValue::Granted),
        allow_run: json
            .allow_run
            .as_ref()
            .map(json_perm_to_host_perm)
            .unwrap_or(host::PermissionValue::Granted),
    }
}

/// Merge values from `esc.json` into the compiler configuration.
///
/// CLI flags always take precedence. Only values that have not been explicitly
/// set by the CLI are filled in from the project configuration file.
///
/// The following fields are merged:
/// - `compilerOptions.target` -> `edition` (if CLI used the default)
/// - `compilerOptions.outDir` -> `out_dir` (if CLI didn't set `--output`)
/// - `compilerOptions.sourceMap` -> `source_map` (if not already set)
/// - `permissions.allowFfi` -> `allow_ffi` (if CLI didn't explicitly set it)
/// - `permissions.allowEval` -> `allow_eval` (if CLI didn't pass `--no-eval`)
/// - `permissions.allowJit` -> `allow_jit` (if CLI didn't pass `--no-jit`)
/// - `permissions.*` -> `permissions` (if CLI didn't set any --allow-* flags)
pub fn merge_config(cli: &mut CompilerConfig, esc: &EscConfig) {
    if let Some(ref opts) = esc.compiler_options {
        // edition: only override if CLI is still on the default
        if cli.edition == common::Edition::default()
            && let Some(ref target) = opts.target
            && let Ok(parsed) = target.parse::<common::Edition>()
        {
            cli.edition = parsed;
        }

        // out_dir: only override if CLI didn't specify --output
        if cli.out_dir.is_none()
            && let Some(ref dir) = opts.out_dir
        {
            cli.out_dir = Some(dir.clone());
        }

        // source_map: only override if CLI didn't already set it
        if !cli.source_map
            && let Some(sm) = opts.source_map
        {
            cli.source_map = sm;
        }
    }

    if let Some(ref perms) = esc.permissions {
        // Compile-time permissions: only override if CLI didn't explicitly set the flags.
        // permissions.allowFfi: only override if CLI didn't explicitly set it
        if cli.ffi_flag.is_none()
            && let Some(allow) = perms.allow_ffi
        {
            cli.allow_ffi = allow;
        }
        if cli.allow_eval
            && let Some(allow) = perms.allow_eval
        {
            cli.allow_eval = allow;
        }
        if cli.allow_jit
            && let Some(allow) = perms.allow_jit
        {
            cli.allow_jit = allow;
        }

        // Runtime resource permissions: only merge from config if CLI didn't
        // explicitly set permissions (i.e., permissions is still the default all-granted).
        if !cli.permissions_from_cli {
            cli.permissions = json_permissions_to_host(perms);
        }
    }
}

/// Discover and load `esc.json` for the given compiler configuration.
///
/// Respects `no_config` (skip discovery) and `config_path` (explicit path).
/// If the config file is found and parsed successfully, merges its values
/// into `config`. Parse errors are printed as warnings to stderr but do
/// not abort compilation.
pub fn load_and_merge_config(config: &mut CompilerConfig) {
    if config.no_config {
        return;
    }

    let config_path = if let Some(ref explicit) = config.config_path {
        // Explicit --config path
        let p = PathBuf::from(explicit);
        if p.is_file() {
            Some(p)
        } else {
            eprintln!("warning: config file not found: {explicit}");
            None
        }
    } else if let Some(first_input) = config.input.first() {
        // Auto-discover from input file's directory
        find_config(Path::new(first_input))
    } else {
        None
    };

    if let Some(ref path) = config_path {
        match parse_config(path) {
            Ok(esc) => {
                merge_config(config, &esc);
                config.esc_config = Some(esc);
            }
            Err(e) => {
                eprintln!("warning: failed to parse {}: {e}", path.display());
            }
        }
    }
}

/// Strip JSONC comments (`//` line comments and `/* */` block comments) from
/// `input`, preserving comment-like sequences inside JSON string literals.
///
/// Returns a new `String` with all comments replaced by whitespace so that
/// line/column offsets remain stable for downstream error reporting.
pub fn strip_jsonc_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        match bytes[i] {
            // JSON string literal — copy verbatim, including any embedded
            // sequences that look like comments.
            b'"' => {
                result.push('"');
                i += 1;
                while i < len {
                    match bytes[i] {
                        b'\\' => {
                            // Escaped character — copy both the backslash and
                            // the following character to avoid mis-detecting a
                            // `\"` as the end of the string.
                            result.push('\\');
                            i += 1;
                            if i < len {
                                result.push(bytes[i] as char);
                                i += 1;
                            }
                        }
                        b'"' => {
                            result.push('"');
                            i += 1;
                            break;
                        }
                        _ => {
                            result.push(bytes[i] as char);
                            i += 1;
                        }
                    }
                }
            }
            // Potential comment start.
            b'/' if i + 1 < len => {
                match bytes[i + 1] {
                    // Line comment — skip until end of line.
                    b'/' => {
                        i += 2;
                        while i < len && bytes[i] != b'\n' {
                            result.push(' ');
                            i += 1;
                        }
                    }
                    // Block comment — skip until `*/`.
                    b'*' => {
                        // Replace the `/*` itself with spaces.
                        result.push(' ');
                        result.push(' ');
                        i += 2;
                        while i + 1 < len {
                            if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                                result.push(' ');
                                result.push(' ');
                                i += 2;
                                break;
                            }
                            if bytes[i] == b'\n' {
                                result.push('\n');
                            } else {
                                result.push(' ');
                            }
                            i += 1;
                        }
                    }
                    // Not a comment — just a `/`.
                    _ => {
                        result.push('/');
                        i += 1;
                    }
                }
            }
            _ => {
                result.push(bytes[i] as char);
                i += 1;
            }
        }
    }

    result
}
