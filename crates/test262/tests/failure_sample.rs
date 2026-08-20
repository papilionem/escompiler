//! Randomly samples 100 failing test262 tests and prints detailed error output.
//!
//! Usage:
//!   cargo test -p test262 --test failure_sample -- --ignored --nocapture test262_failure_sample
//!
//! Environment variables:
//!   FAILURE_SAMPLE_SIZE=N   — Number of failures to sample (default: 100)
//!   FAILURE_SAMPLE_SEED=N   — Random seed for reproducible sampling (default: current timestamp)

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use test262::{RunnerConfig, TestOutcome, TestResult, TestRunner};

/// All 93 categories from the full baseline (matches test262_baseline.rs FULL_CATEGORIES
/// plus the additional categories to reach 93).
const ALL_CATEGORIES: &[&str] = &[
    // Original 13 core categories
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
    "language/statements/for-in",
    "language/statements/for-of",
    "language/statements/async-function",
    "language/statements/async-generator",
    // v0.6 categories
    "language/eval-code",
    "built-ins/Proxy",
    "built-ins/Reflect",
    "built-ins/eval",
    "built-ins/Function",
    // v0.7 categories
    "built-ins/Math",
    "built-ins/Number",
    "built-ins/String",
    "built-ins/JSON",
    "built-ins/Object",
    "built-ins/Array",
    "built-ins/Boolean",
    // v0.8 categories
    "built-ins/Map",
    "built-ins/Set",
    "built-ins/WeakMap",
    "built-ins/WeakSet",
    "built-ins/WeakRef",
    "built-ins/RegExp",
    "built-ins/Date",
];

/// Classify a failure into a human-readable failure mode.
fn classify_failure(result: &TestResult) -> &'static str {
    let detail = result.detail.as_str();
    if detail.starts_with("compile error:") {
        "compile-error"
    } else if detail.starts_with("compiler panicked") {
        "compiler-panic"
    } else if detail.starts_with("timeout") {
        "timeout"
    } else if detail.starts_with("exit code") {
        "runtime-error"
    } else if (detail.starts_with("expected") && detail.contains("but compilation succeeded"))
        || (detail.starts_with("expected runtime") && detail.contains("but exited with code 0"))
    {
        "negative-test-unexpected-success"
    } else if detail.starts_with("cannot") {
        "internal-error"
    } else if detail.is_empty() {
        "unknown"
    } else {
        "other"
    }
}

/// Extract the category from a test path (e.g., "test/language/types/number/foo.js" -> "language/types/number").
fn extract_category(path: &str) -> String {
    let path = path.strip_prefix("test/").unwrap_or(path);
    match path.rfind('/') {
        Some(pos) => path[..pos].to_string(),
        None => "uncategorized".to_string(),
    }
}

/// Read the test file and extract expected behavior from frontmatter.
fn extract_expected_behavior(test262_root: &Path, relative_path: &str) -> String {
    let full_path = test262_root.join(relative_path);
    let source = match fs::read_to_string(&full_path) {
        Ok(s) => s,
        Err(_) => return "(could not read test file)".to_string(),
    };

    let meta = test262::harness::parse_frontmatter(&source);
    let mut parts = Vec::new();

    // Description
    if !meta.description.is_empty() {
        parts.push(format!("description: {}", meta.description));
    }

    // Negative expectation
    if let Some(ref neg) = meta.negative {
        parts.push(format!(
            "negative: phase={:?}, type={}",
            neg.phase, neg.error_type
        ));
    }

    // Flags
    if !meta.flags.is_empty() {
        parts.push(format!("flags: [{}]", meta.flags.join(", ")));
    }

    // Features
    if !meta.features.is_empty() {
        parts.push(format!("features: [{}]", meta.features.join(", ")));
    }

    // Extract assert statements from the source body (after frontmatter)
    let body_start = source.find("---*/").map(|pos| pos + 5).unwrap_or(0);
    let body = &source[body_start..];
    let asserts: Vec<&str> = body
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("assert")
                || trimmed.starts_with("throw ")
                || trimmed.contains("assert.sameValue")
                || trimmed.contains("assert.throws")
                || trimmed.contains("assert.notSameValue")
        })
        .take(5) // Limit to first 5 assert lines
        .collect();

    if !asserts.is_empty() {
        parts.push(format!(
            "assertions:\n{}",
            asserts
                .iter()
                .map(|a| format!("      {}", a.trim()))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if parts.is_empty() {
        "(no metadata found)".to_string()
    } else {
        parts.join("\n    ")
    }
}

/// Simple xorshift64 PRNG for reproducible sampling without external dependencies.
struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        // Avoid zero state which would produce only zeros
        Self {
            state: if seed == 0 {
                0xDEAD_BEEF_CAFE_BABE
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Generate a random index in [0, bound).
    fn next_bounded(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

/// Fisher-Yates shuffle using our PRNG, then take the first `n` elements.
fn sample_n<T: Clone>(items: &[T], n: usize, rng: &mut Xorshift64) -> Vec<T> {
    let mut indices: Vec<usize> = (0..items.len()).collect();
    let len = indices.len();
    // Partial Fisher-Yates: only need to shuffle `n` positions
    let limit = n.min(len);
    for i in 0..limit {
        let j = i + rng.next_bounded(len - i);
        indices.swap(i, j);
    }
    indices[..limit]
        .iter()
        .map(|&idx| items[idx].clone())
        .collect()
}

fn setup_runner() -> Option<(TestRunner, std::path::PathBuf)> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("cannot find workspace root");
    let test262_root = workspace_root.join("tests/test262/test262");
    if !test262_root.exists() {
        eprintln!("test262 not cloned, skipping");
        return None;
    }

    let config = RunnerConfig {
        test262_root: test262_root.clone(),
        harness_dir: workspace_root.join("tests/test262/test262/harness"),
        max_failures: None,
        timeout_secs: 10,
    };
    Some((TestRunner::new(config), test262_root))
}

#[test]
#[ignore] // cargo test -p test262 --test failure_sample -- --ignored --nocapture test262_failure_sample
fn test262_failure_sample() {
    let Some((runner, test262_root)) = setup_runner() else {
        return;
    };

    let sample_size: usize = std::env::var("FAILURE_SAMPLE_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);

    let seed: u64 = std::env::var("FAILURE_SAMPLE_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(42)
        });

    println!("\n{}", "=".repeat(80));
    println!("=== test262 Failure Sample ===");
    println!("  Categories: {}", ALL_CATEGORIES.len());
    println!("  Sample size: {sample_size}");
    println!("  Random seed: {seed} (set FAILURE_SAMPLE_SEED={seed} to reproduce)");
    println!("{}", "=".repeat(80));

    // Phase 1: Run all categories and collect failures
    let run_start = Instant::now();
    let mut all_failures: Vec<TestResult> = Vec::new();
    let mut total_tests = 0usize;
    let mut total_pass = 0usize;
    let mut total_skip = 0usize;
    let mut per_category_stats: Vec<(String, usize, usize, usize, usize)> = Vec::new();

    println!(
        "\nPhase 1: Running all {} categories...\n",
        ALL_CATEGORIES.len()
    );

    for cat in ALL_CATEGORIES {
        let cat_start = Instant::now();
        let report = runner.run_subdir(cat);
        let cat_elapsed = cat_start.elapsed();

        let cat_total =
            report.passed + report.failed + report.fail_crash + report.skipped + report.errors;
        if cat_total == 0 {
            continue;
        }

        println!(
            "  {:50} pass={:4} fail={:4} crash={:3} skip={:4} err={:3} [{:.1}s]",
            cat,
            report.passed,
            report.failed,
            report.fail_crash,
            report.skipped,
            report.errors,
            cat_elapsed.as_secs_f64()
        );
        per_category_stats.push((
            cat.to_string(),
            report.passed,
            report.failed,
            report.skipped,
            report.errors,
        ));

        // Collect individual failures
        for result in report.results {
            match result.outcome {
                TestOutcome::FailWrong | TestOutcome::FailCrash | TestOutcome::Error => {
                    all_failures.push(result);
                }
                TestOutcome::Pass => total_pass += 1,
                TestOutcome::Skip => total_skip += 1,
            }
        }
        total_tests += cat_total;
    }

    let phase1_elapsed = run_start.elapsed();
    println!("\nPhase 1 complete in {:.1}s", phase1_elapsed.as_secs_f64());
    println!(
        "  Total: {total_tests}  Pass: {total_pass}  Fail: {}  Skip: {total_skip}",
        all_failures.len()
    );

    if all_failures.is_empty() {
        println!("\nNo failures found! All tests passed.");
        return;
    }

    // Phase 2: Sample failures
    let mut rng = Xorshift64::new(seed);
    let sampled = sample_n(&all_failures, sample_size, &mut rng);

    println!("\n{}", "=".repeat(80));
    println!(
        "Phase 2: Sampled {} failures (out of {} total)\n",
        sampled.len(),
        all_failures.len()
    );

    // Phase 3: Print detailed info for each sampled failure
    for (i, result) in sampled.iter().enumerate() {
        let failure_mode = classify_failure(result);
        let category = extract_category(&result.path);
        let expected = extract_expected_behavior(&test262_root, &result.path);

        println!("--- Failure #{:03} ---", i + 1);
        println!("  Path: {}", result.path);
        println!("  Category: {category}");
        println!("  Failure mode: {failure_mode}");
        println!("  Error output: {}", result.detail);
        println!("  Expected behavior:");
        println!("    {expected}");
        println!();
    }

    // Phase 4: Summary by failure mode
    println!("{}", "=".repeat(80));
    println!("=== Summary: Failures by Mode ===\n");

    let mut by_mode: HashMap<&str, usize> = HashMap::new();
    for result in &sampled {
        *by_mode.entry(classify_failure(result)).or_insert(0) += 1;
    }
    let mut mode_counts: Vec<(&&str, &usize)> = by_mode.iter().collect();
    mode_counts.sort_by(|a, b| b.1.cmp(a.1));
    for (mode, count) in &mode_counts {
        let pct = **count as f64 / sampled.len() as f64 * 100.0;
        println!("  {:<35} {:>4} ({:.1}%)", mode, count, pct);
    }

    // Phase 5: Summary by category (of the sampled failures)
    println!("\n{}", "=".repeat(80));
    println!("=== Summary: Sampled Failures by Category ===\n");

    let mut by_category: HashMap<String, usize> = HashMap::new();
    for result in &sampled {
        let cat = extract_category(&result.path);
        *by_category.entry(cat).or_insert(0) += 1;
    }
    let mut cat_counts: Vec<(String, usize)> = by_category.into_iter().collect();
    cat_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (cat, count) in &cat_counts {
        println!("  {:<55} {:>4}", cat, count);
    }

    // Phase 6: Overall failure distribution across all categories
    println!("\n{}", "=".repeat(80));
    println!("=== Overall Category Stats (all tests) ===\n");
    println!(
        "  {:<50} {:>6} {:>6} {:>6} {:>6}",
        "Category", "Pass", "Fail", "Skip", "Error"
    );
    println!("  {}", "-".repeat(78));
    for (cat, pass, fail, skip, err) in &per_category_stats {
        println!(
            "  {:<50} {:>6} {:>6} {:>6} {:>6}",
            cat, pass, fail, skip, err
        );
    }

    // Phase 7: Failure modes across ALL failures (not just the sample)
    println!("\n{}", "=".repeat(80));
    println!(
        "=== All Failures by Mode (total: {}) ===\n",
        all_failures.len()
    );

    let mut all_by_mode: HashMap<&str, usize> = HashMap::new();
    for result in &all_failures {
        *all_by_mode.entry(classify_failure(result)).or_insert(0) += 1;
    }
    let mut all_mode_counts: Vec<(&&str, &usize)> = all_by_mode.iter().collect();
    all_mode_counts.sort_by(|a, b| b.1.cmp(a.1));
    for (mode, count) in &all_mode_counts {
        let pct = **count as f64 / all_failures.len() as f64 * 100.0;
        println!("  {:<35} {:>6} ({:.1}%)", mode, count, pct);
    }

    println!("\n{}", "=".repeat(80));
    println!(
        "Done. Total time: {:.1}s",
        run_start.elapsed().as_secs_f64()
    );
}
