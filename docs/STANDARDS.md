# Engineering Standards

The bar viewr holds itself to. These are the practices that let us claim "it just
works" and mean it. They are enforced in CI wherever possible, because a standard
that depends on remembering is not a standard.

The intent is quality, not ceremony. If a rule here ever stops serving the code,
we change the rule in the open rather than quietly ignore it.

## No AI slop

Much of the modern quality problem is "AI slop": code that compiles, passes basic
checks, and looks polished, but is subtly wrong, bloated, or misaligned with what
was actually needed. Industry analysis in 2026 tied AI-heavy code to several times
more duplication and churn. viewr is written with AI assistance, so we guard
against its failure modes explicitly. Reviewers reject, and authors avoid:

- **Over-defensive boilerplate.** No wrapping trivial code in layers of error
  handling "just in case," no custom error types for a one-shot helper, no
  swallowed errors (`let _ = ...` on a fallible call that matters). Handle the
  errors that can actually happen; let genuine invariant violations be loud.
- **Comments that restate the code.** A comment must explain *why*, or a
  non-obvious *what*. `// increment i` is deleted on sight. Doc comments on public
  API are required (missing_docs), but they must add information, not paraphrase
  the signature.
- **Speculative structure.** No abstraction, trait, config knob, or "manager"
  layer added for a future that has not arrived. Delete helpers and indirection
  until only the smallest correct version remains. YAGNI is enforced.
- **Duplicated patterns.** If the same shape appears three times, it becomes one
  well-named function. Copy-paste-tweak is the single strongest slop signal.
- **Generic naming and oversized functions.** Names say what the thing is for, not
  its type. Functions do one thing; if a function needs a paragraph to explain,
  it needs to be split.
- **Tests that mirror the implementation.** A test asserts observable behavior and
  would fail if the behavior regressed. A test that just re-encodes the code's
  steps (or over-mocks until it tests nothing) is worse than no test, because it
  gives false confidence. This is why we run mutation testing.

The bar: the code should read as though one careful engineer wrote the whole thing
with intent, not as though it was assembled from plausible fragments.

## Toolchain

- Pin the toolchain with `rust-toolchain.toml` so every contributor and CI runner
  builds with the same compiler. No "works on my machine."
- Declare and test a Minimum Supported Rust Version (MSRV). Bumping it is a
  deliberate, noted change, not an accident.
- Commit `Cargo.lock`. Builds are reproducible from a known dependency set.

## Formatting and linting (gated in CI)

- `cargo fmt --check`: one canonical style, no debates, no diffs about whitespace.
- `cargo clippy` at the `pedantic` level with warnings treated as errors
  (`-D warnings`). Clippy ships 550+ lints; we opt into the strict set and allow
  specific lints explicitly in `clippy.toml` with a comment saying why. Turning
  stylistic advice into a build failure is the point.
- No warnings of any kind in a merged build. A warning is a bug that has not
  happened yet.

## Safety

- `#![forbid(unsafe_code)]` at the crate root by default. Any crate that genuinely
  needs `unsafe` (likely only a thin platform or GPU boundary) must isolate it in a
  small, separately reviewed module and document every invariant in a `# Safety`
  section, per the Rust API Guidelines.
- Prefer safe, pure-Rust dependencies, especially on the untrusted-input path
  (image decoding). Where a format only exists as C, it is decoded inside the
  sandboxed worker rather than linked into the main process.

## Error handling

- `thiserror` for typed, meaningful errors in library-style crates.
- `anyhow` (or equivalent) only at the application boundary where a human-readable
  chain is what matters.
- No `unwrap`, `expect`, or `panic!` on any path that can be reached by user input
  or a hostile file. Panicking is reserved for genuine invariant violations, and
  those are rare and documented. A viewer must never crash because a file was odd.

## Testing

Target: 85 percent line coverage or higher on the testable logic, measured with
`cargo-llvm-cov` and enforced in CI. Display and IPC glue that cannot be exercised
honestly without a window or an external worker binary is excluded and covered by
end-to-end verification instead:

- `app.rs`, `gpu.rs`, `ui.rs` — windowing, GPU, and egui chrome
- `sandbox.rs` — `viewr-decode` process pool and bounded pixel-stream IPC
- `worker_limit.rs` — OS Job Object / process-group glue
- `error.rs`, `main.rs` — thin entry/error surfaces

Everything else (decode pure paths, edit, fs ordering, view math, theme) is in
the coverage floor. Coverage is a floor and a signal, not the goal. The real goal
is that the tests would catch a regression, which is why we also run mutation
testing.

- `cargo nextest` as the test runner for speed and clear output.
- Unit tests next to the code they cover.
- Integration tests for the real flows: open, navigate, delete-and-undo, crop,
  convert, strip metadata.
- Property tests (`proptest`) on the pure logic: transform math, natural-sort
  ordering, index preservation after delete, cache eviction.
- Golden-file/snapshot tests (`insta`) for decode output on a corpus of known-good
  images across every supported format, so a decoder regression is caught
  immediately.
- Mutation testing (`cargo-mutants`) run regularly. It changes the code and checks
  that a test fails; surviving mutants reveal tests that cover a line without
  actually asserting its behavior. This is how we keep the 80 percent honest.

## The untrusted-input path gets extra rigor

Decoding is where a photo viewer historically gets exploited, so it is held to a
higher standard than the rest of the app.

- Continuous fuzzing (`cargo-fuzz` with the `arbitrary` crate) against every
  decoder, seeded with malformed and adversarial samples. A crash found by the
  fuzzer is a release blocker.
- The operational pure-Rust decoder and worker-protocol targets live in `fuzz/`.
  They compile with a pinned nightly and cargo-fuzz release, run briefly on
  relevant changes, and run for at least 600 seconds per target on the weekly
  schedule. `fuzz/README.md` is the executable local contract.
- A regression corpus: every file that ever caused a crash or hang becomes a
  permanent test case.
- Decode runs in the restricted worker described in ARCHITECTURE.md, so even a
  decoder bug has nothing to reach.

## Supply chain

- `cargo-deny` in CI enforces three things: the allowed license set, a ban on
  duplicate or yanked crates, and the security advisory database.
- `cargo-audit` against the RUSTSEC advisory database on every build and on a
  schedule, so a newly disclosed vulnerability in a dependency is noticed quickly.
- The privacy invariant is a `cargo-deny` ban list: no crate capable of opening a
  network connection may enter the dependency tree. The build fails if one does.
  This is what makes "it cannot phone home" a checked fact rather than a claim.
- Every new dependency is a deliberate decision. Fewer, well-chosen crates over
  many convenient ones. Each crate is code we ship, audit, and are responsible for.

## Dependency policy

The dependency list is a liability to be minimized, not an asset to be grown. Two
rules keep it honest.

- **Trusted core versus optional formats.** Dependencies split into a small trusted
  core that is always present (the windowing, GPU, text, and common pure-Rust
  decoders) and optional format decoders behind Cargo feature flags. The default
  build carries only the lean, pure-Rust core. Formats that need heavy or C-backed
  decoders (HEIC, RAW, and other exotic types) are opt-in features, and when enabled
  they run in the sandboxed decode worker. This lets "minimal dependencies" hold for
  the always-on core while "support every format" is available to anyone who opts
  in, without either promise being a lie. The default-feature dependency tree has a
  budget, and adding to it is a deliberate, reviewed decision.
- **Latest GA, nothing old, nothing pre-release.** We track the latest generally
  available release of each dependency and keep current. We do not ship on beta,
  alpha, or release-candidate versions in the foundation. Where a crate's newest
  publish is a pre-release, we pin to its latest GA line instead (for example,
  windowing stays on the stable winit 0.30 line while 0.31 remains beta). Staying
  current is enforced by review and by `cargo-audit`/`cargo-deny` flagging stale or
  vulnerable versions.

## Documentation

- `#![warn(missing_docs)]` on public items. Public API without docs does not merge.
- Doc examples compile and run as tests (`cargo test --doc`), so the docs cannot
  drift out of sync with the code.
- Every non-obvious decision is recorded where it lives: architectural choices in
  ARCHITECTURE.md, technology choices as decision records in STACK.md.

## Performance is a tested property, not a hope

- Track cold-start time, first-pixel latency, and memory on a fixed image corpus.
- Benchmarks (`criterion`) on the hot paths: decode, GPU upload, navigation.
- CI flags a regression beyond a set threshold. "It got slow" should fail a check,
  not a user's patience.

## Releases

- Semantic versioning, with `cargo-semver-checks` guarding public API changes.
- Reproducible, signed builds so a user can verify the binary matches the source.
- A human-written changelog. People deserve to know what changed and why.

## A note on humility

These standards describe what we aim for, held consistently. They do not make the
software perfect, and we will get things wrong. The value is in the discipline:
tests that fail loudly, a privacy guarantee the build enforces, and a small enough
surface that we can actually understand what we shipped. When reality and a
standard here conflict, we fix it honestly and in the open.
