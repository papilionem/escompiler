//! Integration test runner for escompiler.
//!
//! Runs .js fixture files through the compiler pipeline and compares
//! actual output against expected output in `.expected` sidecar files
//! or `@expected-stdout` annotations within the source.

use std::fs;
use std::path::{Path, PathBuf};

/// A single test case loaded from a .js fixture file.
#[derive(Debug)]
pub struct TestCase {
    /// Path to the .js file.
    pub path: PathBuf,
    /// Source code content.
    pub source: String,
    /// Expected stdout (from annotation or sidecar file).
    pub expected_stdout: Option<String>,
    /// Expected stderr (from `@expected-stderr` annotation).
    pub expected_stderr: Option<String>,
    /// Expected exit code (from `@expected-exit-code` annotation).
    pub expected_exit_code: Option<i32>,
    /// Whether this test is expected to fail compilation.
    pub expect_error: bool,
    /// Expected error substring (if expect_error is true).
    pub expected_error: Option<String>,
}

impl TestCase {
    /// Load a test case from a .js file path.
    pub fn load(path: &Path) -> Self {
        let source = fs::read_to_string(path).expect("failed to read test fixture");
        let expected_stdout = Self::extract_expected_stdout(&source, path);
        let expected_stderr = Self::extract_annotation(&source, "@expected-stderr");
        let expected_exit_code = Self::extract_annotation(&source, "@expected-exit-code")
            .and_then(|s| s.parse::<i32>().ok());
        let expect_error = source.contains("@expect-error");
        let expected_error = Self::extract_annotation(&source, "@expect-error");

        Self {
            path: path.to_path_buf(),
            source,
            expected_stdout,
            expected_stderr,
            expected_exit_code,
            expect_error,
            expected_error,
        }
    }

    /// Extract expected stdout from either an annotation or a sidecar file.
    fn extract_expected_stdout(source: &str, path: &Path) -> Option<String> {
        // Check for inline annotation: // @expected-stdout: <value>
        if let Some(val) = Self::extract_annotation(source, "@expected-stdout") {
            return Some(val);
        }

        // Check for multi-line block annotation:
        // // @expected-stdout-begin
        // // line1
        // // line2
        // // @expected-stdout-end
        if let Some(val) = Self::extract_block_annotation(source, "@expected-stdout-begin", "@expected-stdout-end") {
            return Some(val);
        }

        // Check for .expected sidecar file
        let expected_path = path.with_extension("expected");
        if expected_path.exists() {
            return Some(fs::read_to_string(expected_path).expect("failed to read .expected file"));
        }

        None
    }

    fn extract_annotation(source: &str, tag: &str) -> Option<String> {
        for line in source.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("//") {
                let rest = rest.trim();
                if let Some(val) = rest.strip_prefix(tag) {
                    let val = val.trim_start_matches(':').trim();
                    if !val.is_empty() {
                        return Some(val.to_string());
                    }
                }
            }
        }
        None
    }

    fn extract_block_annotation(source: &str, begin_tag: &str, end_tag: &str) -> Option<String> {
        let mut in_block = false;
        let mut lines = Vec::new();

        for line in source.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("//") {
                let rest = rest.trim();
                if rest.contains(begin_tag) {
                    in_block = true;
                    continue;
                }
                if rest.contains(end_tag) {
                    break;
                }
                if in_block {
                    lines.push(rest.to_string());
                }
            }
        }

        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }
}

/// Discover all .js test fixtures in a directory (recursively).
pub fn discover_fixtures(dir: &Path) -> Vec<PathBuf> {
    let mut fixtures = Vec::new();
    if !dir.exists() {
        return fixtures;
    }
    collect_js_files(dir, &mut fixtures);
    fixtures.sort();
    fixtures
}

fn collect_js_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_js_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "js") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_inline_annotation() {
        let source = r#"// @expected-stdout: hello world
console.log("hello world");
"#;
        let val = TestCase::extract_annotation(source, "@expected-stdout");
        assert_eq!(val, Some("hello world".to_string()));
    }

    #[test]
    fn extract_block_annotation() {
        let source = r#"// @expected-stdout-begin
// line 1
// line 2
// @expected-stdout-end
console.log("line 1");
console.log("line 2");
"#;
        let val = TestCase::extract_block_annotation(source, "@expected-stdout-begin", "@expected-stdout-end");
        assert_eq!(val, Some("line 1\nline 2".to_string()));
    }

    #[test]
    fn no_annotation() {
        let source = "var x = 1;";
        let val = TestCase::extract_annotation(source, "@expected-stdout");
        assert_eq!(val, None);
    }
}
