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

## Current availability

viewr is pre-1.0 and does not yet publish public downloads, signed installers, or
store packages. The supported paths today are a source build or a locally built and
verified dual-binary archive. See [`docs/INSTALL.md`](docs/INSTALL.md) for exact steps
and [`docs/VERIFY.md`](docs/VERIFY.md) for what each check proves and where its
evidence stops.

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
3. Zero persistent activity logs, zero insights, zero background indexing. No
   telemetry, analytics, crash reporting, auto-tagging, or usage-improvement
   toggle. Normal runs emit no diagnostic stream; optional local stderr diagnostics
   are explicit, path-private, and never persisted by viewr. Your filenames,
   folders, and photos never leave your machine, and viewr will **never** silently
   alter your files to add metadata or AI inferences.
4. Safe with hostile files. Opening an image means parsing untrusted bytes. The
   pure-Rust core decoders run off the UI thread with strict decoded dimension and
   allocation bounds; SVG also has a pre-parse input cap. Optional C-backed formats
   run from bounded encoded bytes in a resource-limited worker that receives no
   filesystem path; OS packaging profiles supply the whole-application network
   and filesystem boundary.
5. Simple on the surface, uncompromising underneath. It looks simple and does
   exactly what you want with no friction. That simplicity sits on top of rock-solid
   memory safety, a decode sandbox, strict testing, broad format coverage, and
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
you explicitly save a copy. Spot Heal, Undo, and Redo commit decoded pixels,
bounded history, and displayed pixels together. If the display cannot accept an
edit, decoded pixels and history are restored before failure is reported, so a
later Save As cannot silently differ from the canvas.

Image Information performs a bounded, local EXIF inspection through the retained
source handle that supplied the displayed pixels. It shows privacy-risk presence
categories for location, ownership, unique device or image identifiers, comments,
software history, embedded thumbnails, and maker data without putting sensitive
raw values on screen. The panel explicitly says that this limited EXIF scan cannot
prove that other metadata or hidden pixel data is absent.

**Open File** starts with one image. When operating-system access allows, viewr
scans its containing folder for supported regular-file siblings, and prefetch may
read nearby images for faster navigation. A restrictive sandbox may expose
only the selected item. **Open Folder** explicitly selects a folder for sibling
navigation during the current session. viewr persists neither grant, builds no
library index, and requests no broad photo-library access.

Immediate reverse navigation reuses a pristine frame that is still presented.
For moves within two positions, after a replacement appears, the just-left
pristine decode is retained in the same five-entry, 256 MiB neighbor cache without
copying its pixels when those limits permit. Larger jumps and evicted entries use
the normal loading path. Crop results, Spot Heal results, animation playback
frames, and frames held across an explicit Reload File are never admitted as
reusable source decodes.

On a genuine cache miss, the loading or failure status names the selected target
by a bounded filename only, while the visible filename, dimensions, and zoom keep
describing the pixels still on screen. First-load recovery uses the same specific
filename and Retry label. Decode, display-preview, and GPU upload failures remain
durable and retryable. Immediate reuse and full-resolution cache hits show no
loading state; derived-image preview preparation is labeled separately.

Open and Save As currently use native operating-system dialogs. On Windows, File
and the image right-click surface also offer **Open With...**, which asks the
Windows chooser to pass the exact current source to an app you select. It includes
the original metadata and excludes unsaved viewr edits. The selected app has its
own privacy and file-writing behavior; viewr retains a visible `F5` reminder until
you explicitly reload possible changes. These dialogs and handoffs may update the
operating system's recent-item history according to its own settings.
Decoded pixels remain in bounded process and GPU memory, but viewr does not claim
to prevent operating-system paging or live-system memory inspection. The exact
privacy boundary and practical mitigations are documented in
[`docs/PRIVACY.md`](docs/PRIVACY.md).

Does not, and will not: accounts, cloud sync, sharing services, ads, discover
feeds, face grouping, background indexing, silent AI analysis, automatic or background update checks, telemetry, or a settings screen
with two hundred switches. **We consider background image analysis and silent metadata tagging to be spyware.** If a big-company photos app is famous for it, that is a
strong signal we do not want it.

## Interaction

### Viewing and editing

The image always owns a dedicated viewport. Tools, Folder Previews, and Image
Information are optional docked panels that reserve their own space and never cover
the photo. Opening the first image never changes the current application-window
dimensions. The photo fits inside the existing viewport; only normal window
resizing changes the window itself, while View commands control fit and zoom within
it. `T`, `G`, and `I` show or fully hide the panels, and View > Panels displays
those keys beside the corresponding selected controls. Tools and Folder Previews
can also collapse to quiet disclosure rails. View > Panel Position independently docks
Tools and Image Information on the left or right. Every visibility, collapse, or
position change refits and recenters the image. `J` opens a temporary docked Spot
Heal inspector that also reserves its own space; only its brush mask is drawn over
the photo. Its inspector exposes brush radius, feather, alternate ranked sources,
Undo, and Redo. Image Information separates the bounded Source Privacy summary
from the explicit session-only export-metadata choice. View also exposes Fit Image to View
(primary modifier plus `0`), Actual Size (primary modifier plus `1`), Zoom In (`+`),
and Zoom Out (`-`) so zoom never depends on a mouse or trackpad. The empty,
loading, and load-error states use an opaque
high-contrast surface, so they remain readable even when the image background is
white. File > Reload File (`F5`) explicitly bypasses the decoded-neighbor cache
and refreshes the current file from disk while retaining the last good frame until
the replacement is ready. On Windows, Open With is available from File and the
image right-click surface; it opens the original source, not unsaved viewr edits,
and leaves a persistent `F5` reminder because viewr does not yet watch external
changes automatically. Loading and failure copy names that requested file
without exposing its directory. Crop offers Free, Original, 1:1, ten landscape and
portrait photo/video ratios, reversible orientation, numeric custom ratios, eight
pointer handles, and full keyboard operation. Applying a crop binds its exact
selection to the current image generation and keeps the original decoded and
displayed pixels until renderer presentation succeeds. A failure restores the
same selection for immediate Enter-key retry, while navigation cooperatively
cancels obsolete row-copy work.

Ratings use the familiar folder workflow without turning viewr into a library.
In normal viewing mode, `1` through `5` assign that rating and `0` clears it.
Edit > Rating exposes the same choices, while View > Rating Filter narrows the
current folder to All images or a minimum from 1 through 5. The filter is
session-only, every active threshold stays visible, navigation and Folder
Previews use only matching images, and an empty result offers Show all images.
Fit Image to View and Actual Size therefore use the primary modifier plus `0`
and `1`. There is no separate flag, pick, review, or batch-culling state. One
durable rating scale covers the useful workflow without another hidden catalog.

The durable record is standard embedded `xmp:Rating` inside the image, not a
viewr database, sidecar, alternate stream, filename convention, metadata timestamp
field, separate timestamp record, or activity history. viewr explains the
source-file change before the first rating
write in each session. The initial writer is deliberately narrow: ordinary,
identity-bound JPEG files on Windows with supported metadata are writable;
other containers and platforms remain visibly read-only. Existing valid Windows
`0x4746` SimpleRating fields are kept in agreement without relocating TIFF
metadata, and viewr never writes the unrelated `0x4749` 0-to-99 field. The full
privacy, interoperability, malformed-state, and replacement contract is in
[`docs/RATINGS.md`](docs/RATINGS.md).

### Trash and recovery

`Delete` moves only the currently displayed image to the operating system Trash.
There is no bare-letter trash shortcut and no mark, review, or batch-trash mode.
This keeps a destructive action on the conventional Delete key and makes its
target visible at the moment it runs. File > Move to Trash provides the same
pointer-accessible action.

Press `U`, or choose File > Undo Trash, to restore the latest safely recoverable
Trash action. Undo belongs to that action even if you have opened another folder.
When restoring into a different folder, viewr does not insert the restored file
into the unrelated current view. Reopen its source folder to refresh that list.
If viewr cannot retain an exact receipt, it directs recovery to the system Trash
and preserves any earlier valid in-app Undo action.

#### Safety and recovery details

Trash retains the live file handle that supplied the displayed pixels. Immediately
before moving the file, viewr verifies that the current path still identifies the
same regular filesystem object without following a replacement link. Missing,
replaced, linked, or unverifiable entries fail closed without changing the
playlist or Undo receipt. Windows and Linux retain a new Trash item identifier
only when its native file identity matches that accepted-source handle. macOS
retains the exact resulting Trash URL and the same handle. `U` never falls back to
another item with the same original pathname.

Foreground reload, preview preparation, Crop, Save As, and an active Spot Heal
stroke or worker block Trash and restore until the visible work is settled. Restore
runs through a typed native worker so the window stays responsive. The top bar
reports the operation without inventing a percentage, estimate, or cancellation
promise. If the worker ends without a result, viewr retains the receipt and asks
the user to reconcile it with `U` instead of claiming that nothing happened. A new
Trash move cannot silently replace uncertain restore ownership.

`Shift+Delete` is the separate permanent-delete path. It verifies the accepted
source before opening a bounded filename confirmation, then verifies it again
after the explicit Delete permanently action and immediately before removal.
Cancel performs no filesystem action. A successful permanent delete states that
it cannot be undone and preserves any prior valid Trash action assigned to `U`.

These checks are immediate fail-closed preflights. Desktop Trash and permanent
delete ultimately consume a pathname. Restore later calls the platform with the
checked receipt identifier on Windows and Linux or checked Trash URL on macOS.
These narrow final calls are not an atomic guarantee against another process
swapping the entry before the operating system consumes it. The automated native
bridge check and manual three-platform acceptance matrix live in
[`docs/ACCESSIBILITY.md`](docs/ACCESSIBILITY.md).

### Appearance choices

View summarizes the current choice and describes every option before selection:

- **System** follows the operating system's Light or Dark setting and reports the
  effective mode while active.
- **Light** uses bright neutral chrome, a light window frame, and a soft-white
  canvas.
- **Dark** uses low-glare charcoal chrome, a dark frame, and a deep-ink canvas.
- **Console** is the green-screen look, with a near-black canvas, phosphor-green
  chrome, and monospaced interface type.

Appearance changes app chrome and its default canvas, never image pixels. View >
Image Background can override the canvas independently. The choice is atomically
remembered as one local word and contains no photo path or activity history. A
missing preference quietly uses System. Unusable saved state also uses System but
shows `Could not restore saved appearance. Using System.` without a path, stored
value, or raw error. Choosing an appearance replaces rejected state when storage
is writable; a save failure remains visible through fixed session-only recovery
copy and a path-free diagnostic phase. Help > About viewr opens a
keyboard-dismissible modal with build, license, shortcut, and privacy details.
Help > Update viewr opens a local-only modal with the current version, the trusted
source-channel boundary, and the locked source-build command. It performs no
network check, download, install, or browser launch because no verified public
update source is configured yet. The `viewr update` CLI command prints the same
contract.

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
  Decoder-owned source pixels cross one normalization boundary; embedded RGB ICC
  profiles become pixels explicitly tagged with the RGBA8 sRGB working contract.
  The renderer accepts only that contract and requires an sRGB presentation
  surface instead of silently selecting an untyped fallback. Per-display output
  profiles, wide-gamut surfaces, and HDR presentation remain explicit roadmap work.
- Decoding: [image-rs](https://github.com/image-rs/image) plus jxl-oxide and
  friends, safe decoders across a wide format set. Async image decoding uses
  bounded replace-latest queues and generation-aware readers, so obsolete work
  stops cooperatively instead of delaying the current selection. This includes
  bounded worker-file reads and blocked worker IPC, which terminate when a newer
  image wins. PNG, WebP, and JPEG XL metadata allocations are checked before
  decoder materialization, with one shared 10 MiB embedded-ICC ceiling. Declared
  still-image and animation output sizes are validated before pixel allocation.
- Deletes: native system trash APIs, recoverable, never a raw delete by default.
  Windows and Linux retain a new `trash` item identifier only after its native
  file identity matches the live accepted-source handle; macOS retains the exact
  `NSFileManager` result URL with that handle. In-app Undo restores only those exact
  receipts from the latest recoverable single or batch action, never a pathname
  match. A move without an exact receipt remains recoverable through system Trash
  and does not erase a previous valid in-app Undo action. Permanent delete also
  preserves that prior Trash action while remaining non-recoverable itself.
  External platform failures cross a fixed path-free copy boundary before they
  reach the interface.
- Spot Heal: a bounded pure-Rust worker ranks up to eight spatially distinct
  source patches with robust tone and edge-aware scoring, applies local color
  adaptation and feathered compositing, and falls back to directional inpainting.
  It has no model dependency, source-file write, image cache, or sidecar.
  Pixel-patch undo and redo stay in memory, commit only after presentation, and
  roll back exact pixels and history on presentation failure. Only the changed GPU
  texture region is uploaded after each edit.
- Theme and icon: System, Light, Dark, and phosphor-green Console palettes cover
  native decoration, standard widgets, custom controls, typography, and the
  default image canvas. Black, neutral gray, and white remain independent image
  inspection backgrounds. The chooser describes each outcome and keeps image
  pixels outside theme scope. One validated, atomically replaced appearance word
  is the only persistent UI preference. A custom SVG/ICO app icon is embedded via
  `winres`.

Full reasoning, including the alternatives we rejected, is in
[`docs/STACK.md`](docs/STACK.md).

## Quality bar

viewr targets 85 percent or higher line coverage on its testable logic (currently
90.97 percent lines and 83.58 percent functions), clippy at pedantic with warnings
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
- [`docs/RATINGS.md`](docs/RATINGS.md), the approved embedded-rating, filter,
  privacy, interoperability, and source-write safety contract.
- [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md), what we measure and the current
  numbers.
- [`docs/LOCAL-INTELLIGENCE.md`](docs/LOCAL-INTELLIGENCE.md), the strict product,
  privacy, runtime, and evaluation gate for any optional local model.
- [`docs/INSTALL.md`](docs/INSTALL.md), how to install on each OS and cut a release.
- [`docs/VERIFY.md`](docs/VERIFY.md), what local source and artifact checks prove,
  and where their evidence stops.
- [`SECURITY.md`](SECURITY.md), supported versions, safe vulnerability disclosure,
  and the current private-channel status.
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
