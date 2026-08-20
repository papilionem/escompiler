# test262 — ECMAScript Conformance Tests

## Setup

The test262 test suite is **optional** — all other tests pass without it.

To clone the official test262 suite into this directory:

```bash
cd tests/test262
git clone https://github.com/nicolo-ribaudo/tc39-test262-parser-tests.git test262
# OR for the full official suite:
git clone https://github.com/nicolo-ribaudo/tc39-test262.git test262
```

The runner expects the data at `tests/test262/test262/test/`.

## Running

```bash
# Run the test262 harness unit tests
cargo test -p test262

# Run the full test262 suite (skips gracefully if not cloned)
cargo test -p test262 --test integration
```

## Directory Layout

```
tests/test262/
├── README.md          # This file
├── harness.rs         # Standalone frontmatter parser (legacy, see test262 crate)
├── parser-tests/      # Git submodule (parser-specific tests)
└── test262/           # Git clone of full test262 suite (optional)
    ├── test/          # The actual test files
    └── harness/       # Official harness files (the ONLY harness — ESC-27 removed the local simplified copy)
```

## Harness Crate

The test262 infrastructure lives in `crates/test262/`:

- `src/harness.rs` — YAML frontmatter parser, feature support list
- `src/runner.rs` — test discovery, compilation, execution, reporting
- `tests/integration.rs` — CI regression test wrapper
