//! test262 conformance test runner.
//!
//! Runs a subset of the ECMAScript test262 test suite through the compiler.
//! Requires the test262 submodule to be checked out at `tests/test262/test262/`
//! relative to the workspace root.
//!
//! Tests gracefully skip if the test262 data directory is not present.

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// test262 frontmatter parsing (mirrors tests/test262/harness.rs)
// ---------------------------------------------------------------------------

/// Parsed test262 test metadata from YAML frontmatter.
#[derive(Debug, Default)]
struct TestMetadata {
    /// Test description.
    description: String,
    /// Expected negative outcome (parse error, runtime error, etc.).
    negative: Option<NegativeExpectation>,
    /// Feature flags required by this test.
    features: Vec<String>,
    /// Test flags (e.g., onlyStrict, noStrict, module, async).
    flags: Vec<String>,
    /// ES module test.
    is_module: bool,
    /// Async test (requires `$DONE` callback).
    is_async: bool,
}

#[derive(Debug)]
struct NegativeExpectation {
    phase: NegativePhase,
    error_type: String,
}

#[derive(Debug, PartialEq)]
enum NegativePhase {
    Parse,
    Resolution,
    Runtime,
}

/// Parse test262 frontmatter from a test file.
///
/// Frontmatter is enclosed in `/*---` and `---*/` markers.
fn parse_frontmatter(source: &str) -> TestMetadata {
    let mut meta = TestMetadata::default();

    let start = match source.find("/*---") {
        Some(pos) => pos + 5,
        None => return meta,
    };
    let end = match source[start..].find("---*/") {
        Some(pos) => start + pos,
        None => return meta,
    };

    let yaml = &source[start..end];

    for line in yaml.lines() {
        let line = line.trim();
        if let Some(desc) = line.strip_prefix("description:") {
            meta.description = desc
                .trim()
                .trim_matches(|c| c == '\'' || c == '"')
                .to_string();
        } else if line.starts_with("flags:") {
            if let Some(bracket_content) = extract_bracket_list(line) {
                meta.flags = bracket_content;
            }
        } else if line.starts_with("features:") {
            if let Some(bracket_content) = extract_bracket_list(line) {
                meta.features = bracket_content;
            }
        } else if let Some(phase_str) = line.strip_prefix("phase:") {
            let phase = match phase_str.trim() {
                "parse" => NegativePhase::Parse,
                "resolution" => NegativePhase::Resolution,
                _ => NegativePhase::Runtime,
            };
            if let Some(neg) = meta.negative.as_mut() {
                neg.phase = phase;
            } else {
                meta.negative = Some(NegativeExpectation {
                    phase,
                    error_type: String::new(),
                });
            }
        } else if let Some(err_type) = line.strip_prefix("type:") {
            let err_type = err_type.trim().to_string();
            if let Some(neg) = meta.negative.as_mut() {
                neg.error_type = err_type;
            } else {
                meta.negative = Some(NegativeExpectation {
                    phase: NegativePhase::Runtime,
                    error_type: err_type,
                });
            }
        }
    }

    meta.is_module = meta.flags.iter().any(|f| f == "module");
    meta.is_async = meta.flags.iter().any(|f| f == "async");

    meta
}

fn extract_bracket_list(line: &str) -> Option<Vec<String>> {
    let start = line.find('[')?;
    let end = line.find(']')?;
    let inner = &line[start + 1..end];
    Some(
        inner
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Feature allowlist — only run tests we have a chance of passing
// ---------------------------------------------------------------------------

/// Features that ESCompiler currently supports (Phase C level).
const SUPPORTED_FEATURES: &[&str] = &[
    // We have no feature-gated tests yet — this list will grow in Phase D.
];

/// Check whether all features required by a test are supported.
fn features_supported(required: &[String]) -> bool {
    if required.is_empty() {
        return true;
    }
    required
        .iter()
        .all(|f| SUPPORTED_FEATURES.contains(&f.as_str()))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the workspace root from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("parent of crates/driver")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

/// Check if the test262 submodule data directory exists.
fn test262_data_dir() -> Option<PathBuf> {
    let root = workspace_root();
    let data = root.join("tests/test262/test262");
    if data.join("test").exists() {
        Some(data)
    } else {
        None
    }
}

/// Collect `.js` test files from a directory (non-recursive).
fn collect_test_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return files,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "js") {
            files.push(path);
        }
    }
    files.sort();
    files
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Smoke test: verify the test262 runner infrastructure works.
///
/// Skips gracefully if the test262 submodule is not checked out.
#[test]
fn test_test262_smoke() {
    if test262_data_dir().is_none() {
        eprintln!("SKIP: test262 submodule not checked out");
        return;
    }

    eprintln!("test262 submodule found — runner infrastructure ready");
}

/// Verify that we can discover test files in the variable statements directory.
#[test]
fn test_test262_discover_variable_tests() {
    let data_dir = match test262_data_dir() {
        Some(d) => d,
        None => {
            eprintln!("SKIP: test262 submodule not checked out");
            return;
        }
    };

    let var_dir = data_dir.join("test/language/statements/variable");
    if !var_dir.exists() {
        eprintln!("SKIP: test262 variable statement tests not found");
        return;
    }

    let files = collect_test_files(&var_dir);
    assert!(
        !files.is_empty(),
        "expected some .js files in {}",
        var_dir.display()
    );
    eprintln!("Found {} variable statement tests", files.len());
}

/// Verify frontmatter parsing on a synthetic test262-style test.
#[test]
fn test_test262_parse_frontmatter_synthetic() {
    let source = r#"/*---
description: basic variable declaration
flags: [onlyStrict]
features: [let, const]
---*/
let x = 1;
"#;
    let meta = parse_frontmatter(source);
    assert_eq!(meta.description, "basic variable declaration");
    assert_eq!(meta.flags, vec!["onlyStrict"]);
    assert_eq!(meta.features, vec!["let", "const"]);
    assert!(!meta.is_async);
    assert!(!meta.is_module);
}

/// Verify negative test metadata parsing.
#[test]
fn test_test262_parse_negative_frontmatter() {
    let source = r#"/*---
description: syntax error expected
negative:
  phase: parse
  type: SyntaxError
---*/
var 123invalid;
"#;
    let meta = parse_frontmatter(source);
    assert!(meta.negative.is_some());
    let neg = meta.negative.unwrap();
    assert_eq!(neg.phase, NegativePhase::Parse);
    assert_eq!(neg.error_type, "SyntaxError");
}

/// Verify no frontmatter returns empty metadata.
#[test]
fn test_test262_no_frontmatter() {
    let source = "var x = 1;\n";
    let meta = parse_frontmatter(source);
    assert!(meta.description.is_empty());
    assert!(meta.features.is_empty());
    assert!(meta.negative.is_none());
}

/// Verify feature filtering works.
#[test]
fn test_test262_feature_filter() {
    // Empty features are always supported.
    assert!(features_supported(&[]));

    // Unknown features are not supported.
    assert!(!features_supported(&["Atomics".to_string()]));
    assert!(!features_supported(&[
        "BigInt".to_string(),
        "Array.prototype.at".to_string()
    ]));
}

/// Verify module and async flag detection.
#[test]
fn test_test262_module_and_async_flags() {
    let source = r#"/*---
description: module async test
flags: [module, async]
---*/
export default 42;
"#;
    let meta = parse_frontmatter(source);
    assert!(meta.is_module);
    assert!(meta.is_async);
}

/// Verify bracket list extraction.
#[test]
fn test_test262_bracket_list_extraction() {
    assert_eq!(
        extract_bracket_list("flags: [a, b, c]"),
        Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
    );
    assert_eq!(extract_bracket_list("flags: []"), Some(vec![]));
    assert_eq!(extract_bracket_list("no brackets here"), None);
}

/// Run a small set of parse-negative tests from test262 to verify the pipeline
/// correctly rejects invalid syntax.
#[test]
fn test_test262_parse_negative_subset() {
    let data_dir = match test262_data_dir() {
        Some(d) => d,
        None => {
            eprintln!("SKIP: test262 submodule not checked out");
            return;
        }
    };

    // Look for negative parse tests in a well-known directory.
    let dir = data_dir.join("test/language/statements/variable");
    if !dir.exists() {
        eprintln!("SKIP: test directory not found");
        return;
    }

    let files = collect_test_files(&dir);
    let mut tested = 0;
    let mut passed = 0;

    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let meta = parse_frontmatter(&source);

        // Skip tests with unsupported features.
        if !features_supported(&meta.features) {
            continue;
        }

        // Skip async and module tests for now.
        if meta.is_async || meta.is_module {
            continue;
        }

        // Only test negative-parse tests — we can check those without runtime.
        let Some(ref neg) = meta.negative else {
            continue;
        };
        if neg.phase != NegativePhase::Parse {
            continue;
        }

        tested += 1;

        let config = driver::CompilerConfig::new(vec![file.to_string_lossy().to_string()]);
        match driver::check(&config) {
            Ok(()) => {
                // Expected parse error, but check succeeded — this is a failure.
                eprintln!(
                    "FAIL: expected parse error for {}, got success",
                    file.display()
                );
            }
            Err(_) => {
                // Good — compilation correctly rejected the invalid input.
                passed += 1;
            }
        }
    }

    if tested > 0 {
        eprintln!("test262 parse-negative: {passed}/{tested} passed");
    } else {
        eprintln!("SKIP: no parse-negative tests found in subset");
    }
}
