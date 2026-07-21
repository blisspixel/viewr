# Fuzzing viewr decoders

Continuous fuzz of the decode boundary is a Phase 7 goal. The adversarial corpus
tests in `crates/viewr/tests/corpus.rs` already ensure truncated/garbage inputs
do not panic in CI without a fuzzer toolchain.

## Optional: cargo-fuzz

```bash
cargo install cargo-fuzz
# from repo root, after adding a fuzz crate (not a workspace member by default):
# cargo fuzz run decode_memory
```

Targets should call `DecodedImage::load_from_memory` / path loaders only — never
network, never write outside a temp workspace that is deleted on drop.

Fuzz artifacts must not be committed; they may contain untrusted bytes.
