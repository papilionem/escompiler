//! Standalone test262 baseline — runs broad categories and prints summary.
//!
//! Three modes:
//!   Quick (~5 min):  cargo test -p test262 --test test262_baseline -- --ignored --nocapture test262_quick
//!   Full  (~18 min): cargo test -p test262 --test test262_baseline -- --ignored --nocapture test262_full_baseline
//!   CI    (auto):    cargo test -p test262 --test test262_baseline -- --ignored --nocapture test262_ci
//!
//! Environment variables:
//!   TEST262_SEQUENTIAL=1  — Force sequential execution (default: parallel)
//!   TEST262_CATEGORIES=language/types,built-ins/Math  — Run only specific categories
//!   TEST262_CI=1          — Enable threshold enforcement (fail if passes drop below threshold)

use std::path::Path;
use std::time::Instant;

use test262::{RunnerConfig, TestRunner};

/// Small, representative subset (~4,700 tests) for fast feedback after changes.
/// Covers core language, operators, key statements, and a sample of built-ins.
const QUICK_CATEGORIES: &[&str] = &[
    // Core language (~600 tests)
    "language/types",
    "language/statements/variable",
    "language/statements/if",
    "language/statements/for",
    "language/statements/switch",
    "language/statements/try",
    // Key expressions (~400 tests)
    "language/expressions/assignment",
    "language/expressions/addition",
    "language/expressions/equals",
    "language/expressions/call",
    "language/expressions/object",
    "language/expressions/conditional",
    // Representative built-ins (~800 tests)
    "built-ins/Math",
    "built-ins/Number",
    "built-ins/JSON",
    "built-ins/Boolean",
    "built-ins/Function",
    // Control flow (~400 tests)
    "language/statements/for-in",
    "language/statements/let",
    "language/statements/const",
];

/// Full 85-category baseline for release validation.
const FULL_CATEGORIES: &[&str] = &[
    // Original 13 core categories (Phase G Step 6)
    "language/types",
    "language/statements/variable",
    "language/statements/if",
    "language/statements/for",
    "language/statements/while",
    "language/statements/block",
    "language/statements/return",
    "language/statements/switch",
    "language/statements/try",
    "language/statements/throw",
    "language/expressions/typeof",
    "language/expressions/assignment",
    "language/expressions/conditional",
    // Arithmetic operators
    "language/expressions/addition",
    "language/expressions/subtraction",
    "language/expressions/multiplication",
    "language/expressions/division",
    "language/expressions/modulus",
    "language/expressions/exponentiation",
    "language/expressions/unary-minus",
    "language/expressions/unary-plus",
    // Increment/decrement
    "language/expressions/postfix-increment",
    "language/expressions/postfix-decrement",
    "language/expressions/prefix-increment",
    "language/expressions/prefix-decrement",
    // Equality operators
    "language/expressions/equals",
    "language/expressions/strict-equals",
    "language/expressions/does-not-equals",
    "language/expressions/strict-does-not-equals",
    // Relational operators
    "language/expressions/less-than",
    "language/expressions/greater-than",
    "language/expressions/less-than-or-equal",
    "language/expressions/greater-than-or-equal",
    "language/expressions/in",
    "language/expressions/instanceof",
    // Logical operators
    "language/expressions/logical-and",
    "language/expressions/logical-or",
    "language/expressions/coalesce",
    // Bitwise operators
    "language/expressions/bitwise-and",
    "language/expressions/bitwise-or",
    "language/expressions/bitwise-xor",
    "language/expressions/bitwise-not",
    "language/expressions/left-shift",
    "language/expressions/right-shift",
    "language/expressions/unsigned-right-shift",
    // Unary/misc operators
    "language/expressions/delete",
    "language/expressions/void",
    "language/expressions/comma",
    "language/expressions/grouping",
    "language/expressions/concatenation",
    // Call/new/member
    "language/expressions/call",
    "language/expressions/new",
    "language/expressions/property-accessors",
    "language/expressions/optional-chaining",
    // Literal expressions
    "language/expressions/object",
    "language/expressions/array",
    "language/expressions/function",
    "language/expressions/arrow-function",
    "language/expressions/template-literal",
    "language/expressions/tagged-template",
    // Additional statements
    "language/statements/do-while",
    "language/statements/empty",
    "language/statements/expression",
    "language/statements/labeled",
    "language/statements/with",
    "language/statements/debugger",
    "language/statements/continue",
    "language/statements/break",
    "language/statements/let",
    "language/statements/const",
    // NOTE: language/statements/class excluded — 4,367 tests, ~10min alone
    "language/statements/for-in",
    "language/statements/for-of",
    "language/statements/async-function",
    "language/statements/async-generator",
    // v0.6 categories (metaprogramming + dynamic features)
    "language/eval-code",
    "built-ins/Proxy",
    "built-ins/Reflect",
    "built-ins/eval",
    "built-ins/Function",
    // v0.7 categories (stdlib essential)
    "built-ins/Math",
    "built-ins/Number",
    // v0.7 Wave 4: expanded stdlib categories
    "built-ins/String",
    "built-ins/JSON",
    "built-ins/Object",
    "built-ins/Array",
    "built-ins/Boolean",
    // v0.8 Wave 7: collections + RegExp + Date categories
    "built-ins/Map",
    "built-ins/Set",
    "built-ins/WeakMap",
    "built-ins/WeakSet",
    "built-ins/WeakRef",
    "built-ins/RegExp",
    "built-ins/Date",
    // v0.9 Session 7: expanded coverage — previously untracked categories
    "language/statements/function",
    "language/statements/generators",
    "language/expressions/compound-assignment",
    "language/expressions/generators",
    "language/expressions/yield",
    "language/expressions/super",
    "language/arguments-object",
    "language/identifiers",
    "language/function-code",
    "language/literals/numeric",
    "language/literals/string",
    "language/literals/regexp",
    "built-ins/Error",
    "built-ins/NativeErrors",
    "built-ins/parseInt",
    "built-ins/parseFloat",
    "built-ins/Symbol",
    "built-ins/global",
    "built-ins/GeneratorPrototype",
    "built-ins/GeneratorFunction",
];

/// Threshold configuration loaded from `test262-threshold.json`.
#[derive(Debug)]
struct Threshold {
    min_pass: usize,
    /// Maximum allowed fail-crash count — one-directional ratchet DOWN toward zero.
    /// Tier 0: no spec-compliant program may crash.
    max_fail_crash: usize,
}

/// CI gate mode is active only when TEST262_CI is exactly "1".
/// (`is_ok()` would enforce on TEST262_CI=0 too — wrong.)
fn ci_mode() -> bool {
    std::env::var("TEST262_CI").ok().as_deref() == Some("1")
}

/// Load threshold from `test262-threshold.json` at the workspace root.
fn load_threshold(mode: &str) -> Option<Threshold> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())?;
    let threshold_path = workspace_root.join("test262-threshold.json");
    let content = std::fs::read_to_string(&threshold_path).ok()?;

    // Minimal JSON parsing — avoid adding serde_json as a dependency.
    // Format: { "quick": { "min_pass": N }, "full": { "min_pass": N }, "max_fail_crash": N }
    let mode_key = format!("\"{}\"", mode);
    let mode_start = content.find(&mode_key)?;
    let min_pass = parse_usize_field(&content, "\"min_pass\"", mode_start)?;
    // max_fail_crash is a top-level field, search from position 0.
    let max_fail_crash = parse_usize_field(&content, "\"max_fail_crash\"", 0).unwrap_or(9999);

    Some(Threshold {
        min_pass,
        max_fail_crash,
    })
}

/// Parse `"key": <digits>` from JSON content starting at `from` offset.
fn parse_usize_field(content: &str, key: &str, from: usize) -> Option<usize> {
    let pos = content[from..].find(key)?;
    let after = &content[from + pos + key.len()..];
    let colon = after.find(':')?;
    let num = after[colon + 1..].trim_start();
    let end = num.find(|c: char| !c.is_ascii_digit()).unwrap_or(num.len());
    num[..end].parse().ok()
}
fn setup_runner() -> Option<TestRunner> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("cannot find workspace root");
    let test262_root = workspace_root.join("tests/test262/test262");
    if !test262_root.exists() {
        // Fail closed in CI: a broken/missing clone must never produce a green gate.
        assert!(
            !ci_mode(),
            "test262 repo missing at tests/test262/test262 but TEST262_CI=1 — the gate must fail, not skip"
        );
        eprintln!("test262 not cloned, skipping");
        return None;
    }

    let config = RunnerConfig {
        test262_root,
        harness_dir: workspace_root.join("tests/test262/test262/harness"),
        max_failures: None,
        timeout_secs: std::env::var("TEST262_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10),
    };
    Some(TestRunner::new(config))
}
/// Run categories and return (pass, fail, skip, fail_crash, crash_paths) totals.
fn run_categories(
    runner: &TestRunner,
    categories: &[&str],
) -> (usize, usize, usize, usize, Vec<String>) {
    // Allow overriding categories via env var
    let env_categories = std::env::var("TEST262_CATEGORIES").ok();
    let categories: Vec<&str> = if let Some(ref env_cats) = env_categories {
        env_cats.split(',').map(|s| s.trim()).collect()
    } else {
        categories.to_vec()
    };

    let mut total_pass = 0usize;
    let mut total_fail = 0usize;
    let mut total_skip = 0usize;
    let mut total_fail_crash = 0usize;
    let mut all_crashes: Vec<String> = Vec::new();
    let mut cat_rows: Vec<(String, usize, usize, usize, usize)> = Vec::new();
    let run_start = Instant::now();

    for cat in &categories {
        let cat_start = Instant::now();
        let report = runner.run_subdir(cat);
        let cat_elapsed = cat_start.elapsed();

        if report.passed + report.failed + report.fail_crash + report.skipped == 0 {
            continue;
        }
        let total = report.passed + report.failed + report.fail_crash + report.skipped;
        let pct = if total > 0 {
            report.passed as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        println!(
            "  {:50} pass={:4} fail={:4} crash={:3} skip={:4} ({:.1}%) [{:.1}s]",
            cat,
            report.passed,
            report.failed,
            report.fail_crash,
            report.skipped,
            pct,
            cat_elapsed.as_secs_f64()
        );
        cat_rows.push((
            cat.to_string(),
            report.passed,
            report.failed,
            report.skipped,
            report.fail_crash,
        ));
        total_pass += report.passed;
        total_fail += report.failed;
        total_skip += report.skipped;
        total_fail_crash += report.fail_crash;
        for result in &report.results {
            if result.outcome == test262::TestOutcome::FailCrash {
                all_crashes.push(result.path.clone());
            }
        }
    }

    let total = total_pass + total_fail + total_fail_crash + total_skip;
    let pct = if total > 0 {
        total_pass as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    let elapsed = run_start.elapsed();
    println!("\n=== BASELINE ===");
    println!(
        "  PASS: {} / {} ({:.1}%)  FAIL: {}  FAIL-CRASH: {}  SKIP: {}",
        total_pass, total, pct, total_fail, total_fail_crash, total_skip
    );
    println!(
        "  Time: {:.1}s ({:.0}ms/test)",
        elapsed.as_secs_f64(),
        if total > 0 {
            elapsed.as_millis() as f64 / total as f64
        } else {
            0.0
        }
    );

    // Machine-readable report for shard aggregation in CI. Written when
    // TEST262_REPORT_JSON is set; shards upload it as an artifact and the
    // aggregate step sums `passed` across shards before threshold enforcement.
    if let Ok(report_path) = std::env::var("TEST262_REPORT_JSON") {
        let report_path = if Path::new(&report_path).is_absolute() {
            std::path::PathBuf::from(report_path)
        } else {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(|p| p.parent())
                .expect("cannot find workspace root")
                .join(report_path)
        };
        let shard = std::env::var("TEST262_SHARD").unwrap_or_default();
        let mut json = String::from("{\n");
        json.push_str(&format!("  \"shard\": \"{shard}\",\n"));
        json.push_str(&format!("  \"passed\": {total_pass},\n"));
        json.push_str(&format!("  \"failed\": {total_fail},\n"));
        json.push_str(&format!("  \"skipped\": {total_skip},\n"));
        json.push_str(&format!("  \"fail_crash\": {total_fail_crash},\n"));
        json.push_str("  \"crashes\": [");
        for (i, path) in all_crashes.iter().enumerate() {
            let comma = if i + 1 < all_crashes.len() { "," } else { "" };
            json.push_str(&format!("\n    \"{path}\"{comma}"));
        }
        json.push_str("\n  ],\n");
        json.push_str("  \"categories\": [\n");
        for (i, (cat, p, f, s, fc)) in cat_rows.iter().enumerate() {
            let comma = if i + 1 < cat_rows.len() { "," } else { "" };
            json.push_str(&format!(
                "    {{\"name\": \"{cat}\", \"passed\": {p}, \"failed\": {f}, \"skipped\": {s}, \"fail_crash\": {fc}}}{comma}\n"
            ));
        }
        json.push_str("  ]\n}\n");
        if let Err(e) = std::fs::write(&report_path, json) {
            eprintln!("failed to write test262 report to {report_path:?}: {e}");
        }
    }

    (
        total_pass,
        total_fail,
        total_skip,
        total_fail_crash,
        all_crashes,
    )
}
#[test]
#[ignore] // cargo test -p test262 --test test262_baseline -- --ignored --nocapture test262_quick
fn test262_quick() {
    let Some(runner) = setup_runner() else {
        return;
    };
    if ci_mode() {
        assert!(
            load_threshold("quick").is_some(),
            "TEST262_CI=1 but test262-threshold.json is missing or corrupt — \
             the gate must fail, not skip"
        );
    }
    println!(
        "\n=== QUICK MODE ({} categories) ===\n",
        QUICK_CATEGORIES.len()
    );
    let (pass, _fail, _skip, fail_crash, _crashes) = run_categories(&runner, QUICK_CATEGORIES);

    if ci_mode() {
        let threshold = load_threshold("quick").unwrap_or_else(|| {
            panic!(
                "TEST262_CI=1 but test262-threshold.json is missing or corrupt — \
                 the gate must fail, not skip"
            )
        });
        println!(
            "\n=== CI THRESHOLD CHECK (quick) ===\n  passes: {}  threshold: {}\n  fail-crash: {}  ratchet: {}",
            pass, threshold.min_pass, fail_crash, threshold.max_fail_crash
        );
        assert!(
            pass >= threshold.min_pass,
            "REGRESSION: test262 quick passes ({}) dropped below threshold ({}). \
             If this is expected, update test262-threshold.json.",
            pass,
            threshold.min_pass
        );
        assert!(
            fail_crash <= threshold.max_fail_crash,
            "CRASH RATCHET: test262 quick fail-crash ({}) exceeds baseline ({}). \
             Crashes are Tier 0 violations — fix the crash; do not raise the ratchet.",
            fail_crash,
            threshold.max_fail_crash
        );
        println!("  PASSED — no regression detected");
    }
}

#[test]
#[ignore] // cargo test -p test262 --test test262_baseline -- --ignored --nocapture test262_full_baseline
fn test262_full_baseline() {
    let Some(runner) = setup_runner() else {
        return;
    };
    if ci_mode() {
        assert!(
            load_threshold("full").is_some(),
            "TEST262_CI=1 but test262-threshold.json is missing or corrupt — \
             the gate must fail, not skip"
        );
    }
    println!(
        "\n=== FULL BASELINE ({} categories) ===\n",
        FULL_CATEGORIES.len()
    );
    let (pass, _fail, _skip, fail_crash, _crashes) = run_categories(&runner, FULL_CATEGORIES);

    if ci_mode() {
        let threshold = load_threshold("full").unwrap_or_else(|| {
            panic!(
                "TEST262_CI=1 but test262-threshold.json is missing or corrupt — \
                 the gate must fail, not skip"
            )
        });
        println!(
            "\n=== CI THRESHOLD CHECK (full) ===\n  passes: {}  threshold: {}\n  fail-crash: {}  ratchet: {}",
            pass, threshold.min_pass, fail_crash, threshold.max_fail_crash
        );
        assert!(
            pass >= threshold.min_pass,
            "REGRESSION: test262 full passes ({}) dropped below threshold ({}). \
             If this is expected, update test262-threshold.json.",
            pass,
            threshold.min_pass
        );
        assert!(
            fail_crash <= threshold.max_fail_crash,
            "CRASH RATCHET: test262 full fail-crash ({}) exceeds baseline ({}). \
             Crashes are Tier 0 violations — fix the crash; do not raise the ratchet.",
            fail_crash,
            threshold.max_fail_crash
        );
        println!("  PASSED — no regression detected");
    }
}
#[test]
#[ignore] // cargo test -p test262 --test test262_baseline -- --ignored --nocapture test262_defprop_failures
fn test262_defprop_failures() {
    let Some(runner) = setup_runner() else {
        return;
    };
    println!("\n=== defineProperty Failure Dump ===\n");
    let report = runner.run_subdir("built-ins/Object/defineProperty");
    println!(
        "Pass: {}  Fail: {}  Skip: {}",
        report.passed, report.failed, report.skipped
    );

    let limit: usize = std::env::var("DEFPROP_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);

    let mut count = 0;
    for result in &report.results {
        if result.outcome == test262::TestOutcome::FailWrong {
            count += 1;
            if count > limit {
                break;
            }
            println!("\n--- FAIL #{count}: {} ---", result.path);
            // Print first 4 lines of error detail
            for line in result.detail.lines().take(4) {
                println!("  {line}");
            }
        }
    }
}

#[test]
#[ignore] // cargo test -p test262 --test test262_baseline -- --ignored --nocapture test262_isnan_failures
fn test262_isnan_failures() {
    let Some(runner) = setup_runner() else {
        return;
    };
    println!("\n=== isNaN/isFinite Failure Dump ===\n");

    for cat in &["built-ins/isNaN", "built-ins/isFinite"] {
        let report = runner.run_subdir(cat);
        println!(
            "\n{cat}: Pass: {}  Fail: {}  Skip: {}",
            report.passed, report.failed, report.skipped
        );
        let mut count = 0;
        for result in &report.results {
            if result.outcome == test262::TestOutcome::FailWrong {
                count += 1;
                println!("\n--- FAIL #{count}: {} ---", result.path);
                for line in result.detail.lines().take(8) {
                    println!("  {line}");
                }
            }
        }
    }
}

#[test]
#[ignore] // cargo test -p test262 --test test262_baseline -- --ignored --nocapture test262_parse_int_failures
fn test262_parse_int_failures() {
    let Some(runner) = setup_runner() else {
        return;
    };
    println!("\n=== parseInt/parseFloat Failure Dump ===\n");

    for cat in &["built-ins/parseInt", "built-ins/parseFloat"] {
        let report = runner.run_subdir(cat);
        println!(
            "\n{cat}: Pass: {}  Fail: {}  Skip: {}",
            report.passed, report.failed, report.skipped
        );
        let mut count = 0;
        for result in &report.results {
            if result.outcome == test262::TestOutcome::FailWrong {
                count += 1;
                if count > 10 {
                    break;
                }
                println!("\n--- FAIL #{count}: {} ---", result.path);
                for line in result.detail.lines().take(6) {
                    println!("  {line}");
                }
            }
        }
    }
}
