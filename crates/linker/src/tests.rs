//! Tests for linker.

use crate::error::LinkerError;
use crate::platform::platform_flags;
use crate::system::detect_linker;
use crate::{LinkerConfig, OutputFormat, extra_link_args_from, link};

// ---------------------------------------------------------------------------
// ESC_LINK_* environment overrides
// ---------------------------------------------------------------------------

#[test]
fn test_extra_link_args_fuse_ld_and_flags_for_executable() {
    let got = extra_link_args_from(
        Some("lld"),
        Some("-Wl,--strip-debug"),
        OutputFormat::Executable,
    );
    assert_eq!(got, vec!["-fuse-ld=lld", "-Wl,--strip-debug"]);
}

#[test]
fn test_extra_link_args_applies_to_shared_lib() {
    let got = extra_link_args_from(Some("mold"), None, OutputFormat::SharedLib);
    assert_eq!(got, vec!["-fuse-ld=mold"]);
}

#[test]
fn test_extra_link_args_skipped_for_static_and_object() {
    assert!(extra_link_args_from(Some("lld"), Some("-x"), OutputFormat::StaticLib).is_empty());
    assert!(extra_link_args_from(Some("lld"), Some("-x"), OutputFormat::ObjectFile).is_empty());
}

#[test]
fn test_extra_link_args_empty_and_whitespace_inputs() {
    // No env set at all.
    assert!(extra_link_args_from(None, None, OutputFormat::Executable).is_empty());
    // Blank fuse-ld is ignored; multiple flags split on whitespace.
    let got = extra_link_args_from(Some("  "), Some("-a   -b\t-c"), OutputFormat::Executable);
    assert_eq!(got, vec!["-a", "-b", "-c"]);
}

// ---------------------------------------------------------------------------
// System linker detection
// ---------------------------------------------------------------------------

#[test]
fn test_detect_system_cc() {
    // On most Unix systems, at least `cc` should be available.
    // If not, this test is still valid — it exercises the detection logic.
    let result = detect_linker();
    // We just verify it doesn't panic. On CI without a compiler, it returns Err.
    if let Ok(path) = &result {
        let name = path.to_string_lossy();
        // On Windows, detect_linker may return a full path (e.g. C:\Program Files\LLVM\bin\clang.exe)
        let basename = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        assert!(
            name == "cc"
                || name == "gcc"
                || name == "clang"
                || basename == "cc"
                || basename == "gcc"
                || basename == "clang",
            "detected linker should be one of cc/gcc/clang, got: {name}"
        );
    }
}

// ---------------------------------------------------------------------------
// LinkerConfig validation
// ---------------------------------------------------------------------------

#[test]
fn test_linker_config_no_objects_error() {
    let config = LinkerConfig {
        format: OutputFormat::Executable,
        output_path: "a.out".to_string(),
        objects: vec![],
        runtime_lib: None,
    };
    let result = link(&config);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, LinkerError::NoObjects),
        "expected NoObjects, got: {err}"
    );
}

#[test]
fn test_linker_config_no_output_error() {
    let config = LinkerConfig {
        format: OutputFormat::Executable,
        output_path: String::new(),
        objects: vec!["test.o".to_string()],
        runtime_lib: None,
    };
    let result = link(&config);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, LinkerError::NoOutputPath),
        "expected NoOutputPath, got: {err}"
    );
}

#[test]
fn test_linker_config_both_empty_returns_no_objects() {
    // When both objects and output are empty, NoObjects should be checked first.
    let config = LinkerConfig {
        format: OutputFormat::Executable,
        output_path: String::new(),
        objects: vec![],
        runtime_lib: None,
    };
    let result = link(&config);
    assert!(matches!(result, Err(LinkerError::NoObjects)));
}

// ---------------------------------------------------------------------------
// Platform flags
// ---------------------------------------------------------------------------

#[test]
fn test_platform_flags_not_empty() {
    let flags = platform_flags();
    // On Linux or macOS, we expect at least one flag.
    if cfg!(target_os = "linux") || cfg!(target_os = "macos") {
        assert!(!flags.is_empty(), "platform flags should not be empty");
    }
}

#[test]
fn test_platform_flags_linux() {
    if cfg!(target_os = "linux") {
        let flags = platform_flags();
        assert!(flags.contains(&"-lm".to_string()));
        assert!(flags.contains(&"-lpthread".to_string()));
    }
}

#[test]
fn test_platform_flags_macos() {
    if cfg!(target_os = "macos") {
        let flags = platform_flags();
        assert!(flags.contains(&"-lSystem".to_string()));
    }
}

// ---------------------------------------------------------------------------
// Error display
// ---------------------------------------------------------------------------

#[test]
fn test_linker_error_display_no_linker_found() {
    let err = LinkerError::NoLinkerFound;
    let msg = err.to_string();
    assert!(msg.contains("no system linker found"));
    assert!(msg.contains("cc"));
}

#[test]
fn test_linker_error_display_link_failed() {
    let err = LinkerError::LinkFailed {
        code: 1,
        stderr: "undefined reference to `main`".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("exit code 1"));
    assert!(msg.contains("undefined reference"));
}

#[test]
fn test_linker_error_display_no_objects() {
    let err = LinkerError::NoObjects;
    assert_eq!(err.to_string(), "no object files provided");
}

#[test]
fn test_linker_error_display_no_output_path() {
    let err = LinkerError::NoOutputPath;
    assert_eq!(err.to_string(), "output path not specified");
}

#[test]
fn test_linker_error_display_io() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let err = LinkerError::Io(io_err);
    let msg = err.to_string();
    assert!(msg.contains("I/O error"));
    assert!(msg.contains("file not found"));
}

// ---------------------------------------------------------------------------
// OutputFormat
// ---------------------------------------------------------------------------

#[test]
fn test_output_format_debug() {
    assert_eq!(format!("{:?}", OutputFormat::Executable), "Executable");
    assert_eq!(format!("{:?}", OutputFormat::SharedLib), "SharedLib");
    assert_eq!(format!("{:?}", OutputFormat::StaticLib), "StaticLib");
    assert_eq!(format!("{:?}", OutputFormat::ObjectFile), "ObjectFile");
}

#[test]
fn test_output_format_equality() {
    assert_eq!(OutputFormat::Executable, OutputFormat::Executable);
    assert_ne!(OutputFormat::Executable, OutputFormat::SharedLib);
    assert_ne!(OutputFormat::StaticLib, OutputFormat::ObjectFile);
}

// ---------------------------------------------------------------------------
// LinkerConfig construction
// ---------------------------------------------------------------------------

#[test]
fn test_linker_config_with_runtime_lib() {
    let config = LinkerConfig {
        format: OutputFormat::Executable,
        output_path: "out".to_string(),
        objects: vec!["a.o".to_string()],
        runtime_lib: Some("librt.a".to_string()),
    };
    assert_eq!(config.runtime_lib.as_deref(), Some("librt.a"));
}

// ---------------------------------------------------------------------------
// Platform flags: -ldl and -lgcc_s on Linux
// ---------------------------------------------------------------------------

#[test]
fn test_platform_flags_linux_includes_dl() {
    if cfg!(target_os = "linux") {
        let flags = platform_flags();
        assert!(
            flags.contains(&"-ldl".to_string()),
            "Linux flags should include -ldl"
        );
    }
}

#[test]
fn test_platform_flags_linux_includes_gcc_s() {
    if cfg!(target_os = "linux") {
        let flags = platform_flags();
        assert!(
            flags.contains(&"-lgcc_s".to_string()),
            "Linux flags should include -lgcc_s"
        );
    }
}
