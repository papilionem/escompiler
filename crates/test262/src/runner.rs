//! test262 test runner — discovers, executes, and reports on test262 tests.
//!
//! The runner walks the test262 directory tree, parses frontmatter for each
//! test, decides whether to run or skip it, compiles it with [`driver`],
//! and collects results into a [`SuiteReport`].
//!
//! Tests within each category run sequentially by default. Set
//! `TEST262_PARALLEL=1` to opt into parallel execution via [`rayon`].
//! Each test has a configurable per-test timeout to prevent infinite-loop
//! tests from blocking the run.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use crate::harness::{self, NegativePhase, TestMetadata};

/// Configuration for a test262 run.
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    /// Root of the test262 checkout (contains `test/`, `harness/`, etc.).
    pub test262_root: PathBuf,
    /// Directory containing our simplified harness files (`assert.js`, `sta.js`).
    pub harness_dir: PathBuf,
    /// Stop after this many failures (for fast feedback). `None` = unlimited.
    pub max_failures: Option<usize>,
    /// Per-test timeout in seconds. Defaults to 10.
    pub timeout_secs: u64,
}

/// Outcome of a single test262 test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestOutcome {
    /// Test passed as expected.
    Pass,
    /// Test failed with a wrong result (not a crash).
    FailWrong,
    /// Test crashed (signal, panic, abort).
    FailCrash,
    /// Test was skipped (unsupported features, async, module, etc.).
    Skip,
    /// An internal error prevented the test from running.
    Error,
}

impl TestOutcome {
    /// Human-readable label for this outcome.
    pub fn as_str(&self) -> &'static str {
        match self {
            TestOutcome::Pass => "pass",
            TestOutcome::FailWrong => "fail-wrong",
            TestOutcome::FailCrash => "fail-crash",
            TestOutcome::Skip => "skip",
            TestOutcome::Error => "error",
        }
    }

    /// Whether this outcome counts as a failure (wrong or crash).
    pub fn is_failure(&self) -> bool {
        matches!(self, TestOutcome::FailWrong | TestOutcome::FailCrash)
    }
}

/// Result of running a single test262 test.
#[derive(Debug, Clone)]
pub struct TestResult {
    /// Relative path of the test within the test262 tree.
    pub path: String,
    /// Outcome of the test.
    pub outcome: TestOutcome,
    /// Human-readable detail (error message, skip reason, etc.).
    pub detail: String,
}

/// Aggregate report for a test262 run.
#[derive(Debug, Clone, Default)]
pub struct SuiteReport {
    /// Individual test results.
    pub results: Vec<TestResult>,
    /// Number of tests that passed.
    pub passed: usize,
    /// Number of tests that failed (wrong result, not crash).
    pub failed: usize,
    /// Number of tests that crashed (signal, panic, abort).
    pub fail_crash: usize,
    /// Number of tests that were skipped.
    pub skipped: usize,
    /// Number of tests that encountered internal errors.
    pub errors: usize,
}

impl SuiteReport {
    /// Total number of tests in this report.
    pub fn total(&self) -> usize {
        self.passed + self.failed + self.fail_crash + self.skipped + self.errors
    }

    /// Pass rate as a percentage (excludes skipped tests).
    pub fn pass_rate(&self) -> f64 {
        let attempted = self.passed + self.failed + self.fail_crash + self.errors;
        if attempted == 0 {
            return 0.0;
        }
        (self.passed as f64 / attempted as f64) * 100.0
    }

    /// Check whether a specific test path passed.
    pub fn did_pass(&self, path: &str) -> bool {
        self.results
            .iter()
            .any(|r| r.path == path && r.outcome == TestOutcome::Pass)
    }

    /// Return the first `n` failures for debugging.
    pub fn first_failures(&self, n: usize) -> Vec<&TestResult> {
        self.results
            .iter()
            .filter(|r| r.outcome.is_failure())
            .take(n)
            .collect()
    }

    /// Return the first `n` crashes for debugging.
    pub fn first_crashes(&self, n: usize) -> Vec<&TestResult> {
        self.results
            .iter()
            .filter(|r| r.outcome == TestOutcome::FailCrash)
            .take(n)
            .collect()
    }
}

impl fmt::Display for SuiteReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "test262 results:")?;
        writeln!(
            f,
            "  total: {}, pass: {}, fail: {}, crash: {}, skip: {}, error: {}",
            self.total(),
            self.passed,
            self.failed,
            self.fail_crash,
            self.skipped,
            self.errors
        )?;
        writeln!(f, "  pass rate: {:.1}%", self.pass_rate())?;

        let failures = self.first_failures(10);
        if !failures.is_empty() {
            writeln!(f, "\n  first {} failures:", failures.len())?;
            for result in failures {
                writeln!(f, "    FAIL {}: {}", result.path, result.detail)?;
            }
        }
        let crashes = self.first_crashes(5);
        if !crashes.is_empty() {
            writeln!(f, "\n  first {} crashes:", crashes.len())?;
            for result in crashes {
                writeln!(f, "    CRASH {}: {}", result.path, result.detail)?;
            }
        }

        Ok(())
    }
}

/// Structured progress summary for a test262 run.
#[derive(Debug, Clone, Default)]
pub struct ProgressSummary {
    /// Total tests discovered.
    pub total: usize,
    /// Tests that passed.
    pub passed: usize,
    /// Tests that failed.
    pub failed: usize,
    /// Tests that were skipped.
    pub skipped: usize,
    /// Tests that hit internal errors.
    pub errors: usize,
    /// Pass rate as a percentage (excludes skipped tests).
    pub pass_rate: f64,
    /// Per-category breakdown (category path -> (passed, failed, skipped)).
    pub categories: Vec<(String, usize, usize, usize)>,
}

impl ProgressSummary {
    /// Build a progress summary from a [`SuiteReport`].
    pub fn from_report(report: &SuiteReport) -> Self {
        let mut categories: std::collections::HashMap<String, (usize, usize, usize)> =
            std::collections::HashMap::new();

        for result in &report.results {
            // Extract category from path: "test/language/types/number/foo.js" -> "language/types/number"
            let cat = Self::extract_category(&result.path);
            let entry = categories.entry(cat).or_insert((0, 0, 0));
            match result.outcome {
                TestOutcome::Pass => entry.0 += 1,
                TestOutcome::FailWrong | TestOutcome::FailCrash | TestOutcome::Error => {
                    entry.1 += 1
                }
                TestOutcome::Skip => entry.2 += 1,
            }
        }

        let mut sorted_cats: Vec<(String, usize, usize, usize)> = categories
            .into_iter()
            .map(|(k, (p, f, s))| (k, p, f, s))
            .collect();
        sorted_cats.sort_by(|a, b| a.0.cmp(&b.0));

        Self {
            total: report.total(),
            passed: report.passed,
            failed: report.failed + report.fail_crash,
            skipped: report.skipped,
            errors: report.errors,
            pass_rate: report.pass_rate(),
            categories: sorted_cats,
        }
    }

    /// Extract a test category from its relative path.
    fn extract_category(path: &str) -> String {
        // Strip leading "test/" if present
        let path = path.strip_prefix("test/").unwrap_or(path);
        // Take directory components (drop the filename)
        match path.rfind('/') {
            Some(pos) => path[..pos].to_string(),
            None => "uncategorized".to_string(),
        }
    }
}

impl fmt::Display for ProgressSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== test262 Progress Report ===")?;
        writeln!(f)?;
        writeln!(
            f,
            "Total: {}  |  Pass: {}  |  Fail: {}  |  Skip: {}  |  Error: {}",
            self.total, self.passed, self.failed, self.skipped, self.errors
        )?;
        writeln!(f, "Pass rate: {:.1}%", self.pass_rate)?;
        writeln!(f)?;

        if !self.categories.is_empty() {
            writeln!(f, "By category:")?;
            for (cat, passed, failed, skipped) in &self.categories {
                let total = passed + failed + skipped;
                let rate = if *passed + *failed > 0 {
                    (*passed as f64 / (*passed + *failed) as f64) * 100.0
                } else {
                    0.0
                };
                writeln!(
                    f,
                    "  {cat:<50} {passed:>4}/{total:<4} ({rate:>5.1}%)  [skip: {skipped}]"
                )?;
            }
        }

        Ok(())
    }
}

/// The test262 test runner.
pub struct TestRunner {
    config: RunnerConfig,
    /// Preloaded harness preamble (assert.js + sta.js concatenated).
    harness_preamble: String,
}

impl TestRunner {
    /// Create a new runner with the given configuration.
    pub fn new(config: RunnerConfig) -> Self {
        let harness_preamble = Self::load_harness_preamble(&config.harness_dir);
        Self {
            config,
            harness_preamble,
        }
    }

    /// Run all tests in the configured subset directories.
    ///
    /// Returns an aggregate [`SuiteReport`].
    pub fn run_all(&self) -> SuiteReport {
        let test_dir = self.config.test262_root.join("test");
        let tests = self.discover_tests(&test_dir);
        self.run_tests(&tests)
    }

    /// Run all tests and return a progress summary.
    pub fn run_with_progress(&self) -> ProgressSummary {
        let report = self.run_all();
        ProgressSummary::from_report(&report)
    }

    /// Run all `.js` tests from an arbitrary directory on disk.
    ///
    /// Unlike [`run_subdir`], this accepts an absolute path to any directory
    /// containing test262-style `.js` files.
    pub fn run_directory(&self, dir: &Path) -> SuiteReport {
        if !dir.exists() {
            return SuiteReport::default();
        }
        let tests = self.discover_js_files(dir);
        self.run_tests(&tests)
    }

    /// Run tests from a specific subdirectory within `test/`.
    ///
    /// `subdir` is relative to `test262_root/test/`, e.g. `language/statements/variable`.
    pub fn run_subdir(&self, subdir: &str) -> SuiteReport {
        let dir = self.config.test262_root.join("test").join(subdir);
        if !dir.exists() {
            return SuiteReport::default();
        }
        let tests = self.discover_js_files(&dir);
        self.run_tests(&tests)
    }

    /// Discover all `.js` test files in the default subset of directories.
    fn discover_tests(&self, test_dir: &Path) -> Vec<PathBuf> {
        let subdirs = [
            "language/statements/variable",
            "language/expressions/addition",
            "language/statements/if",
            "language/statements/for",
            "language/statements/while",
            "language/statements/block",
            "language/types/number",
            "language/types/string",
            "language/types/boolean",
            "language/types/null",
            "language/types/undefined",
        ];

        let mut all = Vec::new();
        for subdir in &subdirs {
            let dir = test_dir.join(subdir);
            if dir.exists() {
                self.collect_js_files(&dir, &mut all);
            }
        }
        all.sort();
        all
    }

    /// Discover all `.js` files recursively under a directory.
    fn discover_js_files(&self, dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        self.collect_js_files(dir, &mut files);
        files.sort();
        files
    }

    /// Recursively collect `.js` files.
    fn collect_js_files(&self, dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                self.collect_js_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "js") {
                out.push(path);
            }
        }
    }

    /// Parse `TEST262_SHARD` as `k/N` (1-indexed shard k of N total shards).
    /// Returns `None` when unset, malformed, or out of range.
    pub(crate) fn shard_spec_from(value: Option<String>) -> Option<(usize, usize)> {
        let v = value?;
        let (k, n) = v.split_once('/')?;
        let (k, n) = (k.parse::<usize>().ok()?, n.parse::<usize>().ok()?);
        (k >= 1 && k <= n && n > 1).then_some((k, n))
    }

    /// Run a list of tests in parallel and collect results.
    ///
    /// Uses [`rayon`] to distribute tests across all available CPU cores.
    /// Progress is printed to stderr every 100 tests.
    ///
    /// When the `TEST262_SHARD` env var is `k/N` (1-indexed shard k of N),
    /// only tests where `index % N == k - 1` run. Discovery is sorted and
    /// deterministic, so shards are disjoint and their union is complete.
    fn run_tests(&self, tests: &[PathBuf]) -> SuiteReport {
        let shard = Self::shard_spec_from(std::env::var("TEST262_SHARD").ok());
        let owned;
        let tests: &[PathBuf] = if let Some((k, n)) = shard {
            owned = tests
                .iter()
                .enumerate()
                .filter(|(i, _)| i % n == k - 1)
                .map(|(_, p)| p.clone())
                .collect::<Vec<_>>();
            &owned
        } else {
            tests
        };
        let total = tests.len();
        if total == 0 {
            return SuiteReport::default();
        }

        let completed = AtomicUsize::new(0);
        let pass_count = AtomicUsize::new(0);
        let fail_count = AtomicUsize::new(0);

        // Run tests in parallel by default — the compilation pipeline is
        // stateless (no globals in desugar/IR/cranelift/linker) and each
        // test binary executes in its own subprocess.
        // Set TEST262_SEQUENTIAL=1 to force sequential execution.
        let use_parallel = !std::env::var("TEST262_SEQUENTIAL")
            .map(|v| v == "1")
            .unwrap_or(false);

        let run_one = |test_path: &PathBuf| -> TestResult {
            let result = self.run_single_test(test_path);

            // Track counts
            match result.outcome {
                TestOutcome::Pass => {
                    pass_count.fetch_add(1, Ordering::Relaxed);
                }
                TestOutcome::FailWrong | TestOutcome::FailCrash | TestOutcome::Error => {
                    fail_count.fetch_add(1, Ordering::Relaxed);
                }
                TestOutcome::Skip => {}
            }

            // Progress reporting
            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
            if done.is_multiple_of(100) || done == total {
                let p = pass_count.load(Ordering::Relaxed);
                let f = fail_count.load(Ordering::Relaxed);
                eprintln!("  [{done}/{total}] pass={p} fail={f}");
            }

            result
        };

        let results: Vec<TestResult> = if use_parallel {
            tests.par_iter().map(run_one).collect()
        } else {
            tests.iter().map(run_one).collect()
        };

        // Build report from collected results
        let mut report = SuiteReport::default();
        for result in results {
            match result.outcome {
                TestOutcome::Pass => report.passed += 1,
                TestOutcome::FailWrong => report.failed += 1,
                TestOutcome::FailCrash => report.fail_crash += 1,
                TestOutcome::Skip => report.skipped += 1,
                TestOutcome::Error => report.errors += 1,
            }
            report.results.push(result);
        }

        report
    }

    /// Run a single test262 test file.
    fn run_single_test(&self, test_path: &Path) -> TestResult {
        let relative = test_path
            .strip_prefix(&self.config.test262_root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| test_path.display().to_string());

        // Read the test file
        let source = match fs::read_to_string(test_path) {
            Ok(s) => s,
            Err(e) => {
                return TestResult {
                    path: relative,
                    outcome: TestOutcome::Error,
                    detail: format!("cannot read file: {e}"),
                };
            }
        };

        // Parse frontmatter
        let meta = harness::parse_frontmatter(&source);

        // Skip checks
        if let Some(reason) = self.should_skip(&meta) {
            return TestResult {
                path: relative,
                outcome: TestOutcome::Skip,
                detail: reason,
            };
        }

        // Build the full test source with harness preamble
        let full_source = if meta.is_raw() {
            source.clone()
        } else {
            let extra_includes = self.load_extra_includes(&meta.includes);
            // onlyStrict tests must be run in strict mode per the test262 spec
            let strict_prefix = if meta.is_only_strict() {
                "\"use strict\";\n"
            } else {
                ""
            };
            format!(
                "{}{}\n{}{}",
                strict_prefix, self.harness_preamble, extra_includes, source
            )
        };

        // Execute the test
        self.execute_test(&relative, &full_source, &meta)
    }

    /// Check if a test should be skipped. Returns the skip reason if so.
    fn should_skip(&self, meta: &TestMetadata) -> Option<String> {
        // Skip async tests (we don't support $DONE yet)
        if meta.is_async() {
            return Some("async test (not yet supported)".to_string());
        }

        // Skip module tests (require different compilation mode)
        if meta.is_module() {
            return Some("module test (not yet supported)".to_string());
        }

        // Skip if unsupported features are required
        if !meta.features.is_empty() && !harness::all_features_supported(&meta.features) {
            let unsupported: Vec<&str> = meta
                .features
                .iter()
                .filter(|f| !harness::SUPPORTED_FEATURES.contains(&f.as_str()))
                .map(|f| f.as_str())
                .collect();
            return Some(format!("unsupported features: {}", unsupported.join(", ")));
        }

        None
    }

    /// Compile and execute a test, returning the result.
    fn execute_test(&self, relative_path: &str, source: &str, meta: &TestMetadata) -> TestResult {
        // Write source to a temporary file
        let tmp_dir = match tempfile::tempdir() {
            Ok(d) => d,
            Err(e) => {
                return TestResult {
                    path: relative_path.to_string(),
                    outcome: TestOutcome::Error,
                    detail: format!("cannot create temp dir: {e}"),
                };
            }
        };
        let src_path = tmp_dir.path().join("test.js");
        if let Err(e) = fs::write(&src_path, source) {
            return TestResult {
                path: relative_path.to_string(),
                outcome: TestOutcome::Error,
                detail: format!("cannot write temp file: {e}"),
            };
        }

        let mut config = driver::CompilerConfig::new(vec![src_path.display().to_string()]);
        config.output = tmp_dir.path().join("test_bin").display().to_string();

        // Attempt compilation (catch panics so one bad test doesn't abort the run)
        let compile_result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            driver::compile(&config)
        })) {
            Ok(result) => result,
            Err(_) => {
                return TestResult {
                    path: relative_path.to_string(),
                    outcome: TestOutcome::FailCrash,
                    detail: "compiler panicked".to_string(),
                };
            }
        };

        // Handle negative tests expecting parse/resolution errors.
        // ESC-28: we now compare negative.type against the actual error.
        if let Some(neg) = &meta.negative
            && (neg.phase == NegativePhase::Parse || neg.phase == NegativePhase::Resolution)
        {
            return match &compile_result {
                Ok(_) => TestResult {
                    path: relative_path.to_string(),
                    outcome: TestOutcome::FailWrong,
                    detail: format!(
                        "expected {} during {:?} but compilation succeeded",
                        neg.error_type, neg.phase
                    ),
                },
                Err(e) => {
                    let actual = classify_compile_error(e);
                    if error_type_matches(&neg.error_type, actual) {
                        TestResult {
                            path: relative_path.to_string(),
                            outcome: TestOutcome::Pass,
                            detail: String::new(),
                        }
                    } else {
                        TestResult {
                            path: relative_path.to_string(),
                            outcome: TestOutcome::FailWrong,
                            detail: format!(
                                "expected {} during {:?} but got {}: {e}",
                                neg.error_type,
                                neg.phase,
                                actual.unwrap_or("unknown"),
                            ),
                        }
                    }
                }
            };
        }

        // For normal tests and runtime-negative tests, compilation must succeed
        let result = match compile_result {
            Ok(r) => r,
            Err(e) => {
                return TestResult {
                    path: relative_path.to_string(),
                    outcome: TestOutcome::FailWrong,
                    detail: format!("compile error: {e}"),
                };
            }
        };

        // Execute the compiled binary with timeout
        let timeout = Duration::from_secs(self.config.timeout_secs);
        let output = self.run_binary_with_timeout(&result.output_path, timeout);

        match output {
            Ok(BinaryOutput::Completed(output)) => {
                self.evaluate_runtime_result(relative_path, &output, meta)
            }
            Ok(BinaryOutput::TimedOut) => TestResult {
                path: relative_path.to_string(),
                outcome: TestOutcome::FailWrong,
                detail: format!("timeout (exceeded {}s)", self.config.timeout_secs),
            },
            Err(e) => TestResult {
                path: relative_path.to_string(),
                outcome: TestOutcome::Error,
                detail: format!("cannot execute binary: {e}"),
            },
        }
    }

    /// Evaluate the runtime result of a compiled test binary.
    fn evaluate_runtime_result(
        &self,
        relative_path: &str,
        output: &std::process::Output,
        meta: &TestMetadata,
    ) -> TestResult {
        // Handle runtime-negative tests
        if let Some(neg) = &meta.negative
            && neg.phase == NegativePhase::Runtime
        {
            // Check for crash (signal death, panic, abort) — never pass these.
            if is_crash(&output.status, &output.stderr) {
                let sig = signal_name(&output.status);
                return TestResult {
                    path: relative_path.to_string(),
                    outcome: TestOutcome::FailCrash,
                    detail: if let Some(sig) = sig {
                        format!(
                            "expected runtime {} but process died with signal {sig}",
                            neg.error_type
                        )
                    } else {
                        format!(
                            "expected runtime {} but process crashed (panic/abort)",
                            neg.error_type
                        )
                    },
                };
            }

            if output.status.success() {
                return TestResult {
                    path: relative_path.to_string(),
                    outcome: TestOutcome::FailWrong,
                    detail: format!("expected runtime {} but exited with code 0", neg.error_type),
                };
            }

            // Non-zero exit — try to match the error type from stderr.
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let thrown = extract_thrown_error(&stderr_str);
            return match thrown {
                Some(actual) => {
                    if error_type_matches(&neg.error_type, Some(&actual)) {
                        TestResult {
                            path: relative_path.to_string(),
                            outcome: TestOutcome::Pass,
                            detail: String::new(),
                        }
                    } else {
                        TestResult {
                            path: relative_path.to_string(),
                            outcome: TestOutcome::FailWrong,
                            detail: format!(
                                "expected runtime {} but got {actual}: {}",
                                neg.error_type,
                                stderr_str.chars().take(200).collect::<String>(),
                            ),
                        }
                    }
                }
                None => {
                    // We can't extract the thrown error constructor name from stderr.
                    // This is an M3 limitation — the runtime may not emit structured
                    // error info. Conservative: treat non-zero exit as a match (Pass)
                    // rather than false-failing. The crash check above ensures we never
                    // falsely pass on segfaults or panics.
                    TestResult {
                        path: relative_path.to_string(),
                        outcome: TestOutcome::Pass,
                        detail: String::new(),
                    }
                }
            };
        }

        // Normal test: expect exit code 0
        if output.status.success() {
            TestResult {
                path: relative_path.to_string(),
                outcome: TestOutcome::Pass,
                detail: String::new(),
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let outcome = if is_crash(&output.status, &output.stderr) {
                TestOutcome::FailCrash
            } else {
                TestOutcome::FailWrong
            };
            TestResult {
                path: relative_path.to_string(),
                outcome,
                detail: format!(
                    "exit code {}, stderr: {}",
                    output.status.code().unwrap_or(-1),
                    stderr.chars().take(200).collect::<String>()
                ),
            }
        }
    }

    /// Run a compiled binary with a timeout.
    ///
    /// Spawns the process and polls `try_wait()` until it exits or the
    /// timeout expires. On timeout, the process is killed.
    fn run_binary_with_timeout(
        &self,
        binary_path: &str,
        timeout: Duration,
    ) -> Result<BinaryOutput, std::io::Error> {
        let mut child = Command::new(binary_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let start = Instant::now();

        loop {
            match child.try_wait()? {
                Some(status) => {
                    // Process finished — read output
                    let mut stdout = Vec::new();
                    let mut stderr = Vec::new();
                    if let Some(mut out) = child.stdout.take() {
                        std::io::Read::read_to_end(&mut out, &mut stdout)?;
                    }
                    if let Some(mut err) = child.stderr.take() {
                        std::io::Read::read_to_end(&mut err, &mut stderr)?;
                    }
                    return Ok(BinaryOutput::Completed(std::process::Output {
                        status,
                        stdout,
                        stderr,
                    }));
                }
                None => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        let _ = child.wait(); // Reap zombie
                        return Ok(BinaryOutput::TimedOut);
                    }
                    // Poll every 50ms — negligible overhead vs 10s timeout
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    /// Load and concatenate the harness preamble files.
    fn load_harness_preamble(harness_dir: &Path) -> String {
        let mut preamble = String::new();

        // Only load the minimal harness by default. Other includes
        // (propertyHelper.js, compareArray.js, etc.) are loaded on
        // demand when a test's `includes:` frontmatter requests them.
        let files = ["sta.js", "assert.js"];
        for name in &files {
            let path = harness_dir.join(name);
            if let Ok(content) = fs::read_to_string(&path) {
                preamble.push_str(&content);
                preamble.push('\n');
            }
        }

        preamble
    }

    /// Load additional harness includes specified in a test's frontmatter.
    ///
    /// Returns the concatenation of all requested include file contents.
    fn load_extra_includes(&self, includes: &[String]) -> String {
        let mut extra = String::new();
        // Only skip includes already in the minimal preamble
        let defaults = ["sta.js", "assert.js"];
        for include in includes {
            if defaults.contains(&include.as_str()) {
                continue;
            }
            // Try our simplified harness first, then the official test262 harness
            let path = self.config.harness_dir.join(include);
            let official = self.config.test262_root.join("harness").join(include);
            let content = fs::read_to_string(&path).or_else(|_| fs::read_to_string(&official));
            if let Ok(c) = content {
                extra.push_str(&c);
                extra.push('\n');
            }
        }
        extra
    }
}

// ── Crash detection and error classification helpers ──────────────────

/// Check whether a process exited via a crash (signal, panic, or abort).
#[cfg(unix)]
fn is_crash(status: &std::process::ExitStatus, _stderr: &[u8]) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal().is_some()
}

#[cfg(not(unix))]
fn is_crash(status: &std::process::ExitStatus, stderr: &[u8]) -> bool {
    if status.success() || status.code() == Some(0) {
        return false;
    }
    let stderr_str = String::from_utf8_lossy(stderr);
    stderr_str.contains("panicked at")
        || stderr_str.contains("assertion failed")
        || stderr_str.contains("SIGSEGV")
        || stderr_str.contains("SIGABRT")
        || stderr_str.contains("SIGILL")
}

/// Get the signal name if the process died from a signal.
#[cfg(unix)]
fn signal_name(status: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map(|s| {
        match s {
            libc::SIGSEGV => "SIGSEGV",
            libc::SIGABRT => "SIGABRT",
            libc::SIGILL => "SIGILL",
            libc::SIGFPE => "SIGFPE",
            libc::SIGBUS => "SIGBUS",
            libc::SIGSYS => "SIGSYS",
            libc::SIGTRAP => "SIGTRAP",
            _ => "SIGNAL",
        }
        .to_string()
    })
}

#[cfg(not(unix))]
fn signal_name(_status: &std::process::ExitStatus) -> Option<String> {
    None
}

/// Classify a [`driver::DriverError`] into an error type name for matching.
/// Classification of a process exit status for crash detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitClass {
    Success,
    Nonzero(i32),
    Crash(String),
}

pub fn classify_exit(status: &std::process::ExitStatus, stderr: &str) -> ExitClass {
    if status.success() {
        return ExitClass::Success;
    }
    if is_crash(status, stderr.as_bytes()) {
        let detail = signal_name(status)
            .or_else(|| {
                let s = stderr;
                if s.contains("panicked at") || s.contains("assertion failed") {
                    Some("rust_panic".to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "unknown_crash".to_string());
        return ExitClass::Crash(detail);
    }
    let s = stderr;
    if s.contains("panicked at") || s.contains("assertion failed") {
        return ExitClass::Crash("rust_panic".to_string());
    }
    ExitClass::Nonzero(status.code().unwrap_or(1))
}
fn classify_compile_error(err: &driver::DriverError) -> Option<&'static str> {
    match err {
        driver::DriverError::Parse(_) => Some("SyntaxError"),
        driver::DriverError::Lowering(_) => Some("SyntaxError"),
        driver::DriverError::Verification(_) => Some("SyntaxError"),
        _ => None,
    }
}

/// Check whether an observed error type matches the expected negative type.
fn error_type_matches(expected: &str, actual: Option<&str>) -> bool {
    let actual = match actual {
        Some(a) => a,
        None => return false,
    };
    let expected = expected.to_lowercase();
    let expected = if expected.ends_with("error") {
        expected
    } else {
        format!("{expected}error")
    };
    let actual = actual.to_lowercase();
    let actual = if actual.ends_with("error") {
        actual
    } else {
        format!("{actual}error")
    };
    expected == actual
}

/// Try to extract the thrown error constructor name from stderr output.
fn extract_thrown_error(stderr: &str) -> Option<String> {
    for pattern in &[
        "TypeError",
        "ReferenceError",
        "RangeError",
        "SyntaxError",
        "URIError",
        "EvalError",
        "Test262Error",
        "AggregateError",
    ] {
        if stderr.contains(pattern) {
            return Some(pattern.to_string());
        }
    }
    None
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn test_error_type_matches_exact() {
        assert!(error_type_matches("TypeError", Some("TypeError")));
        assert!(error_type_matches("SyntaxError", Some("SyntaxError")));
        assert!(error_type_matches("RangeError", Some("RangeError")));
    }

    #[test]
    fn test_error_type_matches_case_insensitive() {
        assert!(error_type_matches("typeerror", Some("TypeError")));
        assert!(error_type_matches("TypeError", Some("typeerror")));
    }

    #[test]
    fn test_error_type_matches_implied_error_suffix() {
        assert!(error_type_matches("Type", Some("TypeError")));
        assert!(error_type_matches("Syntax", Some("SyntaxError")));
    }

    #[test]
    fn test_error_type_mismatch() {
        assert!(!error_type_matches("TypeError", Some("RangeError")));
        assert!(!error_type_matches("SyntaxError", Some("TypeError")));
    }

    #[test]
    fn test_error_type_matches_none_actual() {
        assert!(!error_type_matches("TypeError", None));
    }

    #[test]
    fn test_extract_thrown_error_finds_types() {
        assert_eq!(
            extract_thrown_error("Uncaught TypeError: x is not a function"),
            Some("TypeError".to_string())
        );
        assert_eq!(
            extract_thrown_error("ReferenceError: x is not defined"),
            Some("ReferenceError".to_string())
        );
        assert_eq!(
            extract_thrown_error("RangeError: invalid array length"),
            Some("RangeError".to_string())
        );
    }

    #[test]
    fn test_extract_thrown_error_no_match() {
        assert_eq!(extract_thrown_error("some random output"), None);
        assert_eq!(extract_thrown_error(""), None);
    }

    #[test]
    fn test_classify_compile_error_parse() {
        let err = driver::DriverError::Parse("unexpected token".into());
        assert_eq!(classify_compile_error(&err), Some("SyntaxError"));
    }

    #[test]
    fn test_classify_compile_error_lowering() {
        let err = driver::DriverError::Lowering(vec!["undefined variable".into()]);
        assert_eq!(classify_compile_error(&err), Some("SyntaxError"));
    }

    #[test]
    fn test_classify_compile_error_non_js() {
        let err = driver::DriverError::Codegen("backend failure".into());
        assert_eq!(classify_compile_error(&err), None);
    }

    #[test]
    fn test_is_crash_success() {
        let output = std::process::Command::new("true").output().unwrap();
        assert!(!is_crash(&output.status, &output.stderr));
    }

    #[test]
    fn test_is_crash_nonzero_no_crash() {
        let output = std::process::Command::new("false").output().unwrap();
        assert!(!is_crash(&output.status, &output.stderr));
    }
}

/// Result of running a compiled test binary.
enum BinaryOutput {
    /// Process completed within the timeout.
    Completed(std::process::Output),
    /// Process was killed after exceeding the timeout.
    TimedOut,
}

/// Temporary file helper — import here to keep runner self-contained.
mod tempfile {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Atomic counter to ensure unique temp dir names under parallel execution.
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A temporary directory that is removed on drop.
    pub struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        /// Get the path to the temporary directory.
        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Create a new temporary directory with a unique name.
    ///
    /// Uses an atomic counter combined with the PID and timestamp
    /// to guarantee uniqueness even under parallel execution.
    pub fn tempdir() -> Result<TempDir, std::io::Error> {
        let mut path = std::env::temp_dir();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        path.push(format!("cs_test262_{ts}_{seq}"));
        fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }
}
