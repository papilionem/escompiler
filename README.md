# ESCompiler

<!-- esc:version 0.9.0-dev -->
<!-- The version above is checked in CI against Cargo.toml and every other place
     this project states its version. Change it there too, or the check fails. -->

A JavaScript/TypeScript to native code ahead-of-time (AOT) compiler.

Compiles JavaScript source into standalone native binaries with no VM, no interpreter and no tracing garbage collector. Written in Rust, powered by Cranelift (debug) with an LLVM backend in repair.

> **Status:** Pre-release (`v0.9.0-dev`), **not usable for real programs yet.** The compiler is real: a non-trivial JS program (classes, closures, generators, Map, JSON, try/catch) compiles to a working native binary on Linux x86-64. Arithmetic was unsound in every compiled program until 2026-08-12 and is now correct, verified against pinned Node. What still blocks real use is that **multi-file programs do not work**, **every property access leaks memory**, and **uncaught exceptions print nothing**. Read [Known broken](#known-broken-or-missing) before you try it. This README is deliberately honest about what works and what doesn't; where it was previously wrong, the correction is dated rather than deleted.

## Features

- **True AOT compilation.** JavaScript compiles to native machine code, producing standalone executables
- **No virtual machine.** no interpreter loop, no bytecode, no JIT warmup. Startup is 2.08 ms (node 20.81 ms, deno 30.65 ms)
- **Deterministic memory (partly shipped).** the design is zone allocation plus per-object reference counting with a cycle collector, no tracing GC and no pauses. **What runs today is heap allocation + the cycle collector; the zone half has never executed and the reference-counting entry points are stubs.** Corrected 2026-08-11; this previously read as though both halves shipped.
- **Compiles real JavaScript.** sloppy mode, `with`, Proxy, generators, async generators; not a subset language
- **General purpose.** not tied to any framework, runtime, or use case

## Support & Limits

Honesty policy: a platform or feature is only claimed at the level a harness proves (builds / runs / verified-correct).

### Platforms

| Platform | Compiler builds | Output runs | Differential-verified |
|----------|:---:|:---:|:---:|
| Linux x86-64 | ✅ | ✅ | ⏳ harness in progress |
| macOS arm64 | ✅ (weekly CI) | ❌ not yet exercised | ❌ |
| Windows x86-64 | ✅ (weekly CI) | ❌ linker is GCC-only today | ❌ |
| Linux arm64, WASM, mobile | ❌ planned | ❌ | ❌ |

### Works today (verified)

**Read the next section first.** The features below compile and run, but the defects listed there
listed there is present underneath all of them.

Primitives, operators, coercion · `var`/`let`/`const` with TDZ · functions, arrows, default/rest params · closures (capture by reference) · classes with inheritance, static members, class fields · destructuring · template literals · iterators, `for-of`/`for-in` · generators (state-machine transform) and async generators · `with` (incl. `Symbol.unscopables`) · all 13 Proxy traps · Map/Set/WeakMap/WeakSet/WeakRef · JSON round-trip · RegExp · Symbol · property descriptors · exceptions with correct error types · indirect `eval` via self-hosted Cranelift JIT · `esc run` and `esc build`.

Also verified and worth stating plainly: **array bounds behaviour is spec-correct**, including negative and sparse indices; the **regex engine is non-backtracking**, so classic ReDoS shapes do not blow up; and **startup is 2.08 ms** against node's 20.81 ms.

### Known broken or missing

Verified by hand on 2026-08-11 at commit `f782631`. The measurement notes behind each row live in the development repository rather than here; every claim below is reproducible from this tree with the commands in the sections that follow. Earlier versions of this list omitted everything in the first group, which was the most misleading thing in this document.

**Answers that were wrong. These were correctness defects, not missing features:**

> **Corrected 2026-08-19.** This table listed nine wrong answers. Eight of them were fixed on
> 2026-08-12 and this README did not say so for a week, which understated the compiler. Each fix
> is now held by an entry in the verification corpus that runs against pinned Node v24.14.0 on
> every pull request, so a regression turns the build red rather than quietly reappearing here.

| Program | ECMAScript says | ESCompiler produces | |
|---|---|---|---|
| `let a = 7, b = 2; a / b` | `3.5` | `3.5` | fixed 2026-08-12 |
| `1 / 2 === 0.5` | `true` | `true` | fixed 2026-08-12 |
| `(1 / 2).toFixed(2)` | `"0.50"` | `"0.50"` | fixed 2026-08-12 |
| `let a = 2e9; a + a` | `4000000000` | `4000000000` | fixed 2026-08-12 |
| `1e5 * 1e5` | `10000000000` | `10000000000` | fixed 2026-08-12 |
| `1 / 0` | `Infinity` | `Infinity` | fixed 2026-08-12 |
| `9 % 0` | `NaN` | `NaN` | fixed 2026-08-12 |
| `null == undefined` | `true` | `true` | fixed 2026-08-12 |
| unbounded recursion | `RangeError` | **SIGSEGV (crash)** | still broken |

The eight fixes had one root cause: every integral literal was lowered to a 32-bit integer, but
JavaScript Numbers are IEEE-754 doubles and do not wrap at 32 bits. Deleting that specialization
fixed the arithmetic and the two signal deaths together, because the crashes came from integer
division rather than from floating-point division, which does not trap.

Unbounded recursion still reaches a segfault instead of a `RangeError`. There is no stack-depth
check yet.

**Resource and reliability:**

- **Every property access leaks about 62 bytes.** Three million `o.a` reads reach 193 MB RSS where node reaches 53 MB. Property names are string literals, so this is the hottest path in the language, so a server or a file-processing CLI will exhaust memory.
- **Uncaught exceptions exit with zero bytes on stderr.** no message, no stack.
- Throughput is currently **~40–130× slower than node** on arithmetic, property access and object allocation. Startup is the one performance number that is genuinely good.

**Missing or non-functional:**

- **Multi-file programs do not work.** `require("./x.js")` runs and prints nothing; `import` fails to link. Single-file only.
- `super(x)` drops its arguments · `process.argv` is dropped · function declarations are not hoisted · an assignment used as a concise arrow body is silently discarded
- **TypeScript entry files do not parse yet.** the detection exists but is not wired into the driver
- **`--release` (LLVM backend) miscompiles** loops, exceptions, closures, and classes. LLVM is an optional, non-default build feature, so `--release` reports `ESC-E601` rather than emitting a bad binary. Use debug builds.
- **Promise/async ordering is wrong**: `.then()` fails and `await` resumes synchronously (job queue in progress); `await` works for simple cases only
- **`console` is a compile-time special case.** aliasing it (`const c = console`) silently does nothing
- **No binary data**: ArrayBuffer/TypedArrays/DataView not implemented; **no BigInt** (literals are refused at compile time with `ESC-E300` since 2026-08-13); no Intl, no setTimeout
- Direct `eval` does not see the caller scope
- **The `--allow-*` and `--no-jit` permission flags are no-ops** — they produce a byte-identical binary to the default build. Only `--no-eval` has an effect. Do not rely on them as a security boundary.

> **Corrected 2026-08-11:** this list previously led with "`esc run` is broken". `esc run` works.
> The entries it *omitted*, namely arithmetic, the leak, multi-file programs, `null == undefined`,
> `super(x)`, spread over iterables and the no-op permission flags, mattered far more.

### Conformance (measured)

- test262, pinned tc39 commit `93d63969`, measured **2026-08-20**: **11,315 files passing**. Against the **full 50,506-file suite that is 22.4%**.
- *This figure was wrong here for nine days.* It read 11,065 from a 2026-08-11 measurement while the nightly full-suite run had been reporting 11,315 — the README understating the compiler by 250 files. The measurement existed the whole time and nothing carried it into this document. It is still transcribed by hand today; generating it, with a staleness check so a stopped nightly cannot leave a figure that looks fresh, is tracked as ESC-130.
- **On the denominator:** CI tracks 113 categories totalling 23,297 files, and a percentage computed against *that* number reads ~47.5%. That is not the suite. This README quotes the full-suite denominator; if you see a figure near 47%, it is measured against under half the tests.
- The nightly full-suite threshold was re-baselined from an unsatisfiable 12,600 down to the measured 11,065 on 2026-08-12, recorded as ADR-0001. A threshold above the measurement is not a weak gate, it is one that can never pass. **It still sits at 11,065 against today's 11,315**, so it currently cannot detect a 250-file regression; raising it is a deliberate ratchet decision, not a silent edit.
- Caveat, unchanged: the runner still under-tests. Async tests are skipped and some negative-test checks are weak. Expect further downward correction as the oracle is repaired. That is by design.
- Workspace: **4,807** unit/integration tests, 0 failures, 24 ignored (measured 2026-08-20 with `--all-features`) · clippy `-D warnings` clean · `cargo fmt` clean

> **Corrected 2026-08-11:** this section previously claimed 12,659 / 23,297 (~47.5%) and 4,840 tests.
> Both were re-measured by hand and neither reproduced.

## Quick Start

### Prerequisites

- Rust toolchain (1.85+, edition 2024)
- A C linker (`gcc`/`clang`)

That is all. **LLVM is not required.** It is an optional, non-default cargo feature that only the
(currently miscompiling) `--release` backend uses; building and using the supported Cranelift path
needs no LLVM at all.

> **Corrected 2026-08-11:** this section previously listed "LLVM 18 development libraries (currently
> required to build the workspace even for debug use)" with install instructions. That was stale and
> sent contributors to install a dependency they do not need.

### Build & use

```bash
git clone https://github.com/papilionem/escompiler.git
cd escompiler
cargo build --release

# Compile a JavaScript file to a native binary (debug backend, the supported path)
esc build hello.js -o hello
./hello

# Compile and run in one step
esc run hello.js

# Emit intermediate representations
esc build app.js --emit ir
```

Single-file programs only. `require` and `import` do not work yet.

### Example

```javascript
// hello.js
function fibonacci(n) {
  if (n <= 1) return n;
  return fibonacci(n - 1) + fibonacci(n - 2);
}
console.log(fibonacci(35));
```

```bash
$ esc build hello.js -o hello
$ ./hello
9227465
```

## Architecture

```
Source (.js)
    |
  Parser (oxc)
    |
  AST -> IR Lowering (desugar, generator transform)
    |
  SSA Intermediate Representation (197 opcodes, 7-pass verifier)
    |
    +---> Type Inference (local; whole-program inference in development)
    |
  Backend
    +---> Cranelift  (debug, the supported backend)
    +---> LLVM       (release, optional feature, under repair, gated)
    |
  Native Binary (linked with the runtime static library)

  Not in this pipeline yet (implemented as crates, not called by the driver):
    Escape Analysis · Memory Strategy Assignment · Modules · TypeScript entry
```

> **Corrected 2026-08-11:** Escape Analysis and Memory Strategy Assignment were previously drawn as
> pipeline stages. They are not. `crates/memory` has zero reverse dependencies and the lowering
> stage emits no zone-allocation opcodes at all.

**Memory model, as designed:** two allocation worlds: zone allocation (bump-pointer, bulk-free) for short-lived objects, and heap allocation (per-object reference counting with a Bacon–Rajan cycle collector) for escaping objects, with escape analysis deciding automatically and no annotations.

**Memory model, as built:** every object is heap-allocated. The Bacon–Rajan cycle collector is genuinely wired and running; the reference-counting entry points around it are stubs, which is why property-name strings are never freed (see [Known broken](#known-broken-or-missing)). The zone path has never executed. This is a gap between design and driver wiring, not evidence that the design is wrong, but nothing here should be described as shipped until it runs.

## Project Structure

A Rust workspace of 30 crates:

| Layer | Crates | Purpose |
|-------|--------|---------|
| Foundation | `common`, `arena`, `interner`, `nanbox`, `zone`, `strings`, `cycles` | Shared types, allocators, value representation |
| IR | `ir` | SSA intermediate representation (197 opcodes) |
| Frontend | `parser`, `desugar`, `types`, `escape`, `shapes`, `memory`, `generator_transform`, `modules` | Parsing, lowering, analysis (`escape`, `memory` and `modules` are not yet reachable from the driver) |
| Backends | `cranelift`, `llvm`, `linker` | Code generation and linking |
| Runtime | `runtime`, `stdlib`, `exceptions`, `eval`, `regexp`, `host` | Object model, builtins, host layer |
| Tooling | `driver`, `cli`, `diagnostics`, `repl`, `test262` | Pipeline, CLI, conformance harness |

> **Corrected 2026-08-11:** the count read 33 and the table listed `rc`, `ffi` and `ffi_macros`.
> `rc` and `ffi_macros` no longer exist; the workspace is 30 crates.

## Building & testing

```bash
cargo build            # build (no LLVM required)
cargo test             # 4,797 tests, 0 failures, 25 ignored
cargo clippy           # 0 warnings policy
cargo fmt --check
```

## Roadmap

Work is organized as a ladder of five versions, each with a single thesis and exit criteria
written as commands that can fail:

| Version | Thesis |
|---|---|
| v0.9 | Exit 0 means it worked. A zero exit status means the program ran and matched Node; a non-zero one comes with a reason on stderr |
| v0.10 | When it fails, it says so. Every failure names itself, and the repo can state what is actually reachable |
| v0.11 | The artifact is an executable. The produced binary behaves like a Unix program |
| v0.12 | More than one file, more than one dialect. Modules work and TypeScript entry files compile |
| v0.13 | Memory returns. Allocation is reclaimed, and the claim is measured rather than asserted |

The immediate queue is the rest of v0.9: **fix the property-name leak**, **add the missing
arithmetic categories to the per-PR conformance gate**, and **refuse honestly** the things that
currently fail silently. Arithmetic soundness came off this list on 2026-08-12.

v0.13 is not v1.0. Size and performance are not yet planned, and neither is the capability layer
that would let a compiled program read a file or open a socket. Version history: see the
[CHANGELOG](CHANGELOG.md).

## Contributing

Contributions are welcome. `main` is protected: all changes go through pull requests with required CI checks (build, tests, clippy, fmt, test262 quick gate). Please open an issue first to discuss what you'd like to change.

## License

[MIT](LICENSE)
