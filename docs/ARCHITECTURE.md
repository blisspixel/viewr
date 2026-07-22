# Architecture

How viewr is put together, and how a keystroke becomes a pixel. This describes the
target design; code lands per `ROADMAP.md`.

## Design goals that shape the architecture

1. **Instant.** The first frame of an image must appear as fast as the disk and
   decoder allow — never blocked on anything else.
2. **Never janky.** The UI thread never decodes, never touches the disk for a big
   read, never blocks. Heavy work is off-thread; the UI only ever swaps in results.
3. **Safe with hostile input.** Untrusted bytes are decoded in isolation, away from
   the filesystem and network.
4. **Small surface.** Few modules, clear ownership, no framework we can't reason
   about.

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

## Core state (sketch)

```rust
struct Viewr {
    // Selected source plus the naturally sorted, session-scoped folder view.
    image_path: Option<PathBuf>,
    playlist: Option<Playlist>,

    // Exact source/pixel match on screen plus a bounded neighbor cache.
    current_image: Option<DecodedImage>,
    loaded_image_path: Option<PathBuf>,
    prefetch: PrefetchCache,

    // View and crop state for the current image.
    transform: Transform,

    // Docked chrome and the session-only export-privacy choice.
    tools_panel_open: bool,
    filmstrip_panel_open: bool,
    show_image_info: bool,
    retain_exif: bool,

    // Curation.
    flags: FlagSet,
    last_trashed: Vec<TrashedFile>,
}
```

The state is deliberately flat and small. There is no document model, no plugin
registry, no scene graph; a viewer does not need them.

## Modules

Shipped:

- **`app`**: the winit application handler and centralized state/input dispatch.
  It opens command-line and native file requests, schedules background work, and
  feeds one immutable frame snapshot to the UI.
- **`gpu`**: the wgpu pipeline. It clears to the image background and draws one
  textured quad, scissored to the physical-pixel viewport left after docked
  chrome. Egui draws in a separate full-window pass. No scene graph.
- **`decode`**: turns a path into RGBA pixels. Pure-Rust formats are decoded on
  background threads via the `image` crate. For complex C-backed formats, the
  main process opens the selected file and delegates bounded encoded bytes to an
  isolated `viewr-decode` helper using versioned request frames and a
  length-validated RGBA8 stream. The parent acknowledges each complete pixel
  stream before the worker returns to the idle pool and accepts another request.
- **`view`**: pure geometry for placing an image in the viewport, including
  physical-pixel insets reserved by docked chrome, fit math, zoom, and pan.
  Unit-tested without a GPU.
- **`edit`**: crop and save-as/convert. Export re-encodes from pixels, which strips
  metadata by construction.
- **`curate`**: move to the OS trash or recycle bin and restore for undo. On
  macOS, a native receipt retains the exact resulting trash URL so restoration
  does not depend on an unsupported global trash listing.
- **`macos`**: the narrow native bridge for Finder and Open With requests plus
  recoverable `NSFileManager` trash operations. It is absent from other builds.
- **`sandbox` / `worker_limit`**: spawn and pool `viewr-decode`; Job Object or
  process-group lifetime controls, one-process policies, and memory limits for
  helpers.
- **`fs`**: recognizing image files (core and worker extensions) and natural-sort
  ordering (`img2` before `img10`).
- **`theme`**: reads the OS light/dark setting via winit and maps it to the default
  image background, live-updating on change. Chrome keeps its stable dark palette.
- **`error`**: the typed error set for the app.
- **`ui`**: the `egui` layer for the conventional menu bar, collapsible docked
  tools and folder previews, Image Information, crop controls, and transient
  toasts. Persistent chrome never covers the image; its exact insets feed the
  same `view` geometry used by hit testing and rendering. Keyboard dispatch
  remains centralized in `app` rather than adding a second input abstraction.
  Custom controls publish AccessKit semantics. A native adapter is initialized
  before the hidden window becomes visible on Windows and macOS; Linux native
  delivery remains pending to preserve the dependency-level network ban.

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
3. **Neighbors are prefetched.** After the current image, `decode` speculatively
   decodes nearby entries in the background and parks a bounded set of RGBA
   results in RAM. Navigation can upload an already-decoded frame immediately.
4. **Caches are bounded.** Full decoded neighbors use a fixed-capacity RAM cache,
   thumbnail work has an in-flight bound, and the renderer owns only the current
   full-size GPU texture. Image-cache memory does not grow with folder length;
   the lightweight playlist path index necessarily does.
5. **Panning/zooming is pure GPU.** The decoded frame is a texture; pan and zoom
   are just changes to the sampling transform. No re-decode, no CPU work per frame.

The rule: **the UI thread only ever draws things that are already ready.** All
"getting ready" happens off-thread and arrives as messages.

## The cull workflow (why deleting feels right)

Two modes, because two kinds of users:

- **Instant delete:** `Delete` moves the current file to the OS trash through the
  supported native integration, records an `UndoAction`, and shows a non-blocking
  toast ("Moved to Trash · Undo"),
  and **advances to the image that took its place** (new `current`, not a jump to
  the top). Normal trash has no modal. `U` restores the latest trash action.
- **Flag then batch** — tap a key to *flag* the current image (photographers'
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

- **No application networking stack exists.** No socket/HTTP client crate is
  linked, and CI enforces the dependency policy (see `PRIVACY.md`). Syscall-level
  denial comes from Linux worker seccomp and the enclosing OS package profiles.
- **C-backed decoding is process-isolated.** The daemon receives one versioned request containing a validated format identifier and bounded encoded bytes, then returns a validated, exact-length RGBA8 stream over its existing pipe. The main process opens user-selected files, so the worker never receives a path or depends on a dynamic file grant. Linux denies classic and io_uring network paths; AVIF/HEIC builds additionally allow only reviewed runtime syscalls, read-only plugin discovery, and same-process threads. Windows constrains the Job Object to one process and 1.5 GiB aggregate memory; supported non-Linux Unix targets create a private session and apply a one-process resource limit. All workers have containment lifetime controls, typed bounded responses, and a hard request deadline covering both send and receive. Pure-Rust formats remain in the main process but decode off the UI thread under the same dimension, allocation, and aggregate concurrency limits.
- **Trash, not unlink**, by default — the filesystem is treated as precious.

## What is intentionally absent

No account layer. No sync engine. No network client. No telemetry/analytics/crash
pipeline. No plugin host. No embedded database of your library. No background
service or auto-updater that runs when the app is closed. Each of these is a
common source of the bloat, the privacy leaks, and the attack surface we exist to
avoid — leaving them out is an architectural decision, not an omission.
