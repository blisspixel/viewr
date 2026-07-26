# Architecture

How viewr is put together, and how a keystroke becomes a pixel. This describes the
current implementation and names the seams that still need simplification.

## Design goals that shape the architecture

1. **Instant.** The first frame of an image must appear as fast as the disk and
   decoder allow, never blocked on anything else.
2. **Never janky.** The UI thread never decodes, never touches the disk for a big
   read, never blocks. Heavy work is off-thread; the UI only ever swaps in results.
3. **Safe with hostile input.** Untrusted bytes are decoded in isolation, away from
   the filesystem and network.
4. **Auditable surface.** Clear ownership, bounded work, and no framework we
   cannot reason about.

## The message loop (our own, on winit + wgpu)

viewr runs a message-driven application on winit's event loop. One `App` value
owns all mutable UI state. Winit callbacks update that state directly, background
results arrive through bounded channels or user events, and each requested render
pass draws the current state through the wgpu pipeline. There is no second store
or independently mutable UI model.

```
            ┌────────────┐   Message    ┌────────────┐
 input ───▶ │   update   │ ◀─────────── │  workers   │
 (winit:    │  (state ►  │              │ (decode,   │
  keys,     │   state')  │ ───tasks───▶ │  scan, io) │
  mouse)    └─────┬──────┘              └────────────┘
                  │ render
                  ▼
            ┌────────────┐
            │  our wgpu   │ ─▶ screen
            │  pipeline   │
            └────────────┘
```

Image decoding and folder scanning run off the event thread. Their results return
through channels, wake the event loop through a typed `UserEvent`, request a
redraw, and are applied only if they still match the current path or generation.
The initial decode and folder scan start before renderer initialization, so GPU
setup does not unnecessarily serialize first-pixel work. Native dialogs and trash operations are short,
user-triggered platform calls; the performance gate measures them rather than
assuming they are free.

An explicit developer/CI probe uses the same application loop, records the first
successfully presented window frame and image, samples bounded folder positions,
observes a settled idle interval, reads the process peak resident set, and exits.
Normal GUI launches do not construct this probe or collect performance data.

## Core state (sketch)

```rust
struct App {
    // Selected source plus the naturally sorted, session-scoped folder view.
    image_path: Option<PathBuf>,
    playlist: Option<Playlist>,

    // Exact source/pixel match on screen plus a bounded neighbor cache.
    current_image: Option<Arc<DecodedImage>>,
    loaded_image_path: Option<PathBuf>,
    prefetch: PrefetchCache,

    // View, crop, animation, details, and in-memory pixel edit state.
    transform: Transform,
    animation: Option<AnimationPlayback>,
    image_details: Option<ImageDetails>,
    heal: HealTool,

    // At most one foreground operation of each kind, with generation/path
    // checks before a result can replace current state.
    image_loader_rx: Option<Receiver<ImageLoadResult>>,
    crop_worker: Option<CropWorker>,
    save_worker: Option<SaveWorker>,

    // Docked chrome and the session-only export-privacy choice.
    show_tools_panel: bool,
    tools_panel_open: bool,
    tools_panel_side: DockSide,
    show_filmstrip_panel: bool,
    filmstrip_panel_open: bool,
    show_image_info: bool,
    image_info_side: DockSide,
    retain_exif: bool,

    // Curation.
    flags: FlagSet,
    last_trashed: Vec<TrashedFile>,
}
```

There is no document database, plugin registry, or scene graph. The state is
intentionally centralized so the winit event loop has one owner, but the current
`app` and `ui` modules have grown beyond the original "flat and small" sketch.
The next architectural milestone extracts pure session/load, crop, job, and dock
state without introducing a second mutable store. See `ROADMAP.md`.

## Modules

Shipped:

- **`app`**: the winit application handler and centralized state/input dispatch.
  It opens command-line and native file requests, schedules background work, and
  feeds one immutable frame snapshot to the UI.
- **`gpu`**: the wgpu pipeline. It clears to the image background and draws one
  textured quad, scissored to the physical-pixel viewport left after docked
  chrome. Egui draws in a separate full-window pass. The current image is an sRGB
  texture with a complete GPU-generated mip chain and trilinear sampling. A source
  larger than the adapter texture or pixel limit receives an aspect-preserving
  preview prepared by a dedicated replace-latest background worker. Its bounded,
  fallible area resampler works in linear light and premultiplied alpha; the winit
  thread performs only the validated texture upload. Superseded rows cancel by
  image generation, while the full decoded image remains available for export. A
  typed output contract admits only matching RGBA8 sRGB working pixels and an
  sRGB presentation surface; unsupported surface formats fail explicitly instead
  of changing the transfer function. A Spot Heal patch updates the base level and
  regenerates dependent mips. No scene graph.
- **`color`**: the narrow color contract shared by decode, edits, previews, and
  presentation. It names the working color space and pixel format separately from
  the renderer-owned output transform. The only shipping contract is RGBA8 sRGB
  to sRGB. This deliberately prevents a future higher-precision or wide-gamut
  decoder result from entering the old upload path until a compatible output
  transform exists. Preview generation, thumbnail upload, and export enforce the
  same boundary rather than silently reinterpreting unfamiliar pixels.
- **`decode`**: turns a path into RGBA pixels. Pure-Rust formats are decoded on
  background threads via the `image` crate. For complex C-backed formats, the
  main process opens the selected file and delegates bounded encoded bytes to an
  isolated `viewr-decode` helper using versioned request frames and a
  length-validated RGBA8 stream. Each V2 response also carries a bounded ICC
  profile, typed H.273 CICP values, or an explicit unknown color-space state.
  The parent normalizes supported profiles and never silently promotes unknown
  worker output to tagged sRGB. ICC normalization runs after the timed IPC thread
  has joined, remains under the shared decode-concurrency permit, and checks the
  foreground generation between rows. This prevents cancelled requests from
  detaching expensive parent transforms. The HEIC adapter performs size-first,
  fallible ICC extraction and explicitly requests source-NCLX output from
  libheif runtimes whose append-only options ABI supports it. With the version-10
  ABI it also enables bitstream-profile passthrough when a file has no container
  NCLX. Because that contract performs no additional gamut conversion, an ICC
  remains authoritative when ICC and bitstream NCLX coexist; decoded CICP replaces
  it only when source and output evidence demonstrate a changed color encoding.
  Version 8 and 9 runtimes conservatively expose their decoded output CICP, or
  unknown when none is available, instead of retaining an ICC after their implicit
  sRGB transform. A separate embedded libheif 1.23 CI lane, gated on libde265
  1.0.7 or newer, verifies container and HEVC-VUI-only paths, including matching
  ICC plus VUI metadata. The parent acknowledges each complete pixel stream
  before the worker returns to the idle pool and accepts another request.
  Core decoders apply all eight EXIF orientations. Bounded embedded RGB ICC
  profiles are converted into the current sRGB working path; unsupported profiles
  carry an explicit fallback status instead of a false color-managed claim.
  Decoder-owned `SourceImage` pixels cannot enter the renderer directly. The one
  consuming normalization boundary validates their shape, applies any supported
  source transform, checks cancellation between rows, and produces a
  `DecodedImage` tagged with its complete working encoding. A failed or cancelled
  transform drops the source instead of exposing partially converted pixels.
  Foreground core and JPEG XL decodes plus animated frames check their generation
  between ICC rows, matching the worker path's cancellation contract.
  Foreground file readers observe the current image generation at read and seek
  boundaries. Worker-bound file reads do the same, and a superseded request
  terminates its contained worker instead of holding a decode permit until the
  request deadline. PNG and WebP metadata is preflighted across the complete
  container, and the reviewed `jxl-color` patch enforces the same 10 MiB encoded
  and decoded ICC ceiling while JPEG XL is still initializing. Decoder dimensions
  and declared output bytes are checked before `DynamicImage` allocates pixels.
- **`animated`**: bounded GIF, WebP, and APNG frame decode plus deterministic
  deadline, pause/resume, and finite/infinite loop state. It is current-image-only
  RAM state and never a disk cache. Containers are identified by content after
  any misleading rename, APNG receives allocation limits at decoder construction,
  and superseded work exits between frames.
- **`image_info`**: best-effort local format, size, camera, lens, exposure,
  aperture, ISO, focal length, date, and GPS-presence inspection. It publishes GPS
  presence without exposing coordinates in the UI. Container extraction, TIFF
  directory count, component sizes, offsets, recursion, thumbnails, and aggregate
  allocations are preflighted under fixed limits before EXIF decoding. TIFF IFDs
  may live anywhere in the source file: the reader seeks only bounded metadata,
  removes pixel-strip locators, and compacts the result before parsing.
- **`view`**: pure geometry for placing an image in the viewport, including
  physical-pixel insets reserved by docked chrome, fit math, zoom, and pan.
  Unit-tested without a GPU.
- **`edit`**: exact pixel transforms, crop, and save-as/convert. Export re-encodes
  from the visible pixels and strips metadata by default without cloning a second
  RGBA buffer. Crop validates source shape and uses fallible output allocation.
  Save As rejects a destination that aliases the open source before encoding and
  builds the complete output in a sibling temporary file before one atomic
  replacement attempt. Explicit session-only EXIF retention is content-driven,
  supports JPEG, PNG, and WebP destinations, uses the bounded metadata reader,
  and normalizes orientation, output dimensions, and stale thumbnail offsets
  before writing supported tags.
- **`heal`**: pure-Rust spot-heal preparation, bounded region extraction,
  deterministic edge-aware ranking of up to eight spatially distinct patches,
  robust boundary tone adaptation, adjustable feathered compositing,
  distance-ordered directional fallback inpainting, and byte-bounded in-memory
  pixel-patch history. Refresh Source reuses the retained bounded job, replaces
  the latest result without creating another undo step, and never retains the
  full decoded image. A path-free worker shares immutable decoded pixels just
  long enough to copy the bounded working region, drops the full image, and then
  computes the repair. Apply, refresh, undo, and redo upload only the changed
  base-texture rectangle before dependent mips are regenerated. The tool is
  unavailable if the adapter cannot
  represent the complete decoded image in one texture, preventing an ambiguous
  source-to-display coordinate mapping.
- **`curate`**: move to the OS trash or recycle bin and restore for undo. On
  macOS, a native receipt retains the exact resulting trash URL so restoration
  does not depend on an unsupported global trash listing.
- **`macos`**: the narrow native bridge for Finder and Open With requests plus
  recoverable `NSFileManager` trash operations. It is absent from other builds.
- **`sandbox` / `worker_limit`**: spawn and pool `viewr-decode`; Job Object or
  process-group lifetime controls, one-process policies, memory limits, hard
  deadlines, and generation cancellation for helpers.
- **`fs`**: recognizing image files (core and worker extensions) and natural-sort
  ordering (`img2` before `img10`).
- **`prefetch`**: an in-memory LRU bounded to five decoded neighbors and 256 MiB,
  whichever limit is reached first. Entries are never persisted.
- **`thumbs`**: bounded folder-preview decoding and at most nine retained GPU
  thumbnail textures for the visible filmstrip window.
- **`theme`**: resolves System, Light, Dark, or Console against winit's native
  decoration and theme signal, supplies complete GPU and chrome color tokens,
  and reads or writes one validated appearance word in the platform configuration
  directory. The bounded read rejects oversized and unknown values.
- **`performance`**: stable, path-free probe output and narrow platform peak-RSS
  readers used only by the explicit developer/CI performance command.
- **`error`**: the typed error set for the app.
- **`ui`**: the `egui` layer for the conventional menu bar, fully hideable and
  collapsible docked tools and folder previews, left/right Image Information,
  animation controls, crop controls and handles, the temporary docked Spot Heal
  inspector, accessible About modal, appearance picker, load/retry state, and
  transient toasts.
  Visible chrome never covers the image; its
  exact edge-aware insets feed the same `view` geometry used by hit testing and
  rendering. Keyboard dispatch remains centralized in `app` rather than adding a
  second input abstraction. Custom controls publish AccessKit semantics. Native
  adapters are initialized before the hidden window becomes visible on all three
  targets; Linux startup first confines their D-Bus transport to local Unix IPC.
- **`privacy`**: the earliest process-start boundary. On Linux it validates local
  D-Bus environment transports, applies `no_new_privs`, installs the application
  Internet-socket policy, and verifies enforcement before logging, workers, GUI
  initialization, or application threads.

## The hot path: opening and flipping through images

This is the experience that makes or breaks the app, so it gets first-class
treatment.

1. **On open**, foreground decode and the containing-folder scan are scheduled
   before renderer initialization and run independently off-thread. We do not wait
   for the scan or GPU setup before starting image work. If an OS sandbox grants only
   the selected file, viewr keeps that file openable as a one-item playlist and asks
   the user to choose **Open Folder** for explicit, session-scoped sibling access.
2. **Decode is prioritized:** the *current* image is decoded at highest priority.
   Async image decoding runs on background work using `std::sync::mpsc`, so file
   reads and decode do not freeze navigation. As soon as a result is ready, it is
   uploaded to a GPU texture and drawn. First pixels appear as fast as decode
   allows.
3. **The last good frame stays presented.** A cache miss, explicit reload, or
   decode failure does not clear the current GPU texture. Results replace it only
   after successful decode and only when their path/generation still matches the
   selected source. Load errors remain actionable through Retry.
4. **Neighbors are prefetched.** After the current image, `decode` speculatively
   decodes nearby entries in the background and parks a bounded set of RGBA
   results in RAM. Navigation can upload an already-decoded frame immediately.
5. **Caches are bounded.** Full decoded neighbors are capped by both count and
   decoded bytes, thumbnail work has an in-flight bound, and the renderer owns one
   current image texture plus its mip levels. Image-cache memory does not grow with
   folder length; the lightweight playlist path index necessarily does.
6. **Panning/zooming is pure GPU.** The decoded frame is a texture; pan and zoom
   are changes to the sampling transform, with mip selection handled by the
   sampler. No re-decode and no CPU resampling occur per frame.

The rule: **the UI thread only ever draws things that are already ready.** All
"getting ready" happens off-thread and arrives as messages.

## Current architecture pressure

`app.rs` owns the event loop, input table, asynchronous job lifecycle, crop
geometry, curation, and frame assembly. `ui.rs` owns both dock/menu policy and
painting. That concentration was useful while the interaction model converged,
but it now makes race behavior and accessibility enablement harder to test than
the underlying decode/edit logic. The repository coverage command excludes most
native application and rendering glue, so keeping product state inside those
files also hides meaningful logic from the gate.

The correction is not a framework rewrite. Extract pure state transitions with
explicit inputs and outputs, keep `App` as the sole runtime owner, and make winit,
wgpu, dialogs, and native accessibility thin adapters. Each extraction must land
with behavior-preserving tests and no second source of truth.

## The cull workflow (why deleting feels right)

Two modes, because two kinds of users:

- **Instant delete:** `Delete` moves the current file to the OS trash through the
  supported native integration, records an `UndoAction`, and shows a non-blocking
  toast ("Moved to Trash · Undo"),
  and **advances to the image that took its place** (new `current`, not a jump to
  the top). Normal trash has no modal. `U` restores the latest trash action.
- **Flag then batch**: tap a key to *flag* the current image (photographers'
  preferred flow), keep moving, then delete all flagged at the end in one action
  (still to trash, still undoable). Non-destructive until you commit.

`Shift+Delete` is the only permanent delete, and it's the only action that shows a
confirmation. Everything else is fast and reversible.

Index preservation, trash-not-unlink, and undo are the three details that separate
a viewer that "feels broken" from one that feels trustworthy. They are core, not
polish.

## Security boundaries

```
┌─────────────────────────── viewr process ───────────────────────────┐
│  UI thread (winit/wgpu)  fs/curate (privileged: user-chosen paths)   │
│                                                                      │
│            ▲ decoded pixels only          ▲ paths in / results out   │
│            │                               │                         │
│   ┌────────┴───────────────────────────────┴─────────┐              │
│   │  C decode worker: bounded IPC, memory, and time  │  ◀ untrusted │
│   │  Linux C builds use a default-deny syscall set  │     bytes    │
│   └─────────────────────────────────────────────────┘              │
└──────────────────────────────────────────────────────────────────────┘
        OS packaging profiles add a second network-denial boundary when used.
```

- **No remote-service client exists.** No HTTP, TLS, telemetry, or update client is
  linked, and CI enforces the dependency policy (see `PRIVACY.md`). Linux's generic
  D-Bus code is restricted to the AccessKit/AT-SPI path; startup permits Unix-domain
  socket creation only and denies io_uring before application threads. Worker
  seccomp and enclosing OS package profiles add stricter boundaries.
- **C-backed decoding is process-isolated.** The daemon receives one versioned
  request containing a validated format identifier and bounded encoded bytes,
  then returns a validated, exact-length RGBA8 stream over its existing pipe. The
  main process opens user-selected files, so the worker never receives a path or
  depends on a dynamic file grant. Linux denies classic and io_uring network
  paths; AVIF/HEIC builds additionally allow only reviewed runtime syscalls,
  read-only plugin discovery, and same-process threads. Windows constrains the
  Job Object to one process and 1.5 GiB aggregate memory; supported non-Linux Unix
  targets create a private session and apply a one-process resource limit. All
  workers have containment lifetime controls, typed bounded responses, a hard
  request deadline covering send and receive, and foreground-generation
  cancellation that terminates stale blocked requests. Pure-Rust formats remain
  in the main process but decode off the UI thread under the same dimension,
  allocation, and aggregate concurrency limits.
- **Trash, not unlink**, by default: the filesystem is treated as precious.

## What is intentionally absent

No account layer. No sync engine. No network client. No telemetry/analytics/crash
pipeline. No plugin host. No embedded database of your library. No background
service or auto-updater that runs when the app is closed. Each of these is a
common source of the bloat, the privacy leaks, and the attack surface we exist to
avoid. Leaving them out is an architectural decision, not an omission.
