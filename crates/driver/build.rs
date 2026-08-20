//! Build script for the driver crate.
//!
//! Builds the runtime staticlib via a nested `cargo build --release` into
//! `$OUT_DIR/runtime-target` and exports the archive path so the driver can
//! embed it with `include_bytes!`. This makes `esc build` self-contained
//! after `cargo install` (no on-disk `libruntime.a` needed — ESC-59).
//!
//! **crates.io packaging (M8):** when the runtime crate manifest is absent,
//! writing an empty placeholder into `OUT_DIR` keeps `include_bytes!` compiling
//! and the embedded fallback degrades to today's `None` behaviour silently.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .unwrap_or_else(|e| panic!("CARGO_MANIFEST_DIR is not set: {e}")),
    );
    let out_dir = PathBuf::from(
        std::env::var("OUT_DIR").unwrap_or_else(|e| panic!("OUT_DIR is not set: {e}")),
    );
    let workspace_root = manifest_dir.join("..").join("..");

    let lib_name = if cfg!(windows) {
        "runtime.lib"
    } else {
        "libruntime.a"
    };

    let runtime_manifest_path = workspace_root
        .join("crates")
        .join("runtime")
        .join("Cargo.toml");

    // Rebuild only when the runtime source or manifest changes.
    if runtime_manifest_path.exists() {
        let runtime_src = workspace_root.join("crates").join("runtime").join("src");
        println!("cargo:rerun-if-changed={}", runtime_src.display());
        println!("cargo:rerun-if-changed={}", runtime_manifest_path.display());
    }

    let archive = if runtime_manifest_path.exists() {
        build_runtime(&workspace_root, &runtime_manifest_path, &out_dir, lib_name)
    } else {
        // crates.io / standalone packaging: emit a warning and provide an empty
        // placeholder so include_bytes! still compiles.
        println!(
            "cargo:warning=crates/runtime/Cargo.toml not found at {} — will not embed runtime staticlib; users must provide libruntime.a via CARGO_TARGET_DIR or next to the esc binary",
            runtime_manifest_path.display()
        );
        placeholder_archive(&out_dir)
    };

    println!("cargo:rustc-env=ESC_RUNTIME_A={}", archive.display());
}

/// Build the runtime staticlib in release mode inside `$OUT_DIR/runtime-target`.
///
/// Uses the `CARGO` env var so the same cargo binary is used (important when a
/// custom cargo wrapper is in play).
fn build_runtime(
    _workspace_root: &Path,
    manifest_path: &Path,
    out_dir: &Path,
    lib_name: &str,
) -> PathBuf {
    let target_dir = out_dir.join("runtime-target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(&cargo)
        .arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(manifest_path)
        .arg("--target-dir")
        .arg(&target_dir)
        // Do NOT inherit CARGO_TARGET_DIR from the outer build, otherwise the
        // child would write into the workspace target dir and deadlock the
        // outer cargo which holds the workspace-and-target-directory lock.
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn nested cargo build for runtime: {e}"));

    if !status.success() {
        panic!(
            "nested runtime build ({} build --release) failed with {status}",
            cargo
        );
    }

    let archive = target_dir.join("release").join(lib_name);
    if !archive.exists() {
        panic!(
            "expected runtime staticlib at {} after successful build, but it is missing",
            archive.display()
        );
    }
    archive
}

/// Write an empty file so `include_bytes!` compiles when the runtime crate is
/// not available (future crates.io packaging).
fn placeholder_archive(out_dir: &Path) -> PathBuf {
    let p = out_dir.join("empty-runtime.a");
    std::fs::write(&p, b"").unwrap_or_else(|e| {
        panic!(
            "failed to write empty placeholder archive at {}: {e}",
            p.display()
        )
    });
    p
}
