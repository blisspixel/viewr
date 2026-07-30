# Contributing to viewr

Thank you for helping improve viewr. Contributions should strengthen the current
product without expanding it into an account, cloud, telemetry, or library-management
application.

## Before starting

- Read [README.md](README.md), [docs/ROADMAP.md](docs/ROADMAP.md), and the relevant
  design or architecture document.
- Open an issue before a large behavior, dependency, file-format, or architecture
  change. Small bug fixes and documentation corrections can go directly to a pull
  request.
- Report vulnerabilities through the private process in [SECURITY.md](SECURITY.md),
  never through a public issue.

## Local setup

Install the Rust toolchain pinned by `rust-toolchain.toml`, then run:

```text
cargo build --workspace --locked
cargo test --workspace --all-targets --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The full quality, coverage, privacy, fuzzing, and platform checks are documented in
[docs/VERIFY.md](docs/VERIFY.md). Platform-specific build prerequisites are in
[docs/INSTALL.md](docs/INSTALL.md).

## Change standards

- Keep changes focused and reviewable.
- Add a regression test for behavior changes when practical.
- Preserve the no-network, no-telemetry, and no-activity-log product boundaries.
- Treat image bytes, metadata, paths, filenames, and operating-system integration as
  security-sensitive input.
- Update README, CHANGELOG, ROADMAP, or detailed docs whenever public behavior or a
  documented contract changes.
- Do not commit personal photos, private paths, credentials, generated build output,
  logs, coverage output, or agent scratch files.

## Pull requests

A pull request should explain the user-visible problem, the smallest safe fix, and
the exact verification performed. CI must pass, meaningful logic coverage must stay
at or above 85 percent, and new dependencies must pass the repository license,
advisory, and privacy policies.

By submitting a contribution, you agree that it is licensed under the Apache
License 2.0 described in [LICENSE](LICENSE).
