//! Compilation pipeline — orchestrates parse, desugar, verify, codegen, and link.
//!
//! The pipeline is driven by [`run_pipeline`], which reads source, lowers it
//! through the frontend, generates code via Cranelift, and links the result
//! into a native binary.

use std::path::Path;
use std::path::PathBuf;

use crate::error::DriverError;
use crate::module_pipeline;
use crate::phase_timer::PhaseTimer;
use crate::{CompileResult, CompilerConfig, EmitKind};

/// Embedded copy of the runtime staticlib, baked in at build time by
/// [`build.rs`] (ESC-59).
///
/// Empty when the driver was built without the runtime crate available
/// (e.g. future crates.io packaging) — the embedded fallback is skipped
/// silently.
static EMBEDDED_RUNTIME: &[u8] = include_bytes!(env!("ESC_RUNTIME_A"));

/// Locate the runtime static library for linking.
///
/// Searches in order:
/// 1. `CARGO_TARGET_DIR/<profile>/<lib_name>`
/// 2. `./target/debug/<lib_name>` (and parent dirs up to 4 levels)
/// 3. Next to the current executable
/// 4. Extract the embedded staticlib (ESC-59) to a per-user cache directory
///    keyed by compiler version and archive checksum
///
/// On Unix: `libruntime.a`; on Windows: `runtime.lib`.
///
/// Returns an absolute path to ensure the linker can find the file
/// regardless of its working directory.
pub fn find_runtime_lib() -> Option<String> {
    // Opt-in dynamic linking (CI/test path): link test binaries against one
    // shared `libruntime.so` instead of re-linking the ~100 MB static archive
    // per binary. Collapses per-link cost and binary size. Off by default, so
    // release / `esc build` (ESC-59 static embedding) is unchanged. Unix only;
    // Windows keeps the static import lib.
    let dynamic = !cfg!(windows)
        && matches!(
            std::env::var("ESC_RUNTIME_LINK").as_deref(),
            Ok("dynamic") | Ok("so") | Ok("shared")
        );
    let lib_name = if cfg!(windows) {
        "runtime.lib"
    } else if dynamic {
        "libruntime.so"
    } else {
        "libruntime.a"
    };

    // Try CARGO_TARGET_DIR first
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let path = std::path::Path::new(&target_dir)
            .join("debug")
            .join(lib_name);
        if let Some(abs) = try_canonicalize(&path) {
            return Some(abs);
        }
    }

    // Walk up from CWD looking for target/debug/libruntime.a
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = cwd.as_path();
        for _ in 0..5 {
            let path = dir.join("target/debug").join(lib_name);
            if let Some(abs) = try_canonicalize(&path) {
                return Some(abs);
            }
            match dir.parent() {
                Some(p) => dir = p,
                None => break,
            }
        }
    }

    // Try next to current exe
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let path = dir.join(lib_name);
        if let Some(abs) = try_canonicalize(&path) {
            return Some(abs);
        }
    }

    // 4. Fall back to the runtime archive embedded in the esc binary (ESC-59):
    //    extract it to a per-user cache dir on first use.
    extract_embedded_runtime()
}

/// Canonicalize a path if it exists.
fn try_canonicalize(path: &std::path::Path) -> Option<String> {
    if path.exists() {
        path.canonicalize()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Embedded runtime extraction (ESC-59)
// ---------------------------------------------------------------------------

/// SHA-256 round constants (first 32 bits of the fractional parts of the
/// cube roots of the first 64 primes).
const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Compute the SHA-256 digest of `data` and return it as a 64-char lowercase
/// hex string.
///
/// This is a minimal, dependency-free implementation used solely to key the
/// embedded-runtime cache (ESC-59), avoiding a full hashing crate dependency.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // Padding: append 0x80, zero bytes so len ≡ 56 mod 64, then bit-length as u64.
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        // Message schedule
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        // Compression
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    // Hex encode
    let mut hex = String::with_capacity(64);
    for word in &h {
        hex.push_str(&format!("{word:08x}"));
    }
    hex
}

/// Resolve the directory used to cache the extracted runtime archive.
///
/// Resolution order (pure — env vars are passed as explicit arguments):
/// 1. `$XDG_CACHE_HOME/esc/`
/// 2. `$HOME/.cache/esc/`
/// 3. `<temp_dir>/esc/`
pub(crate) fn resolve_runtime_cache_dir(
    xdg_cache_home: Option<&Path>,
    home: Option<&Path>,
    temp: &Path,
) -> PathBuf {
    if let Some(xdg) = xdg_cache_home.filter(|p| !p.as_os_str().is_empty()) {
        return xdg.join("esc");
    }
    if let Some(home) = home.filter(|p| !p.as_os_str().is_empty()) {
        return home.join(".cache").join("esc");
    }
    temp.join("esc")
}

/// The actual runtime cache directory using process environment.
fn runtime_cache_dir() -> PathBuf {
    let xdg = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    resolve_runtime_cache_dir(xdg.as_deref(), home.as_deref(), &std::env::temp_dir())
}

/// Generate the cache file name for the runtime archive.
///
/// Format: `libruntime-{version}-{sha256_hex_prefix}.a` (or `runtime.lib` on
/// Windows). The first 16 hex chars of the SHA-256 digest provide collision
/// resistance without excessively long filenames.
fn embedded_archive_name(bytes: &[u8]) -> String {
    let digest = sha256_hex(bytes);
    let ext = if cfg!(windows) { "lib" } else { "a" };
    format!(
        "libruntime-{}-{}.{ext}",
        env!("CARGO_PKG_VERSION"),
        &digest[..16]
    )
}

/// Extract `bytes` (the embedded runtime staticlib) into `cache_dir`.
///
/// Returns the path to the on-disk archive on success.
///
/// Cache hit: an archive with the same versioned, content-hashed name and
/// the expected file length already exists — it is reused without
/// re-writing.  Writes are atomic (temp file + rename), safe even when
/// multiple `esc` processes execute concurrently.
///
/// Returns `None` when `bytes` is empty (embedded archive unavailable).
pub(crate) fn extract_runtime_archive(bytes: &[u8], cache_dir: &Path) -> Option<PathBuf> {
    if bytes.is_empty() {
        return None;
    }
    let dest = cache_dir.join(embedded_archive_name(bytes));

    // Cache hit: file exists with the right length.
    if let Ok(meta) = dest.metadata()
        && meta.len() as usize == bytes.len()
    {
        return Some(dest);
    }

    // Cache miss — write atomically (temp file + rename).
    if std::fs::create_dir_all(cache_dir).is_err() {
        return None;
    }
    let tmp_name = format!(
        ".{}.tmp.{}",
        embedded_archive_name(bytes),
        std::process::id()
    );
    let tmp = cache_dir.join(tmp_name);
    if std::fs::write(&tmp, bytes).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    if std::fs::rename(&tmp, &dest).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(dest)
}

/// Extract the embedded runtime archive and return its absolute path.
///
/// Returns `None` when extraction fails or the embedded archive is empty
/// (e.g. driver built from a non-workspace package).
fn extract_embedded_runtime() -> Option<String> {
    if EMBEDDED_RUNTIME.is_empty() {
        return None;
    }
    let dir = runtime_cache_dir();
    match extract_runtime_archive(EMBEDDED_RUNTIME, &dir) {
        Some(path) => path
            .canonicalize()
            .ok()
            .map(|p| p.to_string_lossy().to_string()),
        None => {
            eprintln!(
                "warning: could not extract embedded runtime library to {}; link may fail",
                dir.display()
            );
            None
        }
    }
}

/// Check the FFI security gate after lowering.
///
/// If the lowered code uses FFI features (`has_ffi_usage`) and the
/// `allow_ffi` permission is not set, returns [`DriverError::FfiNotAllowed`]
/// (ESC-E700). If FFI is allowed, emits a warning (ESC-W700) to stderr.
///
/// # Errors
///
/// Returns [`DriverError::FfiNotAllowed`] if FFI is used without permission.
pub fn check_ffi_gate(config: &CompilerConfig, has_ffi_usage: bool) -> Result<(), DriverError> {
    // If FFI is enabled (--allow-ffi or config), always emit the warning
    if config.allow_ffi {
        let diag = diagnostics::Diagnostic::ffi_enabled_warning();
        eprintln!(
            "warning[{}]: {}",
            diag.code.map(|c| c.to_string()).unwrap_or_default(),
            diag.message
        );
        if let Some(ref help) = diag.help {
            eprintln!("  help: {help}");
        }
    }

    // If code uses FFI but permission is not granted, emit error
    if has_ffi_usage && !config.allow_ffi {
        return Err(DriverError::FfiNotAllowed);
    }

    Ok(())
}

/// Check whether the lowered program uses `eval()` or `Function()` and reject
/// if the compiler's permission settings forbid them.
///
/// - `--no-eval` (ESC-E400): rejects any `eval()` or `Function()` usage.
/// - `--no-jit` (ESC-E401): rejects `eval()`/`Function()` because they require
///   the JIT compiler, which has been excluded.
///
/// # Errors
///
/// Returns [`DriverError::EvalDisabled`] or [`DriverError::JitDisabled`] if the
/// source violates the configured permissions.
pub fn check_eval_permissions(
    config: &CompilerConfig,
    result: &desugar::LoweringResult,
) -> Result<(), DriverError> {
    let uses_dynamic_code = result.has_eval || result.has_function_constructor;
    if !uses_dynamic_code {
        return Ok(());
    }
    if !config.allow_eval {
        return Err(DriverError::EvalDisabled);
    }
    if !config.allow_jit {
        return Err(DriverError::JitDisabled);
    }
    Ok(())
}

/// Run the full compilation pipeline according to the given configuration.
///
/// Phases: load config -> read source -> resolve module graph -> lower
/// (parse + desugar) -> generator transform -> verify IR -> specialize ->
/// constfold -> codegen -> link.
///
/// For module files (`.mjs`/`.mts`), the pipeline first builds a module graph
/// to discover transitive dependencies. Single-file inputs with no imports
/// are compiled through the same fast path as before.
///
/// # Errors
///
/// Returns [`DriverError`] if any phase fails.
pub fn run_pipeline(config: &CompilerConfig) -> Result<CompileResult, DriverError> {
    // 0. Load esc.json and merge into config (CLI flags take precedence).
    let mut config = config.clone();
    crate::config::load_and_merge_config(&mut config);
    let config = &config;

    let mut timer = PhaseTimer::new(config.time_phases);

    // 1. Read source
    if config.input.is_empty() {
        return Err(DriverError::NoInput);
    }
    let input_path = &config.input[0];

    // 1a. For module files, build the module graph to discover dependencies.
    //     If the graph has multiple modules, use the multi-module path.
    let is_module = input_path.ends_with(".mjs") || input_path.ends_with(".mts");
    if is_module {
        timer.start("module_graph");
        let path = Path::new(input_path);
        if path.exists() {
            if let Ok(graph) = module_pipeline::build_module_graph(path)
                && graph.modules().len() > 1
            {
                timer.end("module_graph");
                // Multi-module path: lower all modules, then compile the entry.
                // For now, we still compile only the entry module's IR through
                // the rest of the pipeline; full multi-module merge comes in 0.5.4.
                timer.start("lower");
                let (lowered_modules, _export_map) = module_pipeline::lower_all_modules(&graph)?;
                timer.end("lower");

                // Find the entry module (last in topological order for a root module)
                let entry_result = lowered_modules
                    .into_iter()
                    .find(|m| m.path == path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
                    .ok_or_else(|| {
                        DriverError::Lowering(vec![
                            "entry module not found in lowered results".to_string(),
                        ])
                    })?;

                return run_pipeline_from_lowering(config, entry_result.lowering, timer);
            }
            timer.end("module_graph");
        }
    }

    timer.start("read");
    let source = std::fs::read_to_string(input_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            DriverError::FileNotFound(input_path.clone())
        } else {
            DriverError::Io(e)
        }
    })?;
    timer.end("read");

    // 2. Lower (parse + desugar)
    //    Use module mode (.mjs) or script mode (.js) based on extension.
    //    Script mode uses sloppy semantics by default; module mode is strict.
    timer.start("lower");
    let result = if is_module {
        desugar::lower_program(&source)
    } else {
        desugar::lower_script(&source)
    }
    .map_err(|errs| DriverError::Lowering(errs.iter().map(|e| e.to_string()).collect()))?;
    timer.end("lower");

    // 2. Deliberate refusals — constructs we decline to compile, each with a
    //    declared code. Checked before anything else so the diagnostic names the
    //    real cause rather than whatever fails downstream of it. Exits 2, not 1:
    //    a refusal and a compile failure must stay distinguishable.
    if !result.refusals.is_empty() {
        return Err(DriverError::Refused(
            result
                .refusals
                .iter()
                .map(|r| format!("error[{}]: {}", r.code, r.message))
                .collect(),
        ));
    }

    // 2a. FFI security gate — check before continuing the pipeline
    check_ffi_gate(config, result.has_ffi_usage)?;

    // 2b. Eval/JIT permission check
    check_eval_permissions(config, &result)?;

    // 3. Generator/async transform
    //    Rewrites generator functions into state machines (ramp + resume functions).
    //    Must run after desugaring and before verification.
    timer.start("generator_transform");
    let mut module = result.module;
    let _transform_results = generator_transform::transform_module(&mut module)
        .map_err(|e| DriverError::Codegen(e.to_string()))?;
    timer.end("generator_transform");

    // 4. Early exit for --emit ir (print before verify so we can debug IR)

    // 3a. --emit llvm-ir requires the llvm cargo feature
    #[cfg(not(feature = "llvm"))]
    if config.emit == Some(EmitKind::LlvmIr) {
        return Err(DriverError::LlvmNotAvailable);
    }
    if config.emit == Some(EmitKind::Ir) {
        let ir_text = ir::printer::print_typed_module(&module);
        println!("{ir_text}");
        // Still verify and report errors, but don't fail
        if let Err(errs) = ir::verify::verify_typed_module(&module) {
            for e in &errs {
                eprintln!("verify warning: {e}");
            }
        }
        return Ok(CompileResult {
            output_path: String::new(),
            phase_times: timer.finish(),
        });
    }

    // 5. Verify IR (now verifies the transformed IR)
    timer.start("verify");
    ir::verify::verify_typed_module(&module)
        .map_err(|errs| DriverError::Verification(errs.iter().map(|e| e.to_string()).collect()))?;
    timer.end("verify");

    // 6. Type inference + specialization
    //    Infer concrete types for all values, then rewrite generic JS opcodes
    //    (e.g. AddJS) to specialized native opcodes (e.g. AddF64) where both
    //    operands have a proven type.
    timer.start("specialize");
    types::specialize_module(&mut module);
    timer.end("specialize");

    // 7. Constant folding
    //    Evaluate operations on constant operands at compile time
    //    (e.g. AddF64(1.0, 2.0) → ConstF64(3.0)).
    timer.start("constfold");
    types::constfold_module(&mut module);
    timer.end("constfold");

    // 8. Codegen (Cranelift for Debug, LLVM for Release)
    timer.start("codegen");
    let object_bytes = match config.mode {
        #[cfg(feature = "llvm")]
        crate::CompileMode::Release => {
            let backend = llvm::codegen::LlvmBackend::new_release();
            backend
                .compile_module(&module, &result.string_table)
                .map_err(|e| DriverError::Codegen(e.to_string()))?
        }
        #[cfg(not(feature = "llvm"))]
        crate::CompileMode::Release => {
            return Err(DriverError::LlvmNotAvailable);
        }
        #[cfg(feature = "cranelift")]
        crate::CompileMode::Debug => {
            let backend = cranelift::CraneliftBackend::new()
                .map_err(|e| DriverError::Codegen(e.to_string()))?;
            backend
                .compile_module(&module, &result.string_table)
                .map_err(|e| DriverError::Codegen(e.to_string()))?
        }
    };
    timer.end("codegen");

    // 9. Write .o to tempdir
    timer.start("link");
    let temp_dir = tempfile::tempdir()?;
    let obj_path = temp_dir.path().join("module.o");
    std::fs::write(&obj_path, &object_bytes)?;

    // 10. Link
    let output_path = if config.output.is_empty() {
        "./a.out".to_string()
    } else {
        config.output.clone()
    };
    let linker_config = linker::LinkerConfig {
        format: linker::OutputFormat::Executable,
        output_path: output_path.clone(),
        objects: vec![obj_path.to_string_lossy().to_string()],
        runtime_lib: find_runtime_lib(),
    };
    linker::link(&linker_config)?;
    timer.end("link");

    Ok(CompileResult {
        output_path,
        phase_times: timer.finish(),
    })
}

/// Run the pipeline from a pre-lowered result (generator transform through link).
///
/// Used by the multi-module path to avoid duplicating the post-lowering phases.
fn run_pipeline_from_lowering(
    config: &CompilerConfig,
    result: desugar::LoweringResult,
    mut timer: PhaseTimer,
) -> Result<CompileResult, DriverError> {
    // 2a. FFI security gate — check before continuing the pipeline
    check_ffi_gate(config, result.has_ffi_usage)?;

    // 2b. Eval/JIT permission check
    check_eval_permissions(config, &result)?;

    // 3. Generator/async transform
    timer.start("generator_transform");
    let mut module = result.module;
    let _transform_results = generator_transform::transform_module(&mut module)
        .map_err(|e| DriverError::Codegen(e.to_string()))?;
    timer.end("generator_transform");

    // 4. Early exit for --emit ir

    // 3a. --emit llvm-ir requires the llvm cargo feature
    #[cfg(not(feature = "llvm"))]
    if config.emit == Some(EmitKind::LlvmIr) {
        return Err(DriverError::LlvmNotAvailable);
    }
    if config.emit == Some(EmitKind::Ir) {
        let ir_text = ir::printer::print_typed_module(&module);
        println!("{ir_text}");
        if let Err(errs) = ir::verify::verify_typed_module(&module) {
            for e in &errs {
                eprintln!("verify warning: {e}");
            }
        }
        return Ok(CompileResult {
            output_path: String::new(),
            phase_times: timer.finish(),
        });
    }

    // 5. Verify IR
    timer.start("verify");
    ir::verify::verify_typed_module(&module)
        .map_err(|errs| DriverError::Verification(errs.iter().map(|e| e.to_string()).collect()))?;
    timer.end("verify");

    // 6. Type inference + specialization
    timer.start("specialize");
    types::specialize_module(&mut module);
    timer.end("specialize");

    // 7. Constant folding
    timer.start("constfold");
    types::constfold_module(&mut module);
    timer.end("constfold");

    // 8. Codegen
    timer.start("codegen");
    let object_bytes = match config.mode {
        #[cfg(feature = "llvm")]
        crate::CompileMode::Release => {
            let backend = llvm::codegen::LlvmBackend::new_release();
            backend
                .compile_module(&module, &result.string_table)
                .map_err(|e| DriverError::Codegen(e.to_string()))?
        }
        #[cfg(not(feature = "llvm"))]
        crate::CompileMode::Release => {
            return Err(DriverError::LlvmNotAvailable);
        }
        #[cfg(feature = "cranelift")]
        crate::CompileMode::Debug => {
            let backend = cranelift::CraneliftBackend::new()
                .map_err(|e| DriverError::Codegen(e.to_string()))?;
            backend
                .compile_module(&module, &result.string_table)
                .map_err(|e| DriverError::Codegen(e.to_string()))?
        }
    };
    timer.end("codegen");

    // 9. Write .o to tempdir
    timer.start("link");
    let temp_dir = tempfile::tempdir()?;
    let obj_path = temp_dir.path().join("module.o");
    std::fs::write(&obj_path, &object_bytes)?;

    // 10. Link
    let output_path = if config.output.is_empty() {
        "./a.out".to_string()
    } else {
        config.output.clone()
    };
    let linker_config = linker::LinkerConfig {
        format: linker::OutputFormat::Executable,
        output_path: output_path.clone(),
        objects: vec![obj_path.to_string_lossy().to_string()],
        runtime_lib: find_runtime_lib(),
    };
    linker::link(&linker_config)?;
    timer.end("link");

    Ok(CompileResult {
        output_path,
        phase_times: timer.finish(),
    })
}
