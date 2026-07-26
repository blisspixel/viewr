# Changelog

All notable changes to this project are documented here. The format is human-written and kept short.

## Unreleased

### Fixed

- Optional AVIF and HEIC workers now attach bounded ICC or typed CICP color
  evidence to protocol V2 pixel streams. AVIF decode preserves libavif metadata;
  HEIC reads source ICC size before fallible allocation, preserves source NCLX
  when newer libheif versions would otherwise choose a different output target,
  enables libheif 1.23 bitstream-profile passthrough when no container NCLX
  exists, and keeps the source ICC when passthrough performs no extra gamut
  conversion. Decoded output CICP replaces ICC only when a requested transform
  demonstrably changes the color encoding.
  System-libheif and embedded 1.23 release tests cover the compatibility floor,
  dual-profile precedence, HEVC VUI-only color, matching ICC plus VUI metadata,
  and a libde265 version that can propagate that VUI. The main process converts
  worker ICC input after the cancellable IPC transaction has joined, recognizes
  tagged sRGB, and exposes unsupported or unknown color as an explicit fallback
  instead of silently assuming tagged sRGB.
- Crop export now validates source buffers, uses fallible allocation, and
  quantizes locked selections to exact reduced integer ratios even on odd-sized
  images. The same quantizer supplies exact accessible output bounds. Save As
  validates aliases, formats, pixels, and retention support before touching its
  destination, then persists a completed sibling temporary output. EXIF detection
  follows content rather than extensions; retention supports JPEG, PNG, and WebP.
- Current-image animation, image-information, and GPU-preview work now use
  independent replace-latest queues. Rapid navigation cannot strand the final
  selection behind a full speculative queue. Still-image readers stop at I/O
  boundaries after supersession, animation stops between frames, and worker
  input reads and blocked IPC requests terminate cooperatively. Renamed animated
  containers are detected by content.
- Over-limit GPU previews are now prepared off the window thread with a
  cancellable linear-light, alpha-correct area filter and fallible bounded
  allocation. The release performance gate exercises that path and a high-resolution
  corpus where entry-only cache eviction would exceed the 256 MiB byte budget.
- Image Information now reports detected file content before a misleading filename
  extension. Bounded TIFF metadata is sought anywhere in a large source and
  compacted without pixel strips. PNG text, EXIF, and ICC plus WebP EXIF and ICC
  payloads are bounded before decoder materialization, including post-IDAT APNG
  metadata. JPEG XL initialization now enforces the same 10 MiB ICC ceiling and
  rejects command-stream output amplification. Decoder dimensions and declared
  output bytes, including animation canvases, are validated before pixel-buffer
  allocation.
- The native Windows accessibility gate now verifies the exact Console preference,
  relaunches the process to prove restoration, and enforces one absolute suite
  deadline in addition to per-operation deadlines.
- Tools and Folder Previews can now be fully hidden instead of leaving a disclosure
  rail in the image viewport. View owns explicit visibility controls for all three
  panels, while disclosure chevrons remain available for compact collapse.
- Egui redraw requests no longer schedule another redraw from inside the current
  redraw event. The settled viewer now returns to the sleeping event loop instead
  of continuously presenting frames, and repeated identical chrome styling no
  longer requests needless visual updates. The performance probe now waits for
  legitimate delayed hover repaints to finish before measuring settled idle, so
  machine cursor placement cannot create a false regression.
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
- Coverage gate again measures meaningful logic only; CI excludes display/IPC glue (`app`, `gpu`, `ui`, `sandbox`, `worker_limit`, `error`, `main`) per `docs/STANDARDS.md`. Current measured logic coverage remains above 88% lines under that floor, including above 86% for the new healing core.
- Initial image decode now runs off the winit event thread, invalidates stale displayed pixels, and applies only if its path is still current. A two-slot, foreground-priority decode gate bounds aggregate work, and superseded foreground jobs cancel before file access.
- Decode resource limits reject zero, oversized, or inconsistent pixel shapes before parent allocation and pixel-stream copy. SVG and worker inputs are capped, while the C-worker address-space ceiling also bounds allocations performed inside third-party decoders.
- Worker-bound host files are verified as regular files and read with a bounded, fallible allocator before a worker is reserved. The IPC deadline thread now contains only cancellable child-pipe work, and encoded bytes are released immediately after transfer.
- Sandboxed file opens now degrade safely to a one-image playlist when sibling enumeration is denied. **Open Folder** provides explicit session-scoped directory consent for next/previous navigation without broad filesystem capabilities.
- The Windows accessibility smoke gate now allows its complete multi-action UIA
  flow 60 seconds while retaining per-step polling, process-exit detection, and
  the existing hard 120-second parameter ceiling. This removes a reproducible
  cold-machine CI timeout without weakening any accessibility assertion.

### Added

- Added bounded GIF, WebP, and APNG playback with deterministic timing,
  pause/resume state, finite and infinite loop handling, and current-image-only
  memory limits.
- Added Free, Original, 1:1, ten standard landscape and portrait crop ratios,
  orientation swap, exact numeric custom ratios, eight pointer handles, and full
  keyboard movement, resizing, apply, and cancel behavior.
- Added Image Information, a functional accessible About modal, and complete
  System, Light, Dark, and phosphor-green Console appearances. The selected
  appearance is stored as one validated local word and survives restart.
- Added bounded embedded RGB ICC conversion into the current sRGB pipeline, an
  explicit fallback status, and a complete GPU-generated mip chain for stable
  minification. Per-display output transforms and higher-precision wide-gamut or
  HDR presentation remain roadmap work.
- Added focused Spot Heal for small blemishes. `J` opens a temporary docked
  inspector that reserves image space; sparse image-space strokes run through a
  bounded pure-Rust patch-matching worker with feathered compositing and
  byte-bounded in-memory undo/redo. Repairs never write the source file and add
  no model or native dependency. Done and Esc leave the tool without dropping an
  already-submitted repair, and integration coverage verifies repair, undo, redo,
  pixel-only export, and reopen as one flow. Repair, undo, and redo upload only
  the changed texture region; editing is unavailable when a GPU texture limit
  prevents the complete source image from being displayed.
- Documented the strict gate for any future optional local description model:
  explicit one-image activation, separate model packs, path-free pixel IPC,
  process-level network and write denial, zero app-owned logs, no automatic
  speech, and a cross-platform offline model bake-off. No model runtime ships yet.
- Added a dependency-free Windows UI Automation smoke gate over the real app and
  native AccessKit provider. It verifies focusable menus, image context, default
  panel state, panel and disclosure actions, distinct left/right docking state,
  metadata checked state, the Spot Heal action path, previews, and accessible
  thumbnail navigation. The
  canonical manual Narrator, VoiceOver, and Orca matrix remains required and is
  documented separately.
- Added independent left/right docking for Tools and Image Information through
  View > Panel Position. Insets accumulate correctly when panels share an edge,
  every layout refits the image-safe viewport, and hidden panels reserve zero space.
- Added native Linux AccessKit/AT-SPI delivery behind an early fail-closed privacy
  boundary. Startup accepts only Unix-domain D-Bus environment addresses, permits
  local Unix IPC, denies Internet socket families and io_uring before application
  threads start, mirrors every blocked syscall onto its x86-64 x32 ABI alias, and
  fails launch if the policy cannot be verified. The worker baseline filter closes
  the same x32 alias path. Cargo-deny confines the generic D-Bus and process-helper
  crates to the reviewed AccessKit dependency path. Cross-target builds and policy
  contracts are automated; manual Orca acceptance remains release evidence rather
  than an implementation claim.
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
  contrast checks; and native AccessKit delivery on Windows, macOS, and Linux.
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
  listing, notarized DMG as a store path) is **out of scope for now**, maybe later.
  Phase 7/8 stay local-first (build, sandbox profiles, simple release artifacts).
- Extracted `sandbox.rs` for the `viewr-decode` worker client so process/IPC glue is separate from pure decode logic.
- Phase 6 residuals closed in ROADMAP; next milestone is Phase 7 OS sandbox packaging.
- `deny.toml`: allow OFL/Ubuntu font licenses for egui default fonts; retain a documented exception for the unmaintained but vulnerability-free `paste` crate while required by EXR and metadata dependencies.
