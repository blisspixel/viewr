# Coverage-guided fuzzing

The fuzz workspace exercises viewr's two untrusted byte boundaries with
`cargo-fuzz` and LLVM libFuzzer. It is intentionally separate from the stable
root workspace because sanitizer instrumentation requires nightly Rust.

## Reproducible setup

Run these commands from the repository root:

```bash
rustup toolchain install nightly-2026-07-20 --profile minimal --component rustfmt --component clippy
cargo install cargo-fuzz --locked --version 0.13.2
cargo +nightly-2026-07-20 fuzz build decode_memory
cargo +nightly-2026-07-20 fuzz build protocol_frames
```

The exact nightly and cargo-fuzz version match `.github/workflows/fuzz.yml`.
`fuzz/Cargo.lock` pins the target dependencies.

`cargo-fuzz` 0.13.2 officially executes libFuzzer only on x86-64 and AArch64
Unix-like hosts. Windows can compile both targets, but its sanitizer executable
is not supported; the Ubuntu CI run is authoritative for coverage-guided
execution. Windows contributors still exercise the same entry points through
the stable unit and adversarial corpus tests.

## Targets

- `decode_memory` selects from the same `CORE_EXTENSIONS` list as folder
  navigation, invokes each enabled pure-Rust decoder with explicit format
  dispatch, also exercises signature-based dispatch, and checks the shared
  decoded-shape invariant after every success.
- `protocol_frames` exercises hostile and generated request, response, shape,
  and acknowledgement frames without spawning a worker or touching the file
  system.

The initial corpus contains one explicit malformed selector seed for every core
extension, a successful synthetic sample for every distinct core decoder, and
protocol boundary cases. `cargo test` verifies that every successful seed still
decodes. Regenerate every successful image seed, including the embedded
2-by-2 lossless JPEG XL codestream, with
`cargo run --example gen_fuzz_seeds`. New crash or timeout inputs are release
blockers. Minimize a finding, add it to the appropriate committed corpus and
stable regression test, fix the root cause, then rerun both the stable suite
and the affected fuzzer.

## Local runs

```bash
cargo +nightly-2026-07-20 fuzz run decode_memory -- -max_len=1048576 -timeout=30
cargo +nightly-2026-07-20 fuzz run protocol_frames -- -max_len=1048592 -timeout=30
```

Pull requests and pushes receive a short smoke run. The scheduled workflow runs
each target for at least 600 seconds and uploads crash artifacts on failure.
The harness intentionally contains no network or file APIs. Its only expected
runtime writes come from libFuzzer under `fuzz/corpus`, `fuzz/artifacts`, and
Cargo's ignored target directory. Crash artifacts remain gitignored because
they contain untrusted bytes; the CI runner itself is not a security sandbox.

Optional C-backed AVIF and HEIC worker features require native system libraries
and are not part of this pure-Rust fuzz workspace. They remain a separate
production-hardening requirement before those features can ship enabled.
