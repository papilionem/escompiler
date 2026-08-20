//! test262 conformance testing infrastructure for ESCompiler.
//!
//! Provides a YAML frontmatter parser, test runner, and reporting for running
//! the official ECMAScript test262 suite against the compiler pipeline.
//!
//! # Key types
//!
//! - [`TestMetadata`] — parsed test262 frontmatter (description, features, flags, negative)
//! - [`TestRunner`] — discovers and executes test262 tests, collecting results
//! - [`TestResult`] — outcome of a single test (pass, fail, skip, error)
//! - [`SuiteReport`] — aggregate summary of a test run
//!
//! # Usage
//!
//! ```rust,no_run
//! use test262::{TestRunner, RunnerConfig};
//! use std::path::Path;
//!
//! let config = RunnerConfig {
//!     test262_root: Path::new("tests/test262/test262").to_path_buf(),
//!     harness_dir: Path::new("tests/test262/test262/harness").to_path_buf(),
//!     max_failures: Some(50),
//!     timeout_secs: 10,
//! };
//! let runner = TestRunner::new(config);
//! let report = runner.run_all();
//! println!("{report}");
//! ```

pub mod harness;
pub mod runner;

#[cfg(test)]
mod tests;

pub use harness::{NegativeExpectation, NegativePhase, TestMetadata};
pub use runner::{ProgressSummary, RunnerConfig, SuiteReport, TestOutcome, TestResult, TestRunner};
