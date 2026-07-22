# Roadmap

The plan from an empty repository to a viewer people install to escape the bloat.
Phases are ordered by dependency, so each one builds on what came before. There are
no dates and no time estimates here on purpose. The order is the plan, and a phase
is finished when its Definition of done is true, not when a calendar says so.

Two rules hold across every phase:

1. A feature ships only if it earns its place. When in doubt, leave it out. The
   restraint is the product.
2. The quality bar in STANDARDS.md applies from the first commit. Coverage target
   is 85 percent or higher on logic, the privacy invariant is enforced in CI, and
   the decode path is fuzzed. Quality is not a later phase, it is the baseline.

## Current status

Phases 0-7 are complete for the product scope defined in this roadmap. System
trash, operational core fuzzing, locally verifiable OS sandbox profiles,
checksummed local/CI release artifacts, and the feature-gated production
C-decoder syscall allowlist are complete. Phase 8 local install guidance,
user-controlled file associations, and canonical documentation are complete;
keyboard access, native Windows/macOS/Linux screen-reader delivery, semantic
labels, a native Windows provider/action smoke gate, configurable panel-safe
chrome, contrast checks, and enforced GUI performance budgets are complete.
Manual cross-platform assistive-technology validation remains.

**Next focus: manual target-OS assistive-technology validation.**

## Phase 0: Foundations

Establish the ground truth so quality is enforced from the very first commit.

- Cargo workspace and the module skeleton described in ARCHITECTURE.md.
- Apache 2.0 LICENSE in place.
- Pinned toolchain (rust-toolchain.toml), committed Cargo.lock, declared MSRV.
- CI on Linux, macOS, and Windows running: fmt check, clippy at pedantic with
  warnings as errors, nextest, and coverage via cargo-llvm-cov.
- The privacy invariant as CI and runtime gates: cargo-deny bans remote-service
  client stacks and constrains Linux D-Bus to AccessKit, while Linux startup denies
  Internet socket creation before application threads. This lands before features.
- cargo-audit and cargo-deny wired for supply-chain and license checks.

Definition of done: an empty window builds and runs on all three platforms in CI,
every quality gate is green, and adding an HTTP crate would fail the build.

## Phase 1: It opens an image

The smallest thing that is genuinely useful and genuinely fast.

- Open from a command-line argument, an Open dialog (rfd), and the operating
  system "open with" association.
- Decode the common baseline formats (JPEG, PNG, GIF, WebP, BMP) via image-rs.
- Display through our own winit and wgpu pipeline, fit to window by default, large images scaled
  correctly on first paint.
- First-pixel latency tracked as a metric from day one.

Definition of done: double-clicking a JPEG or PNG opens it near-instantly and
scaled correctly on Linux, macOS, and Windows, with tests covering the open path.

## Phase 2: Folder navigation that feels instant

The core experience, which is flipping through a folder with no perceptible lag.

- Scan the containing folder off-thread, in natural-sort order so img2 comes before
  img10.
- Left and right arrows, Home and End, navigate the folder.
- Neighbor prefetch into a bounded decoded-image RAM cache, so the next image is
  usually decoded before it is requested and needs only a GPU upload.
- Animated GIF and WebP playback with correct frame timing.

Definition of done: holding the arrow key through a folder of 4K images is smooth
with no stutter, memory stays flat on a folder of 50,000 images, and property tests
cover ordering and cache eviction.

## Phase 3: Look at it properly

Make viewing excellent, not merely functional.

- [x] GPU pan by dragging or holding Space, focal-point scroll zoom, and explicit
  keyboard commands for fit (`0`), actual pixels (`1`), zoom in (`+`), and zoom
  out (`-`). A Space tap resets fit; double-click toggles fit and actual pixels.
- [x] Rotate 90 degrees either direction, and flip.
- [x] Fullscreen and a frameless immersive mode that is just the picture.
- [x] System-driven default image background via winit, updating live when the
  operating-system setting changes, with explicit black, neutral-gray, and white
  alternatives. Chrome retains a stable high-contrast dark surface.
- [x] Compact docked `egui` controls with keyboard shortcuts and explicit
  disclosure rails. Persistent chrome reserves viewport space and never covers
  the image.

Definition of done: viewing feels polished and obvious, the default image
background follows the operating system live, persistent chrome stays compact and
collapsible, and no control or preview covers the photo.

## Phase 4: Curation, delete and cull

The feature that makes viewr a daily tool, done carefully.

- [x] Delete to the system trash via the trash crate, with a non-blocking Undo toast
  and index preservation, so the view lands on the image that replaced the deleted
  one rather than jumping to the top.
- [x] Undo (`U`) restores the latest trash action, including every successful
  item in a batch, while retaining failed receipts for retry.
- [x] Flag-then-batch cull: `X` flags, `B` batch-trashes flagged (tests on `FlagSet` /
  playlist removal).
- [x] Shift+Delete permanent delete with explicit confirmation dialog (only modal).

Definition of done: a user can move through a folder deleting junk quickly, never
loses a file to a misfire, never hits a modal during normal culling, and
integration tests cover delete, undo, and index preservation.

## Phase 5: Basic tools, save, convert, crop

The simple tools people actually reach for, and nothing beyond them.

- [x] Crop with a GPU preview, usable by keyboard and mouse, with aspect
  presets (Free, 1:1, 4:3, 16:9) and applying crops directly.
- Save As and convert between formats.
- Metadata strip on export, presented prominently, with location and identifying
  fields stripped by default for privacy-sensitive output.

Definition of done: a user can crop an image, export it to another format, and be
confident their location data did not ride along, with tests over the export and
metadata-strip paths.

## Phase 6: Support every format, the VLC of image viewers

The goal here is simple to state: if it is an image, viewr opens it, and the user
never has to think about which app handles which file. Formats are added in order
of how many people they serve and how safely they can be decoded.

- Pure-Rust formats covered by image-rs and friends: JPEG, PNG, GIF, WebP, BMP,
  TIFF, ICO, PNM, TGA, QOI, DDS, HDR, OpenEXR, farbfeld.
- Modern formats: AVIF, and JPEG XL via jxl-oxide.
- Vector: SVG via a pure-Rust renderer (resvg).
- High-value formats that need care: HEIC and HEIF, and camera RAW (Canon, Nikon,
  Sony, Fujifilm, and the rest), all decoded inside the sandboxed worker rather than
  linked into the main process.
- A clear, honest capability list in the docs stating exactly which formats are
  supported and which are decoded in isolation, so there are no surprises.

Every format added ships with golden-file decode tests and is added to the fuzz
corpus. Breadth never lowers the safety or coverage bar.

Definition of done: the supported-format list covers what ordinary people and
photographers actually have on disk, each format has decode tests, and opening any
of them just works.

### Phase 6 residuals (tracked)

- [x] SVG via pure-Rust `resvg` (shapes/paths; text shaping feature intentionally off to keep the trusted core lean).
- [x] Add `viewr-decode` as a workspace member with feature-gated C deps (`avif` / `heic` / `raw`; default empty for CI).
- [x] List AVIF/HEIC/RAW extensions in `fs` for browsing; decode routes through the worker.
- [x] RAW deferred with stable errors and docs (feature `raw` reserved; no false claim of support).
- [x] Honest format capability table: `docs/FORMATS.md`.

## Phase 7: Hardening and the privacy proof

Turn "we designed it to be private and safe" into something a third party can
verify **locally** (build, run, inspect). This phase is **not** about app-store
submission.

- [x] Sandbox *profiles* on all three platforms with the network denied (local
  profiles and runtime limits, not store listing):
  - [x] Flatpak 25.08 runtime profile (`packaging/flatpak/…`) with an exact tested grant set and no `--share=network`.
  - [x] macOS main/helper App Sandbox entitlements without network client/server keys, plus an ad-hoc signed local bundle verifier.
  - [x] Windows packaged-classic AppContainer manifest with an empty capability set and a schema-validating local MSIX builder.
  - [x] Explicit Open Folder consent for sibling navigation, with a safe one-file fallback when a sandbox grants only the selected file.
- The isolated decode worker fully in place, with seccomp on Linux and reduced
  privileges elsewhere.
  - [x] Workspace worker + versioned bounded encoded-input frames + bounded pixel-stream IPC; the helper receives no filesystem path.
  - [x] Windows one-process Job Object kill-on-close + Unix private session and one-process policy (`worker_limit`), with fail-closed setup and a 1.5 GiB containment memory ceiling.
  - [x] Linux `no_new_privs` + post-exec `dumpable=0` + default-allow seccomp-bpf that EPERMs classic and io_uring network paths, with startup failure if hardening cannot apply (`worker_limit` + `packaging/linux/SECCOMP.md`).
  - [x] Shared 512 MiB decoded-output limit, strict dimension validation, fallible large allocations, typed bounded responses, and a hard 30-second send/receive deadline with bounded cleanup. Host file reads occur before worker reservation and outside the IPC deadline thread.
  - [x] Two-slot foreground-priority file-decode gate, stale load cancellation, and exact source/pixel state matching for path-sensitive actions.
  - [x] Feature-gated default-deny allowlist for AVIF/HEIC production builds, with argument-filtered read-only plugin discovery, thread-only clone, fail-closed activation proof, and release-mode runtime decodes on Ubuntu 24.04 (`viewr-seccomp` + C-decoder CI).
- Continuous fuzzing of every decoder, with any crash a release blocker.
  - [x] Adversarial non-panic corpus tests for truncated/garbage inputs (stable CI).
  - [x] Buildable cargo-fuzz targets and seed corpora for every core decoder and the worker protocol (`fuzz/`).
  - [x] Pinned nightly cargo-fuzz smoke runs on changes plus 600-second scheduled runs (`.github/workflows/fuzz.yml`).
- [x] Neighbor full-decode prefetch into a bounded in-memory LRU (no disk cache).
- [x] Reproducibly buildable local/CI release artifacts: a pinned Rust toolchain,
  locked dependencies, exact target validation, deterministic dual-binary ZIP
  assembly, an internal file manifest, SHA-256 sidecars, and a read-only four-target
  CI workflow gated by the complete CI and fuzz contracts. This is repeatable
  source-to-artifact verification, not a claim of bit-identical linker output
  across different host images. **Not** notarization, public release creation, or
  store signing (see out-of-scope below).
- [x] Deletes use the system trash (`trash` crate), not a local `_trash` folder.

Definition of done: the app runs correctly with network denied by packaging
profile and/or process policy where implemented, fuzzing finds no crashes at the
decode boundary, and a release binary can be built and verified from this repo
without requiring third-party store accounts.

## Phase 8: 1.0, the viewer people recommend

Polish and **local-first** distribution so switching costs nothing for people who
install from source or a simple GitHub-style release artifact.

- [x] Local/CI install paths: locked source builds, verified dual-binary release
  archives (`viewr` + `viewr-decode`), native profile build commands, and
  platform-specific local installation guidance. Optional public installers
  remain outside the local-first requirement.
- [x] Sensible file-association setup that never hijacks defaults silently:
  exact core-format Linux desktop, macOS Launch Services, and Windows MSIX
  declarations; Flatpak desktop assets; native open delivery; and opt-in docs.
- [x] Canonical tracked documentation and a human-written changelog, with no
  analytics, remote scripts, or tracker-bearing website required for 1.0.
- [ ] Accessibility pass:
  - [x] Keyboard-complete menus, docked controls, navigation, zoom, and crop.
  - [x] Screen-reader labels and state for custom controls and exact crop bounds.
  - [x] Automated WCAG AA checks for the production chrome palette.
  - [x] Native AccessKit delivery on Windows and macOS without adding a remote-service client dependency.
  - [x] Privacy-compatible Linux AccessKit/AT-SPI delivery with local-only D-Bus
    validation, dependency-path enforcement, and an early fail-closed Internet
    socket policy.
  - [x] External Windows UI Automation smoke coverage for the native tree, state,
    focusability, and action path (`scripts/accessibility-smoke.ps1`).
  - [ ] Manual screen-reader validation on Windows, macOS, and Linux using
    `docs/ACCESSIBILITY.md`.
- [x] Performance budget locked in and regression-tested in CI: first presented
  window frame and image, sampled navigation, settled idle redraws,
  50,000-file memory scaling, and bounded decoded/thumbnail caches.

Definition of done: a careful user can build or download a release artifact, set
viewr as their image viewer if they choose, and never think about bloat again.
**Store shelves are not required for 1.0.**

## Explicitly out of scope for now (maybe later)

**Not** on the active roadmap until we deliberately opt in. Tracked here so it is
not mistaken for Phase 7/8 work:

- Apple notarized `.dmg` / Mac App Store
- Microsoft Store MSIX / Partner Center publish
- Flathub (or other store) *publication* (local Flatpak *build* sketches may still
  exist for sandbox testing)
- Any pipeline that requires paid developer accounts, store review, or
  third-party signing secrets as a gate for product progress

Revisit only after 1.0 local distribution and privacy proof are solid.

## Beyond 1.0, candidates held to the same bar

Listed so the answer to "will you add X" is "it is tracked and weighed," not
silence, and so that scope creep stays visible and deliberate.

- Optional, local-only, one-click-clearable recent folders.
- Simple non-destructive adjustments (lossless rotate, straighten, basic exposure),
  only if they stay simple and never turn viewr into an editor.
- A simple slideshow.
- Localization.
- A user-initiated **Check for Updates** command after a canonical release
  repository and signed release policy exist. It must never run at launch or in
  the background, and it must show the destination before opening a browser or
  downloading anything.

## Explicit non-goals, the anti-bloat charter

viewr will not add, now or later: accounts, cloud sync, sharing services, ads,
discover or feed surfaces, face or AI grouping, background services, automatic or
background update checks, telemetry or analytics of any kind, or a plugin marketplace. These
are the features that turned every big-company photo app into the thing we are
replacing. Leaving them out is a permanent part of the product, not a stage of it.
