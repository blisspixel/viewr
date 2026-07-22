# Changelog

All notable changes to this project are documented here. The format is human-written and kept short.

## Unreleased

### Fixed

- Egui redraw requests no longer schedule another redraw from inside the current
  redraw event. The settled viewer now returns to the sleeping event loop instead
  of continuously presenting frames, and repeated identical chrome styling no
  longer requests needless visual updates.
- Bounded prefetch and thumbnail work now refills from completion events instead
  of depending on incidental paint events. Explicit timer wakes make the
  performance probe's idle interval and one-minute deadline deterministic, while
  normal directory scans avoid redundant per-entry metadata lookups.
- The empty-state privacy line now reads "Maximum privacy. It just works." instead
  of exposing implementation-detail copy inside the viewer.
- Initial image decode and sibling-folder discovery now start before GPU
  initialization and wake the event loop when their background work completes.
  The first window stays responsive with an explicit loading state, and small
  images no longer collapse the application into a cramped layout.
- Persistent tools, folder previews, and Image Information now reserve viewport
  space. Expanding them refits and recenters the photo instead of covering it.
  Empty-state guidance remains readable on a pure-white image background.
- Image drawing is GPU-scissored to the panel-safe viewport at every zoom and pan
  level, and crop geometry is converted from physical pixels to logical UI points
  on high-DPI displays. Load failures now remain visible in the empty state.
- Keyboard shortcuts remain active when a non-text control has accessibility
  focus, while open menus and control activation keep ownership of their keys.
  macOS now uses Command for Open File and Open Folder instead of displaying or
  requiring a Windows-style Control shortcut.
- Delayed egui repaints now drive the sleeping event loop, so stationary-hover
  tooltips and other timed UI appear without unrelated input. Static toasts
  schedule only their expiry repaint instead of forcing a continuous redraw.
- Folder-preview textures are retained only for the bounded visible window and
  completed off-screen thumbnail work is discarded, keeping image-cache memory
  independent of folder length.
- macOS bundles now receive Finder and Open With requests by adding the missing
  Launch Services selector to winit's existing application delegate, preserving
  winit lifecycle ownership and waking its event loop through `EventLoopProxy`.
  macOS trash Undo also preserves the exact resulting item URL and refuses to
  replace an existing restore target.
- Trash receipts now store absolute original paths so relative command-line
  opens restore correctly on Windows and Linux. Undo restores every successful
  item from the latest batch, retains failed receipts for retry, and keeps files
  that failed to move flagged instead of silently dropping them from the batch.
- Restored the quality baseline: `cargo fmt`, pedantic `clippy -D warnings`, and the full test suite are green again.
- Raised measured logic coverage from 79.61% to 89.16% by testing CLI behavior and decode-boundary invariants, including diagnostics, benchmark paths, resource limits, explicit in-memory format dispatch, trash receipts, viewport geometry, and the corpus contract.
- Prevented one process from deleting another process's live temporary test workspace by holding and respecting standard-library file locks during stale-debris cleanup.
- Serialized tests that invoke global stale-debris cleanup so parallel test execution cannot erase another test's scrub-safe fixture.
- Implemented SVG decode with pure-Rust `resvg` (corpus and unit tests pass). Default features avoid system fonts and text shaping so the trusted core stays free of unmaintained shaping crates.
- Coverage gate again measures meaningful logic only; CI excludes display/IPC glue (`app`, `gpu`, `ui`, `sandbox`, `worker_limit`, `error`, `main`) per `docs/STANDARDS.md`. Current measured logic coverage is 89.06% lines under that floor.
- Initial image decode now runs off the winit event thread, invalidates stale displayed pixels, and applies only if its path is still current. A two-slot, foreground-priority decode gate bounds aggregate work, and superseded foreground jobs cancel before file access.
- Decode resource limits reject zero, oversized, or inconsistent pixel shapes before parent allocation and pixel-stream copy. SVG and worker inputs are capped, while the C-worker address-space ceiling also bounds allocations performed inside third-party decoders.
- Worker-bound host files are verified as regular files and read with a bounded, fallible allocator before a worker is reserved. The IPC deadline thread now contains only cancellable child-pipe work, and encoded bytes are released immediately after transfer.
- Sandboxed file opens now degrade safely to a one-image playlist when sibling enumeration is denied. **Open Folder** provides explicit session-scoped directory consent for next/previous navigation without broad filesystem capabilities.

### Added

- Added a dependency-free black-box GUI performance gate for first-window-frame
  and first-image latency, sampled navigation, settled idle redraws, peak resident
  memory, 50,000-file folder scaling, and exact decoded/thumbnail cache bounds.
  The explicit path-free probe is absent from normal launches and CI enforces
  conservative release-mode limits under a virtual display. Large test corpora
  shard hard links across bounded source counts and clean their temporary
  workspace on successful and handled-error exits.
- Added exact-set, opt-in core-image associations for the Linux desktop entry,
  macOS application bundle, and Windows AppContainer manifest. The Flatpak build
  now installs its desktop entry and scalable icon. Contract tests keep all
  declarations aligned with the default pure-Rust decoder set and reject silent
  default-viewer takeover behavior.
- Added a shared, feature-gated Linux default-deny seccomp policy for production AVIF/HEIC workers. It permits only measured decoder, read-only plugin, thread, memory, signal-runtime, time, and pipe syscalls; denies direct and inherited-pipe cross-process signaling; proves activation with an unlisted syscall; and is exercised by release-mode AVIF and HEIC protocol decodes on Ubuntu 24.04 CI.
- Added deterministic dual-binary release archives for Linux x86-64, Windows x86-64, and Intel/Apple Silicon macOS. Each archive revalidates the exact archived executable structures, contains a canonical per-file manifest, has a SHA-256 sidecar, and is built only after the reusable complete CI and fuzz gates pass. The workflow is read-only and does not publish or sign.
- Added exact-set verified Flatpak, macOS App Sandbox, and Windows AppContainer profiles. Platform CI performs a checksum-pinned offline Flatpak build and worker probe, verifies and probes an ad-hoc signed macOS bundle, and validates an unsigned dual-binary MSIX with the Windows SDK. Destructive packaging outputs are fixed beneath `target/profile-check` and reject symlink or reparse-point staging paths.
- Added buildable coverage-guided fuzz targets for every declared pure-Rust decoder and the worker protocol, self-contained deterministic seed regeneration covering every decoder, pinned short-on-change plus 600-second scheduled CI runs, and supply-chain checks for the separate fuzz lockfile.
- Pinned every executable GitHub Action to a reviewed immutable commit and added a weekly audit-only CI run so newly published advisories do not wait for repository activity.
- A dependency-free `viewr-protocol` crate owns versioned bounded encoded-input requests, typed response frames, acknowledgements, and the shared 512 MiB RGBA8 output invariant.
- Packaged `viewr doctor` now requires an exact typed encoded-input protocol handshake from `viewr-decode`, so a present but obsolete helper cannot satisfy the package smoke test with an arbitrary error frame.
- Worker requests have a hard 30-second send/receive deadline, bounded cleanup grace, a 1.5 GiB containment memory ceiling, and tested one-process containment, pipe-saturating termination, and pixel-stream acknowledgement.
- CLI: `viewr help`, `doctor`, `benchmark [dir]`, `update` (local instructions only),
  `version`, `open <path>`; image path still opens the GUI. Windows console attach
  for subcommands under the GUI subsystem.
- Interaction polish: responsive File/Edit/View menus with aligned shortcuts,
  stable filename and folder status, a high-contrast empty/loading card, trash
  toast, an honest crop border and ratio strip, Esc cancel crop, double-click
  fit/actual-size toggle, and grab cursors.
- Cursor-anchored wheel/trackpad zoom; compact disclosure rails for docked Tools
  and Folder Previews; asynchronous real thumbnails; consistent vector icons;
  and keyboard/menu commands for fit, actual size, zoom in, and zoom out.
- The zoom readout now reports physical image scale, so Actual Size is 100 percent
  instead of exposing the internal multiplier relative to Fit.
- Accessibility semantics for custom-painted tool, disclosure, and thumbnail
  controls and exact crop bounds; visible keyboard focus; automated WCAG AA
  contrast checks; and a native AccessKit bridge on Windows and macOS. The bridge
  is integrated without admitting AccessKit's network-capable Unix D-Bus stack.
- Filmstrip shows async real thumbnails (`thumbs` module); Space-hold temporary pan, tap Space resets view.
- Linux worker: `no_new_privs`, non-dumpable, and seccomp-bpf that EPERMs classic and io_uring network paths (`seccompiler`).
- Flag/batch cull (`X` / `B`) and Shift+Delete permanent delete with confirmation.
- `viewr-decode` is a workspace member with feature-gated C backends (`avif`, `heic`, `raw`); default build needs no system libraries.
- Folder navigation recognizes worker formats (AVIF/HEIC/RAW extensions); decode routes through the worker when present.
- `docs/FORMATS.md` capability table (core vs worker, RAW deferred).
- System trash via the `trash` crate (`curate` module) with undo from the OS recycle bin / trash.
- Adversarial truncated/garbage image fixtures assert decode returns errors without panicking.
- packaging sketches for Flatpak (no network), macOS sandbox entitlements, Windows AppContainer.
- Decode workers join a one-process Windows Job Object (kill-on-close with parent) or a private Unix session with a one-process policy; discarded workers are terminated on drop.

### Changed

- The main process now opens user-selected C-decoder inputs and sends bounded encoded bytes to `viewr-decode`; the worker receives no path and therefore does not depend on inherited dynamic file grants.
- Replaced worker-owned shared memory with exact-length pixel streaming over the existing pipe, removing mutable-mapping size races and the `shared_memory` dependency.
- Linux seccomp and privilege setup now fail worker spawn on error; network denial covers classic socket and io_uring paths, and non-dumpable state is applied after exec. Windows Job Object creation, assignment, termination, and memory limits also fail closed.
- Removed the unused `muda` dependency and its GTK3 dependency chain, reducing the lockfile by 41 packages and eliminating eight obsolete advisory exceptions.
- CI now runs on pushes to both `main` and the repository's current `master` branch.
- ROADMAP: store/notarized publish (Mac App Store, Microsoft Store, Flathub
  listing, notarized DMG as a store path) is **out of scope for now**—maybe later.
  Phase 7/8 stay local-first (build, sandbox profiles, simple release artifacts).
- Extracted `sandbox.rs` for the `viewr-decode` worker client so process/IPC glue is separate from pure decode logic.
- Phase 6 residuals closed in ROADMAP; next milestone is Phase 7 OS sandbox packaging.
- `deny.toml`: allow OFL/Ubuntu font licenses for egui default fonts; retain a documented exception for the unmaintained but vulnerability-free `paste` crate while required by EXR and metadata dependencies.
