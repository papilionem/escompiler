//! Executing one manifest entry.
//!
//! Every path through here either produces a Pass or a Fail with a reason.
//! Nothing returns early with an "ok, but" — the preconditions (esc binary,
//! runtime archive, node, matching pin) are checked once up front and are hard
//! errors, because each of them used to be a silent skip.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::manifest::{Entry, Kind};
use crate::verdict::{ExitClass, StderrClass, Verdict};

pub struct Env {
    pub root: PathBuf,
    pub esc: PathBuf,
    pub node: String,
    pub workdir: PathBuf,
}

/// Where the `esc` binary is, honouring `CARGO_TARGET_DIR`.
///
/// CI backends differ: the Dell fleet sets `CARGO_TARGET_DIR=/cache/target/<job>`,
/// so `<root>/target/debug/esc` does not exist there. `check_preconditions`
/// below already resolves `libruntime.a` this way; this function exists so the
/// binary is resolved by the SAME rule, in ONE place.
///
/// It is a free function rather than inline in `verify()` because it was inline
/// once and `selftest.rs` built its own `Env` with the hardcoded path — so the
/// self-test spawned nothing on CI while `verify` worked. Six of the eight
/// controls assert only "the verifier returned Fail", and a spawn failure IS a
/// Fail, so they reported "caught" having exercised nothing. One resolver, used
/// by both entry points, is what stops that recurring.
pub fn esc_path(root: &Path) -> PathBuf {
    std::env::var("CARGO_TARGET_DIR")
        .ok()
        .map(|d| Path::new(&d).join("debug/esc"))
        .unwrap_or_else(|| root.join("target/debug/esc"))
}

struct Run {
    stdout: String,
    stderr: String,
    class: ExitClass,
}

fn run(cmd: &mut Command) -> Result<Run, String> {
    let out = cmd.output().map_err(|e| format!("spawn failed: {e}"))?;
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        out.status.signal()
    };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    Ok(Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        class: ExitClass::from_raw(out.status.code(), signal),
    })
}

/// Compile `program` and run the artifact. The artifact is deleted immediately:
/// the fixture harness once kept every linked binary for a whole run and reached
/// 9.6 GB, failing on disk *inside* a green exit.
fn compile_and_run(env: &Env, entry: &Entry) -> Result<(Run, Run), String> {
    let src = env.root.join(&entry.program);
    if !src.exists() {
        return Err(format!("program not found: {}", src.display()));
    }
    let out_bin = env.workdir.join(format!("{}.out", entry.id));

    let build = run(Command::new(&env.esc)
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&out_bin))?;

    if !out_bin.exists() {
        // Build produced nothing; the caller decides whether that is expected.
        return Ok((
            build,
            Run {
                stdout: String::new(),
                stderr: String::new(),
                class: ExitClass::Error(-1),
            },
        ));
    }
    let exec = run(&mut Command::new(&out_bin))?;
    let _ = std::fs::remove_file(&out_bin);
    Ok((build, exec))
}

fn node_run(env: &Env, entry: &Entry) -> Result<Run, String> {
    run(Command::new(&env.node).arg(env.root.join(&entry.program)))
}

pub fn check(env: &Env, entry: &Entry) -> Verdict {
    match &entry.kind {
        Kind::Match => check_match(env, entry),
        Kind::Refused { code } => check_refused(env, entry, code),
        Kind::Artifact { absent_symbols } => check_artifact(env, entry, absent_symbols),
    }
}

/// stdout byte-identical to pinned Node, plus stderr class and exit class.
fn check_match(env: &Env, entry: &Entry) -> Verdict {
    let oracle = match node_run(env, entry) {
        Ok(o) => o,
        Err(e) => return Verdict::fail(format!("oracle unreachable: {e}")),
    };
    if !oracle.class.is_clean() {
        return Verdict::fail(format!(
            "the ORACLE failed on this program ({}) — the entry is wrong, not the compiler",
            oracle.class
        ));
    }
    let (build, exec) = match compile_and_run(env, entry) {
        Ok(v) => v,
        Err(e) => return Verdict::fail(e),
    };
    if !build.class.is_clean() {
        return Verdict::fail(format!(
            "build failed ({}): {}",
            build.class,
            first_line(&build.stderr)
        ));
    }
    if exec.stdout != oracle.stdout {
        return Verdict::fail(format!(
            "stdout differs from node {}\n      node: {:?}\n      esc:  {:?}",
            env.node,
            truncate(&oracle.stdout),
            truncate(&exec.stdout)
        ));
    }
    let (want, got) = (
        StderrClass::of(&oracle.stderr),
        StderrClass::of(&exec.stderr),
    );
    if want != got {
        return Verdict::fail(format!("stderr class: node {want}, esc {got}"));
    }
    if exec.class != oracle.class {
        return Verdict::fail(format!(
            "exit class: node {}, esc {}",
            oracle.class, exec.class
        ));
    }
    Verdict::Pass
}

/// The compiler must refuse: exit 2, with the declared code named on stderr.
fn check_refused(env: &Env, entry: &Entry, code: &str) -> Verdict {
    let (build, _exec) = match compile_and_run(env, entry) {
        Ok(v) => v,
        Err(e) => return Verdict::fail(e),
    };
    match build.class {
        ExitClass::Refused => {}
        ExitClass::Ok => {
            return Verdict::fail(
                "compiled successfully; a program using an unimplemented feature must be REFUSED \
                 (exit 2), not accepted"
                    .to_string(),
            );
        }
        other => {
            return Verdict::fail(format!(
                "exited {other}; refusal is exit 2 and must stay distinguishable from a \
                 compile failure, which is exit 1"
            ));
        }
    }
    if !build.stderr.contains(code) {
        return Verdict::fail(format!(
            "refused, but stderr does not name {code}: {:?}",
            truncate(&build.stderr)
        ));
    }
    Verdict::Pass
}

/// A property of the built artifact — currently, symbols that must be absent.
fn check_artifact(env: &Env, entry: &Entry, absent: &[String]) -> Verdict {
    let src = env.root.join(&entry.program);
    let out_bin = env.workdir.join(format!("{}.artifact", entry.id));
    let build = match run(Command::new(&env.esc)
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&out_bin))
    {
        Ok(b) => b,
        Err(e) => return Verdict::fail(e),
    };
    if !build.class.is_clean() || !out_bin.exists() {
        return Verdict::fail(format!("build failed ({})", build.class));
    }
    let nm = Command::new("nm")
        .arg("-D")
        .arg("--defined-only")
        .arg(&out_bin)
        .output();
    let syms = match nm {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(e) => {
            let _ = std::fs::remove_file(&out_bin);
            return Verdict::fail(format!("nm unavailable: {e}"));
        }
    };
    // Also read the full symbol table, since a static binary's dynamic table is
    // nearly empty and would make any absence assertion trivially true.
    let nm_all = Command::new("nm").arg(&out_bin).output();
    let all = nm_all
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let _ = std::fs::remove_file(&out_bin);

    let haystack = format!("{syms}\n{all}");
    if haystack.trim().is_empty() {
        return Verdict::fail(
            "nm produced no symbols at all — an absence assertion over an empty symbol table \
             is vacuously true and proves nothing"
                .to_string(),
        );
    }
    for want_absent in absent {
        if haystack.contains(want_absent.as_str()) {
            return Verdict::fail(format!("symbol {want_absent:?} is present and must not be"));
        }
    }
    Verdict::Pass
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}

fn truncate(s: &str) -> String {
    let s = s.trim_end_matches('\n');
    if s.chars().count() > 120 {
        format!("{}…", s.chars().take(120).collect::<String>())
    } else {
        s.to_string()
    }
}

/// Preconditions. Each of these was a silent skip in the previous harness, and
/// each therefore produced a green run that had verified nothing.
pub fn check_preconditions(env: &Env, pin: &str) -> Result<(), String> {
    if !env.esc.exists() {
        return Err(format!(
            "esc binary not found at {} — run `cargo build`. This is a failure, not a skip.",
            env.esc.display()
        ));
    }
    let lib_debug = env.root.join("target/debug/libruntime.a");
    let lib_env = std::env::var("CARGO_TARGET_DIR")
        .ok()
        .map(|d| Path::new(&d).join("debug/libruntime.a"));
    let have_lib = lib_debug.exists() || lib_env.as_ref().is_some_and(|p| p.exists());
    if !have_lib {
        return Err(
            "libruntime.a not found — run `cargo build -p runtime`. This is a failure, not a skip: \
             a run that links nothing verifies nothing."
                .to_string(),
        );
    }
    let v = Command::new(&env.node).arg("--version").output();
    let found = match v {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => {
            return Err(format!(
                "node not runnable as {:?} — the MATCH oracle is unavailable, so MATCH entries \
                 cannot be checked. This is a failure, not a skip.",
                env.node
            ));
        }
    };
    if found != pin {
        return Err(format!(
            "node version mismatch: manifest pins {pin}, found {found}. A differential against \
             whatever node happens to be installed is not a differential. Update the pin \
             deliberately, with a re-baseline."
        ));
    }
    Ok(())
}
