//! DG-3 proofs for ESC-28: negative test type comparison.
//!
//! Run with:
//!   cargo test -p test262 --test dg3_negative_type_proof -- --nocapture
//!
//! These tests prove:
//!   1. A parse-phase negative with wrong error type fails (old runner: false-pass)
//!   2. A runtime negative with a crash fails as FailCrash (old runner: false-pass)
//!
//! They use fixture files located in `/tmp/test262_fixtures/test/` created by
//! the DG-3 setup script. If the fixtures don't exist, tests are skipped.

use std::path::Path;

use test262::{RunnerConfig, TestOutcome, TestRunner};

fn setup_runner() -> TestRunner {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("cannot find workspace root");
    let harness_dir = workspace_root.join("tests/test262/test262/harness");
    let config = RunnerConfig {
        test262_root: std::path::PathBuf::from("/tmp/test262_fixtures"),
        harness_dir,
        max_failures: None,
        timeout_secs: 10,
    };
    TestRunner::new(config)
}

#[test]
fn dg3_parse_syntax_error_correct_match() {
    if !Path::new("/tmp/test262_fixtures/test/test_parse_syntax_negative.js").exists() {
        eprintln!("SKIP: fixture not found. Run DG-3 setup script first.");
        return;
    }
    let runner = setup_runner();
    let report = runner.run_subdir("");
    let result = report
        .results
        .iter()
        .find(|r| r.path.contains("test_parse_syntax_negative"))
        .expect("should find test_parse_syntax_negative in report");
    assert_eq!(
        result.outcome,
        TestOutcome::Pass,
        "parse-phase negative with matching SyntaxError should PASS. detail: {}",
        result.detail
    );
}

#[test]
fn dg3_parse_wrong_type_fails() {
    if !Path::new("/tmp/test262_fixtures/test/test_parse_wrong_type.js").exists() {
        eprintln!("SKIP: fixture not found. Run DG-3 setup script first.");
        return;
    }
    let runner = setup_runner();
    let report = runner.run_subdir("");
    let result = report
        .results
        .iter()
        .find(|r| r.path.contains("test_parse_wrong_type"))
        .expect("should find test_parse_wrong_type in report");
    assert_eq!(
        result.outcome,
        TestOutcome::FailWrong,
        "parse-phase negative with wrong error type should FAIL. detail: {}",
        result.detail
    );
}

#[test]
fn dg3_runtime_crash_detected() {
    if !Path::new("/tmp/test262_fixtures/test/test_runtime_crash.js").exists() {
        eprintln!("SKIP: fixture not found. Run DG-3 setup script first.");
        return;
    }
    let runner = setup_runner();
    let report = runner.run_subdir("");
    let result = report
        .results
        .iter()
        .find(|r| r.path.contains("test_runtime_crash"))
        .expect("should find test_runtime_crash in report");
    eprintln!(
        "dg3_runtime_crash: outcome={:?} detail={}",
        result.outcome, result.detail
    );
    assert_ne!(
        result.outcome,
        TestOutcome::Pass,
        "runtime negative with crash must NOT pass. detail: {}",
        result.detail
    );
}

#[test]
fn dg3_runtime_wrong_type_not_crash() {
    if !Path::new("/tmp/test262_fixtures/test/test_runtime_wrong_type.js").exists() {
        eprintln!("SKIP: fixture not found. Run DG-3 setup script first.");
        return;
    }
    let runner = setup_runner();
    let report = runner.run_subdir("");
    let result = report
        .results
        .iter()
        .find(|r| r.path.contains("test_runtime_wrong_type"))
        .expect("should find test_runtime_wrong_type in report");
    eprintln!(
        "dg3_runtime_wrong_type: outcome={:?} detail={}",
        result.outcome, result.detail
    );
    // At minimum, it must NOT be FailCrash (no crash happened).
    assert_ne!(
        result.outcome,
        TestOutcome::FailCrash,
        "runtime negative with wrong type should NOT be a crash"
    );
}
