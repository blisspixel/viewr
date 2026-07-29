# Build and artifact verification

This guide explains what can be verified from the current repository and where
the evidence stops. viewr does not yet publish a public release or a canonical
checksum set. Until it does, verification is local and applies to the exact source,
target, toolchain, dependency lockfiles, build environment, and artifacts you use.

## What each check establishes

- A Git commit identifies the source tree you reviewed.
- `rust-toolchain.toml` and committed Cargo lockfiles constrain the compiler and
  Rust dependency graph.
- `cargo deny check` verifies that the resolved dependency graph complies with the
  repository's source, license, advisory, and banned-dependency policy. It does not
  prove that arbitrary source code cannot perform networking.
- The privacy check enforces repository-owned dependency and packaging boundaries
  and keeps narrow source tripwires around the reviewed orchestration and ephemeral
  contracts. Rust behavior tests separately prove that absent logging variables and
  unsupported external-only directives construct no logger.
- A SHA-256 checksum identifies one exact artifact. A checksum match proves that
  two byte sequences match; it does not independently prove which source produced
  those bytes.
- The release-artifact verifier checks the archive's internal manifest, expected
  dual-binary contents, and checksum sidecar.

## 1. Pin the source and toolchain

Record the commit you intend to verify and ensure the repository is not silently
using a different toolchain or dependency graph:

```text
git rev-parse HEAD
git status --short
rustup show active-toolchain
cargo metadata --locked --format-version 1
```

A dirty tree is valid for local development, but its output is not evidence for
the unmodified commit alone. Review and record the diff if local changes are part
of the build.

## 2. Run the local trust and quality gates

Use the commands documented in `docs/INSTALL.md` and `docs/STANDARDS.md`. The core
verification set includes formatting, strict Clippy, tests, coverage, privacy,
dependency policy, advisory checks, and the separate fuzz workspace lockfile.

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo deny check
cargo audit
pwsh -NoProfile -File scripts/privacy-check.ps1
cargo check --manifest-path fuzz/Cargo.toml --locked
```

Platform package and optional C-decoder checks require their documented local
SDKs or system libraries. A skipped platform check is an evidence gap, not a pass.
## 3. Build the exact target

Build both workspace binaries from locked dependencies:

```text
cargo build --release --workspace --locked
```

Record the target triple, operating-system image, linker and SDK versions, enabled
features, environment variables that affect compilation, and the commit. These
inputs can change executable bytes even when Rust source and Cargo dependencies
are unchanged.

## 4. Build and verify the release archive

Use the target-specific `scripts/release_artifact.py build` and `verify` commands
in `docs/INSTALL.md`. The builder creates a deterministic archive from the binary
inputs it receives, includes the security policy and canonical Markdown set, writes
an internal manifest, and emits a SHA-256 sidecar. The verifier then checks those
exact bytes, the declared archive structure, and local README links written in the
repository's simple inline Markdown form with repository-relative destinations.
Reference-style links, Markdown images, and raw HTML destinations are not parsed
and must not be used for the README's portable documentation navigation.

The archive process is deterministic for identical inputs. The repository does
not claim that separate operating-system images or linker versions produce
bit-identical executables.

## 5. Compare published artifacts when they exist

For a future public release, obtain the checksum and provenance from the release's
canonical page, verify signatures when available, and compare the downloaded
artifact's SHA-256 value before opening it. A matching published checksum verifies
download integrity relative to that release record. Independent source-to-binary
reproduction requires a documented, controlled build environment and comparison
against the unsigned executable produced there.

Current release-readiness gaps, including hosted multi-OS evidence, signing,
notarization, public checksums, and independent reproduction, remain tracked in
`docs/ROADMAP.md`.
