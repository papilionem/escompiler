//! System linker invocation for producing native binaries.
//!
//! This crate detects the system C compiler/linker (`cc`, `gcc`, or `clang`),
//! adds platform-specific flags, and invokes it to link compiled object files
//! into an executable, shared library, or static archive.
//!
//! # Key types
//!
//! - [`LinkerConfig`] — configuration for a single link invocation
//! - [`OutputFormat`] — what kind of output to produce
//! - [`LinkerError`] — errors that can occur during linking

/// Linker error types.
pub mod error;
/// Platform-specific linker flags.
pub mod platform;
/// System linker detection.
pub mod system;

#[cfg(test)]
mod tests;

pub use error::LinkerError;

use std::process::Command;

/// The kind of output artifact to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// A standalone executable binary.
    Executable,
    /// A shared/dynamic library (`.so` / `.dylib`).
    SharedLib,
    /// A static library (`.a`).
    StaticLib,
    /// A relocatable object file (`.o`).
    ObjectFile,
}

/// Configuration for a single linker invocation.
#[derive(Debug, Clone)]
pub struct LinkerConfig {
    /// What kind of output to produce.
    pub format: OutputFormat,
    /// Path where the linked output should be written.
    pub output_path: String,
    /// Paths to object files (`.o`) to link together.
    pub objects: Vec<String>,
    /// Optional path to a runtime support library to link in.
    pub runtime_lib: Option<String>,
}

/// Extra linker arguments taken from the environment, for CI-side link
/// acceleration. Kept env-driven (not hardcoded) so the macOS/Windows tiers
/// and contributors without a fast linker are unaffected by default.
///
/// - `ESC_LINK_FUSE_LD=<name>` selects an alternate linker via `-fuse-ld=<name>`
///   (e.g. `lld` for rust-lld, `mold`). Escompiler's test262 path is
///   link-dominated (a large static runtime linked once per test), so this is
///   the single biggest CI lever.
/// - `ESC_LINK_ARGS` is a whitespace-separated list of extra flags, e.g.
///   `-Wl,--strip-debug` (these are throwaway conformance binaries).
///
/// Only applied to real link steps (executables and shared libraries); static
/// archives and relocatable objects are unaffected.
fn extra_link_args_from(
    fuse_ld: Option<&str>,
    args: Option<&str>,
    format: OutputFormat,
) -> Vec<String> {
    if !matches!(format, OutputFormat::Executable | OutputFormat::SharedLib) {
        return Vec::new();
    }
    let mut extra = Vec::new();
    if let Some(ld) = fuse_ld {
        let ld = ld.trim();
        if !ld.is_empty() {
            extra.push(format!("-fuse-ld={ld}"));
        }
    }
    if let Some(args) = args {
        extra.extend(args.split_whitespace().map(str::to_string));
    }
    extra
}

/// Read [`extra_link_args_from`] inputs from the process environment.
fn env_extra_link_args(format: OutputFormat) -> Vec<String> {
    extra_link_args_from(
        std::env::var("ESC_LINK_FUSE_LD").ok().as_deref(),
        std::env::var("ESC_LINK_ARGS").ok().as_deref(),
        format,
    )
}

/// Build the linker `Command` for `config`, appending `extra` flags last.
fn build_link_command(
    linker: &std::path::Path,
    config: &LinkerConfig,
    extra: &[String],
) -> Command {
    let mut cmd = Command::new(linker);
    cmd.arg("-o").arg(&config.output_path);

    // Add format-specific flags
    match config.format {
        OutputFormat::SharedLib => {
            cmd.arg("-shared");
        }
        OutputFormat::StaticLib => {
            // Static archives are typically created with `ar`, not the linker.
            // For now, pass -static as a hint; full ar support is Phase C+.
            cmd.arg("-static");
        }
        OutputFormat::ObjectFile => {
            cmd.arg("-r"); // relocatable output
        }
        OutputFormat::Executable => {}
    }

    for obj in &config.objects {
        cmd.arg(obj);
    }

    // Add runtime library if provided
    if let Some(ref rt_lib) = config.runtime_lib {
        cmd.arg(rt_lib);
        // When linking against a shared runtime (libruntime.so/.dylib), embed
        // an rpath to its directory so the produced binary can locate it at
        // run time without LD_LIBRARY_PATH. Static archives need no rpath.
        if (rt_lib.ends_with(".so") || rt_lib.ends_with(".dylib"))
            && let Some(dir) = std::path::Path::new(rt_lib).parent()
        {
            cmd.arg(format!("-Wl,-rpath,{}", dir.display()));
        }
    }

    cmd.args(platform::platform_flags());
    cmd.args(extra);
    cmd
}

/// Invoke the system linker to produce the final output.
///
/// Detects the system linker, adds platform-specific flags, applies any
/// `ESC_LINK_*` environment overrides (see [`extra_link_args_from`]), and links
/// the configured object files into the requested output format. If a link with
/// environment overrides fails, it is retried once with the default linker, so a
/// misconfigured `ESC_LINK_FUSE_LD`/`ESC_LINK_ARGS` degrades gracefully instead
/// of hard-failing the build.
///
/// # Errors
///
/// Returns [`LinkerError`] if no linker is found, no objects are provided,
/// no output path is set, or the linker process fails.
pub fn link(config: &LinkerConfig) -> Result<(), LinkerError> {
    if config.objects.is_empty() {
        return Err(LinkerError::NoObjects);
    }
    if config.output_path.is_empty() {
        return Err(LinkerError::NoOutputPath);
    }

    let linker = system::detect_linker()?;
    let extra = env_extra_link_args(config.format);

    let output = build_link_command(&linker, config, &extra).output()?;
    if output.status.success() {
        return Ok(());
    }

    // Graceful fallback: a bad -fuse-ld or extra flag must not brick the build.
    if !extra.is_empty() {
        eprintln!(
            "[linker] link with ESC_LINK overrides {extra:?} failed (code {:?}); \
             retrying with the default linker",
            output.status.code(),
        );
        let retried = build_link_command(&linker, config, &[]).output()?;
        if retried.status.success() {
            return Ok(());
        }
        return Err(LinkerError::LinkFailed {
            code: retried.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&retried.stderr).to_string(),
        });
    }

    Err(LinkerError::LinkFailed {
        code: output.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}
