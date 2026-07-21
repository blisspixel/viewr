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

Phases 0–6 are complete for the product scope defined in this roadmap (including
Phase 6 residuals: workspace worker, format table, RAW deferral). System trash
polish and continuous fuzz remain quality follow-ups; OS sandbox packaging is
Phase 7.

**Next focus: Phase 7 — Hardening and the privacy proof.**
## Phase 0: Foundations

Establish the ground truth so quality is enforced from the very first commit.

- Cargo workspace and the module skeleton described in ARCHITECTURE.md.
- Apache 2.0 LICENSE in place.
- Pinned toolchain (rust-toolchain.toml), committed Cargo.lock, declared MSRV.
- CI on Linux, macOS, and Windows running: fmt check, clippy at pedantic with
  warnings as errors, nextest, and coverage via cargo-llvm-cov.
- The privacy invariant as a CI gate: cargo-deny bans any network-capable crate, so
  the build fails if one ever enters the tree. This lands before any features.
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
- Neighbor prefetch into a bounded GPU LRU cache, so the next image is already
  decoded and uploaded before it is requested.
- Animated GIF and WebP playback with correct frame timing.

Definition of done: holding the arrow key through a folder of 4K images is smooth
with no stutter, memory stays flat on a folder of 50,000 images, and property tests
cover ordering and cache eviction.

## Phase 3: Look at it properly

Make viewing excellent, not merely functional.

- [x] GPU pan by dragging with the Hand Tool, zoom by scroll and keyboard, spacebar toggles fit against
  actual pixels.
- [x] Rotate 90 degrees either direction, and flip.
- [x] Fullscreen and a frameless immersive mode that is just the picture.
- [x] System-driven light and dark theme via dark-light, updating live when the
  operating system setting changes.
- [x] Slick left-aligned floating toolbar built with `egui` that auto-hides, keyboard first, still discoverable.

Definition of done: viewing feels polished and obvious, the theme matches and
follows the operating system live, and there is no visible interface when the user
just wants the photo.

## Phase 4: Curation, delete and cull

The feature that makes viewr a daily tool, done carefully.

- [x] Delete to the system trash via the trash crate, with a non-blocking Undo toast
  and index preservation, so the view lands on the image that replaced the deleted
  one rather than jumping to the top.
- [x] Undo (`U`) restores the last deleted file from the trash.
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
verify.

- [ ] Sandbox packaging on all three platforms with the network denied: macOS App
  Sandbox, Windows AppContainer, and Linux Flatpak with no network share.
  - [x] Flatpak manifest sketch (`packaging/flatpak/…`) with no `--share=network`.
  - [x] macOS entitlements sketch without network client/server keys.
  - [x] Windows AppContainer packaging notes (`packaging/windows/APPCONTAINER.md`).
- The isolated decode worker fully in place, with seccomp on Linux and reduced
  privileges elsewhere.
  - [x] Workspace worker + SHM IPC (process isolation).
  - [x] Windows Job Object kill-on-close + Unix process group (`worker_limit`).
  - [x] Linux `no_new_privs` + `dumpable=0` + default-allow seccomp-bpf that EPERMs network syscalls (`worker_limit` + `packaging/linux/SECCOMP.md`).
  - [ ] Optional default-deny allowlist for C-decoder builds (when avif/heic features are used in production).
- Continuous fuzzing of every decoder, with any crash a release blocker.
  - [x] Adversarial non-panic corpus tests for truncated/garbage inputs (stable CI).
  - [ ] cargo-fuzz continuous job still open.
- Reproducible and signed builds, so a user can confirm the binary matches the
  source.
- [x] Deletes use the system trash (`trash` crate), not a local `_trash` folder.

Definition of done: the app runs correctly with the network entitlement off,
fuzzing finds no crashes at the decode boundary, and the release binary is
independently reproducible.

## Phase 8: 1.0, the viewer people recommend

Polish, packaging, and distribution so switching costs nothing.

- Native installers and packages: Flatpak and AUR on Linux, a notarized disk image
  on macOS, an installer and Store package on Windows.
- Sensible file-association setup that never hijacks defaults silently.
- Documentation, a static website with no trackers because we practice what we
  preach, and a human-written changelog.
- Accessibility pass: keyboard complete, screen-reader labels, high-contrast check.
- Performance budget locked in and regression-tested in CI: cold start, first
  pixel, and memory all within target.

Definition of done: a non-technical person can install viewr, set it as their
default image viewer, and never think about it again, which is the entire point.

## Beyond 1.0, candidates held to the same bar

Listed so the answer to "will you add X" is "it is tracked and weighed," not
silence, and so that scope creep stays visible and deliberate.

- Optional, local-only, one-click-clearable recent folders.
- Simple non-destructive adjustments (lossless rotate, straighten, basic exposure),
  only if they stay simple and never turn viewr into an editor.
- A simple slideshow.
- Localization.

## Explicit non-goals, the anti-bloat charter

viewr will not add, now or later: accounts, cloud sync, sharing services, ads,
discover or feed surfaces, face or AI grouping, background services, phone-home
update checks, telemetry or analytics of any kind, or a plugin marketplace. These
are the features that turned every big-company photo app into the thing we are
replacing. Leaving them out is a permanent part of the product, not a stage of it.
