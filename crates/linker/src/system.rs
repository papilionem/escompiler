//! System linker detection.
//!
//! Probes for a usable system C compiler/linker by trying `cc`, `gcc`,
//! and `clang` in order. The first one that responds to `--version` wins.
//!
//! ESC-8 / ESC-24: `cl.exe` (MSVC) is not a candidate — it rejects
//! `--version` and requires VCVARS environment setup. The Windows tier
//! targets MinGW `-gnu` which uses `gcc`/`cc` with GCC-compatible flags.

use std::path::PathBuf;
use std::process::Command;

use crate::error::LinkerError;

/// Candidate linker program names, tried in order.
///
/// On Windows, MinGW `gcc`/`cc` are preferred (ESC-24: msvc-vs-gnu ruling).
#[cfg(windows)]
const CANDIDATES: &[&str] = &["cc", "gcc", "clang"];

/// Candidate linker program names, tried in order.
#[cfg(not(windows))]
const CANDIDATES: &[&str] = &["cc", "gcc", "clang"];

/// Detect a usable system linker by probing candidates on `PATH` and
/// well-known installation directories.
///
/// Returns the path to the first linker that successfully responds to
/// `--version`, or [`LinkerError::NoLinkerFound`] if none work.
pub fn detect_linker() -> Result<PathBuf, LinkerError> {
    for &name in CANDIDATES {
        let ok = Command::new(name)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(PathBuf::from(name));
        }
    }

    // On Windows, check well-known install paths if PATH lookup failed
    #[cfg(windows)]
    {
        let well_known = [
            r"C:\Program Files\LLVM\bin\clang.exe",
            r"C:\Program Files (x86)\LLVM\bin\clang.exe",
            r"C:\msys64\mingw64\bin\gcc.exe",
            r"C:\msys64\ucrt64\bin\gcc.exe",
        ];
        for path in &well_known {
            let p = PathBuf::from(path);
            if p.exists() {
                return Ok(p);
            }
        }
    }

    Err(LinkerError::NoLinkerFound)
}
