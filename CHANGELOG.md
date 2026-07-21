# Changelog

All notable changes to this project are documented here. The format is human-written and kept short.

## Unreleased

### Fixed

- Restored the quality baseline: `cargo fmt`, pedantic `clippy -D warnings`, and the full test suite are green again.
- Implemented SVG decode with pure-Rust `resvg` (corpus and unit tests pass). Default features avoid system fonts and text shaping so the trusted core stays free of unmaintained shaping crates.
- Coverage gate again measures meaningful logic only; CI excludes display/IPC glue (`app`, `gpu`, `ui`, `sandbox`, `error`, `main`) per `docs/STANDARDS.md`. Measured logic coverage is above 90% lines under that floor.

### Added

- `viewr-decode` is a workspace member with feature-gated C backends (`avif`, `heic`, `raw`); default build needs no system libraries.
- Folder navigation recognizes worker formats (AVIF/HEIC/RAW extensions); decode routes through the worker when present.
- `docs/FORMATS.md` capability table (core vs worker, RAW deferred).
- System trash via the `trash` crate (`curate` module) with undo from the OS recycle bin / trash.

### Changed

- Extracted `sandbox.rs` for the `viewr-decode` worker client so process/IPC glue is separate from pure decode logic.
- Phase 6 residuals closed in ROADMAP; next milestone is Phase 7 OS sandbox packaging.
- `deny.toml`: allow OFL/Ubuntu font licenses for egui default fonts; ignore known unmaintained gtk3-rs advisories pulled only by `muda` on Linux (not on the decode path).
