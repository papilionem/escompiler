//! test262 test harness utilities.
//!
//! Parses test262 metadata (frontmatter), classifies tests, and provides
//! the `$262` host object stubs needed by the test suite.

use std::path::Path;

/// Parsed test262 test metadata from YAML frontmatter.
#[derive(Debug, Clone, Default)]
pub struct TestMetadata {
    /// Test description.
    pub description: String,
    /// Expected negative outcome (parse error, runtime error, etc.).
    pub negative: Option<NegativeExpectation>,
    /// Feature flags required by this test.
    pub features: Vec<String>,
    /// Test flags (e.g., onlyStrict, noStrict, module, async).
    pub flags: Vec<String>,
    /// ES module test.
    pub is_module: bool,
    /// Async test (requires $DONE callback).
    pub is_async: bool,
}

#[derive(Debug, Clone)]
pub struct NegativeExpectation {
    pub phase: NegativePhase,
    pub error_type: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NegativePhase {
    Parse,
    Resolution,
    Runtime,
}

/// Parse test262 frontmatter from a test file.
///
/// Frontmatter is enclosed in `/*---` and `---*/` markers.
pub fn parse_frontmatter(source: &str) -> TestMetadata {
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
            meta.description = desc.trim().trim_matches(|c| c == '\'' || c == '"').to_string();
        } else if line.starts_with("flags:") {
            // flags: [onlyStrict, async]
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

/// Check if test262 data directory exists.
pub fn test262_data_dir() -> Option<&'static Path> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .join("tests")
        .join("test262")
        .join("test262");

    // We leak the path to get a 'static reference — this is fine for test infra.
    let path: &'static Path = Box::leak(path.into_boxed_path());
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_frontmatter() {
        let source = r#"/*---
description: basic test
flags: [onlyStrict]
features: [BigInt, Array.prototype.at]
---*/
var x = 1;
"#;
        let meta = parse_frontmatter(source);
        assert_eq!(meta.description, "basic test");
        assert_eq!(meta.flags, vec!["onlyStrict"]);
        assert_eq!(meta.features, vec!["BigInt", "Array.prototype.at"]);
        assert!(!meta.is_async);
        assert!(!meta.is_module);
    }

    #[test]
    fn parse_negative_frontmatter() {
        let source = r#"/*---
description: syntax error test
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

    #[test]
    fn no_frontmatter() {
        let source = "var x = 1;";
        let meta = parse_frontmatter(source);
        assert!(meta.description.is_empty());
        assert!(meta.features.is_empty());
    }
}
