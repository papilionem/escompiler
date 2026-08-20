//! Integration test — runs the test262 suite if the submodule is present.
//!
//! Skips gracefully when the `test262/` data directory has not been cloned,
//! making this safe to run in CI even without the submodule.

use std::path::Path;

use test262::{RunnerConfig, TestRunner};

/// Known-passing test paths that MUST NOT regress.
///
/// This list is intentionally empty for now — add entries as tests begin
/// passing to prevent regressions.
const MUST_PASS_TESTS: &[&str] = &[];

#[test]
#[ignore] // Run explicitly: cargo test -p test262 --test integration -- --ignored --nocapture
fn test262_regression_suite() {
    // Locate workspace root (crates/test262 -> workspace root)
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent());

    let workspace_root = match workspace_root {
        Some(root) => root,
        None => {
            eprintln!("test262: cannot determine workspace root, skipping");
            return;
        }
    };

    let test262_root = workspace_root.join("tests").join("test262").join("test262");
    if !test262_root.exists() {
        eprintln!(
            "test262: data directory not present at {}, skipping",
            test262_root.display()
        );
        eprintln!(
            "test262: clone with: git clone https://github.com/nicolo-ribaudo/tc39-test262-parser-tests.git tests/test262/test262"
        );
        return;
    }

    let config = RunnerConfig {
        test262_root: test262_root.clone(),
        harness_dir: workspace_root
            .join("tests")
            .join("test262")
            .join("test262")
            .join("harness"),
        max_failures: Some(100),
        timeout_secs: 10,
    };

    let runner = TestRunner::new(config);
    let report = runner.run_all();

    // Print summary
    println!("{report}");

    // Regression guard: all known-passing tests must still pass
    for test_name in MUST_PASS_TESTS {
        assert!(
            report.did_pass(test_name),
            "Regression: {test_name} should pass but did not"
        );
    }
}
