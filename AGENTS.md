# Repository instructions

These instructions apply to the whole repository.

## Start here

Read `README.md`, `docs/ROADMAP.md`, `docs/STANDARDS.md`, `SECURITY.md`, and
`docs/README.md` before changing behavior. Load more detailed documentation only
for the surface being changed. Keep `README.md`, `CHANGELOG.md`, the roadmap, and
focused guides accurate when behavior or durable process changes.

## Product boundary

viewr is a fast, focused, local-only desktop image viewer. Preserve these core
constraints:

- no account, telemetry, advertising, cloud library, activity history, or
  background update checks;
- no application HTTP or TLS client;
- no catalog, sidecar, or hidden rating database;
- no unrelated feature work or broad rewrite when a bounded fix is sufficient;
- no silent failure on important user, operator, privacy, or recovery paths.

## Engineering bar

- Keep changes small, readable, secure, accessible, and test-near.
- Treat image bytes, metadata, paths, worker responses, and external tool output
  as untrusted input.
- Do not add placeholders, TODOs, fake tests, fake metrics, dead code, or
  commented-out implementations.
- Do not add generated-by lines, assistant names, model names, emojis, or em
  dashes.
- Keep meaningful Rust logic coverage and Python tooling coverage at or above 85
  percent. Prefer tests that prove the behavior over tests that merely execute it.
- Keep GitHub Actions pinned to full commit SHAs. Release notes are reviewed files
  under `docs/releases/`, and public installer commands must use immutable release
  assets rather than moving branches.

## Required validation

Run the checks relevant to the changed area. Before publishing or merging a
release-impacting change, run the complete gate documented in `docs/VERIFY.md`,
including:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

CI, dependency policy, privacy checks, release tooling, native accessibility,
coverage, and performance checks must remain green. Never weaken a threshold to
make a change pass.

## Repository hygiene

Keep application source under `crates/`, canonical documentation under `docs/`,
packaging under `packaging/`, and automation under `scripts/`. Standard root
project files remain at the root. Put local logs in ignored `logs/` and agent
working files in ignored `.agent/`. Do not commit caches, generated coverage,
build output, local secrets, or scratch files. Stage only intentional changes and
keep `main` clean, linear, and passing.
