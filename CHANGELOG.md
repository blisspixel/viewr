# Changelog

All notable changes to this project are documented here. The format is human-written and kept short.

## Unreleased

### Fixed

- Restored the quality baseline: `cargo fmt`, pedantic `clippy -D warnings`, and the full test suite are green again.
- Raised measured logic coverage from 79.61% to 89.74% by testing CLI behavior through injected output streams, including diagnostics, benchmark success/error paths, and the in-memory corpus contract.
- Prevented one process from deleting another process's live temporary test workspace by holding and respecting standard-library file locks during stale-debris cleanup.
- Implemented SVG decode with pure-Rust `resvg` (corpus and unit tests pass). Default features avoid system fonts and text shaping so the trusted core stays free of unmaintained shaping crates.
- Coverage gate again measures meaningful logic only; CI excludes display/IPC glue (`app`, `gpu`, `ui`, `sandbox`, `error`, `main`) per `docs/STANDARDS.md`. Measured logic coverage is 89.74% lines under that floor.

### Added

- CLI: `viewr help`, `doctor`, `benchmark [dir]`, `update` (local instructions only),
  `version`, `open <path>`; image path still opens the GUI. Windows console attach
  for subcommands under the GUI subsystem.
- Interaction polish: progressive left floating toolbar (auto-hide), empty-state guidance, bottom status chip (name · size · position · zoom), trash toast, amber crop handles + ratio strip, Esc cancel crop, double-click fit/1:1 toggle, grab cursors.
- Cursor-anchored wheel/trackpad zoom; progressive bottom filmstrip (near-bottom hover); monochrome painted toolbar icons.
- Filmstrip shows async real thumbnails (`thumbs` module); Space-hold temporary pan, tap Space resets view.
- Linux worker: `no_new_privs`, non-dumpable, and seccomp-bpf that EPERMs network syscalls (`seccompiler`).
- Flag/batch cull (`X` / `B`) and Shift+Delete permanent delete with confirmation.
- `viewr-decode` is a workspace member with feature-gated C backends (`avif`, `heic`, `raw`); default build needs no system libraries.
- Folder navigation recognizes worker formats (AVIF/HEIC/RAW extensions); decode routes through the worker when present.
- `docs/FORMATS.md` capability table (core vs worker, RAW deferred).
- System trash via the `trash` crate (`curate` module) with undo from the OS recycle bin / trash.
- Adversarial truncated/garbage image fixtures assert decode returns errors without panicking.
- packaging sketches for Flatpak (no network), macOS sandbox entitlements, Windows AppContainer.
- Decode workers join a Windows Job Object (kill-on-close with parent) and a private Unix process group; discarded workers are terminated on drop.

### Changed

- Removed the unused `muda` dependency and its GTK3 dependency chain, reducing the lockfile by 41 packages and eliminating eight obsolete advisory exceptions.
- CI now runs on pushes to both `main` and the repository's current `master` branch.
- ROADMAP: store/notarized publish (Mac App Store, Microsoft Store, Flathub
  listing, notarized DMG as a store path) is **out of scope for now**—maybe later.
  Phase 7/8 stay local-first (build, sandbox profiles, simple release artifacts).
- Extracted `sandbox.rs` for the `viewr-decode` worker client so process/IPC glue is separate from pure decode logic.
- Phase 6 residuals closed in ROADMAP; next milestone is Phase 7 OS sandbox packaging.
- `deny.toml`: allow OFL/Ubuntu font licenses for egui default fonts; retain a documented exception for the unmaintained but vulnerability-free `paste` crate while required by EXR and metadata dependencies.
