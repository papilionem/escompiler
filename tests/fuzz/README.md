# Fuzz Testing

Fuzz testing infrastructure for escompiler using `cargo-fuzz` / `libfuzzer`.

## Setup

```bash
cargo install cargo-fuzz
```

## Fuzz Targets (to be added)

- `fuzz_parser` — feed random bytes to the parser, ensure no panics
- `fuzz_ir_builder` — generate random IR instruction sequences, verify they don't crash the verifier
- `fuzz_nanbox` — random bit patterns through NaN-box encode/decode cycles

## Running

```bash
cargo fuzz run fuzz_parser -- -max_total_time=300
```
