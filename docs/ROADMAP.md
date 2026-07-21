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

Phases 0–5 are complete for product behavior. Phase 6 is **mostly complete** with known residuals before calling it done:

- Pure-Rust core formats (image-rs family + JPEG XL + SVG via resvg) decode and are covered by corpus/unit tests.
- `viewr-decode` exists as a side crate with AVIF/HEIC paths and SHM IPC; it is **not yet a workspace member**, and RAW remains a deliberate stub.
- System trash (`trash` crate), continuous fuzz CI, and OS-level worker sandboxing are **not** Phase 6 residuals so much as Phase 7 / hardening work—except workspace integration of the worker, which should land before packaging.

**Next focus: finish Phase 6 residuals (workspace-integrate `viewr-decode`, register delegated extensions in `fs`, complete RAW or document deferral), then Phase 7.**
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

- Delete to the system trash via the trash crate, with a non-blocking Undo toast
  and index preservation, so the view lands on the image that replaced the deleted
  one rather than jumping to the top.
- Undo (Ctrl+Z) restores the last deleted file from the trash.
- Flag-then-batch cull mode: flag images while browsing, then delete all flagged at
  once, still to trash and still undoable. This is the default recommended flow.
- Shift+Delete performs a permanent delete and is the only action that asks for
  confirmation.

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
- [ ] Add `viewr-decode` as a workspace member with feature-gated C deps.
- [ ] List AVIF/HEIC (and RAW when ready) in `fs` supported extensions when the worker is shipped beside the main binary.
- [ ] Finish RAW decode in the worker or document explicit deferral.
- [ ] Honest format capability table in docs (core vs worker).

## Phase 7: Hardening and the privacy proof

Turn "we designed it to be private and safe" into something a third party can
verify.

- Sandbox packaging on all three platforms with the network denied: macOS App
  Sandbox, Windows AppContainer, and Linux Flatpak with no network share.
- The isolated decode worker fully in place, with seccomp on Linux and reduced
  privileges elsewhere.
- Continuous fuzzing of every decoder, with any crash a release blocker.
- Reproducible and signed builds, so a user can confirm the binary matches the
  source.

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
