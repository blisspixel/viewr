# Stack & Technology Decisions

This document records *what* viewr is built on and *why*, including the options we
evaluated and rejected. It's written as a set of decisions so that a future
contributor can see the reasoning, not just the result.

## Decision 1: Language: Rust

**Chosen: Rust.**

The single most dangerous thing a photo viewer does is parse an untrusted file.
Image decoders (JPEG, PNG, GIF, WebP, …) are historically the largest source of
remote-code-execution vulnerabilities across every operating system. A malformed
file overflows a buffer and the attacker is running code. Rust eliminates that
entire bug class at compile time. For an app whose headline promise is *"safe and
private,"* the language's core guarantee **is** the product requirement.

Secondary wins that also matter:

- Compiles to a single small native binary: no runtime, no interpreter, no GC.
- No garbage-collector pauses, so panning and folder-flipping stay smooth.
- Excellent cross-compilation to Linux, macOS, and Windows.
- A mature ecosystem of *pure-Rust* image and GUI crates, so the safety story
  holds end to end rather than stopping at an FFI boundary.

**Rejected:**

- **C++ / Qt**: the most mature native GUI toolkit in existence, but no memory
  safety (wrong for our threat model) and licensing friction. This is what the
  *old* good viewers used; it's exactly the surface we want to avoid.
- **Swift / SwiftUI + WinUI + GTK**: the most genuinely native per-OS feel, but
  three codebases. The opposite of small and maintainable.
- **Go**: great tooling and easy static binaries, but the weakest desktop-GUI
  ecosystem of the serious options; no path to the fit-and-finish we require.
- **Dart / Flutter**: the strongest runner-up for *polish*: GPU-rendered,
  pixel-identical everywhere, fast to build. Rejected because (a) it paints its
  own UI rather than being truly native and ships a heavier binary, and (b) Dart
  is memory-*managed* but decoding still drops into native libraries, so the exact
  CVE surface we most want to close stays open.
- **Mojo / Slang / Triton / Taichi / Bend / MoonBit / Gleam / Hylo**: a category
  error for this project. The first group are GPU-kernel / shader languages: they
  make GPU cores fast, they do not open windows, read folders, or draw UI. The
  rest are application languages aimed at servers, WebAssembly, or research, and
  several are pre-1.0. None targets trustworthy native desktop GUI. viewr does not
  hand-write GPU kernels. The GUI framework's renderer handles pan/zoom.

## Decision 2: UI foundation: winit + wgpu + egui

**Chosen: [winit](https://lib.rs/crates/winit) for windowing and input, plus
[wgpu](https://wgpu.rs/) for rendering, with our own render pipeline and an `egui` overlay for chrome.**

viewr is not a forms application with an image in it. It is a GPU image canvas with
a small amount of chrome (menus, docked tools, toasts, a crop border, and optional folder previews). The
parts that have to be exceptional (decode-to-texture upload, high-quality
resampling, pan and zoom latency, flat memory under a very large folder, and color
management) are exactly the parts a general GUI framework abstracts away and
handles in a generic, good-enough manner. To be genuinely the best rather than
merely fine, those must be ours to control down to the frame. So we own the
pipeline instead of inheriting one.

- **winit** is the de-facto standard for windowing and input, and is what Iced,
  egui, and the other frameworks sit on top of anyway. Using it directly removes a
  layer rather than adding one.
- **wgpu** gives us the GPU, and we write our own pipeline for the image, so
  texture upload, mipmaps, resampling quality, and color are decisions we make.
- **egui** provides immediate-mode desktop chrome for the docked menu, tools,
  folder previews, Image Information panel, crop controls, and dialogs. Optional
  panels are fully hideable; tools and information independently dock left or
  right; every visible panel reserves image-viewport space instead of floating.
- **AccessKit** nodes describe custom controls and crop state. A direct,
  default-feature-disabled `accesskit_winit` adapter delivers them to native
  assistive technology on all three targets. On Linux, cargo-deny confines the
  generic D-Bus implementation to the AccessKit/AT-SPI path and viewr's reviewed
  OpenURI Open With chooser, environment validation
  accepts only Unix transports, and startup seccomp denies Internet socket creation
  before the adapter or application threads begin.

This yields the smallest, most auditable dependency tree, which directly serves the
trust and privacy goals, and total control over the details that separate
exceptional from adequate.

**Rejected:**

- **Iced**: pure-Rust, GPU-rendered, MIT (composes fine with our license), and the
  fastest path to a polished result. Rejected not on licensing but on control and
  dependency weight: it abstracts the exact render and memory details we most need
  to own, and pulls a large tree. The decision was explicit: we are optimizing for
  the best possible outcome, not for the least effort.
- **Slint**: polished and follows the system theme for free, but dual-licensed
  GPL-or-commercial, incompatible with our permissive intent, and still a framework
  layer over the pipeline.
- **GTK-rs / Qt bindings**: heavier native dependencies and a less clean Rust-first
  story.
- **Tauri / any WebView**: the bloat we exist to replace. Non-starter.

**Cost we accept:** building our own chrome is more work than adopting a framework,
and the risk is spending effort on UI plumbing instead of the viewer. We accept it
deliberately because viewr's UI surface is genuinely small and tightly scoped, and
because control over the canvas is where "exceptional" is won. We still study the
frameworks we rejected (Iced's message architecture, egui's immediate-mode chrome)
for ideas; we just do not depend on them.

## Decision 3: Rendering: our own wgpu pipeline

We render through **`wgpu`** directly, giving GPU-accelerated scaling, panning, and
zooming with a pipeline we control. Large (4K+) images stay smooth by uploading
decoded frames to GPU textures once and reusing them, rather than re-uploading on
every redraw. This is the difference between "flips through a folder instantly" and
"stutters on big files." Owning the pipeline is also what lets us get resampling
quality and color management right rather than accepting a framework's defaults.
See `ARCHITECTURE.md` for the texture-cache strategy.

The shipped SDR path uses an sRGB image texture, generates every mip level on the
GPU after upload or a pixel patch, and samples trilinearly. Sources larger than the
adapter texture or pixel limit receive a bounded preview prepared on a cancellable
replace-latest background worker. The preview uses a linear-light, alpha-correct
area filter and fallible output allocation, while full decoded pixels remain
available to export. Embedded RGB ICC profiles are normalized into sRGB before
upload. Source pixels, the normalized working encoding, and the renderer-owned
output transform are distinct contracts. The renderer currently accepts only
RGBA8 sRGB working pixels and an sRGB presentation surface. Unmanaged
Windows-legacy and real X11 convert those working pixels into the admitted
display ICC at upload. That is a strong SDR baseline, not a completed
wide-gamut or HDR pipeline; higher-precision working pixels remain in
`ROADMAP.md`.

A note on GPU compute languages (CUDA, Mojo, and similar): they are not used and
not needed. Displaying an image is drawing one textured rectangle, which a small
wgpu shader does. GPU compute kernels are for matrix math and simulation, not for a
viewer. wgpu already provides cross-vendor hardware acceleration (Metal on macOS,
Vulkan and D3D12 on Windows, Vulkan and OpenGL on Linux) across AMD, Intel, Apple,
and Nvidia, in pure Rust.

## Decision 4: Image decoding: image-rs (+ jxl-oxide), toward every format

**Goal:** the VLC of image viewers. If it is an image, viewr opens it, and the user
never has to think about which app handles which file. See ROADMAP.md Phase 6 for
the order in which formats are added.

**Chosen:** the [`image`](https://github.com/image-rs/image) crate as the primary
decoder, covering JPEG, PNG, GIF (including animation), WebP (including animation),
BMP, TIFF, ICO, PNM, TGA, QOI, DDS, HDR, OpenEXR, and farbfeld out of the box.
**[`jxl-oxide`](https://lib.rs/crates/jxl-oxide)** adds JPEG XL, and
**[`resvg`](https://lib.rs/crates/resvg)** (pure Rust) renders SVG. These core
decoders are pure Rust. AVIF is a separate optional C-backed worker feature in the
current implementation.

**Formats that need care** (AVIF, HEIC/HEIF, and camera RAW) are routed to a
resource-limited worker rather than linked into the main process. AVIF and HEIC
are feature-gated; RAW remains explicitly deferred. Every format must ship with
golden-file decode tests and enter the fuzz corpus before it is claimed complete
(see STANDARDS.md).

## Decision 5: Complete local appearance themes

**Chosen:** use winit's native window theme signal, already part of the event
loop, for System and native decoration. View > Appearance also offers explicit
Light, Dark, and Console choices. Each resolved mode supplies complete tokens for
the GPU canvas, standard widgets, custom-painted controls, focus, overlays, and
text. Console switches interface type to monospace. Black, neutral gray, and white
remain independent image-inspection background overrides.

The explicit choice is persisted as one validated lower-case word in the platform
configuration directory. Reads are capped at 32 bytes. Saves sync a complete
same-directory temporary file before atomic replacement. Missing state quietly
uses System; invalid, oversized, unreadable, or unavailable state uses System and
produces one path-free recovery status plus a fixed opt-in diagnostic category.
If an explicit save fails, the selected mode remains active for the session,
the interface gives fixed recovery guidance, and opt-in diagnostics expose only
the failed persistence phase. Startup never rewrites unusable state. This adds
no settings framework, serialization dependency, photo history, or background
service.

## Decision 6: Deletes: the `trash` crate + sandbox

**Chosen:** the [`trash`](https://lib.rs/crates/trash) crate. Deleting the current
image moves it to the **OS trash / recycle bin**, never a raw unlink. viewr is a
curation tool, not a shredder; mistakes must be recoverable. A deliberate,
separate gesture (Shift+Delete with confirmation) offers permanent deletion for
users who explicitly want it.

## Decision 7: Security posture: sandboxed, no network

- **No remote-service client is linked.** The application implements no HTTP,
  TLS, telemetry, or update client. Linux local accessibility IPC uses the generic
  D-Bus code supplied upstream, restricted to the AccessKit dependency path and
  the reviewed OpenURI Open With chooser, and
  protected by a verified Internet-socket deny policy before threads start. This
  layered invariant is enforced in CI and at runtime (see `PRIVACY.md`).
- **Ship sandboxed with network denied:** repository profiles target macOS App
  Sandbox, Windows AppContainer, and Linux Flatpak without `--share=network`.
  Local package construction and schema/profile verification are implemented;
  bare Cargo builds do not inherit the package boundaries.
- **Split decoding by risk.** Pure-Rust formats decode in-process with resource
  and concurrency limits. Optional C-backed formats use a bounded worker; Linux
  also denies its classic and io_uring networking syscalls. Filesystem narrowing
  comes from the enclosing package profile.

## Decision 8: accepted-source integrity: retained handles plus native evidence

**Chosen:** decode, inspection, export, rating, and destructive actions consume a
retained file handle and validate native identity plus version evidence. Unix
version evidence includes inode change time. Windows adds a streaming SHA-256
content witness through the operating system CNG implementation because a
same-length rewrite can preserve every writable timestamp and a retained handle
can report cached metadata. The digest exists only inside the live source object,
is never logged or persisted, and is not exposed as an image identifier.

This uses the existing `windows-sys` boundary rather than adding a hashing crate.
It uses fixed 64 KiB streaming reads without buffering the image, keeps the
default dependency graph unchanged, and makes detected mutation fail closed. The
shared 512 MiB encoded-input ceiling is enforced before hashing. Decode-generation
cancellation is checked between chunks for both initial and comparison witnesses.
Full comparison witnesses run only on owned background work. UI-only destination
consent retains a capability that exposes native identity and version but no
reader or digest. Folder-rating discovery uses another distinct read-only source
instead of hashing full files: it verifies native identity/version around two
bounded 16 MiB header reads, requires their exact consumed bytes and parsed results
to match, polls cancellation between segments, and cannot authorize writes. The
exact optimized Windows GUI remains subject to the
startup, first-pixel, navigation, idle, cache, and large-folder performance gate.

## Supporting crates (shipped)

| Need | Crate | Note |
|---|---|---|
| Windowing + input | `winit` | de-facto standard, minimal |
| GPU rendering | `wgpu` | our own pipeline on top |
| Text rendering | `egui` | immediate mode GUI overlay |
| Accessibility | `accesskit` / `accesskit_winit` | semantic tree and native delivery on Windows/macOS/Linux; Linux local IPC is runtime confined |
| Decode (common formats) | `image` | pure-Rust |
| Decode JPEG XL | `jxl-oxide` | pure-Rust; reviewed local `jxl-color` and `jxl-render` patches enforce the 10 MiB ICC initialization ceiling and reject a malformed unused LF-frame level without panicking |
| Input ICC conversion | `moxcms` | bounded pure-Rust RGB profile transform into the current sRGB path |
| Trash / recycle | `trash` | recoverable deletes |
| OS theme | `winit` window theme | default image background only; no extra dependency |
| File dialogs | `rfd` | native open, folder, save, and warning dialogs; OS-managed recent-item history is documented as a platform boundary |
| EXIF retain (opt-in) | `little_exif` | Save As strips by default; retain is session-only |
| Spot heal | no additional crate | pure-Rust bounded patch matching and in-memory undo |
| Error handling | `thiserror` / `anyhow` | app-level ergonomics |
| App icon | `winres` | embeds custom SVG/ICO app icons |

The dependency list is treated as a liability, not an asset: every crate added is
new code we ship and audit. Keeping it short is a feature.

No model runtime is part of the current stack. The optional local-description
evaluation and the process boundary required before one may ship are documented
in `LOCAL-INTELLIGENCE.md`. A localhost model server is intentionally outside the
accepted architecture.

## Prior art we studied (and how viewr differs)

We wrote every line of viewr ourselves. We still studied the field closely, to
borrow good ideas and to avoid known pitfalls.

- **Oculante** (Rust, cross-platform) is the closest existing project: fast,
  bloat-free, privacy-respecting, and wide on formats. We learned two things from
  it. First, its format strategy of putting heavy or C-backed decoders behind
  feature flags is the right pattern, and we adopt it (see the dependency policy in
  STANDARDS.md). Second, threaded loading with caching is what makes it feel fast,
  which confirms our prefetch-and-cache design. viewr differs deliberately in two
  ways: it aims for a more polished, custom-rendered interface rather than an
  immediate-mode debug-style UI, and it holds a stricter pure-Rust-by-default and
  sandboxed-decode posture instead of linking C libraries into the main process.
- **qView** (Qt, cross-platform) is our reference for minimal-but-genuinely-polished
  interaction design. We study its UX, not its stack.
- **emulsion** (Rust) is a lesson in leanness, and a cautionary tale about a
  single-maintainer project going stale. It reinforces our emphasis on a small,
  well-tested, well-documented codebase that others can pick up.

These are references and lessons, not sources of code.
