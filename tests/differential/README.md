# Differential Testing

Compares esc output against reference engines (Node.js, V8, SpiderMonkey) to detect semantic divergences.

## How It Works

1. Run a JS file through esc → capture stdout + exit code
2. Run the same file through Node.js → capture stdout + exit code
3. Compare outputs — any difference is a potential bug

## Modes

- **Cranelift vs LLVM**: Same source compiled with both backends, outputs must match
- **heap-only vs production**: Same source with `--heap-only` flag vs normal mode
- **esc vs Node.js**: Cross-engine comparison for JS semantics

## Running

```bash
cargo test --test differential_runner
```

Requires Node.js installed for cross-engine tests.
