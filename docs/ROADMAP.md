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

Phases 0 through 5 and Phase 7 are complete for their local repository scope.
Phase 6 has broad core-format coverage, isolated optional AVIF/HEIC decoding, and
honest capability reporting, but its original definition is not complete while
camera RAW and multi-page viewing remain absent. Phase 8 has local install paths,
file associations, accessibility automation, native AccessKit delivery, and
enforced GUI performance budgets. It is not complete until the manual
three-platform assistive-technology matrix, hosted multi-OS evidence, display
fidelity, and a public verifiable release are complete.

The current viewer also has bounded GIF/WebP/APNG playback, eight-way EXIF
orientation, input RGB ICC conversion to sRGB, a trilinear GPU mip chain,
GPU-limited previews that retain full-resolution export, last-good-frame
navigation, asynchronous crop and Save As, image information, manual disk reload,
the refined Spot Heal workflow, a functional accessible About modal, and complete
System, Light, Dark, and Console appearances. The appearance choice is the only
persistent UI preference and contains no image or activity data.

**Next code focus: display fidelity. Next release focus: target-OS validation and
public verifiable artifacts.** Optional model-backed description remains a gated
post-1.0 candidate, not active Phase 8 scope.

## What keeps viewr from exceptional

This is a bounded product plan, not a request to copy every feature from a larger
viewer. The research signal is consistent:

- Minimal qView treats fast preloading and animation as baseline, while recent
  releases added Reload File and fixed embedded-profile, CMYK, and per-display ICC
  failures. Its public downloads cover a Windows installer, macOS disk image,
  AppImage, Flatpak, and native repositories. See the official
  [feature page](https://interversehq.com/qview/),
  [changelog](https://interversehq.com/qview/changelog/), and
  [downloads](https://interversehq.com/qview/download/).
- ImageGlass treats live file-change refresh, multi-frame navigation, animation,
  color management, thumbnails, and touch input as viewer capabilities rather
  than editor bloat. See its official
  [feature matrix](https://imageglass.org/docs/features).
- nomacs demonstrates the remaining format-depth bar with optional RAW and
  multi-page TIFF support. See its official
  [repository and build options](https://github.com/nomacs/nomacs).

viewr already has a stronger privacy and hostile-input story than those references.
What is missing is not another toolbar. It is end-to-end fidelity, complete edge
behavior, installability, and maintainable proof of correctness.

### Priority 1: color that is correct on the actual display

Why first: a viewer that renders the wrong color is failing its primary job, even
when it is fast. The current RGB ICC-to-sRGB normalization prevents the most common
embedded-profile error, but an RGBA8 sRGB working path cannot preserve wide-gamut
or HDR source values, and the output is not transformed for the monitor that owns
the window. Apple exposes display profiles and transforms through
[ColorSync](https://developer.apple.com/documentation/colorsync), while Windows
exposes device profile associations and transforms through the
[Windows Color System](https://learn.microsoft.com/en-us/windows/win32/api/_wcs/).
wgpu 30 adds explicit surface color spaces and display HDR information; viewr is
currently on wgpu 29. See the official
[`SurfaceConfiguration`](https://docs.rs/wgpu/latest/wgpu/type.SurfaceConfiguration.html)
and [`Surface`](https://docs.rs/wgpu/latest/wgpu/struct.Surface.html) APIs.

- [x] Read bounded embedded RGB ICC data and convert it into the current sRGB
  path, including animated frames, with an explicit fallback status.
- [x] Generate the full GPU mip chain in the sRGB texture pipeline so minification
  is stable and linear-light filtered.
- [ ] Carry trustworthy color metadata through the optional worker protocol rather
  than silently treating AVIF/HEIC output as untagged sRGB.
- [ ] Separate source pixels, working color space, and output transform so future
  wide-gamut values are not clipped by the current RGBA8 sRGB intermediate.
- [ ] Upgrade the wgpu/egui-wgpu integration only after a focused compatibility
  spike proves surface color-space and HDR behavior on all three backends.
- [ ] Resolve and refresh the profile for the display that currently contains the
  window, including a move between differently profiled monitors.
- [ ] Add CMYK/profile fallback fixtures plus sRGB, Display P3, and Adobe RGB
  reference-vector tests. Keep a deterministic sRGB fallback when platform profile
  information is unavailable.
- [ ] Enable wide-gamut and HDR presentation only after a higher-precision working
  path, tone mapping, capability checks, and real-display acceptance tests exist.

Definition of done: tagged SDR images match reference conversions, moving the
window between profiled displays updates output without a restart, worker-decoded
images never lose color status silently, and HDR or wide-gamut modes cannot engage
without an end-to-end higher-precision path.

### Priority 2: file and format coherence

Why second: image viewers commonly sit beside editors, exporters, scanners, and
download tools. A stale view or a container that exposes only its first page makes
the application feel unreliable even when the decoder technically succeeded.

- [x] Keep the last good image visible during a cache miss or failed replacement.
- [x] Add File > Reload File (`F5`) with cache bypass and no blank frame.
- [ ] Add a session-scoped file watcher for the current image and folder. Coalesce
  noisy events, preserve the old frame until a successful refresh, update the
  playlist deterministically, and write no history or database.
- [ ] Add first-class frame/page navigation for multi-page TIFF and ICO, reusing
  the bounded animation/page model without auto-playing documents.
- [ ] Ship camera RAW only through the path-free bounded worker, with orientation,
  color metadata, representative camera fixtures, fuzz seeds, and the same memory
  and deadline contracts as AVIF/HEIC.
- [ ] Decide clipboard open/copy and touch gestures from measured user workflows,
  not from feature-count pressure. They remain behind the work above.

Definition of done: external edits appear predictably, every selected page/frame
is identifiable and bounded, and the format table distinguishes container support
from page, animation, metadata, and color behavior.

### Priority 3: a release people can actually trust and install

Why third: local build scripts prove engineering intent, but a viewer cannot become
recommendable while ordinary users cannot obtain a verified build. This work also
closes the gap between repository claims and hosted evidence.

- [ ] Run the complete hosted Linux, macOS, and Windows workflow for one pinned
  commit and retain links to every green job and generated checksum.
- [ ] Complete Narrator, VoiceOver, and Orca acceptance using
  `docs/ACCESSIBILITY.md`, including crop, reload, animation, errors, and busy
  states.
- [ ] Publish checksummed dual-binary archives from the green commit with a human
  changelog, SBOM/provenance where the release platform supports it, and clear
  optional file-association instructions.
- [ ] Produce and locally verify a normal Windows installer, macOS disk image, and
  Linux AppImage or Flatpak. Sign or notarize those artifacts when external
  credentials are available. Store publication remains optional; trustworthy
  direct installation does not.
- [ ] Repeat cold-launch, animation, large-image, mixed-DPI, and profiled-monitor
  smoke tests on representative hardware for all three platforms.

Definition of done: a user can download, verify, install, exercise, and remove
viewr without compiling it, changing defaults silently, or trusting an unrecorded
manual build.

### Priority 4: make correctness easier to preserve

Why now: `app.rs` and `ui.rs` own too many independent state transitions, and the
coverage gate currently excludes most native orchestration. The behavior is tested
in many focused helpers, but future race and accessibility work will get harder if
load, edit, and dock state remain concentrated in two large files.

- [ ] Extract pure crop/output geometry and its keyboard/pointer transitions into
  a covered module.
- [ ] Extract a session/load state machine that owns selected path, presented path,
  generations, retry/reload, and stale-result rejection.
- [ ] Extract bounded job coordination for image details, animation, crop, save,
  thumbnails, and prefetch, leaving `App` responsible for platform events.
- [ ] Move dock/menu view models out of paint code so enablement and accessibility
  state can be exhaustively tested without a window.
- [ ] Narrow the coverage exclusion as each seam becomes pure. Keep logic coverage
  above 85 percent and add race-contract tests before deleting old paths.

Definition of done: important state transitions have one owner and one pure test
surface, native glue is thin, and a late worker result cannot mutate a newer image,
edit, or panel state.

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
- Animated GIF, WebP, and APNG playback with bounded frames, correct frame timing,
  pause/resume, and container loop behavior.

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
  alternatives.
- [x] Complete System, Light, Dark, and Console appearances covering native
  decoration, GPU canvas, standard widgets, custom controls, overlays, and
  typography. All resolved palettes have automated AA contrast checks and the
  one-word selection persists locally.
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

- [x] Crop with a GPU preview, eight pointer handles, keyboard movement/resizing,
  output-oriented Free, Original, 1:1, 3:2, 2:3, 4:3, 3:4, 5:4, 4:5, 5:3,
  3:5, 16:9, and 9:16 presets, reversible orientation, numeric custom ratios,
  exact dimensions, and direct full-resolution application.
- [x] Focused Spot Heal for small blemishes: sparse image-space brush input,
  bounded deterministic edge-aware ranking of up to eight distinct sources off
  the UI thread, robust boundary tone adaptation, adjustable feathering,
  directional fallback inpainting, Refresh Source (`/`), in-memory undo/redo,
  bounded GPU texture-region updates, and a temporary docked inspector that never
  covers the photo. It adds no model or native dependency, refuses ambiguous
  GPU-clamped source mappings, and never changes the source file.
- [x] Save As and convert between supported output formats off the UI thread,
  applying the visible rotation and flips exactly.
- [x] Metadata strip on export, presented prominently, with location and
  identifying fields stripped by default. Explicit session-only retention
  normalizes orientation, dimensions, and stale thumbnail offsets while retaining
  descriptive, camera, and GPS tags.

Definition of done: a user can crop or spot-heal an image, export it to another
format, and be confident their location data did not ride along, with tests over
the edit, undo/redo, export, and metadata-strip paths.

### Spot Heal quality residuals

The current refinement follows the size, feather, and resample controls in the
official [Adobe Lightroom Heal documentation](https://helpx.adobe.com/lightroom/desktop/using/heal-tool.html)
and uses a bounded deterministic candidate set rather than a global synthesis
pass. The research basis for later work is the primary
[PatchMatch paper](https://gfx.cs.princeton.edu/pubs/Barnes_2009_PAR/index.php),
[exemplar-based structure propagation](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/criminisi_tip2004.pdf),
and [Poisson image editing](https://legacy.sites.fas.harvard.edu/~cs278/papers/poisson.pdf).

- [x] Add defect fixtures for edge agreement, tone-shift seam reduction, ranked
  source determinism and wrapping, zero feather, and directional ramp
  continuation.
- [x] Expose adjustable feather and deterministic alternate-source refresh while
  preserving one undo step for the repair.
- [ ] Add an explicit manual source anchor only after pointer and keyboard
  interaction can expose its source-to-target relationship accessibly.
- [ ] Add a high-contrast Visualize Spots inspection mode only with real dust and
  low-contrast blemish fixtures that prove it improves discovery without changing
  pixels.
- [ ] Build a licensed small-repair corpus with hidden clean references and gate
  seam error, edge continuity, defect removal, latency, and peak memory. Do this
  before considering multi-patch synthesis or a gradient-domain blend.

Why these remain after display fidelity and release proof: automatic healing is
already useful and bounded, while manual sourcing and inspection add interaction
surface. They should land only when objective fixtures prove a quality gain and
the controls work equally with pointer, keyboard, and assistive technology.

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
- [x] RAW currently returns a stable, documented unsupported error instead of a
  false success claim.
- [ ] Implement and ship representative camera RAW families through the isolated
  worker.
- [ ] Add multi-page TIFF and ICO navigation instead of exposing only one decoded
  image.
- [ ] Carry worker color metadata into the main process and test optional release
  builds as complete viewing pipelines.
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
  - [x] Two-slot foreground-priority file-decode gate, generation cancellation
    across core reads, worker reads, and blocked worker IPC, plus exact
    source/pixel state matching for path-sensitive actions.
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
- [ ] Display-fidelity acceptance from Priority 1: worker color metadata,
  per-display output, reference-profile fixtures, and honest wide-gamut/HDR gates.
- [ ] Public, checksummed artifacts from a recorded green multi-OS workflow, with
  native install surfaces once external signing credentials are available.

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
- Optional **Describe Image** after the offline bake-off and process-level privacy
  proof in `docs/LOCAL-INTELLIGENCE.md` pass on Windows, Linux, and macOS. It must
  be absent without a separately installed model pack, run **only** on explicit
  manual activation, receive decoded pixels rather than a source path, retain no
  result after navigation, and produce no app-owned logs or files. **Under no
  circumstances will it write descriptions to the file's EXIF data or a
  background database.** Built-in speech and model-assisted large-area removal
  remain separate later decisions.

## Explicit non-goals, the anti-bloat charter

viewr will not add, now or later: accounts, cloud sync, sharing services, ads,
discover or feed surfaces, face or AI grouping, background services, automatic or
background update checks, telemetry or analytics of any kind, or a plugin marketplace. These
are the features that turned every big-company photo app into the thing we are
replacing. Leaving them out is a permanent part of the product, not a stage of it.

An explicit one-image local model action does not relax this charter. Optional
models may not become a library scanner, automatic classifier, required runtime,
background process, download client, or reason to retain user data. **Adding
generated metadata to a user's files without explicit intent is spyware and is
an absolute non-starter.**
