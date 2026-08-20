//! End-to-end compile-and-run integration tests.
//!
//! Compiles JS fixture files through the full pipeline (parse -> desugar -> IR ->
//! Cranelift -> link) and verifies the resulting binary produces expected output.
//!
//! Tests gracefully skip if `libruntime.a` is not available for linking.

use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use driver::CompilerConfig;

// ---------------------------------------------------------------------------
// Fixture loading (mirrors tests/integration/runner.rs logic)
// ---------------------------------------------------------------------------

/// A single test case loaded from a .js fixture file.
struct TestCase {
    /// Expected stdout (from `@expected-stdout` annotation or sidecar file).
    expected_stdout: Option<String>,
    /// Expected stderr (from `@expected-stderr` annotation).
    expected_stderr: Option<String>,
    /// Expected exit code (from `@expected-exit-code` annotation, defaults to 0).
    expected_exit_code: Option<i32>,
    /// Whether this test is expected to fail compilation.
    expect_error: bool,
}

impl TestCase {
    /// Load a test case from source text and its file path.
    fn load(path: &Path) -> Self {
        let source = fs::read_to_string(path).expect("failed to read test fixture");
        let expected_stdout = Self::extract_expected_stdout(&source, path);
        let expected_stderr = Self::extract_annotation(&source, "@expected-stderr");
        let expected_exit_code = Self::extract_annotation(&source, "@expected-exit-code")
            .and_then(|s| s.parse::<i32>().ok());
        let expect_error = source.contains("@expect-error");

        Self {
            expected_stdout,
            expected_stderr,
            expected_exit_code,
            expect_error,
        }
    }

    /// Extract expected stdout from inline annotation or sidecar file.
    fn extract_expected_stdout(source: &str, path: &Path) -> Option<String> {
        // Inline annotation: // @expected-stdout: <value>
        if let Some(val) = Self::extract_annotation(source, "@expected-stdout") {
            return Some(val);
        }

        // Multi-line block annotation
        if let Some(val) =
            Self::extract_block_annotation(source, "@expected-stdout-begin", "@expected-stdout-end")
        {
            return Some(val);
        }

        // Sidecar .expected file
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
                    // Ensure the tag is followed by ':' or whitespace, not more
                    // tag text (e.g., "@expected-stdout" must not match
                    // "@expected-stdout-begin").
                    if val.is_empty() || val.starts_with(':') || val.starts_with(' ') {
                        let val = val.trim_start_matches(':').trim();
                        if !val.is_empty() {
                            return Some(val.to_string());
                        }
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

/// Discover all `.js` fixture files in a directory (recursively).
fn discover_fixtures(dir: &Path) -> Vec<PathBuf> {
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the workspace root from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/driver -> workspace root is two levels up
    manifest_dir
        .parent()
        .expect("parent of crates/driver")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

/// Check if the runtime library the linker will actually use is available.
///
/// Delegates to the driver's own `find_runtime_lib`, deliberately. The previous
/// version hardcoded `<workspace>/target/debug/libruntime.a`, which diverged from
/// the driver in two ways that mattered:
///
///   * it ignored `CARGO_TARGET_DIR`, which CI sets to `/cache/target/<job>` — so
///     the archive was built and the harness still reported it missing;
///   * it accepted `target/release/libruntime.a`, letting a stale release archive
///     stand in for a debug build that was never produced.
///
/// Two implementations of "where is the runtime" is one more than the number of
/// answers that can be right. This asks the component that does the linking.
fn runtime_lib_available() -> bool {
    driver::pipeline::find_runtime_lib().is_some()
}

/// Compile a JS file to a native binary via `driver::compile`.
fn compile_fixture(fixture_path: &Path, output_path: &Path) -> Result<(), String> {
    let mut config = CompilerConfig::new(vec![fixture_path.to_string_lossy().to_string()]);
    config.output = output_path.to_string_lossy().to_string();
    config.no_config = true;
    driver::compile(&config)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Output captured from running a compiled binary.
struct BinaryOutput {
    /// Captured stdout, trimmed.
    stdout: String,
    /// Captured stderr, trimmed.
    stderr: String,
    /// Process exit code.
    exit_code: i32,
}

/// Run a compiled binary and capture its stdout, stderr, and exit code.
///
/// Uses a 10-second timeout to prevent infinite loops in compiled binaries
/// from blocking the test suite.
fn run_binary(path: &Path) -> Result<BinaryOutput, String> {
    use std::time::{Duration, Instant};

    let mut child = Command::new(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn binary: {e}"))?;

    let timeout = Duration::from_secs(10);
    let start = Instant::now();

    // Poll until the process exits or timeout is reached.
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                // Process exited — collect output.
                let output = child
                    .wait_with_output()
                    .map_err(|e| format!("failed to collect output: {e}"))?;
                return Ok(BinaryOutput {
                    stdout: String::from_utf8_lossy(&output.stdout)
                        .trim_end()
                        .to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr)
                        .trim_end()
                        .to_string(),
                    exit_code: output.status.code().unwrap_or(-1),
                });
            }
            Ok(None) => {
                // Still running — check timeout.
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("binary timed out after 10s (possible infinite loop)".to_string());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("error waiting for binary: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

/// Fixtures known to fail, with the reason. See `load_xfail`.
const XFAIL_REGISTRY: &str = "tests/integration/xfail.txt";

/// Read the XFAIL registry: fixture stem -> reason.
///
/// The registry is the mechanism that lets this harness be switched ON while
/// defects remain. Three rules, and the third is the one that matters:
///
///   * a fixture that fails AND is registered  -> expected, not a failure
///   * a fixture that fails and is NOT registered -> hard failure
///   * a fixture that PASSES while registered  -> **hard failure**
///
/// The third makes progress un-ignorable: fixing a defect turns the suite red
/// until the entry is deleted, so DONE becomes an event CI announces rather than
/// a judgement someone makes. The count only ever ratchets down.
fn load_xfail(root: &std::path::Path) -> std::collections::HashMap<String, String> {
    let path = root.join(XFAIL_REGISTRY);
    let mut map = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return map;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, reason) = match line.split_once('#') {
            Some((n, r)) => (n.trim(), r.trim()),
            None => (line, ""),
        };
        map.insert(name.to_string(), reason.to_string());
    }
    map
}

#[test]
fn test_integration_fixtures() {
    // A missing runtime archive is a FAILURE, never a skip. The previous version
    // printed "SKIP: libruntime.a not found" and returned, so the whole suite
    // passed in 0.00s having executed nothing — the exact shape the sealed v0.9
    // criterion forbids, and indistinguishable from a clean run.
    assert!(
        runtime_lib_available(),
        "libruntime.a not found — run `cargo build -p runtime` first.\n\
         This is a failure and not a skip on purpose: a run that executes zero \
         entries must never be reported as a passing run."
    );

    let root = workspace_root();
    let fixtures_dir = root.join("tests/integration/fixtures");
    let fixtures = discover_fixtures(&fixtures_dir);

    if fixtures.is_empty() {
        panic!("no fixtures found in {}", fixtures_dir.display());
    }

    // ESC_FIXTURE_FILTER=a,b,c restricts the run to named fixtures. Exists so the
    // negative control (scripts/harness-negative-control.sh) can prove the XFAIL
    // rules in seconds instead of the ~6 minutes a full run takes — a control too
    // slow to run is a control that does not get run.
    let filter: Option<std::collections::HashSet<String>> = std::env::var("ESC_FIXTURE_FILTER")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect());

    let xfail = load_xfail(&root);
    let mut xfail_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut unexpected_failures: Vec<String> = Vec::new();
    let mut unexpected_passes: Vec<String> = Vec::new();

    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let mut skipped = 0;

    // PHASE 1 — decide what runs. Serial, cheap, and deliberately separate from
    // execution: the skip/filter decision is what determines `executed`, and it
    // must stay deterministic and independent of scheduling.
    let mut runnable: Vec<(PathBuf, String, TestCase)> = Vec::new();
    for fixture_path in &fixtures {
        let test_case = TestCase::load(fixture_path);
        let fixture_name = fixture_path
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();

        if let Some(f) = &filter
            && !f.contains(fixture_name.as_str())
        {
            continue;
        }

        let has_any_expectation = test_case.expected_stdout.is_some()
            || test_case.expected_stderr.is_some()
            || test_case.expected_exit_code.is_some()
            || test_case.expect_error;
        if !has_any_expectation {
            skipped += 1;
            continue;
        }

        runnable.push((fixture_path.clone(), fixture_name, test_case));
    }

    // PHASE 2 — run them. Each fixture compiles, links and executes a real
    // program (~1.6s), and they are wholly independent, so this is the 94% of CI
    // wall-clock ESC-113 is about. `map(...).collect()` on a rayon parallel
    // iterator is ORDER-PRESERVING, so the classification below still sees
    // fixtures in discovery order and the counters cannot depend on scheduling.
    //
    // Thread count comes from `available_parallelism`, which honours the CPU
    // affinity mask — the Dell runners are taskset-pinned to a 4-CPU block, so
    // this yields 4 there rather than 48. `ESC_FIXTURE_THREADS` overrides it. A
    // private pool, not the global one, so nothing else inherits this sizing.
    let threads = std::env::var("ESC_FIXTURE_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(1);
    eprintln!(
        "running {} fixture(s) on {threads} thread(s) ({skipped} skipped for lack of an expectation)",
        runnable.len()
    );
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("failed to build the fixture thread pool");

    let results: Vec<(String, bool)> = pool.install(|| {
        runnable
            .par_iter()
            .enumerate()
            .map(|(idx, (fixture_path, fixture_name, test_case))| {
                // One directory per fixture. Stems are unique today, so the
                // binary paths would not collide on their own — but the compiler
                // and linker also drop intermediates next to the output, and a
                // future duplicate stem in a subdirectory would silently make two
                // fixtures overwrite each other. Isolating by index costs nothing
                // and removes the whole class. Same collision class as ESC-112.
                let fixture_dir = temp_dir.path().join(format!("f{idx}"));
                std::fs::create_dir_all(&fixture_dir)
                    .expect("failed to create the per-fixture temp dir");
                let output_path = fixture_dir.join(fixture_name.as_str());

                eprintln!("  testing {fixture_name}...");
                let ok = run_one_fixture(fixture_path, fixture_name, test_case, &output_path);

                // Drop the artifact immediately. The version before this one kept
                // ONE tempdir for the whole run, so every linked binary from all
                // 373 fixtures accumulated — measured at 9.6+ GB, enough to hit
                // "Disk quota exceeded" on a 16 GB tmpfs *inside* a green exit.
                // Running in parallel makes that worse, not better, if unbounded:
                // peak usage is now threads x artifact rather than all of them.
                let _ = std::fs::remove_dir_all(&fixture_dir);

                (fixture_name.clone(), ok)
            })
            .collect()
    });

    // PHASE 3 — classify, serially, in discovery order.
    let mut passed = 0;
    let mut failed = 0;
    for (name, fixture_ok_outer) in results {
        match (fixture_ok_outer, xfail.contains_key(&name)) {
            (true, false) => passed += 1,
            (false, false) => {
                failed += 1;
                unexpected_failures.push(name);
            }
            (false, true) => {
                failed += 1;
                xfail_seen.insert(name);
            }
            (true, true) => {
                // Fixed but still registered. Red until the entry is deleted.
                passed += 1;
                xfail_seen.insert(name.clone());
                unexpected_passes.push(name);
            }
        }
    }

    let executed = passed + failed;
    eprintln!(
        "\nIntegration results: {passed} passed, {failed} failed, {skipped} skipped \
         ({executed} executed, {} registered as expected-fail)",
        xfail.len()
    );

    // "0 failures" and "0 entries executed" must never print the same verdict.
    assert!(
        executed > 0,
        "the harness executed 0 entries. That is a failure, not a pass — \
         {} fixtures were discovered and all were skipped for lack of an expectation.",
        fixtures.len()
    );

    // Parallelising must not quietly run fewer fixtures than it planned to. This
    // is the failure mode a "make it faster" change is most likely to introduce
    // and least likely to be noticed for, so it is asserted rather than trusted.
    assert_eq!(
        executed,
        runnable.len(),
        "the harness planned {} fixtures but classified {executed}. A speedup that \
         drops fixtures is a regression, not a speedup.",
        runnable.len()
    );

    if !unexpected_passes.is_empty() {
        panic!(
            "{} fixture(s) are registered in {XFAIL_REGISTRY} but now PASS. \
             This is the intended way to find out that something got fixed: \
             delete these lines from the registry.\n  {}",
            unexpected_passes.len(),
            unexpected_passes.join("\n  ")
        );
    }

    assert!(
        unexpected_failures.is_empty(),
        "{} fixture(s) failed that are not registered in {XFAIL_REGISTRY}:\n  {}",
        unexpected_failures.len(),
        unexpected_failures.join("\n  ")
    );

    // A registry entry naming a fixture that no longer exists is rot. Skipped
    // under a filter, where entries legitimately do not run.
    let stale: Vec<&String> = if filter.is_some() {
        Vec::new()
    } else {
        xfail.keys().filter(|k| !xfail_seen.contains(*k)).collect()
    };
    assert!(
        stale.is_empty(),
        "{} entr(y/ies) in {XFAIL_REGISTRY} name a fixture that did not run — \
         renamed or deleted. Remove them:\n  {}",
        stale.len(),
        stale
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// Compile, run and check a single fixture. Returns whether it met its
/// expectations. Split out of `test_integration_fixtures` so the per-fixture work
/// is a pure function of its inputs — which is what makes it safe to run the
/// fixtures concurrently.
fn run_one_fixture(
    fixture_path: &Path,
    fixture_name: &str,
    test_case: &TestCase,
    output_path: &Path,
) -> bool {
    match compile_fixture(fixture_path, output_path) {
        Ok(()) => {
            if test_case.expect_error {
                eprintln!("FAIL {fixture_name}: expected compilation error but succeeded");
                false
            } else {
                match run_binary(output_path) {
                    Ok(bin_output) => {
                        let mut fixture_ok = true;

                        // Check expected exit code (default: 0 if not specified).
                        let expected_code = test_case.expected_exit_code.unwrap_or(0);
                        if bin_output.exit_code != expected_code {
                            eprintln!(
                                "FAIL {fixture_name}: expected exit code {expected_code}, got {}",
                                bin_output.exit_code
                            );
                            fixture_ok = false;
                        }

                        // Check expected stdout.
                        if let Some(expected) = &test_case.expected_stdout
                            && bin_output.stdout.trim() != expected.trim()
                        {
                            eprintln!(
                                "FAIL {fixture_name}: stdout expected '{expected}', got '{}'",
                                bin_output.stdout
                            );
                            fixture_ok = false;
                        }

                        // Check expected stderr.
                        if let Some(expected_err) = &test_case.expected_stderr
                            && bin_output.stderr.trim() != expected_err.trim()
                        {
                            eprintln!(
                                "FAIL {fixture_name}: stderr expected '{expected_err}', got '{}'",
                                bin_output.stderr
                            );
                            fixture_ok = false;
                        }

                        fixture_ok
                    }
                    Err(e) => {
                        eprintln!("FAIL {fixture_name}: runtime error: {e}");
                        false
                    }
                }
            }
        }
        Err(e) => {
            if test_case.expect_error {
                true
            } else {
                eprintln!("FAIL {fixture_name}: compilation error: {e}");
                false
            }
        }
    }
}

/// Verify that all fixture files parse successfully through the check pipeline
/// (parse -> desugar -> verify, no codegen/linking needed).
#[test]
fn test_fixtures_parse_and_verify() {
    let root = workspace_root();
    let fixtures_dir = root.join("tests/integration/fixtures");
    let fixtures = discover_fixtures(&fixtures_dir);

    assert!(
        !fixtures.is_empty(),
        "no fixtures found in {}",
        fixtures_dir.display()
    );

    let mut passed = 0;
    let mut failed = 0;

    let mut skipped = 0;

    for fixture_path in &fixtures {
        let fixture_name = fixture_path.file_stem().unwrap().to_string_lossy();
        let test_case = TestCase::load(fixture_path);

        // Skip fixtures that are expected to fail.
        if test_case.expect_error {
            skipped += 1;
            continue;
        }

        let config = CompilerConfig::new(vec![fixture_path.to_string_lossy().to_string()]);

        match driver::check(&config) {
            Ok(()) => passed += 1,
            Err(e) => {
                eprintln!("FAIL {fixture_name}: check failed: {e}");
                failed += 1;
            }
        }
    }

    eprintln!(
        "\nParse+verify results: {passed} passed, {failed} failed, {skipped} skipped (expect-error)"
    );
    // All non-expect-error fixtures should parse and verify successfully.
    assert_eq!(failed, 0, "{failed} fixture(s) failed parse+verify");
}

/// Verify that compiling a nonexistent file returns the right error.
#[test]
fn test_compile_missing_file_error() {
    let config = CompilerConfig::new(vec!["/nonexistent/fixture.js".to_string()]);
    let result = driver::compile(&config);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, driver::DriverError::FileNotFound(_)),
        "expected FileNotFound, got: {err}"
    );
}

/// Verify that compiling invalid syntax produces a lowering error.
#[test]
fn test_compile_invalid_syntax_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("bad.js");
    fs::write(&file, "let x = ;\n").expect("write fixture");

    let config = CompilerConfig::new(vec![file.to_string_lossy().to_string()]);
    let result = driver::compile(&config);
    assert!(result.is_err(), "invalid syntax should fail compilation");
}

/// Verify that `--emit ir` works for all fixtures (no codegen/link needed).
#[test]
fn test_emit_ir_for_fixtures() {
    let root = workspace_root();
    let fixtures_dir = root.join("tests/integration/fixtures");
    let fixtures = discover_fixtures(&fixtures_dir);

    assert!(
        !fixtures.is_empty(),
        "no fixtures found in {}",
        fixtures_dir.display()
    );

    for fixture_path in &fixtures {
        let fixture_name = fixture_path.file_stem().unwrap().to_string_lossy();
        let test_case = TestCase::load(fixture_path);

        // Skip fixtures that are expected to fail.
        if test_case.expect_error {
            continue;
        }

        let mut config = CompilerConfig::new(vec![fixture_path.to_string_lossy().to_string()]);
        config.emit = Some(driver::EmitKind::Ir);

        let result = driver::compile(&config);
        assert!(
            result.is_ok(),
            "emit-ir failed for {fixture_name}: {:?}",
            result.err()
        );
    }
}

/// Verify the fixture discovery helper finds expected files.
#[test]
fn test_discover_fixtures_finds_js_files() {
    let root = workspace_root();
    let fixtures_dir = root.join("tests/integration/fixtures");
    let fixtures = discover_fixtures(&fixtures_dir);

    assert!(
        fixtures.len() >= 4,
        "expected at least 4 fixtures, found {}",
        fixtures.len()
    );

    // All discovered files should have .js extension.
    for path in &fixtures {
        assert_eq!(
            path.extension().unwrap().to_str().unwrap(),
            "js",
            "non-JS file discovered: {}",
            path.display()
        );
    }
}

/// Verify that an empty/missing fixtures directory returns an empty list.
#[test]
fn test_discover_fixtures_empty_dir() {
    let fixtures = discover_fixtures(Path::new("/nonexistent/fixtures"));
    assert!(fixtures.is_empty());
}

/// Verify TestCase annotation extraction.
#[test]
fn test_annotation_extraction_inline() {
    let source = "// @expected-stdout: hello world\nconsole.log(\"hello world\");\n";
    let val = TestCase::extract_annotation(source, "@expected-stdout");
    assert_eq!(val, Some("hello world".to_string()));
}

/// Verify TestCase block annotation extraction.
#[test]
fn test_annotation_extraction_block() {
    let source = "// @expected-stdout-begin\n// line 1\n// line 2\n// @expected-stdout-end\n";
    let val = TestCase::extract_block_annotation(
        source,
        "@expected-stdout-begin",
        "@expected-stdout-end",
    );
    assert_eq!(val, Some("line 1\nline 2".to_string()));
}

/// Verify TestCase annotation extraction returns None when no annotation.
#[test]
fn test_annotation_extraction_none() {
    let source = "var x = 1;\n";
    let val = TestCase::extract_annotation(source, "@expected-stdout");
    assert_eq!(val, None);
}

/// Verify TestCase expect_error detection.
#[test]
fn test_expect_error_detection() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("err.js");
    fs::write(&file, "// @expect-error: SyntaxError\nlet x = ;\n").expect("write");
    let tc = TestCase::load(&file);
    assert!(tc.expect_error);
}

// ---------------------------------------------------------------------------
// Default output path: esc run a.out resolution (ESC-18)
// ---------------------------------------------------------------------------

/// Compile and run a fixture using the DEFAULT output path (empty `config.output`).
///
/// Verifies that the compiler produces a runnable binary without an explicit
/// `--output` flag — the binary must be findable by `Command::new`.
/// This is an E2E regression test for ESC-18 (a.out PATH-lookup bug).
#[test]
fn test_compile_run_default_output_path() {
    if !runtime_lib_available() {
        eprintln!("SKIP: libruntime.a not found — cannot link");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = dir.path().join("hello.js");
    fs::write(&fixture, r#"console.log("hello default output");"#).expect("write fixture");

    // Compile WITHOUT setting config.output — exercises the default path.
    let mut config = CompilerConfig::new(vec![fixture.to_string_lossy().to_string()]);
    config.no_config = true;
    let result = driver::compile(&config)
        .map_err(|e| format!("compile failed: {e}"))
        .expect("compilation should succeed with default output");

    // The binary must exist and be runnable at the path compile() reports.
    let binary_path = Path::new(&result.output_path);
    assert!(
        binary_path.exists(),
        "default output binary not found at '{}'",
        result.output_path
    );

    let output = run_binary(binary_path).expect("run default output binary");
    assert_eq!(output.stdout, "hello default output");
    assert_eq!(output.exit_code, 0);
}
