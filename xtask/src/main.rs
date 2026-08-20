//! `cargo xtask verify` — the corpus runner.
//!
//! Every sealed rung exit criterion (docs/planning/rungs/v0.9.md … v0.13.md) is a
//! command that invokes this binary. It is therefore the single thing that decides
//! whether a rung has shipped, which is exactly why it carries its own negative
//! control: `cargo xtask verify --self-test` seeds eight defects and asserts each
//! one is caught.
//!
//! The property it exists to guarantee, stated once:
//!
//!   **A run that verified nothing must never look like a run that verified
//!   everything and found no failures.**
//!
//! The harness this replaces printed "318 passed, 46 failed" followed by "test
//! result: ok", and printed the same "ok" in 0.00s when it had executed nothing.

mod manifest;
mod notes;
mod runner;
mod selftest;
mod verdict;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use manifest::Manifest;
use verdict::Verdict;

const DEFAULT_MANIFEST: &str = "tests/verify/manifest.toml";

fn repo_root() -> PathBuf {
    // xtask/ -> repo root
    // No .expect(): Rule 2 forbids it outside tests, and a tool that panics on
    // its own layout is worse than one that says what it could not find.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    match manifest.parent() {
        Some(p) => p.to_path_buf(),
        None => {
            eprintln!(
                "FATAL: cannot locate the repo root above {}",
                manifest.display()
            );
            std::process::exit(2);
        }
    }
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  \
         cargo xtask verify [--manifest PATH] [--node BIN]\n  \
         cargo xtask verify --self-test\n  \
         cargo xtask notes --version X.Y.Z\n  \
         cargo xtask notes --check BASE_REF\n  \
         cargo xtask notes --self-test\n\n\
         Exit codes:\n  \
         0  every entry checked, none failed unexpectedly\n  \
         1  at least one entry failed, or a registered xfail entry now PASSES\n  \
         2  the run could not be trusted (no manifest, no oracle, zero entries)"
    );
    std::process::exit(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    match args[0].as_str() {
        "verify" => verify(&args[1..]),
        "notes" => notes_cmd(&args[1..]),
        _ => usage(),
    }
}

/// `cargo xtask notes` — assemble release notes, and refuse when one is owed.
///
/// ADR-0002 V11 and V12. The three modes are deliberately one command: the
/// generator and the check that a note exists share the definition of what a
/// fragment is, so they cannot drift apart the way two scripts would.
fn notes_cmd(args: &[String]) -> ExitCode {
    let root = repo_root();
    let dir = root.join(".changes");

    match args.first().map(String::as_str) {
        Some("--version") => {
            let Some(version) = args.get(1) else {
                eprintln!("notes: --version needs a value");
                return ExitCode::from(2);
            };
            let frags = match notes::load_fragments(&dir) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("notes: {e}");
                    return ExitCode::from(2);
                }
            };
            if frags.is_empty() {
                // V12 in the assembly path. An empty release note and a release
                // nobody described are indistinguishable in the output, and the
                // second is far more common.
                eprintln!(
                    "notes: no fragments in .changes/ — refusing to write an empty \
                     section for {version}"
                );
                return ExitCode::from(2);
            }
            let date = args
                .get(2)
                .cloned()
                .unwrap_or_else(|| "UNDATED".to_string());
            let rendered = notes::render(version, &date, &frags);
            let path = root.join("CHANGELOG.md");
            let current = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("notes: {}: {e}", path.display());
                    return ExitCode::from(2);
                }
            };
            match notes::splice(&current, &rendered) {
                Ok(next) => {
                    if let Err(e) = std::fs::write(&path, next) {
                        eprintln!("notes: {}: {e}", path.display());
                        return ExitCode::from(2);
                    }
                    println!("notes: wrote {} entries for {version}", frags.len());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("notes: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Some("--check") => {
            let Some(base) = args.get(1) else {
                eprintln!("notes: --check needs a base ref");
                return ExitCode::from(2);
            };
            // Refuse on a dirty tree before computing anything. semantic_changes
            // compares COMMITTED history, so on a dirty tree the check answers a
            // question nobody asked and answers it green.
            let dirty = notes::uncommitted_semantic(&root);
            if !dirty.is_empty() {
                eprintln!(
                    "REFUSED: {} semantic file(s) have uncommitted changes, so a diff \n\
                     against {base} is not the change being proposed:",
                    dirty.len()
                );
                for d in dirty.iter().take(10) {
                    eprintln!("  {d}");
                }
                eprintln!("Commit first, then re-run. A green here would mean nothing.");
                return ExitCode::from(2);
            }
            let changed = notes::semantic_changes(&root, base);
            let frags = match notes::load_fragments(&dir) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("notes: {e}");
                    return ExitCode::from(2);
                }
            };
            // Print the classification either way. A check that says only "ok"
            // cannot be audited afterwards, and this one decides whether a change
            // is allowed to ship without a release note.
            println!("semantic files changed vs {base}: {}", changed.len());
            for c in changed.iter().take(10) {
                println!("  {c}");
            }
            if changed.len() > 10 {
                println!("  ... and {} more", changed.len() - 10);
            }
            println!("fragments in .changes/: {}", frags.len());
            // Print each fragment's surface and witness, not just the count.
            // ADR-0002 V4 turns on the witness: a behaviour change with none is
            // breaking, not fixed. A check that prints only a count hides the
            // one field that decides which.
            for f in &frags {
                let name = f
                    .file
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "?".to_string());
                println!(
                    "  {name}  section={}  surface={}  witness={}",
                    f.section, f.surface, f.witness
                );
                if f.surface == "tc39" && f.witness == "none" {
                    println!(
                        "    note: a tc39-surface change with no witness is BREAKING, \
                         not a fix (ADR-0002 V4)"
                    );
                }
            }

            if changed.is_empty() {
                println!("no semantic source touched — no fragment required");
                return ExitCode::SUCCESS;
            }
            if frags.is_empty() {
                eprintln!(
                    "REFUSED: {} semantic file(s) changed and .changes/ is empty.\n\
                     ADR-0002 V12: absence is a refusal, not a default. See \
                     .changes/README.md",
                    changed.len()
                );
                return ExitCode::from(1);
            }
            println!(
                "ok: semantic change is described by {} fragment(s)",
                frags.len()
            );
            ExitCode::SUCCESS
        }
        Some("--self-test") => notes_self_test(&root),
        _ => {
            eprintln!(
                "usage: cargo xtask notes --version X.Y.Z [DATE] | --check BASE | --self-test"
            );
            ExitCode::from(2)
        }
    }
}

/// The controls for `notes`.
///
/// A generator that has never been shown to refuse anything is a generator
/// nobody has tested. Each case here is a shape that would otherwise publish a
/// plausible-but-wrong entry.
fn notes_self_test(root: &std::path::Path) -> ExitCode {
    use std::fs;
    let tmp = std::env::temp_dir().join(format!("esc-notes-selftest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    if fs::create_dir_all(&tmp).is_err() {
        eprintln!("self-test: could not create a scratch directory");
        return ExitCode::from(2);
    }
    let mut rc = 0u8;
    let mut check = |name: &str, ok: bool| {
        println!("  {} {name}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            rc = 1;
        }
    };

    // Seed: a fragment declaring a section Common Changelog does not have.
    let bad = tmp.join("1-bad.md");
    let _ = fs::write(
        &bad,
        "---\nsection: Improved\nsurface: owned\nwitness: none\npr: 1\n---\n\nBody.\n",
    );
    check(
        "seed: unknown section is refused",
        notes::parse_fragment(&bad).is_err(),
    );

    // Seed: no witness. This is the field carrying ADR-0002 V4's argument, so a
    // default would silently turn a breaking change into a "fix".
    let nowit = tmp.join("2-nowitness.md");
    let _ = fs::write(
        &nowit,
        "---\nsection: Fixed\nsurface: owned\npr: 2\n---\n\nBody.\n",
    );
    check(
        "seed: absent witness is refused, never defaulted",
        notes::parse_fragment(&nowit).is_err(),
    );

    // Inverse: a well-formed fragment parses and renders with its PR reference.
    let good = tmp.join("3-good.md");
    let _ = fs::write(
        &good,
        "---\nsection: Fixed\nsurface: owned\nwitness: none\npr: 3\n---\n\nIt works.\n",
    );
    let parsed = notes::parse_fragment(&good);
    let rendered = parsed
        .as_ref()
        .map(|f| notes::render("0.0.0", "DATE", std::slice::from_ref(f)))
        .unwrap_or_default();
    check(
        "inverse: a valid fragment renders with its PR link",
        parsed.is_ok() && rendered.contains("([#3])"),
    );

    // Empty: splicing into a document with no generated region must refuse
    // rather than append. Guessing where generated content belongs is how a
    // generator destroys hand-written text.
    check(
        "empty: no generated region is refused, not appended to",
        notes::splice("nothing here\n", "X").is_err(),
    );

    // Empty: the real CHANGELOG must actually contain the region. Without this,
    // every splice test above passes against a document that could not be
    // written to in practice.
    let real = fs::read_to_string(root.join("CHANGELOG.md")).unwrap_or_default();
    check(
        "empty: the real CHANGELOG.md has a generated region",
        notes::splice(&real, "X").is_ok(),
    );

    let _ = fs::remove_dir_all(&tmp);
    if rc == 0 {
        println!("self-test: 5 controls passed");
        ExitCode::SUCCESS
    } else {
        eprintln!("self-test: FAILED");
        ExitCode::from(1)
    }
}

fn verify(args: &[String]) -> ExitCode {
    let root = repo_root();
    let mut manifest_path = root.join(DEFAULT_MANIFEST);
    let mut node = "node".to_string();
    let mut self_test = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--self-test" => self_test = true,
            "--manifest" => {
                i += 1;
                manifest_path = root.join(args.get(i).unwrap_or_else(|| usage()));
            }
            "--node" => {
                i += 1;
                node = args.get(i).unwrap_or_else(|| usage()).clone();
            }
            _ => usage(),
        }
        i += 1;
    }

    if self_test {
        return selftest::run(&root, &node);
    }

    let m = match Manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("FATAL: {e}");
            return ExitCode::from(2);
        }
    };

    let workdir = std::env::temp_dir().join(format!("esc-verify-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&workdir) {
        eprintln!("FATAL: cannot create work dir: {e}");
        return ExitCode::from(2);
    }

    let env = runner::Env {
        esc: runner::esc_path(&root),
        root: root.clone(),
        node,
        workdir: workdir.clone(),
    };

    if let Err(e) = runner::check_preconditions(&env, &m.node_pin) {
        eprintln!("FATAL: {e}");
        let _ = std::fs::remove_dir_all(&workdir);
        return ExitCode::from(2);
    }

    println!("cargo xtask verify");
    println!("==================");
    println!("manifest: {}", manifest_path.display());
    println!("oracle:   node {}", m.node_pin);
    println!("entries:  {}", m.entry.len());
    println!();

    let (mut executed, mut passed, mut failed) = (0usize, 0usize, 0usize);
    let mut unexpected_fail: Vec<String> = Vec::new();
    let mut unexpected_pass: Vec<String> = Vec::new();

    for e in &m.entry {
        let v = runner::check(&env, e);
        executed += 1;
        match (&v, &e.xfail) {
            (Verdict::Pass, None) => {
                passed += 1;
                println!("  PASS      {}", e.id);
            }
            (Verdict::Fail(why), None) => {
                failed += 1;
                unexpected_fail.push(e.id.clone());
                println!("  FAIL      {}\n      {why}", e.id);
                if !e.note.is_empty() {
                    println!("      note: {}", e.note);
                }
            }
            (Verdict::Fail(_), Some(reason)) => {
                failed += 1;
                println!("  xfail     {}  ({reason})", e.id);
            }
            (Verdict::Pass, Some(_)) => {
                passed += 1;
                unexpected_pass.push(e.id.clone());
                println!("  FIXED     {}  <- registered xfail, now passes", e.id);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&workdir);

    println!();
    println!(
        "executed {executed} of {} · {passed} passed · {failed} failed \
         ({} registered xfail)",
        m.entry.len(),
        m.entry.iter().filter(|e| e.xfail.is_some()).count()
    );

    // The property this whole binary exists for.
    if executed == 0 {
        eprintln!(
            "\nFATAL: executed 0 entries. That is a failure, not a pass — a verifier \
             with nothing to verify must never report success."
        );
        return ExitCode::from(2);
    }

    if !unexpected_pass.is_empty() {
        eprintln!(
            "\n{} entr(y/ies) are registered as xfail but now PASS. This is how the tool \
             tells you something got fixed — remove the `xfail` line:\n  {}",
            unexpected_pass.len(),
            unexpected_pass.join("\n  ")
        );
        return ExitCode::from(1);
    }
    if !unexpected_fail.is_empty() {
        eprintln!(
            "\n{} entr(y/ies) failed and are not registered as xfail:\n  {}",
            unexpected_fail.len(),
            unexpected_fail.join("\n  ")
        );
        return ExitCode::from(1);
    }

    println!("\nall {executed} entries verified.");
    ExitCode::SUCCESS
}
