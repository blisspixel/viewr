# Changelog

All notable changes to this project are documented here. The format is human-written and kept short.

## Unreleased

### Fixed

- Restored the quality baseline: `cargo fmt`, pedantic `clippy -D warnings`, and the full test suite are green again.
- Implemented SVG decode with pure-Rust `resvg` (corpus and unit tests pass). Default features avoid system fonts and text shaping so the trusted core stays free of unmaintained shaping crates.
- Coverage gate again measures meaningful logic only; CI excludes display/IPC glue (`app`, `gpu`, `ui`, `sandbox`, `error`, `main`) per `docs/STANDARDS.md`. Measured logic coverage is above 90% lines under that floor.

### Changed

- Extracted `sandbox.rs` for the `viewr-decode` worker client so process/IPC glue is separate from pure decode logic.
- Documented Phase 6 residuals honestly in README and ROADMAP (worker not yet a workspace member; RAW stub; Phase 7 still owns OS sandbox packaging).
- `deny.toml`: allow OFL/Ubuntu font licenses for egui default fonts; ignore known unmaintained gtk3-rs advisories pulled only by `muda` on Linux (not on the decode path).
