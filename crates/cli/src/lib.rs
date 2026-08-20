//! CLI argument parsing and configuration for the compiler.
//!
//! Exposes the [`Cli`] struct (built on clap) and [`Commands`] enum so that
//! argument parsing can be tested independently of the binary entry point.

use clap::{Parser, Subcommand};
use common::Edition;
use driver::{CompileMode, CompileTarget, CompilerConfig, EmitKind};
use host::{PermissionValue, PermissionsConfig};

/// Top-level CLI parser.
#[derive(Parser, Debug)]
#[command(name = "esc", about = "JavaScript/TypeScript AOT compiler", version)]
pub struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Commands,
}

/// Supported CLI subcommands.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Compile to native binary or library.
    Build {
        /// Input files.
        input: Vec<String>,
        /// Output path.
        #[arg(short, long)]
        output: Option<String>,
        /// Build in release mode (LLVM backend).
        #[arg(long)]
        release: bool,
        /// Emit intermediate representation: ast, ir, llvm-ir, asm.
        #[arg(long)]
        emit: Option<String>,
        /// Use heap-only allocation (no zones, for differential testing).
        #[arg(long)]
        heap_only: bool,
        /// Print timing for each compilation phase.
        #[arg(long)]
        time_phases: bool,
        /// Target ECMAScript edition (e.g., es5, es2020, es2025, esnext).
        #[arg(long, default_value = "es2025")]
        edition: String,
        /// Explicit path to esc.json config file.
        #[arg(long = "config")]
        config_path: Option<String>,
        /// Skip esc.json discovery.
        #[arg(long)]
        no_config: bool,
        /// Allow FFI (foreign function interface) usage. Bypasses safety guarantees.
        #[arg(long, conflicts_with = "no_ffi")]
        allow_ffi: bool,
        /// Explicitly disable FFI usage (this is the default).
        #[arg(long, conflicts_with = "allow_ffi")]
        no_ffi: bool,
        /// Reject all eval() and new Function() usage at compile time.
        #[arg(long)]
        no_eval: bool,
        /// Exclude the JIT compiler from the compiled binary.
        #[arg(long)]
        no_jit: bool,
        /// Allow file read access (optionally restricted to specific paths).
        #[arg(long, value_name = "PATHS", num_args = 0..=1, default_missing_value = "")]
        allow_read: Option<String>,
        /// Allow file write access (optionally restricted to specific paths).
        #[arg(long, value_name = "PATHS", num_args = 0..=1, default_missing_value = "")]
        allow_write: Option<String>,
        /// Allow network access (optionally restricted to specific hosts).
        #[arg(long, value_name = "HOSTS", num_args = 0..=1, default_missing_value = "")]
        allow_net: Option<String>,
        /// Allow environment variable access (optionally restricted to specific vars).
        #[arg(long, value_name = "VARS", num_args = 0..=1, default_missing_value = "")]
        allow_env: Option<String>,
        /// Allow subprocess execution (optionally restricted to specific programs).
        #[arg(long, value_name = "PROGRAMS", num_args = 0..=1, default_missing_value = "")]
        allow_run: Option<String>,
        /// Allow all permissions (shorthand for enabling everything).
        #[arg(long)]
        allow_all: bool,
    },
    /// Compile and execute.
    Run {
        /// Input file.
        input: String,
        /// Arguments passed to the program.
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
        /// Use heap-only allocation (no zones, for differential testing).
        #[arg(long)]
        heap_only: bool,
        /// Print timing for each compilation phase.
        #[arg(long)]
        time_phases: bool,
        /// Target ECMAScript edition (e.g., es5, es2020, es2025, esnext).
        #[arg(long, default_value = "es2025")]
        edition: String,
        /// Explicit path to esc.json config file.
        #[arg(long = "config")]
        config_path: Option<String>,
        /// Skip esc.json discovery.
        #[arg(long)]
        no_config: bool,
        /// Allow FFI (foreign function interface) usage. Bypasses safety guarantees.
        #[arg(long, conflicts_with = "no_ffi")]
        allow_ffi: bool,
        /// Explicitly disable FFI usage (this is the default).
        #[arg(long, conflicts_with = "allow_ffi")]
        no_ffi: bool,
        /// Reject all eval() and new Function() usage at compile time.
        #[arg(long)]
        no_eval: bool,
        /// Exclude the JIT compiler from the compiled binary.
        #[arg(long)]
        no_jit: bool,
        /// Allow file read access (optionally restricted to specific paths).
        #[arg(long, value_name = "PATHS", num_args = 0..=1, default_missing_value = "")]
        allow_read: Option<String>,
        /// Allow file write access (optionally restricted to specific paths).
        #[arg(long, value_name = "PATHS", num_args = 0..=1, default_missing_value = "")]
        allow_write: Option<String>,
        /// Allow network access (optionally restricted to specific hosts).
        #[arg(long, value_name = "HOSTS", num_args = 0..=1, default_missing_value = "")]
        allow_net: Option<String>,
        /// Allow environment variable access (optionally restricted to specific vars).
        #[arg(long, value_name = "VARS", num_args = 0..=1, default_missing_value = "")]
        allow_env: Option<String>,
        /// Allow subprocess execution (optionally restricted to specific programs).
        #[arg(long, value_name = "PROGRAMS", num_args = 0..=1, default_missing_value = "")]
        allow_run: Option<String>,
        /// Allow all permissions (shorthand for enabling everything).
        #[arg(long)]
        allow_all: bool,
    },
    /// Type check + analysis (no codegen).
    Check {
        /// Input files.
        input: Vec<String>,
    },
    /// Scaffold a new project.
    #[command(hide = true)]
    Init {
        /// Project name.
        name: Option<String>,
    },
    /// Incremental rebuild on file changes.
    #[command(hide = true)]
    Watch {
        /// Input files.
        input: Vec<String>,
    },
    /// Interactive REPL (Phase 3+).
    #[command(hide = true)]
    Repl {},
    /// Run tests.
    #[command(hide = true)]
    Test {
        /// Filter tests by name.
        #[arg(long)]
        filter: Option<String>,
    },
}

/// Parse an emit kind string into an [`EmitKind`].
///
/// Returns `None` for unrecognized strings.
pub fn parse_emit_kind(s: &str) -> Option<EmitKind> {
    match s {
        "ast" => Some(EmitKind::Ast),
        "ir" => Some(EmitKind::Ir),
        "llvm-ir" => Some(EmitKind::LlvmIr),
        "asm" => Some(EmitKind::Asm),
        _ => None,
    }
}

/// Parse an edition string into an [`Edition`], falling back to ES2025 on error.
pub fn parse_edition(s: &str) -> Edition {
    s.parse::<Edition>().unwrap_or_default()
}

/// Compute the explicit FFI flag from CLI arguments.
///
/// Returns `Some(true)` if `--allow-ffi` was passed, `Some(false)` if
/// `--no-ffi` was passed, or `None` if neither was specified.
pub fn resolve_ffi_flag(allow_ffi: bool, no_ffi: bool) -> Option<bool> {
    if allow_ffi {
        Some(true)
    } else if no_ffi {
        Some(false)
    } else {
        None
    }
}

/// Parse a permission flag value into a [`PermissionValue`].
///
/// - `None` means the flag was not specified (no restriction for this kind).
/// - `Some("")` means `--allow-X` was specified without a value (grant all).
/// - `Some("path1,path2")` means `--allow-X=path1,path2` (restrict to listed).
pub fn parse_permission_flag(flag: Option<&str>) -> Option<PermissionValue> {
    match flag {
        None => None,
        Some("") => Some(PermissionValue::Granted),
        Some(paths) => {
            let items: Vec<String> = paths.split(',').map(|s| s.trim().to_string()).collect();
            Some(PermissionValue::Restricted(items))
        }
    }
}

/// Build a [`PermissionsConfig`] from the CLI permission flags.
///
/// If `allow_all` is true, all permissions are granted.
/// If any `--allow-*` flag is present, we enter restricted mode where
/// unspecified permissions are denied. If no flags are present, all
/// permissions default to granted (permissive default).
///
/// Returns `(config, from_cli)` where `from_cli` is `true` if any
/// permission flag was specified.
pub fn build_permissions(
    allow_read: Option<&str>,
    allow_write: Option<&str>,
    allow_net: Option<&str>,
    allow_env: Option<&str>,
    allow_run: Option<&str>,
    allow_all: bool,
) -> (PermissionsConfig, bool) {
    if allow_all {
        return (PermissionsConfig::new(), true);
    }

    let read = parse_permission_flag(allow_read);
    let write = parse_permission_flag(allow_write);
    let net = parse_permission_flag(allow_net);
    let env = parse_permission_flag(allow_env);
    let run = parse_permission_flag(allow_run);

    // Check if any permission flag was specified
    let any_specified =
        read.is_some() || write.is_some() || net.is_some() || env.is_some() || run.is_some();

    if !any_specified {
        // No permission flags — default to all granted (permissive)
        return (PermissionsConfig::new(), false);
    }

    // At least one flag was specified — unspecified permissions are denied
    let config = PermissionsConfig {
        allow_read: read.unwrap_or(PermissionValue::Denied),
        allow_write: write.unwrap_or(PermissionValue::Denied),
        allow_net: net.unwrap_or(PermissionValue::Denied),
        allow_env: env.unwrap_or(PermissionValue::Denied),
        allow_run: run.unwrap_or(PermissionValue::Denied),
    };

    (config, true)
}

/// Build a [`CompilerConfig`] from the Build subcommand arguments.
// Each parameter maps 1:1 to a CLI flag; grouping into an intermediate struct
// would add complexity without reducing the surface area.
#[allow(clippy::too_many_arguments)]
pub fn build_config(
    input: Vec<String>,
    output: Option<String>,
    release: bool,
    emit: Option<String>,
    heap_only: bool,
    time_phases: bool,
    edition: &str,
    config_path: Option<String>,
    no_config: bool,
    allow_ffi: bool,
    no_ffi: bool,
    no_eval: bool,
    no_jit: bool,
    permissions: PermissionsConfig,
    permissions_from_cli: bool,
) -> CompilerConfig {
    let mode = if release {
        CompileMode::Release
    } else {
        CompileMode::Debug
    };
    let emit_kind = emit.as_deref().and_then(parse_emit_kind);
    let ffi_flag = resolve_ffi_flag(allow_ffi, no_ffi);
    CompilerConfig {
        mode,
        target: CompileTarget::Executable,
        input,
        output: output.unwrap_or_else(|| "a.out".to_string()),
        emit: emit_kind,
        heap_only,
        time_phases,
        edition: parse_edition(edition),
        esc_config: None,
        source_map: false,
        out_dir: None,
        config_path,
        no_config,
        allow_ffi: ffi_flag.unwrap_or(false),
        ffi_flag,
        allow_eval: !no_eval,
        allow_jit: !no_jit,
        permissions,
        permissions_from_cli,
    }
}

/// Build a [`CompilerConfig`] from the Run subcommand arguments.
#[allow(clippy::too_many_arguments)]
pub fn run_config(
    input: String,
    heap_only: bool,
    time_phases: bool,
    edition: &str,
    config_path: Option<String>,
    no_config: bool,
    allow_ffi: bool,
    no_ffi: bool,
    no_eval: bool,
    no_jit: bool,
    permissions: PermissionsConfig,
    permissions_from_cli: bool,
) -> CompilerConfig {
    let ffi_flag = resolve_ffi_flag(allow_ffi, no_ffi);
    CompilerConfig {
        mode: CompileMode::Debug,
        target: CompileTarget::Executable,
        input: vec![input],
        output: String::new(),
        emit: None,
        heap_only,
        time_phases,
        edition: parse_edition(edition),
        esc_config: None,
        source_map: false,
        out_dir: None,
        config_path,
        no_config,
        allow_ffi: ffi_flag.unwrap_or(false),
        ffi_flag,
        allow_eval: !no_eval,
        allow_jit: !no_jit,
        permissions,
        permissions_from_cli,
    }
}

#[cfg(test)]
mod tests;
