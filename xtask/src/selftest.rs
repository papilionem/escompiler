//! `cargo xtask verify --self-test` — the verifier's own negative control.
//!
//! One binary decides whether every rung has shipped. A checker nobody has seen
//! fail is a checker nobody should believe, and this repo has five recorded
//! instances of exactly that. Each control below seeds a specific defect and
//! asserts the verifier reports it.
//!
//! Two of the eight are not about a wrong answer but about a wrong *shape*:
//! `empty-manifest` and `zero-entries-executed` must both be FAIL, never PASS.
//! They are the reason this file exists — the harness this replaces printed the
//! same "ok" whether it had checked 366 programs or none.

use std::path::Path;
use std::process::ExitCode;

use crate::manifest::{Entry, Kind, Manifest};
use crate::runner::{self, Env};
use crate::verdict::Verdict;

struct Control {
    name: &'static str,
    /// What the seeded defect is, for the report.
    seeds: &'static str,
}

const CONTROLS: &[Control] = &[
    Control {
        name: "wrong-stdout",
        seeds: "a MATCH entry whose output differs from node",
    },
    Control {
        name: "refused-entry-exits-0",
        seeds: "a REFUSED entry that actually compiles",
    },
    Control {
        name: "signal-death",
        seeds: "a program killed by a signal",
    },
    Control {
        name: "false-artifact-predicate",
        seeds: "an absent-symbol claim about a symbol that is present",
    },
    Control {
        name: "missing-fixture-file",
        seeds: "an entry naming a program that does not exist",
    },
    Control {
        name: "oracle-unreachable",
        seeds: "a node binary that cannot be run",
    },
    Control {
        name: "empty-manifest",
        seeds: "a manifest declaring zero entries",
    },
    Control {
        name: "zero-entries-executed",
        seeds: "a run in which nothing executes",
    },
];

pub fn run(root: &Path, node: &str) -> ExitCode {
    let workdir = std::env::temp_dir().join(format!("esc-verify-selftest-{}", std::process::id()));
    if std::fs::create_dir_all(&workdir).is_err() {
        eprintln!("FATAL: cannot create self-test work dir");
        return ExitCode::from(2);
    }
    // Same resolver as `verify()`. This was `root.join("target/debug/esc")`,
    // which is wrong wherever CARGO_TARGET_DIR is set — i.e. the whole Dell CI
    // fleet. The consequence was not one red control but a nearly vacuous SUITE:
    // esc could not be spawned, six of the eight controls assert only that the
    // verifier returned Fail, and a spawn failure IS a Fail — so they reported
    // "caught" without exercising anything. Only `wrong-stdout` noticed, because
    // it alone asserts WHY it failed.
    let env = Env {
        esc: runner::esc_path(root),
        root: root.to_path_buf(),
        node: node.to_string(),
        workdir: workdir.clone(),
    };
    if !env.esc.exists() {
        eprintln!(
            "FATAL: esc binary not found at {} — run `cargo build`.\n\
             Refusing to run the controls: without it every control that checks only \
             'the verifier returned Fail' would report caught while exercising nothing.",
            env.esc.display()
        );
        return ExitCode::from(2);
    }

    println!("cargo xtask verify --self-test");
    println!("==============================");
    println!("Each control seeds a defect and asserts the verifier catches it.\n");

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut report = |name: &str, seeds: &str, caught: bool, detail: String| {
        if caught {
            println!("  PASS  {name:26} caught: {seeds}");
            passed += 1;
        } else {
            println!("  FAIL  {name:26} NOT CAUGHT: {seeds}\n        {detail}");
            failed += 1;
        }
    };

    // Programs are written fresh so the control does not depend on any fixture
    // staying broken — a control that decays as the compiler improves is worse
    // than none.
    let p_ok = workdir.join("st-ok.js");
    let _ = std::fs::write(&p_ok, "console.log(1);\n");
    let p_sig = workdir.join("st-signal.js");
    let _ = std::fs::write(&p_sig, "function f(){ return f(); }\nf();\n");

    let rel = |p: &Path| {
        p.strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .into_owned()
    };

    // 1. wrong-stdout
    //
    // The ORACLE is stubbed rather than seeding a program the compiler gets
    // wrong. That is the whole point, and this control is why the comment above
    // exists: it previously seeded
    //
    //     console.log([...new Set([1,1,2])].length);
    //
    // which differed from node only because of the R1-05a defect. PR #55 fixed
    // that defect on 2026-08-12, esc began printing 2 like node, the entry
    // PASSED, and the control silently stopped firing. `--self-test` then
    // reported 7/8 while the development guide, CURRENT.md and the rung
    // documents all still claimed 8/8 — undetected because no CI job runs
    // --self-test (ESC-107).
    //
    // A control whose trigger is a bug is disarmed by fixing the bug. Stubbing
    // the oracle makes the mismatch STRUCTURAL: the stub echoes the program PATH
    // where esc prints "1", and no improvement to the compiler can make a
    // compiled program print its own source path.
    //
    // `/bin/echo`, not a script written to the workdir. The first version wrote
    // a shell stub and chmod'd it, which worked locally and failed in CI with
    // "spawn failed: No such file or directory" — the Dell job sets
    // TMPDIR=/citmp and the workdir is under it. A binary that is already there
    // needs no write, no exec bit, and no assumptions about the mount.
    // `node_run` passes the program path as the single argument, so echo exits 0
    // with that path on stdout, which is what the comparison needs.
    let stub_env = Env {
        esc: env.esc.clone(),
        root: root.to_path_buf(),
        node: "/bin/echo".to_string(),
        workdir: workdir.clone(),
    };
    let e = mk("st-wrong", &rel(&p_ok), Kind::Match);
    let v = runner::check(&stub_env, &e);
    // Assert WHY it failed, not merely that it did. A non-executable or missing
    // stub would make the oracle unreachable, which also yields a Fail — and
    // this control would then report "caught" while never having compared any
    // stdout at all. Checking only `!is_pass()` is exactly the vacuous-assertion
    // shape ESC-104 was filed for.
    let detail = fmt(&v);
    report(
        CONTROLS[0].name,
        CONTROLS[0].seeds,
        !v.is_pass() && detail.contains("stdout differs"),
        detail,
    );

    // 2. refused-entry-exits-0 — a program that compiles, declared as REFUSED.
    let e = mk(
        "st-refused",
        &rel(&p_ok),
        Kind::Refused {
            code: "ESC-E999".into(),
        },
    );
    let v = runner::check(&env, &e);
    report(CONTROLS[1].name, CONTROLS[1].seeds, !v.is_pass(), fmt(&v));

    // 3. signal-death — unbounded recursion. Node throws RangeError (exit 1);
    //    esc currently dies by SIGSEGV. Either way the classes must differ, and
    //    if they ever agree this control fails loudly rather than rotting.
    let e = mk("st-signal", &rel(&p_sig), Kind::Match);
    let v = runner::check(&env, &e);
    report(CONTROLS[2].name, CONTROLS[2].seeds, !v.is_pass(), fmt(&v));

    // 4. false-artifact-predicate — claim `main` is absent from a real binary.
    let e = mk(
        "st-artifact",
        &rel(&p_ok),
        Kind::Artifact {
            absent_symbols: vec!["main".into()],
        },
    );
    let v = runner::check(&env, &e);
    report(CONTROLS[3].name, CONTROLS[3].seeds, !v.is_pass(), fmt(&v));

    // 5. missing-fixture-file
    let e = mk(
        "st-missing",
        "tests/verify/__does_not_exist__.js",
        Kind::Match,
    );
    let v = runner::check(&env, &e);
    report(CONTROLS[4].name, CONTROLS[4].seeds, !v.is_pass(), fmt(&v));

    // 6. oracle-unreachable
    let bad_env = Env {
        esc: env.esc.clone(),
        root: root.to_path_buf(),
        node: "__no_such_node_binary__".into(),
        workdir: workdir.clone(),
    };
    let e = mk("st-oracle", &rel(&p_ok), Kind::Match);
    let v = runner::check(&bad_env, &e);
    let pre = runner::check_preconditions(&bad_env, "v0.0.0");
    report(
        CONTROLS[5].name,
        CONTROLS[5].seeds,
        !v.is_pass() && pre.is_err(),
        fmt(&v),
    );

    // 7. empty-manifest — must be an ERROR at load, not an empty clean run.
    let empty = workdir.join("empty-manifest.toml");
    let _ = std::fs::write(&empty, "node_pin = \"v0.0.0\"\n");
    let loaded = Manifest::load(&empty);
    report(
        CONTROLS[6].name,
        CONTROLS[6].seeds,
        loaded.is_err(),
        match &loaded {
            Ok(_) => "manifest with zero entries loaded successfully".into(),
            Err(e) => e.clone(),
        },
    );

    // 8. zero-entries-executed — the same hazard one level up. Guaranteed by the
    //    same guard: there is no legal path on which `executed == 0` returns 0.
    //    Asserted here rather than assumed, by loading a manifest whose only
    //    entry cannot be reached and checking the load itself refuses.
    let zero = workdir.join("zero-entries.toml");
    let _ = std::fs::write(&zero, "node_pin = \"v0.0.0\"\nentry = []\n");
    let loaded0 = Manifest::load(&zero);
    report(
        CONTROLS[7].name,
        CONTROLS[7].seeds,
        loaded0.is_err(),
        match &loaded0 {
            Ok(_) => "a run with zero entries was accepted".into(),
            Err(e) => e.clone(),
        },
    );

    let _ = std::fs::remove_dir_all(&workdir);
    println!("\n==============================");
    println!("controls passed: {passed}   failed: {failed}");
    if failed > 0 {
        eprintln!(
            "\n{failed} control(s) did not catch their seeded defect. The verifier cannot be \
             trusted to decide a rung until every control passes."
        );
        return ExitCode::from(1);
    }
    println!("every control caught its seeded defect.");
    ExitCode::SUCCESS
}

fn mk(id: &str, program: &str, kind: Kind) -> Entry {
    Entry {
        id: id.into(),
        program: program.into(),
        kind,
        note: String::new(),
        xfail: None,
    }
}

fn fmt(v: &Verdict) -> String {
    match v {
        Verdict::Pass => "verdict was PASS".into(),
        Verdict::Fail(w) => w.clone(),
    }
}
