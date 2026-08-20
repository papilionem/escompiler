//! Compiler driver — orchestrates the full compilation pipeline.
//!
//! This crate ties together all frontend, backend, and linker stages into
//! a single entry point. The CLI (`cli`) calls into this crate's
//! [`compile`], [`check`], and [`run`] functions.
//!
//! # Key types
//!
//! - [`CompilerConfig`] — what to compile, how to compile it
//! - [`CompileResult`] — what was produced (output path, optional timings)
//! - [`DriverError`] — any error from any phase

/// JSONC configuration file parser for `esc.json`.
pub mod config;
/// Error types for the compilation pipeline.
pub mod error;
/// Multi-module compilation pipeline (module graph, per-module lowering).
pub mod module_pipeline;
/// Per-phase timing infrastructure for `--time-phases`.
pub mod phase_timer;
/// The main compilation pipeline (parse, desugar, verify, codegen, link).
pub mod pipeline;
/// Compile-and-execute caching for `esc run`.
pub mod run_cache;

#[cfg(test)]
mod tests;

pub use error::DriverError;
pub use module_pipeline::{
    BindingKind, LiveBindingMap, MergeResult, ModuleExportMap, ModuleLoweringResult,
    NamespaceExport, ResolvedReExport, TdzExportSet,
};
pub use phase_timer::PhaseTimings;

/// Whether to use the debug (Cranelift) or release (LLVM) backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileMode {
    /// Cranelift backend — fast compilation, moderate optimization.
    Debug,
    /// LLVM backend — slower compilation, aggressive optimization.
    Release,
}

/// What kind of output artifact to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileTarget {
    /// A standalone executable binary.
    Executable,
    /// A shared/dynamic library.
    SharedLib,
    /// A static library.
    StaticLib,
    /// A relocatable object file.
    ObjectFile,
    /// A WebAssembly module.
    Wasm,
}

/// What intermediate representation to emit instead of a binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitKind {
    /// Dump the parsed AST.
    Ast,
    /// Dump the SSA IR.
    Ir,
    /// Dump LLVM IR (release mode only).
    LlvmIr,
    /// Dump native assembly.
    Asm,
}

/// Full configuration for a compilation invocation.
#[derive(Debug, Clone)]
pub struct CompilerConfig {
    /// Backend selection: debug (Cranelift) or release (LLVM).
    pub mode: CompileMode,
    /// Output artifact type.
    pub target: CompileTarget,
    /// Input file paths.
    pub input: Vec<String>,
    /// Output file path (empty string means use default).
    pub output: String,
    /// If set, emit an intermediate representation instead of a binary.
    pub emit: Option<EmitKind>,
    /// Use heap-only allocation (disables zones, for differential testing).
    pub heap_only: bool,
    /// Print per-phase timing information.
    pub time_phases: bool,
    /// Target ECMAScript edition (default: ES2025).
    pub edition: common::Edition,
    /// Parsed `esc.json` configuration, if found.
    pub esc_config: Option<config::EscConfig>,
    /// Whether to generate source maps.
    pub source_map: bool,
    /// Output directory override (from `esc.json` or CLI).
    pub out_dir: Option<String>,
    /// Explicit path to `esc.json` (from `--config` CLI flag).
    pub config_path: Option<String>,
    /// Skip `esc.json` discovery entirely (from `--no-config` CLI flag).
    pub no_config: bool,
    /// Whether FFI (Foreign Function Interface) usage is permitted.
    ///
    /// When `true`, FFI features (extern declarations, native bindings) are
    /// allowed but a warning (ESC-W700) is emitted. When `false` (default),
    /// any FFI usage produces a compile error (ESC-E700).
    pub allow_ffi: bool,
    /// The explicit CLI flag for FFI permission, if any.
    ///
    /// `Some(true)` means `--allow-ffi` was passed, `Some(false)` means
    /// `--no-ffi` was passed, `None` means neither was specified (use
    /// config file or default). This is used to determine whether the CLI
    /// explicitly overrides the config file setting.
    pub ffi_flag: Option<bool>,
    /// Whether `eval()` and `new Function()` are permitted in the source.
    ///
    /// Defaults to `true`. Set to `false` via `--no-eval` to reject all dynamic
    /// code execution at compile time (ESC-E400).
    pub allow_eval: bool,
    /// Whether the JIT compiler (Cranelift) may be included in the output binary.
    ///
    /// Defaults to `true`. Set to `false` via `--no-jit` to exclude the JIT
    /// entirely. Source using `eval()`/`Function()` will fail with ESC-E401.
    pub allow_jit: bool,
    /// Runtime permission configuration for the compiled binary.
    pub permissions: host::PermissionsConfig,
    /// Whether permissions were explicitly set via CLI flags.
    ///
    /// When `true`, CLI permissions take precedence over `esc.json`.
    pub permissions_from_cli: bool,
}

impl CompilerConfig {
    /// Create a minimal config for the given input files.
    pub fn new(input: Vec<String>) -> Self {
        Self {
            mode: CompileMode::Debug,
            target: CompileTarget::Executable,
            input,
            output: String::new(),
            emit: None,
            heap_only: false,
            time_phases: false,
            edition: common::Edition::default(),
            esc_config: None,
            source_map: false,
            out_dir: None,
            config_path: None,
            no_config: false,
            allow_ffi: false,
            ffi_flag: None,
            allow_eval: true,
            allow_jit: true,
            permissions: host::PermissionsConfig::new(),
            permissions_from_cli: false,
        }
    }

    /// Returns `true` if FFI usage is permitted by the current configuration.
    pub fn ffi_allowed(&self) -> bool {
        self.allow_ffi
    }
}

/// The result of a successful compilation.
#[derive(Debug)]
pub struct CompileResult {
    /// Path to the produced output file (empty if `--emit` was used).
    pub output_path: String,
    /// Per-phase timings, if `--time-phases` was enabled.
    pub phase_times: Option<PhaseTimings>,
}

/// Run the full compilation pipeline: parse -> desugar -> verify -> codegen -> link.
///
/// # Errors
///
/// Returns [`DriverError`] if any compilation phase fails.
pub fn compile(config: &CompilerConfig) -> Result<CompileResult, DriverError> {
    pipeline::run_pipeline(config)
}

/// Type-check only (parse -> desugar -> verify, no codegen or linking).
///
/// # Errors
///
/// Returns [`DriverError`] if parsing, lowering, or verification fails.
pub fn check(config: &CompilerConfig) -> Result<(), DriverError> {
    if config.input.is_empty() {
        return Err(DriverError::NoInput);
    }
    let input_path = &config.input[0];
    let source = std::fs::read_to_string(input_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            DriverError::FileNotFound(input_path.clone())
        } else {
            DriverError::Io(e)
        }
    })?;
    let is_module = input_path.ends_with(".mjs") || input_path.ends_with(".mts");
    let result = if is_module {
        desugar::lower_program(&source)
    } else {
        desugar::lower_script(&source)
    }
    .map_err(|errs| DriverError::Lowering(errs.iter().map(|e| e.to_string()).collect()))?;
    ir::verify::verify_typed_module(&result.module)
        .map_err(|errs| DriverError::Verification(errs.iter().map(|e| e.to_string()).collect()))?;
    Ok(())
}

/// Compile and then execute the resulting binary.
///
/// Uses a cache to skip recompilation when the source content and compiler
/// version have not changed. On cache hit the previously compiled binary is
/// executed directly. On cache miss the normal compilation pipeline runs and
/// the result is stored in the cache for future invocations.
///
/// Returns the exit code of the executed process.
///
/// # Errors
///
/// Returns [`DriverError`] if compilation fails or the process cannot be spawned.
pub fn run(config: &CompilerConfig) -> Result<i32, DriverError> {
    // Read source for cache key computation.
    if config.input.is_empty() {
        return Err(DriverError::NoInput);
    }
    let input_path = &config.input[0];
    let source = std::fs::read_to_string(input_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            DriverError::FileNotFound(input_path.clone())
        } else {
            DriverError::Io(e)
        }
    })?;

    let version = run_cache::compiler_version();
    let target = run_cache::target_id();
    let key = run_cache::RunCache::cache_key(&source, version, &target);

    // Try cache lookup. If the cache itself fails to initialize, fall back
    // to a normal compile-and-run (cache is an optimization, not required).
    if let Ok(cache) = run_cache::RunCache::new() {
        if let Some(cached_binary) = cache.get(&key) {
            let status = std::process::Command::new(&cached_binary).status()?;
            return Ok(status.code().unwrap_or(1));
        }

        // Cache miss — compile, cache the result, then execute.
        let result = compile(config)?;
        let binary_to_run =
            if let Ok(cached) = cache.put(&key, std::path::Path::new(&result.output_path)) {
                cached.display().to_string()
            } else {
                // Cache store failed — run from the original path.
                result.output_path
            };
        let status = std::process::Command::new(&binary_to_run).status()?;
        return Ok(status.code().unwrap_or(1));
    }

    // Cache unavailable — fall back to compile-and-run without caching.
    let result = compile(config)?;
    let status = std::process::Command::new(&result.output_path).status()?;
    Ok(status.code().unwrap_or(1))
}
