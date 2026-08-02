# Build and artifact verification

This guide explains what can be verified from source and published release records,
and where the evidence stops. Local verification applies to the exact source,
target, toolchain, dependency lockfiles, build environment, and artifacts used.
Published checksums and attestations add release-record and workflow provenance;
they do not replace source review or platform signing.

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
- The committed third-party license inventory is regenerated from the locked default
  release graph. Its verifier compares every package/version, license assignment,
  and license text while ignoring presentation order, line endings, and repository
  link changes that do not alter the shipped license obligations. Host-sensitive
  license detection is resolved only through checksum-backed source-file
  clarifications in `about.toml`.
- A GitHub artifact attestation binds a published asset digest to the repository and
  workflow identity recorded by GitHub. It does not make an unsigned executable a
  platform-signed application.

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
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo build --workspace --all-targets --locked
cargo deny --locked check --hide-inclusion-graph -D warnings -A license-not-encountered -A unmatched-skip -A unnecessary-skip
cargo deny --manifest-path fuzz/Cargo.toml --locked check --hide-inclusion-graph -D warnings
cargo audit -D warnings
cargo about generate about.hbs --workspace --locked --offline --fail --output-file target/THIRD_PARTY_LICENSES.html
python -B scripts/verify_license_inventory.py THIRD_PARTY_LICENSES.html target/THIRD_PARTY_LICENSES.html
pwsh -NoProfile -File scripts/privacy-check.ps1
cargo check --manifest-path fuzz/Cargo.toml --locked
```

The locally patched `jxl-color` and `jxl-render` crates are workspace members, so
the formatting, Clippy, build, and test commands above execute their behavioral
regression suites rather than relying on source-text patch checks. CI excludes
those dependency packages from the owned-logic coverage denominator, preserving
the same application coverage set and 85 percent threshold used before they became
test members.

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
inputs it receives; includes LICENSE, NOTICE, third-party licenses, desktop assets,
the security policy, canonical Markdown, and the bounded PNG interface capture used
by the README; writes an internal manifest; and emits a SHA-256 sidecar. The verifier
then checks those exact bytes, the declared archive structure, and local README links
written in the repository's simple inline Markdown form with repository-relative
destinations.
Reference-style links, Markdown images, and raw HTML destinations are not parsed
and must not be used for the README's portable documentation navigation.

The archive process is deterministic for identical inputs. The repository does
not claim that separate operating-system images or linker versions produce
bit-identical executables.

## 5. Verify a published release

Obtain the archive and checksum only from the canonical GitHub release page. Verify
the sidecar before extraction, then verify GitHub provenance when GitHub CLI is
available:

```text
gh release verify <tag> --repo blisspixel/viewr
gh attestation verify <archive> --repo blisspixel/viewr
python scripts/release_artifact.py verify <archive>
```

A matching checksum verifies download integrity relative to that release record.
The attestation identifies the GitHub repository and workflow that produced the
asset. Independent source-to-binary reproduction still requires a documented,
controlled build environment and comparison against the unsigned executable
produced there.

Current release-readiness gaps, including platform signing, notarization,
representative-hardware acceptance, and independent reproduction, remain tracked
in `docs/ROADMAP.md`.
