# viewr

**A photo viewer that just shows your photos.**

viewr opens an image and gets out of the way. No account. No cloud. No AI-enhanced
memories. No telemetry. No background service. No update nag. It starts image
decoding while the window initializes, lets you flip through a folder,
play animated images, crop with standard or custom output ratios, repair a small
blemish, convert, and delete the junk, and that is the whole
product, on purpose. Cold
start, first-pixel, navigation, settled-idle redraw, and folder-scaling budgets are
regression-tested in CI. Those conservative virtual-runner limits are not presented
as universal launch numbers.

It is built for people who are tired of watching a simple image viewer turn into an
advertising surface with a photos app bolted on. It looks simple, and that simplicity
is the visible result of a great deal of underlying discipline, not the absence of it.

## Principles

These are not marketing lines. Every one of them is a constraint the codebase is
held to, and most are enforced in CI.

1. It just works. Double-click an image and decode begins without waiting for the
   renderer or sibling-folder scan. Arrow keys flip through the folder. Nothing
   needs configuration for the obvious behavior.
2. Maximum privacy, by construction. The shipped dependency graph contains no
   HTTP, TLS, or remote-service client, and CI fails if one enters it. Linux uses
   local D-Bus only for native accessibility: startup rejects non-Unix transports
   and installs a fail-closed kernel policy that denies Internet socket creation
   and io_uring before GUI threads start, including x32 syscall aliases on x86-64.
   Network-denied OS packaging profiles add another boundary when those packages
   are used.
3. Zero logs, zero insights, zero background indexing. No telemetry, no analytics, no crash reporting, no auto-tagging, no usage improvement toggle. There is no opt-out because there is nothing to opt out of. Your filenames, folders, and photos never leave your machine, and viewr will **never** silently alter your files to add metadata or AI inferences.
4. Safe with hostile files. Opening an image means parsing untrusted bytes. The
   pure-Rust core decoders run off the UI thread with strict decoded dimension and
   allocation bounds; SVG also has a pre-parse input cap. Optional C-backed formats
   run from bounded encoded bytes in a resource-limited worker that receives no
   filesystem path; OS packaging profiles supply the whole-application network
   and filesystem boundary.
5. Simple on the surface, uncompromising underneath. It looks simple and does
   exactly what you want with no friction. That simplicity sits on top of rock-solid
   memory safety, a decode sandbox, elite-level testing, broad format coverage, and
   layered privacy. viewr is minimal in surface area and maximal in engineering
   rigor. This is not a lightweight app that does less. It is a deeply engineered app
   that shows you a picture and asks nothing of you.

## What it does, and deliberately does not

Does: open a broad and explicitly documented set of image formats, pan and zoom on
the GPU, flip through a folder without blanking the last good image, play GIF,
WebP, and APNG animation, rotate, crop, repair a small blemish, Save As and convert
with metadata stripped by default, and delete to the system trash with undo. Spot
Heal is deterministic, local, and in-memory. It ranks clean source patches using
color, tone, and edge structure, adapts the selected source to its boundary, and
offers adjustable feather plus Refresh Source (`/`). The edit persists only if
you explicitly save a copy.

Network-denied packages expose both **Open File** and **Open Folder**. Opening a
folder is the explicit, session-only consent path that enables sibling navigation
without granting viewr broad access to a photo library.

Does not, and will not: accounts, cloud sync, sharing services, ads, discover
feeds, face grouping, background indexing, silent AI analysis, automatic or background update checks, telemetry, or a settings screen
with two hundred switches. **We consider background image analysis and silent metadata tagging to be spyware.** If a big-company photos app is famous for it, that is a
strong signal we do not want it.

## Interaction

The image always owns a dedicated viewport. Tools, Folder Previews, and Image
Information are optional docked panels that reserve their own space and never cover
the photo. `T`, `G`, and `I` show or fully hide them; Tools and Folder Previews can
also collapse to quiet disclosure rails. View > Panel Position independently docks
Tools and Image Information on the left or right. Every visibility, collapse, or
position change refits and recenters the image. `J` opens a temporary docked Spot
Heal inspector that also reserves its own space; only its brush mask is drawn over
the photo. Its inspector exposes brush radius, feather, alternate ranked sources,
Undo, and Redo. Image Information contains the
explicit session-only export-metadata choice. View also exposes Fit Image to View
(`0`), Actual Size (`1`), Zoom In (`+`), and Zoom Out (`-`) so zoom never depends on
a mouse or trackpad. The empty, loading, and load-error states use an opaque
high-contrast surface, so they remain readable even when the image background is
white. File > Reload File (`F5`) explicitly bypasses the decoded-neighbor cache
and refreshes the current file from disk while retaining the last good frame until
the replacement is ready. Crop offers Free, Original, 1:1, ten landscape and
portrait photo/video ratios, reversible orientation, numeric custom ratios, eight
pointer handles, and full keyboard operation. The automated native bridge check
and manual three-platform acceptance
matrix live in [`docs/ACCESSIBILITY.md`](docs/ACCESSIBILITY.md).

View > Appearance selects System, Light, Dark, or Console. The complete chrome,
native window decoration, and default image canvas change together. The choice is
remembered as one local word and contains no photo path or activity history. Help
> About viewr opens a keyboard-dismissible modal with build, license, shortcut,
and privacy details.

## Formats

The goal is the VLC of image viewers, while the current claim remains narrower and
testable. The always-on pure-Rust core covers
JPEG, PNG, GIF, WebP, BMP, TIFF, ICO, PNM, TGA, QOI, DDS, HDR, OpenEXR, farbfeld,
JPEG XL, and SVG. GIF, WebP, and APNG animate. AVIF and HEIC/HEIF go through the
feature-gated `viewr-decode` worker, whose V2 protocol preserves bounded ICC/CICP
color evidence or reports an explicit fallback; camera RAW is still deferred.
Multi-page TIFF and ICO currently show one decoded image rather than a page
navigator. Full table:
[`docs/FORMATS.md`](docs/FORMATS.md).

## Stack, short version

- Language: Rust, for memory safety on the exact code that touches untrusted files,
  one compact desktop binary plus an isolated decode helper, no runtime, no GC pauses.
- UI foundation: [winit](https://lib.rs/crates/winit) plus [wgpu](https://wgpu.rs/)
  and an `egui` UI layer. We still own the render pipeline; persistent controls use
  compact, fully hideable docked panels that never cover the image. Tools and Image
  Information can independently dock on either horizontal edge.
- Rendering: our own wgpu pipeline, for GPU-accelerated pan, zoom, and scaling.
  The current image uses an sRGB texture with a generated, trilinear-filtered mip
  chain, and over-limit sources get a bounded aspect-preserving preview while
  export retains the full decoded pixels. Preview preparation is cancellable and
  runs away from the window thread with a linear-light, alpha-correct area filter.
  Embedded RGB ICC profiles are converted into the sRGB working pipeline.
  Per-display output profiles, wide-gamut surfaces, and HDR presentation remain
  explicit roadmap work.
- Decoding: [image-rs](https://github.com/image-rs/image) plus jxl-oxide and
  friends, safe decoders across a wide format set. Async image decoding uses
  bounded replace-latest queues and generation-aware readers, so obsolete work
  stops cooperatively instead of delaying the current selection. This includes
  bounded worker-file reads and blocked worker IPC, which terminate when a newer
  image wins. PNG, WebP, and JPEG XL metadata allocations are checked before
  decoder materialization, with one shared 10 MiB embedded-ICC ceiling. Declared
  still-image and animation output sizes are validated before pixel allocation.
- Deletes: native system trash APIs, recoverable, never a raw delete by default.
  Windows and Linux use the `trash` crate; macOS retains the exact
  `NSFileManager` result URL so in-app Undo can restore the same item safely.
  Undo covers every successful item in the latest single or batch trash action.
- Spot Heal: a bounded pure-Rust worker ranks up to eight spatially distinct
  source patches with robust tone and edge-aware scoring, applies local color
  adaptation and feathered compositing, and falls back to directional inpainting.
  It has no model dependency, source-file write, image cache, or sidecar.
  Pixel-patch undo and redo stay in memory, and only the changed GPU texture
  region is uploaded after each edit.
- Theme and icon: System, Light, Dark, and phosphor-green Console palettes cover
  native decoration, standard widgets, custom controls, typography, and the
  default image canvas. Black, neutral gray, and white remain independent image
  inspection backgrounds. One validated appearance word is the only persistent
  UI preference. A custom SVG/ICO app icon is embedded via `winres`.

Full reasoning, including the alternatives we rejected, is in
[`docs/STACK.md`](docs/STACK.md).

## Quality bar

viewr targets 85 percent or higher line coverage on its testable logic (currently
90.64 percent lines and 81.62 percent functions), clippy at pedantic with warnings
as errors, continuous fuzzing of the decode path, and behavior-level contract tests
that keep the coverage honest.
The full set of engineering standards is in
[`docs/STANDARDS.md`](docs/STANDARDS.md), including the additional quality gates
required before a 1.0 release.

## Platforms

Linux, macOS, and Windows from a single codebase.

## Documentation

- [`docs/ACCESSIBILITY.md`](docs/ACCESSIBILITY.md), native automation and the
  manual Narrator, VoiceOver, and Orca release matrix.
- [`docs/FORMATS.md`](docs/FORMATS.md), which formats are core vs worker-decoded.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), how viewr is structured and how a
  keystroke becomes a pixel.
- [`docs/STACK.md`](docs/STACK.md), every technology choice and why, plus what we
  said no to.
- [`docs/STANDARDS.md`](docs/STANDARDS.md), the engineering standards we hold to.
- [`docs/DESIGN.md`](docs/DESIGN.md), the visual and interaction system.
- [`docs/PRIVACY.md`](docs/PRIVACY.md), the privacy guarantee stated plainly, and
  how the code enforces it.
- [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md), what we measure and the current
  numbers.
- [`docs/LOCAL-INTELLIGENCE.md`](docs/LOCAL-INTELLIGENCE.md), the strict product,
  privacy, runtime, and evaluation gate for any optional local model.
- [`docs/INSTALL.md`](docs/INSTALL.md), how to install on each OS and cut a release.
- [`docs/ROADMAP.md`](docs/ROADMAP.md), the phased plan from first window to 1.0 and
  beyond.

## License

Apache License 2.0. See [`LICENSE`](LICENSE).

Status: **Phases 0 through 5 and Phase 7 are complete for their local repository scope; format depth and 1.0 release acceptance remain in progress**. Pure-Rust core decode (including SVG), bounded animation, core and worker color-profile evidence, the path-free `viewr-decode` boundary, Linux fail-closed process policies, three locally verifiable OS sandbox profiles, checksummed release archives, configurable panel-safe chrome, native AccessKit delivery on Windows, macOS, and Linux, local CLI (`doctor`, `benchmark`, `update`, `help`), opt-in core-format associations, and enforced GUI performance budgets are in place. The named remaining gates are per-display color and HDR work, RAW and multi-page depth, manual target-OS screen-reader validation, hosted multi-OS evidence, and public verifiable distribution. Packaging only makes viewr available as an Open With choice. It never changes a user's default viewer. No public installer or store release exists yet. See `docs/ROADMAP.md`, `docs/FORMATS.md`, `docs/PERFORMANCE.md`, and `docs/INSTALL.md`.

```
cargo run --release -- path/to/image.png
cargo run --release -- doctor
cargo run --release -- help
```
