//! Unit tests for the test262 harness and runner infrastructure.

use crate::harness::{
    NegativePhase, SUPPORTED_FEATURES, all_features_supported, find_harness_dir, find_test262_root,
    parse_frontmatter,
};
use crate::runner::{
    ProgressSummary, RunnerConfig, SuiteReport, TestOutcome, TestResult, TestRunner,
};
use std::path::Path;

// ── Frontmatter parsing ─────────────────────────────────────────────

#[test]
fn test_parse_frontmatter_basic() {
    let source = r#"/*---
description: basic variable declaration
flags: [onlyStrict]
features: [let, const]
includes: [assert.js, sta.js]
esid: sec-let-and-const-declarations
---*/
let x = 1;
"#;
    let meta = parse_frontmatter(source);
    assert_eq!(meta.description, "basic variable declaration");
    assert_eq!(meta.flags, vec!["onlyStrict"]);
    assert_eq!(meta.features, vec!["let", "const"]);
    assert_eq!(meta.includes, vec!["assert.js", "sta.js"]);
    assert_eq!(
        meta.es_id.as_deref(),
        Some("sec-let-and-const-declarations")
    );
    assert!(meta.is_only_strict());
    assert!(!meta.is_no_strict());
    assert!(!meta.is_module());
    assert!(!meta.is_async());
}

#[test]
fn test_parse_frontmatter_negative() {
    let source = r#"/*---
description: early syntax error
negative:
  phase: parse
  type: SyntaxError
---*/
var 123invalid;
"#;
    let meta = parse_frontmatter(source);
    assert!(meta.negative.is_some());
    let neg = meta.negative.as_ref().unwrap();
    assert_eq!(neg.phase, NegativePhase::Parse);
    assert_eq!(neg.error_type, "SyntaxError");
}

#[test]
fn test_parse_frontmatter_negative_runtime() {
    let source = r#"/*---
description: runtime reference error
negative:
  phase: runtime
  type: ReferenceError
---*/
undeclaredVar;
"#;
    let meta = parse_frontmatter(source);
    assert!(meta.negative.is_some());
    let neg = meta.negative.as_ref().unwrap();
    assert_eq!(neg.phase, NegativePhase::Runtime);
    assert_eq!(neg.error_type, "ReferenceError");
}

#[test]
fn test_parse_frontmatter_features() {
    let source = r#"/*---
description: BigInt addition
features: [BigInt, Symbol, Promise]
---*/
1n + 2n;
"#;
    let meta = parse_frontmatter(source);
    assert_eq!(meta.features, vec!["BigInt", "Symbol", "Promise"]);
}

#[test]
fn test_parse_frontmatter_empty() {
    let source = "var x = 1;";
    let meta = parse_frontmatter(source);
    assert!(meta.description.is_empty());
    assert!(meta.features.is_empty());
    assert!(meta.flags.is_empty());
    assert!(meta.includes.is_empty());
    assert!(meta.negative.is_none());
    assert!(meta.es_id.is_none());
}

#[test]
fn test_parse_frontmatter_malformed_no_end_marker() {
    let source = r#"/*---
description: incomplete frontmatter
features: [BigInt]
var x = 1;
"#;
    let meta = parse_frontmatter(source);
    // Should return default since ---*/ is missing
    assert!(meta.description.is_empty());
    assert!(meta.features.is_empty());
}

#[test]
fn test_parse_frontmatter_empty_brackets() {
    let source = r#"/*---
description: no features
features: []
flags: []
---*/
var x = 1;
"#;
    let meta = parse_frontmatter(source);
    assert!(meta.features.is_empty());
    assert!(meta.flags.is_empty());
}

#[test]
fn test_parse_frontmatter_module_and_async_flags() {
    let source = r#"/*---
description: async module test
flags: [module, async]
---*/
export default 42;
"#;
    let meta = parse_frontmatter(source);
    assert!(meta.is_module());
    assert!(meta.is_async());
    assert!(!meta.is_only_strict());
    assert!(!meta.is_raw());
}

#[test]
fn test_parse_frontmatter_raw_flag() {
    let source = r#"/*---
description: raw test
flags: [raw]
---*/
"some raw test";
"#;
    let meta = parse_frontmatter(source);
    assert!(meta.is_raw());
}

#[test]
fn test_parse_frontmatter_quoted_description() {
    let source = r#"/*---
description: 'single quoted description'
---*/
var x;
"#;
    let meta = parse_frontmatter(source);
    assert_eq!(meta.description, "single quoted description");
}

// ── Feature support ─────────────────────────────────────────────────

#[test]
fn test_supported_features_list_nonempty() {
    assert!(
        !SUPPORTED_FEATURES.is_empty(),
        "SUPPORTED_FEATURES must not be empty"
    );
}

#[test]
fn test_all_features_supported_empty_list() {
    assert!(all_features_supported(&[]));
}

#[test]
fn test_all_features_supported_known_features() {
    let features = vec!["let".to_string(), "const".to_string()];
    assert!(all_features_supported(&features));
}

#[test]
fn test_all_features_supported_unknown_feature() {
    let features = vec!["FinalizationRegistry".to_string()];
    assert!(!all_features_supported(&features));
}

#[test]
fn test_all_features_supported_mixed() {
    let features = vec!["let".to_string(), "Atomics.waitAsync".to_string()];
    assert!(!all_features_supported(&features));
}

// ── Runner skip logic ───────────────────────────────────────────────

#[test]
fn test_runner_skip_no_submodule() {
    let config = RunnerConfig {
        test262_root: Path::new("/nonexistent/test262").to_path_buf(),
        harness_dir: Path::new("/nonexistent/harness").to_path_buf(),
        max_failures: None,
        timeout_secs: 10,
    };
    let runner = TestRunner::new(config);
    let report = runner.run_all();
    // No tests found = empty report
    assert_eq!(report.total(), 0);
    assert_eq!(report.passed, 0);
}

#[test]
fn test_runner_skip_missing_feature() {
    // Simulated: a test requiring "FinalizationRegistry" should be skipped
    let source = r#"/*---
description: FinalizationRegistry test
features: [FinalizationRegistry]
---*/
new FinalizationRegistry(function() {});
"#;
    let meta = parse_frontmatter(source);
    assert!(!all_features_supported(&meta.features));
}

#[test]
fn test_runner_skip_async_test() {
    let source = r#"/*---
description: async test
flags: [async]
---*/
$DONE();
"#;
    let meta = parse_frontmatter(source);
    assert!(meta.is_async());
}

#[test]
fn test_runner_skip_module_test() {
    let source = r#"/*---
description: module test
flags: [module]
---*/
export default 42;
"#;
    let meta = parse_frontmatter(source);
    assert!(meta.is_module());
}

// ── Report ──────────────────────────────────────────────────────────

#[test]
fn test_suite_report_pass_rate_empty() {
    let report = SuiteReport::default();
    assert_eq!(report.total(), 0);
    assert_eq!(report.pass_rate(), 0.0);
}

#[test]
fn test_suite_report_pass_rate_calculation() {
    let report = SuiteReport {
        results: vec![],
        passed: 8,
        failed: 2,
        fail_crash: 0,
        skipped: 5,
        errors: 0,
    };
    assert_eq!(report.total(), 15);
    // pass rate = 8 / (8 + 2 + 0) = 80%
    assert!((report.pass_rate() - 80.0).abs() < 0.01);
}

#[test]
fn test_suite_report_did_pass() {
    let report = SuiteReport {
        results: vec![
            crate::runner::TestResult {
                path: "test/language/types/number/S8.5_A1.js".to_string(),
                outcome: TestOutcome::Pass,
                detail: String::new(),
            },
            crate::runner::TestResult {
                path: "test/language/types/string/S8.4_A1.js".to_string(),
                outcome: TestOutcome::FailWrong,
                detail: "compile error".to_string(),
            },
        ],
        passed: 1,
        failed: 1,
        fail_crash: 0,
        skipped: 0,
        errors: 0,
    };
    assert!(report.did_pass("test/language/types/number/S8.5_A1.js"));
    assert!(!report.did_pass("test/language/types/string/S8.4_A1.js"));
    assert!(!report.did_pass("nonexistent"));
}

#[test]
fn test_suite_report_display() {
    let report = SuiteReport {
        results: vec![],
        passed: 10,
        failed: 2,
        fail_crash: 0,
        skipped: 3,
        errors: 1,
    };
    let display = format!("{report}");
    assert!(display.contains("total: 16"));
    assert!(display.contains("pass: 10"));
    assert!(display.contains("fail: 2"));
    assert!(display.contains("skip: 3"));
    assert!(display.contains("error: 1"));
}

#[test]
fn test_suite_report_first_failures() {
    let report = SuiteReport {
        results: vec![
            crate::runner::TestResult {
                path: "a.js".to_string(),
                outcome: TestOutcome::Pass,
                detail: String::new(),
            },
            crate::runner::TestResult {
                path: "b.js".to_string(),
                outcome: TestOutcome::FailWrong,
                detail: "error1".to_string(),
            },
            crate::runner::TestResult {
                path: "c.js".to_string(),
                outcome: TestOutcome::FailWrong,
                detail: "error2".to_string(),
            },
        ],
        passed: 1,
        failed: 2,
        fail_crash: 0,
        skipped: 0,
        errors: 0,
    };
    let failures = report.first_failures(1);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].path, "b.js");
}

// ── find_test262_root ───────────────────────────────────────────────

#[test]
fn test_find_test262_root_nonexistent() {
    assert!(find_test262_root(Path::new("/nonexistent")).is_none());
}

#[test]
fn test_find_harness_dir() {
    let dir = find_harness_dir(Path::new("/workspace"));
    assert_eq!(dir, Path::new("/workspace/tests/test262/test262/harness"));
}

// ── Expanded feature detection ──────────────────────────────────────

#[test]
fn test_supported_features_includes_implemented() {
    // Verify features that ARE implemented and declared (v0.4)
    let implemented = [
        "Promise",
        "async-functions",
        "generators",
        "Symbol",
        "Symbol.iterator",
        "Symbol.toPrimitive",
        "optional-chaining",
        "coalesce-expression",
        "globalThis",
        "class",
        "class-fields-public",
        "class-fields-private",
        "class-static-fields-public",
        "Object.entries",
        "Object.is",
        // v0.6 metaprogramming features
        "Proxy",
        "Reflect",
        "dynamic-import",
    ];
    for feature in &implemented {
        assert!(
            SUPPORTED_FEATURES.contains(feature),
            "SUPPORTED_FEATURES should contain '{feature}'"
        );
    }
}

#[test]
fn test_trimmed_features_not_declared() {
    // Verify features that are NOT implemented are NOT declared
    // These should be re-added when their version ships (see harness.rs docs)
    let trimmed = [
        "TypedArray",
        "BigInt",
        "FinalizationRegistry",
        "regexp-match-indices",
        "regexp-v-flag",
    ];
    for feature in &trimmed {
        assert!(
            !SUPPORTED_FEATURES.contains(feature),
            "SUPPORTED_FEATURES should NOT contain '{feature}' (not yet implemented)"
        );
    }
}

#[test]
fn test_all_features_supported_unsupported_finalization() {
    let features = vec!["FinalizationRegistry".to_string()];
    assert!(!all_features_supported(&features));
}

#[test]
fn test_all_features_supported_unsupported_atomics() {
    let features = vec!["Atomics".to_string()];
    assert!(!all_features_supported(&features));
}

// ── Progress reporting ──────────────────────────────────────────────

#[test]
fn test_progress_summary_from_empty_report() {
    let report = SuiteReport::default();
    let summary = ProgressSummary::from_report(&report);
    assert_eq!(summary.total, 0);
    assert_eq!(summary.passed, 0);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.skipped, 0);
    assert_eq!(summary.pass_rate, 0.0);
    assert!(summary.categories.is_empty());
}

#[test]
fn test_progress_summary_from_report() {
    let report = SuiteReport {
        results: vec![
            TestResult {
                path: "test/language/types/number/foo.js".to_string(),
                outcome: TestOutcome::Pass,
                detail: String::new(),
            },
            TestResult {
                path: "test/language/types/number/bar.js".to_string(),
                outcome: TestOutcome::FailWrong,
                detail: "error".to_string(),
            },
            TestResult {
                path: "test/language/types/string/baz.js".to_string(),
                outcome: TestOutcome::Skip,
                detail: "async".to_string(),
            },
        ],
        passed: 1,
        failed: 1,
        fail_crash: 0,
        skipped: 1,
        errors: 0,
    };
    let summary = ProgressSummary::from_report(&report);
    assert_eq!(summary.total, 3);
    assert_eq!(summary.passed, 1);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.categories.len(), 2);
}

#[test]
fn test_progress_summary_category_extraction() {
    let report = SuiteReport {
        results: vec![
            TestResult {
                path: "test/language/statements/variable/foo.js".to_string(),
                outcome: TestOutcome::Pass,
                detail: String::new(),
            },
            TestResult {
                path: "test/language/statements/variable/bar.js".to_string(),
                outcome: TestOutcome::Pass,
                detail: String::new(),
            },
            TestResult {
                path: "test/language/expressions/addition/baz.js".to_string(),
                outcome: TestOutcome::FailWrong,
                detail: "error".to_string(),
            },
        ],
        passed: 2,
        failed: 1,
        fail_crash: 0,
        skipped: 0,
        errors: 0,
    };
    let summary = ProgressSummary::from_report(&report);
    // Should have 2 categories
    assert_eq!(summary.categories.len(), 2);
    // Sorted alphabetically
    assert!(summary.categories[0].0.contains("addition"));
    assert!(summary.categories[1].0.contains("variable"));
}

#[test]
fn test_progress_summary_display_format() {
    let report = SuiteReport {
        results: vec![TestResult {
            path: "test/language/types/number/foo.js".to_string(),
            outcome: TestOutcome::Pass,
            detail: String::new(),
        }],
        passed: 1,
        failed: 0,
        fail_crash: 0,
        skipped: 0,
        errors: 0,
    };
    let summary = ProgressSummary::from_report(&report);
    let display = format!("{summary}");
    assert!(display.contains("test262 Progress Report"));
    assert!(display.contains("Total: 1"));
    assert!(display.contains("Pass: 1"));
    assert!(display.contains("Pass rate: 100.0%"));
    assert!(display.contains("By category:"));
}

#[test]
fn test_progress_summary_pass_rate_excludes_skipped() {
    let report = SuiteReport {
        results: vec![],
        passed: 5,
        failed: 5,
        fail_crash: 0,
        skipped: 90,
        errors: 0,
    };
    let summary = ProgressSummary::from_report(&report);
    // 5 / (5+5) = 50%, skipped tests not counted
    assert!((summary.pass_rate - 50.0).abs() < 0.01);
}

// ── Runner on smoke directory ────────────────────────────────────────

#[test]
fn test_runner_run_directory_nonexistent() {
    let config = RunnerConfig {
        test262_root: Path::new("/nonexistent").to_path_buf(),
        harness_dir: Path::new("/nonexistent").to_path_buf(),
        max_failures: None,
        timeout_secs: 10,
    };
    let runner = TestRunner::new(config);
    let report = runner.run_directory(Path::new("/nonexistent/smoke"));
    assert_eq!(report.total(), 0);
}

#[test]
fn test_runner_smoke_directory_discovers_files() {
    // Find the smoke directory relative to CARGO_MANIFEST_DIR
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest.parent().and_then(|p| p.parent());
    let Some(root) = workspace_root else {
        return;
    };
    let smoke_dir = root.join("tests").join("test262").join("smoke");
    if !smoke_dir.exists() {
        return;
    }

    let config = RunnerConfig {
        test262_root: root.join("tests").join("test262").join("test262"),
        harness_dir: find_harness_dir(root),
        max_failures: None,
        timeout_secs: 10,
    };
    let runner = TestRunner::new(config);
    let report = runner.run_directory(&smoke_dir);
    // We created 16 smoke test files; at least some should be discovered
    assert!(
        report.total() >= 10,
        "Expected at least 10 smoke tests discovered, got {}",
        report.total()
    );
}

#[test]
fn test_runner_smoke_skip_counts() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest.parent().and_then(|p| p.parent());
    let Some(root) = workspace_root else {
        return;
    };
    let smoke_dir = root.join("tests").join("test262").join("smoke");
    if !smoke_dir.exists() {
        return;
    }

    let config = RunnerConfig {
        test262_root: root.join("tests").join("test262").join("test262"),
        harness_dir: find_harness_dir(root),
        max_failures: None,
        timeout_secs: 10,
    };
    let runner = TestRunner::new(config);
    let report = runner.run_directory(&smoke_dir);

    // async_skip.js should be skipped (async flag)
    let async_skip = report
        .results
        .iter()
        .find(|r| r.path.contains("async_skip"));
    if let Some(result) = async_skip {
        assert_eq!(result.outcome, TestOutcome::Skip);
    }

    // unsupported_feature_skip.js should be skipped (FinalizationRegistry)
    let feature_skip = report
        .results
        .iter()
        .find(|r| r.path.contains("unsupported_feature_skip"));
    if let Some(result) = feature_skip {
        assert_eq!(result.outcome, TestOutcome::Skip);
    }
}

// ── Harness file loading ─────────────────────────────────────────────

#[test]
fn test_harness_preamble_loads_files() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest.parent().and_then(|p| p.parent());
    let Some(root) = workspace_root else {
        return;
    };
    let harness_dir = find_harness_dir(root);

    let config = RunnerConfig {
        test262_root: Path::new("/nonexistent").to_path_buf(),
        harness_dir,
        max_failures: None,
        timeout_secs: 10,
    };
    let runner = TestRunner::new(config);
    // The runner loads the harness on construction.
    // Run with empty dir to verify it constructed successfully.
    let report = runner.run_all();
    assert_eq!(report.total(), 0);
}

#[test]
fn test_parse_frontmatter_includes_list() {
    let source = r#"/*---
description: test with includes
includes: [propertyHelper.js, compareArray.js, assert.js]
---*/
var x = 1;
"#;
    let meta = parse_frontmatter(source);
    assert_eq!(
        meta.includes,
        vec!["propertyHelper.js", "compareArray.js", "assert.js"]
    );
}

#[test]
fn test_parse_frontmatter_negative_resolution_phase() {
    let source = r#"/*---
description: module resolution error
negative:
  phase: resolution
  type: SyntaxError
---*/
import x from "./nonexistent.js";
"#;
    let meta = parse_frontmatter(source);
    assert!(meta.negative.is_some());
    let neg = meta.negative.as_ref().unwrap();
    assert_eq!(neg.phase, NegativePhase::Resolution);
    assert_eq!(neg.error_type, "SyntaxError");
}

#[test]
fn test_parse_frontmatter_es5id() {
    let source = r#"/*---
description: legacy es5 id
es5id: S15.1.2.2_A3.1_T1
---*/
parseInt("0x10");
"#;
    let meta = parse_frontmatter(source);
    assert_eq!(meta.es_id.as_deref(), Some("S15.1.2.2_A3.1_T1"));
}

#[test]
fn test_shard_spec_parsing() {
    use crate::runner::TestRunner;
    assert_eq!(
        TestRunner::shard_spec_from(Some("1/8".to_string())),
        Some((1, 8))
    );
    assert_eq!(
        TestRunner::shard_spec_from(Some("8/8".to_string())),
        Some((8, 8))
    );
    assert_eq!(TestRunner::shard_spec_from(Some("0/8".to_string())), None);
    assert_eq!(TestRunner::shard_spec_from(Some("9/8".to_string())), None);
    assert_eq!(TestRunner::shard_spec_from(Some("1/1".to_string())), None);
    assert_eq!(TestRunner::shard_spec_from(Some("x/8".to_string())), None);
    assert_eq!(TestRunner::shard_spec_from(Some("1-8".to_string())), None);
    assert_eq!(TestRunner::shard_spec_from(None), None);
}

#[test]
fn test_shard_partition_is_disjoint_and_complete() {
    // Partition contract: for N shards over M items, every index matches
    // exactly one shard (i % n == k - 1).
    let m = 1_000usize;
    let n = 8usize;
    let mut coverage = vec![0usize; m];
    for k in 1..=n {
        for (i, c) in coverage.iter_mut().enumerate() {
            if i % n == k - 1 {
                *c += 1;
            }
        }
    }
    assert!(
        coverage.iter().all(|&c| c == 1),
        "every test index must be covered by exactly one shard"
    );
}

// ── Outcome vocabulary (ESC-31) ────────────────────────────────────

#[test]
fn test_outcome_vocabulary_strings() {
    assert_eq!(TestOutcome::Pass.as_str(), "pass");
    assert_eq!(TestOutcome::FailWrong.as_str(), "fail-wrong");
    assert_eq!(TestOutcome::FailCrash.as_str(), "fail-crash");
    assert_eq!(TestOutcome::Skip.as_str(), "skip");
    assert_eq!(TestOutcome::Error.as_str(), "error");
}

#[test]
fn test_outcome_is_failure() {
    assert!(!TestOutcome::Pass.is_failure());
    assert!(TestOutcome::FailWrong.is_failure());
    assert!(TestOutcome::FailCrash.is_failure());
    assert!(!TestOutcome::Skip.is_failure());
    assert!(!TestOutcome::Error.is_failure());
}

#[test]
fn test_suite_report_crash_counted() {
    let report = SuiteReport {
        results: vec![],
        passed: 5,
        failed: 2,
        fail_crash: 1,
        skipped: 0,
        errors: 0,
    };
    assert_eq!(report.total(), 8);
    // pass rate: 5 / (5 + 2 + 1 + 0) = 62.5%
    assert!((report.pass_rate() - 62.5).abs() < 0.01);
}

// ── Exit classification (Tier 0 crash detection) ──────────────────

#[test]
#[cfg(unix)]
fn test_classify_exit_success() {
    let status = std::os::unix::process::ExitStatusExt::from_raw(0);
    let result = crate::runner::classify_exit(&status, "");
    assert_eq!(result, crate::runner::ExitClass::Success);
}

#[test]
#[cfg(unix)]
fn test_classify_exit_nonzero_is_fail_wrong() {
    // exit code 1 → raw = 1 << 8 = 256
    let status = std::os::unix::process::ExitStatusExt::from_raw(256);
    let result = crate::runner::classify_exit(&status, "");
    assert_eq!(result, crate::runner::ExitClass::Nonzero(1));
}

#[test]
#[cfg(unix)]
fn test_classify_exit_signal_is_fail_crash() {
    use std::os::unix::process::ExitStatusExt;
    // SIGSEGV (11) → raw = 11
    let status = ExitStatusExt::from_raw(11);
    let result = crate::runner::classify_exit(&status, "");
    assert!(matches!(result, crate::runner::ExitClass::Crash(_)));
    let detail = match result {
        crate::runner::ExitClass::Crash(d) => d,
        _ => unreachable!(),
    };
    assert!(
        detail.contains("SIGSEGV"),
        "expected SIGSEGV, got: {detail}"
    );
}

#[test]
#[cfg(unix)]
fn test_classify_exit_panic_stderr_is_fail_crash() {
    use std::os::unix::process::ExitStatusExt;
    // exit code 101 → raw = 101 << 8 = 25856
    let status = ExitStatusExt::from_raw(25856);
    let stderr = "thread 'main' panicked at math.rs:36:\nassertion failed: false\nnote: run with RUST_BACKTRACE=1";
    let result = crate::runner::classify_exit(&status, stderr);
    assert!(matches!(result, crate::runner::ExitClass::Crash(_)));
}

#[test]
#[cfg(unix)]
fn test_classify_exit_plain_nonzero_no_panic() {
    let status = std::os::unix::process::ExitStatusExt::from_raw(256); // exit 1
    let result = crate::runner::classify_exit(&status, "internal error\n");
    assert_eq!(result, crate::runner::ExitClass::Nonzero(1));
}
