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
- Repository-owned Python release tooling is gated by a version- and wheel-hash-
  pinned Ruff check/format pass, cross-platform unit tests, and an 85 percent line
  coverage floor. Python bytecode caches remain ignored build debris.

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
- `sandbox.rs`: `viewr-decode` process pool and bounded input/pixel-stream IPC
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
- External native accessibility tests must exercise the platform provider and
  action path, not only inspect an in-process semantic tree. Windows CI runs the
  dependency-free UI Automation smoke script. Manual Narrator, VoiceOver, and
  Orca acceptance remains a distinct release gate in `docs/ACCESSIBILITY.md`.
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
- Optional C-backed decode runs in the restricted worker described in
  ARCHITECTURE.md. The parent sends bytes rather than a path, limiting what a
  decoder bug can reach beyond its bounded request and response pipes. Linux C
  builds must pass release-mode AVIF and HEIC protocol decodes under the shared
  default-deny syscall policy; adding an allowed syscall requires code, runtime
  evidence, and documentation in `packaging/linux/SECCOMP.md`.
- Whole-app package profiles use exact reviewed permission sets. Tests fail if a
  Flatpak grant, macOS entitlement, or Windows AppContainer capability appears
  without an explicit review. Native CI also performs an offline Flatpak build
  and worker probe, verifies and probes an ad-hoc signed macOS bundle, and
  schema-validates an unsigned MSIX.
- Sandboxed navigation must use explicit file or folder picker consent. A denied
  containing-folder scan degrades to the selected image without broadening the
  package permission set or silently breaking the open operation.

## Supply chain

- `cargo-deny` in CI enforces three things: the allowed license set, a ban on
  duplicate or yanked crates, and the security advisory database.
- `cargo-audit` against the RUSTSEC advisory database on every build and on a
  schedule, so a newly disclosed vulnerability in a dependency is noticed quickly.
- The privacy invariant is layered and deterministic. `cargo-deny` rejects HTTP,
  TLS, websocket, QUIC, and remote-service clients, and permits Linux's generic
  D-Bus implementation only behind AccessKit/AT-SPI. Linux startup accepts only
  Unix D-Bus environment transports, installs `no_new_privs`, denies non-Unix
  socket creation and io_uring before application threads, covers x32 syscall
  aliases on x86-64, verifies `EPERM`, and fails closed. Package profiles add an
  independent whole-application boundary.
  Together these controls make "it cannot phone home" checked rather than claimed.
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
- Exercise the real release GUI through a black-box probe for window readiness,
  first presentation, navigation, idle redraw behavior, and folder-scaling memory.
- Keep hot-path microbenchmarks dependency-free when a framework would add more
  supply-chain surface than measurement value. Decode remains separately measurable.
- CI flags a regression beyond explicit thresholds. "It got slow" should fail a
  check, not a user's patience. Thresholds are regression limits, not universal
  hardware promises.

## Releases

- Semantic versioning, with `cargo-semver-checks` guarding public API changes.
- Release builds use the exact compiler in `rust-toolchain.toml`, the committed
  lockfile, and `--locked`. The two required executables are target-validated and
  assembled into a deterministic stored ZIP with normalized documentation,
  commit-derived timestamps, an internal per-file SHA-256 manifest, and an
  external SHA-256 sidecar. The verifier rejects extra members, unsafe paths,
  leading or trailing ZIP data, target-label mismatches, non-canonical metadata,
  and checksum drift.
- CI retains these archives as read-only workflow artifacts for the four declared
  desktop targets only after the complete reusable CI and fuzz workflows pass. It
  does not create a public release, install software, sign a package, or claim
  cross-environment bit-for-bit linker reproducibility.
- Publicly distributed builds must be signed and, where required, notarized.
  That is later distribution work, not part of the local Phase 7 proof.
- A human-written changelog. People deserve to know what changed and why.

## A note on humility

These standards describe what we aim for, held consistently. They do not make the
software perfect, and we will get things wrong. The value is in the discipline:
tests that fail loudly, a privacy guarantee the build enforces, and a small enough
surface that we can actually understand what we shipped. When reality and a
standard here conflict, we fix it honestly and in the open.
