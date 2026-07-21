# viewr

**A photo viewer that just shows your photos.**

viewr opens an image and gets out of the way. No account. No cloud. No AI-enhanced
memories. No telemetry. No background service. No update nag. It launches instantly,
shows the picture, lets you flip through a folder, crop, convert, and delete the
junk, and that is the whole product, on purpose.

It is built for people who are tired of watching a simple image viewer turn into an
advertising surface with a photos app bolted on. It looks simple, and that simplicity
is the visible result of a great deal of underlying discipline, not the absence of it.

## Principles

These are not marketing lines. Every one of them is a constraint the codebase is
held to, and most are enforced in CI.

1. It just works. Double-click an image and it is on screen before you finish
   letting go of the mouse. Arrow keys flip through the folder. Nothing to
   configure to get the obvious behavior.
2. Maximum privacy, by construction. viewr contains no networking code. It cannot
   phone home because there is nothing in it that can open a socket, and CI fails
   the build if a network-capable dependency ever sneaks in. It ships sandboxed
   with the network denied, so even a compromised build cannot leak.
3. Zero logs, zero insights. No telemetry, no analytics, no crash reporting, no
   usage improvement toggle. There is no opt-out because there is nothing to opt out
   of. Your filenames, folders, and photos never leave your machine.
4. Safe with hostile files. Opening an image means parsing an untrusted file, which
   is historically the largest source of remote code-execution bugs. viewr is
   written in Rust and decodes images in a locked-down sandbox, so a booby-trapped
   file has nowhere to go.
5. Simple on the surface, uncompromising underneath. It looks simple and does
   exactly what you want with no friction. That simplicity sits on top of rock-solid
   memory safety, a decode sandbox, elite-level testing, broad format coverage, and
   layered privacy. viewr is minimal in surface area and maximal in engineering
   rigor. This is not a lightweight app that does less. It is a deeply engineered app
   that shows you a picture and asks nothing of you.

## What it does, and deliberately does not

Does: open essentially any image format you have (see below), pan and zoom on the
GPU, flip through a folder instantly, rotate, crop, Save As and convert with an
optional metadata strip, and delete to the system trash with undo.

Does not, and will not: accounts, cloud sync, sharing services, ads, discover
feeds, face grouping, phone-home update checks, telemetry, or a settings screen
with two hundred switches. If a big-company photos app is famous for it, that is a
strong signal we do not want it.

## Formats

The goal is the VLC of image viewers: if it is an image, viewr opens it, and you
never think about which app handles which file. Target coverage includes JPEG, PNG,
GIF (animated), WebP (animated), BMP, TIFF, ICO, PNM, TGA, QOI, DDS, HDR, OpenEXR,
farbfeld, JPEG XL (via `jxl-oxide`), and SVG (via `resvg`/`usvg`).
Formats that lack a safe pure-Rust decoder, or that require complex C dependencies
like AVIF, HEIC, HEIF, and camera RAW, will be supported next, decoded inside a
sandboxed worker rather than linked into the main process. Format support is built
up in a defined order, see [`docs/ROADMAP.md`](docs/ROADMAP.md).

## Stack, short version

- Language: Rust, for memory safety on the exact code that touches untrusted files,
  one small native binary, no runtime, no GC pauses.
- UI foundation: [winit](https://lib.rs/crates/winit) plus [wgpu](https://wgpu.rs/)
  and an `egui` UI overlay. We still own the render pipeline but use `egui` for a slick left-aligned floating toolbar.
- Rendering: our own wgpu pipeline, for GPU-accelerated pan, zoom, and scaling with
  control over resampling quality and color.
- Decoding: [image-rs](https://github.com/image-rs/image) plus jxl-oxide and
  friends, safe decoders across a wide format set. Async image decoding runs on a
  background thread (using `std::sync::mpsc`) so navigation stays perfectly snappy.
- Deletes: the trash crate, recoverable, never a raw delete by default.
- Theme & Icon: [dark-light](https://docs.rs/dark-light) follows the operating system, and a custom SVG/ICO app icon is embedded via `winres`.

Full reasoning, including the alternatives we rejected, is in
[`docs/STACK.md`](docs/STACK.md).

## Quality bar

viewr targets 85 percent or higher test coverage on its logic (currently above 95
percent), clippy at pedantic with warnings as errors, continuous fuzzing of the
decode path, and mutation testing to keep the coverage honest. The full set of
engineering standards is in [`docs/STANDARDS.md`](docs/STANDARDS.md), including how
we guard against AI slop.

## Platforms

Linux, macOS, and Windows from a single codebase.

## Documentation

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

Status: **Phase 6 largely shipped; residual gaps tracked in ROADMAP**. viewr is a fast, zero-bloat image viewer with pure-Rust decode for the core format set (including SVG via `resvg`) and a `viewr-decode` worker path for C-backed formats (AVIF/HEIC; RAW still stubbed). The worker is not yet a first-class workspace member, and OS-level sandbox packaging is Phase 7. Try it with `cargo run --release -- path/to/image.png`.
