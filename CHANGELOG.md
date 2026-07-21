# Changelog

All notable changes to this project are documented here. The format is human-written and kept short.

## Unreleased

### Fixed

- Restored the quality baseline: `cargo fmt`, pedantic `clippy -D warnings`, and the full test suite are green again.
- Implemented SVG decode with pure-Rust `resvg` (corpus and unit tests pass). Default features avoid system fonts and text shaping so the trusted core stays free of unmaintained shaping crates.
- Coverage gate again measures meaningful logic only; CI excludes display/IPC glue (`app`, `gpu`, `ui`, `sandbox`, `error`, `main`) per `docs/STANDARDS.md`. Measured logic coverage is above 90% lines under that floor.

### Added

- Interaction polish: progressive left floating toolbar (auto-hide), empty-state guidance, bottom status chip (name · size · position · zoom), trash toast, amber crop handles + ratio strip, Esc cancel crop, double-click fit/1:1 toggle, grab cursors.
- Cursor-anchored wheel/trackpad zoom; progressive bottom filmstrip (near-bottom hover); monochrome painted toolbar icons.
- Linux worker: `PR_SET_DUMPABLE=0` plus seccomp packaging plan (`packaging/linux/SECCOMP.md`).
- Flag/batch cull (`X` / `B`) and Shift+Delete permanent delete with confirmation.
- `viewr-decode` is a workspace member with feature-gated C backends (`avif`, `heic`, `raw`); default build needs no system libraries.
- Folder navigation recognizes worker formats (AVIF/HEIC/RAW extensions); decode routes through the worker when present.
- `docs/FORMATS.md` capability table (core vs worker, RAW deferred).
- System trash via the `trash` crate (`curate` module) with undo from the OS recycle bin / trash.
- Adversarial truncated/garbage image fixtures assert decode returns errors without panicking.
- packaging sketches for Flatpak (no network), macOS sandbox entitlements, Windows AppContainer.
- Decode workers join a Windows Job Object (kill-on-close with parent) and a private Unix process group; discarded workers are terminated on drop.

### Changed

- Extracted `sandbox.rs` for the `viewr-decode` worker client so process/IPC glue is separate from pure decode logic.
- Phase 6 residuals closed in ROADMAP; next milestone is Phase 7 OS sandbox packaging.
- `deny.toml`: allow OFL/Ubuntu font licenses for egui default fonts; ignore known unmaintained gtk3-rs advisories pulled only by `muda` on Linux (not on the decode path).
