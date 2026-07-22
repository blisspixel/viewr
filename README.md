# viewr

**A photo viewer that just shows your photos.**

viewr opens an image and gets out of the way. No account. No cloud. No AI-enhanced
memories. No telemetry. No background service. No update nag. It starts image
decoding while the window initializes, lets you flip through a folder,
crop, convert, and delete the junk, and that is the whole product, on purpose. Cold
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
3. Zero logs, zero insights. No telemetry, no analytics, no crash reporting, no
   usage improvement toggle. There is no opt-out because there is nothing to opt out
   of. Your filenames, folders, and photos never leave your machine.
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

Does: open essentially any image format you have (see below), pan and zoom on the
GPU, flip through a folder, rotate, crop, Save As and convert with metadata stripped
by default, and delete to the system trash with undo.

Network-denied packages expose both **Open File** and **Open Folder**. Opening a
folder is the explicit, session-only consent path that enables sibling navigation
without granting viewr broad access to a photo library.

Does not, and will not: accounts, cloud sync, sharing services, ads, discover
feeds, face grouping, automatic or background update checks, telemetry, or a settings screen
with two hundred switches. If a big-company photos app is famous for it, that is a
strong signal we do not want it.

## Interaction

The image always owns a dedicated viewport. Tools, Folder Previews, and Image
Information are optional docked panels that reserve their own space and never cover
the photo. `T`, `G`, and `I` show or fully hide them; Tools and Folder Previews can
also collapse to quiet disclosure rails. View > Panel Position independently docks
Tools and Image Information on the left or right. Every visibility, collapse, or
position change refits and recenters the image. Image Information contains the
explicit session-only export-metadata choice. View also exposes Fit Image to View
(`0`), Actual Size (`1`), Zoom In (`+`), and Zoom Out (`-`) so zoom never depends on
a mouse or trackpad. The empty, loading, and load-error states use an opaque
high-contrast surface, so they remain readable even when the image background is
white.

## Formats

The goal is the VLC of image viewers: if it is an image, viewr opens it, and you
never think about which app handles which file. The always-on pure-Rust core covers
JPEG, PNG, GIF, WebP, BMP, TIFF, ICO, PNM, TGA, QOI, DDS, HDR, OpenEXR, farbfeld,
JPEG XL, and SVG. AVIF/HEIC/HEIF and camera RAW go through the `viewr-decode`
worker (feature-gated C backends; RAW deferred). Full table:
[`docs/FORMATS.md`](docs/FORMATS.md).

## Stack, short version

- Language: Rust, for memory safety on the exact code that touches untrusted files,
  one compact desktop binary plus an isolated decode helper, no runtime, no GC pauses.
- UI foundation: [winit](https://lib.rs/crates/winit) plus [wgpu](https://wgpu.rs/)
  and an `egui` UI layer. We still own the render pipeline; persistent controls use
  compact, fully hideable docked panels that never cover the image. Tools and Image
  Information can independently dock on either horizontal edge.
- Rendering: our own wgpu pipeline, for GPU-accelerated pan, zoom, and scaling with
  control over resampling quality and color.
- Decoding: [image-rs](https://github.com/image-rs/image) plus jxl-oxide and
  friends, safe decoders across a wide format set. Async image decoding runs on a
  background thread (using `std::sync::mpsc`) so navigation stays perfectly snappy.
- Deletes: native system trash APIs, recoverable, never a raw delete by default.
  Windows and Linux use the `trash` crate; macOS retains the exact
  `NSFileManager` result URL so in-app Undo can restore the same item safely.
  Undo covers every successful item in the latest single or batch trash action.
- Theme and icon: the image background follows the operating-system theme unless
  the user selects black, neutral gray, or white. Chrome stays neutral dark for
  stable contrast around every photo. A custom SVG/ICO app icon is embedded via
  `winres`.

Full reasoning, including the alternatives we rejected, is in
[`docs/STACK.md`](docs/STACK.md).

## Quality bar

viewr targets 85 percent or higher test coverage on its logic (currently 89.04
percent), clippy at pedantic with warnings as errors, continuous fuzzing of the
decode path, and behavior-level contract tests that keep the coverage honest.
The full set of engineering standards is in
[`docs/STANDARDS.md`](docs/STANDARDS.md), including the additional quality gates
required before a 1.0 release.

## Platforms

Linux, macOS, and Windows from a single codebase.

## Documentation

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
- [`docs/INSTALL.md`](docs/INSTALL.md), how to install on each OS and cut a release.
- [`docs/ROADMAP.md`](docs/ROADMAP.md), the phased plan from first window to 1.0 and
  beyond.

## License

Apache License 2.0. See [`LICENSE`](LICENSE).

Status: **Phase 7 hardening and privacy proof is complete; Phase 8 polish is in progress**. Pure-Rust core decode (including SVG), the path-free `viewr-decode` boundary, Linux fail-closed process policies, three locally verifiable OS sandbox profiles, checksummed release archives, configurable panel-safe chrome, native AccessKit delivery on Windows, macOS, and Linux, local CLI (`doctor`, `benchmark`, `update`, `help`), opt-in core-format associations, and enforced GUI performance budgets are in place. Manual target-OS screen-reader validation remains before Phase 8 is complete. Packaging only makes viewr available as an Open With choice. It never changes a user's default viewer. No public installer or store release exists yet. See `docs/FORMATS.md`, `docs/PERFORMANCE.md`, and `docs/INSTALL.md`.

```
cargo run --release -- path/to/image.png
cargo run --release -- doctor
cargo run --release -- help
```
