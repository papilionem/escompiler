//! test262 YAML frontmatter parser.
//!
//! Each test262 file contains a `/*--- ... ---*/` YAML block with metadata
//! describing the test's requirements, expected behavior, and classification.

use std::path::Path;

/// Parsed test262 test metadata from YAML frontmatter.
#[derive(Debug, Clone, Default)]
pub struct TestMetadata {
    /// Human-readable test description.
    pub description: String,
    /// Expected negative outcome (parse error, runtime error, etc.).
    pub negative: Option<NegativeExpectation>,
    /// Feature flags required by this test (e.g. `BigInt`, `Promise`).
    pub features: Vec<String>,
    /// Test flags (e.g. `onlyStrict`, `noStrict`, `module`, `async`, `raw`).
    pub flags: Vec<String>,
    /// Harness include files required (e.g. `assert.js`, `sta.js`).
    pub includes: Vec<String>,
    /// ECMAScript specification ID (e.g. `sec-abstract-equality-comparison`).
    pub es_id: Option<String>,
}

impl TestMetadata {
    /// Whether this test must be run in strict mode only.
    pub fn is_only_strict(&self) -> bool {
        self.flags.iter().any(|f| f == "onlyStrict")
    }

    /// Whether this test must NOT be run in strict mode.
    pub fn is_no_strict(&self) -> bool {
        self.flags.iter().any(|f| f == "noStrict")
    }

    /// Whether this test is an ES module test.
    pub fn is_module(&self) -> bool {
        self.flags.iter().any(|f| f == "module")
    }

    /// Whether this test is async (requires `$DONE` callback).
    pub fn is_async(&self) -> bool {
        self.flags.iter().any(|f| f == "async")
    }

    /// Whether this test should be used as-is without any harness preamble.
    pub fn is_raw(&self) -> bool {
        self.flags.iter().any(|f| f == "raw")
    }
}

/// Expected negative outcome for a test.
#[derive(Debug, Clone)]
pub struct NegativeExpectation {
    /// The phase at which the error should occur.
    pub phase: NegativePhase,
    /// The expected error type name (e.g. `SyntaxError`, `ReferenceError`).
    pub error_type: String,
}

/// The phase at which a negative test expects an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegativePhase {
    /// Error during parsing.
    Parse,
    /// Error during module resolution.
    Resolution,
    /// Error during execution.
    Runtime,
}

/// Parse test262 YAML frontmatter from a test source file.
///
/// Frontmatter is enclosed in `/*---` and `---*/` markers. Returns a default
/// [`TestMetadata`] if no frontmatter is found.
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

    // Track whether we're inside the `negative:` block
    let mut in_negative = false;
    let mut neg_phase: Option<NegativePhase> = None;
    let mut neg_type: Option<String> = None;

    for line in yaml.lines() {
        let trimmed = line.trim();

        // Detect indented sub-keys under `negative:`
        if in_negative {
            if let Some(phase_str) = trimmed.strip_prefix("phase:") {
                neg_phase = Some(match phase_str.trim() {
                    "parse" => NegativePhase::Parse,
                    "resolution" => NegativePhase::Resolution,
                    _ => NegativePhase::Runtime,
                });
                continue;
            }
            if let Some(err_type) = trimmed.strip_prefix("type:") {
                neg_type = Some(err_type.trim().to_string());
                continue;
            }
            // If we hit a non-indented line that isn't a sub-key, we've left the block
            if !line.starts_with(' ') && !line.starts_with('\t') && !trimmed.is_empty() {
                in_negative = false;
            }
        }

        if let Some(desc) = trimmed.strip_prefix("description:") {
            let desc = desc.trim();
            // Handle both quoted and unquoted values, and multiline `>` / `|`
            if desc == ">" || desc == "|" {
                // Multiline description — skip for now, not critical
                continue;
            }
            meta.description = desc.trim_matches(|c| c == '\'' || c == '"').to_string();
        } else if trimmed.starts_with("flags:") {
            if let Some(items) = extract_bracket_list(trimmed) {
                meta.flags = items;
            }
        } else if trimmed.starts_with("features:") {
            if let Some(items) = extract_bracket_list(trimmed) {
                meta.features = items;
            }
        } else if trimmed.starts_with("includes:") {
            if let Some(items) = extract_bracket_list(trimmed) {
                meta.includes = items;
            }
        } else if trimmed.starts_with("esid:")
            || trimmed.starts_with("es5id:")
            || trimmed.starts_with("es6id:")
        {
            if let Some((_key, val)) = trimmed.split_once(':') {
                let val = val.trim();
                if !val.is_empty() {
                    meta.es_id = Some(val.to_string());
                }
            }
        } else if trimmed.starts_with("negative:") {
            in_negative = true;
            // Some tests have inline `negative: { phase: parse, type: SyntaxError }`
            // but test262 standard uses the indented form
        }
    }

    // Assemble negative expectation if we found phase or type
    if neg_phase.is_some() || neg_type.is_some() {
        meta.negative = Some(NegativeExpectation {
            phase: neg_phase.unwrap_or(NegativePhase::Runtime),
            error_type: neg_type.unwrap_or_default(),
        });
    }

    meta
}

/// Extract a YAML inline list like `[item1, item2, item3]` from a line.
fn extract_bracket_list(line: &str) -> Option<Vec<String>> {
    let start = line.find('[')?;
    let end = line.find(']')?;
    if start >= end {
        return None;
    }
    let inner = &line[start + 1..end];
    Some(
        inner
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

/// Features currently supported by the compiler.
///
/// Tests requiring features NOT in this list will be **skipped** (excluded from
/// both numerator and denominator). Only declare features that have a reasonable
/// chance of passing end-to-end (desugar → IR → verify → codegen → link → run).
///
/// # Rules for maintaining this list
///
/// 1. **Only add a feature when it actually works end-to-end**, not when
///    "infrastructure exists." Declaring support for a non-functional feature
///    inflates the denominator and deflates the pass rate.
/// 2. **When adding a feature, note the version** it was added in.
/// 3. **Review this list at the start of each version** — add features that
///    the new version implements, verify existing entries still make sense.
/// 4. **See `docs/research/29-test262-re-estimation.md`** for the full analysis
///    of why previous over-declaration caused systematic target misses.
///
/// # Features NOT listed here (intentionally excluded until implemented)
///
/// The following are deferred and should be re-added when their version ships:
///
/// | Feature | Re-add in | Reason excluded |
/// |---------|-----------|-----------------|
/// | `TypedArray` | v0.9 | Not implemented |
/// | ~~`Proxy`~~ | ~~v0.6~~ | Added in v0.6 |
/// | ~~`Reflect`~~ | ~~v0.6~~ | Added in v0.6 |
/// | ~~`Map`~~ | ~~v0.8~~ | Added in v0.8 |
/// | ~~`Set`~~ | ~~v0.8~~ | Added in v0.8 |
/// | ~~`WeakMap`~~ | ~~v0.8~~ | Added in v0.8 |
/// | ~~`WeakSet`~~ | ~~v0.8~~ | Added in v0.8 |
/// | ~~`WeakRef`~~ | ~~v0.8~~ | Added in v0.8 |
/// | `FinalizationRegistry` | v0.9 | Requires GC integration |
/// | ~~`regexp-dotall`~~ | ~~v0.8~~ | Added in v0.8 |
/// | ~~`regexp-named-groups`~~ | ~~v0.8~~ | Added in v0.8 |
/// | `regexp-unicode-property-escapes` | v0.9 | Requires Unicode property tables |
/// | `regexp-match-indices` | v0.9 | Requires `d` flag + indices array |
/// | `regexp-v-flag` | v0.9 | Requires `v` flag implementation |
/// | `regexp-modifiers` | v0.9 | Requires inline modifier syntax |
/// | ~~`dynamic-import`~~ | ~~v0.6~~ | Added in v0.6 |
/// | `import-assertions` / `import-attributes` | v0.9 | Not implemented |
/// | ~~`String.prototype.matchAll`~~ | ~~v0.7~~ | Added in v0.7 |
/// | ~~`String.prototype.replaceAll`~~ | ~~v0.8~~ | Added in v0.8 |
/// | ~~`Array.prototype.flat`~~ | ~~v0.7~~ | Added in v0.7 |
/// | ~~`Array.prototype.flatMap`~~ | ~~v0.7~~ | Added in v0.7 |
/// | ~~`Object.fromEntries`~~ | ~~v0.7~~ | Added in v0.7 |
/// | `well-formed-json-stringify` | v0.9 | Not implemented |
/// | `BigInt` | v0.9 | Not implemented |
/// | `Atomics` | v0.9+ | Not implemented |
/// | `SharedArrayBuffer` | v0.9+ | Not implemented |
/// | `Temporal` | v1.0+ | Stage 3 proposal |
/// | `decorators` | v1.0+ | Stage 3 proposal |
/// | `using` / `await using` | v1.0+ | Explicit Resource Management |
/// | `iterator-helpers` | v0.9 | Runtime-only, needs full built-in verification |
/// | `Symbol.species` | v0.9 | Not wired into built-in constructors |
pub const SUPPORTED_FEATURES: &[&str] = &[
    // =========================================================================
    // Core language (v0.1-v0.2) — stable, well-tested
    // =========================================================================
    "let",
    "const",
    "for-of",
    "for-in",
    "arrow-function",
    "default-parameters",
    "rest-parameters",
    "spread",
    "template",
    "computed-property-names",
    "optional-catch-binding",
    "optional-chaining",
    "coalesce-expression",
    "numeric-separator-literal",
    "globalThis", // v0.3
    "logical-assignment-operators",
    // =========================================================================
    // Destructuring (v0.1-v0.2) — partially working, many edge cases
    // =========================================================================
    "destructuring-binding",
    "destructuring-assignment",
    "object-spread",
    "object-rest",
    // =========================================================================
    // Classes (v0.4) — infrastructure built, edge cases being hardened
    // =========================================================================
    "class",
    "class-fields-public",
    "class-fields-private",
    "class-methods-private",
    "class-static-fields-public",
    "class-static-fields-private",
    "class-static-methods-private",
    // =========================================================================
    // Symbols (v0.4) — NaN-boxed, well-known symbols wired
    // =========================================================================
    "Symbol",
    "Symbol.hasInstance",
    "Symbol.iterator",
    "Symbol.toPrimitive",
    "Symbol.toStringTag",
    // =========================================================================
    // Generators + Async (v0.4) — state machine transform, async_wrap
    // =========================================================================
    "generators",
    "Promise",
    "async-functions",
    // =========================================================================
    // Async iteration + Promise combinators (v0.5) — async generators,
    // for-await-of, top-level await, Promise.allSettled/any
    // =========================================================================
    "async-iteration",
    "top-level-await",
    "Promise.allSettled",
    "Promise.any",
    // =========================================================================
    // Object/Array methods — only those verified working
    // =========================================================================
    "Object.entries",
    "Object.values",
    "Object.is",
    "Array.from",
    "Array.of",
    "Array.prototype.includes",
    "Array.prototype.at",
    "Array.prototype.values",
    "Array.prototype.keys",
    "Array.prototype.entries",
    // =========================================================================
    // String methods — only those verified working
    // =========================================================================
    "String.prototype.includes",
    "String.prototype.trimEnd",
    "String.prototype.trimStart",
    "String.prototype.endsWith",
    "String.prototype.startsWith",
    "String.prototype.at",       // v0.7
    "String.prototype.matchAll", // v0.7 (string pattern only)
    "string-trimming",           // v0.7 (trim/trimEnd/trimStart)
    // =========================================================================
    // Array methods — v0.7 completions
    // =========================================================================
    "Array.prototype.flatMap",       // v0.7
    "Array.prototype.flat",          // v0.7
    "Array.prototype.findLast",      // v0.7
    "Array.prototype.findLastIndex", // v0.7
    "change-array-by-copy",          // v0.7 (toSorted, toReversed, toSpliced)
    // =========================================================================
    // Object methods — v0.7
    // =========================================================================
    "Object.fromEntries", // v0.7
    "Object.hasOwn",      // v0.7
    // =========================================================================
    // Collections (v0.8) — Map, Set, WeakMap, WeakSet, WeakRef
    // =========================================================================
    "Map",         // v0.8 — constructor iterable, all methods, Symbol.iterator
    "Set",         // v0.8 — constructor iterable, ES2025 methods
    "set-methods", // v0.8 — union, intersection, difference, symmetricDifference, etc.
    "WeakMap",     // v0.8 — constructor iterable, get/set/has/delete
    "WeakSet",     // v0.8 — constructor iterable, add/has/delete
    "WeakRef",     // v0.8 — constructor, deref
    // =========================================================================
    // RegExp (v0.8) — fancy-regex backend, dotAll, named groups, lookbehind
    // =========================================================================
    "regexp-dotall",       // v0.8 — `s` flag supported
    "regexp-named-groups", // v0.8 — `(?<name>...)` via fancy-regex
    "regexp-lookbehind",   // v0.8 — `(?<=...)` and `(?<!...)` via fancy-regex
    "Symbol.match",        // v0.8 — RegExp.prototype[Symbol.match]
    "Symbol.replace",      // v0.8 — RegExp.prototype[Symbol.replace]
    "Symbol.split",        // v0.8 — RegExp.prototype[Symbol.split]
    "Symbol.search",       // v0.8 — RegExp.prototype[Symbol.search]
    "Symbol.matchAll",     // v0.8 — RegExp.prototype[Symbol.matchAll]
    // =========================================================================
    // String methods — v0.8 additions
    // =========================================================================
    "String.prototype.replaceAll", // v0.8 — now with RegExp support
    // =========================================================================
    // Metaprogramming (v0.6) — Proxy traps, Reflect, dynamic import
    // =========================================================================
    "Proxy",
    "Reflect",
    "Reflect.apply",
    "Reflect.construct",
    "Reflect.defineProperty",
    "Reflect.deleteProperty",
    "Reflect.get",
    "Reflect.getOwnPropertyDescriptor",
    "Reflect.getPrototypeOf",
    "Reflect.has",
    "Reflect.isExtensible",
    "Reflect.ownKeys",
    "Reflect.preventExtensions",
    "Reflect.set",
    "Reflect.setPrototypeOf",
    "dynamic-import",
];

/// Check whether all required features for a test are supported.
pub fn all_features_supported(features: &[String]) -> bool {
    features
        .iter()
        .all(|f| SUPPORTED_FEATURES.contains(&f.as_str()))
}

/// Locate the test262 data directory relative to the workspace root.
///
/// Returns `None` if the test262 submodule has not been cloned.
pub fn find_test262_root(workspace_root: &Path) -> Option<std::path::PathBuf> {
    let candidate = workspace_root.join("tests").join("test262").join("test262");
    if candidate.exists() && candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

/// Locate the harness directory containing `assert.js`, `sta.js`, etc.
///
/// Returns the pinned upstream test262 checkout's harness directory
/// (`tests/test262/test262/harness`). The local simplified harness was
/// removed in ESC-27 — tests must be evaluated against the harness the
/// suite authors wrote, not a hand-simplified copy.
pub fn find_harness_dir(workspace_root: &Path) -> std::path::PathBuf {
    workspace_root
        .join("tests")
        .join("test262")
        .join("test262")
        .join("harness")
}
