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

viewr runs a message-driven loop of our own on winit's event loop. We borrow the
shape of the Elm architecture without depending on a framework: there is exactly
one application state value, everything that happens is a `Message`, an `update`
function folds messages into the state, and a `render` pass draws the state through
our wgpu pipeline. No callbacks, no shared mutable UI state.

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

Long-running work (decode a file, scan a folder, move to trash) runs on a worker
thread pool. When it finishes it posts a `Message` (e.g.
`ImageDecoded { index, texture }`) back to the loop, which `update` applies on the
next frame. The UI thread itself does no blocking work: it only handles input,
folds messages, and renders.

## Core state (sketch)

```rust
struct Viewr {
    // The working set: the folder being browsed, in display order.
    entries: Vec<PathBuf>,
    current: usize,

    // What's on screen and nearby, decoded and ready.
    cache: LruTextureCache,        // decoded frames living in GPU memory

    // View transform for the current image.
    zoom: f32,
    pan: Vec2,
    fit_mode: FitMode,             // FitToWindow | ActualPixels

    // Curation.
    flagged: HashSet<PathBuf>,     // "mark for delete" set (batch cull mode)
    undo: Vec<UndoAction>,         // e.g. TrashedFile { from, trash_handle }

    theme: Theme,                  // driven by the OS light/dark setting
    settings: Settings,           // tiny; e.g. confirm-on-permanent-delete
}
```

The state is deliberately flat and small. There is no document model, no plugin
registry, no scene graph; a viewer does not need them.

## Modules

Shipped:

- **`app`**: the message loop on winit (state, `Message`, `update`, `render`). The
  spine. Opens the image named on the command line.
- **`gpu`**: the wgpu pipeline. Clears to the theme background and draws the current
  image as a textured quad scaled to fit. The neighbor texture cache lands in
  Phase 2.
- **`decode`**: turns a path into RGBA pixels. Pure-Rust formats are decoded on background threads via the `image` crate. Complex C-backed formats are delegated to a persistent daemon pool (`viewr-decode`) using versioned native-path frames and a length-validated RGBA8 stream. The parent acknowledges each complete pixel stream before the worker accepts another request.
- **`view`**: pure geometry for placing an image in the viewport (fit math, later
  zoom and pan). Unit-tested without a GPU.
- **`edit`**: crop and save-as/convert. Export re-encodes from pixels, which strips
  metadata by construction.
- **`curate`**: move to the OS trash / recycle bin and restore for undo.
- **`sandbox` / `worker_limit`**: spawn and pool `viewr-decode`; Job Object or
  process-group lifetime controls, one-process policies, and memory limits for
  helpers.
- **`fs`**: recognizing image files (core and worker extensions) and natural-sort
  ordering (`img2` before `img10`).
- **`theme`**: reads the OS light/dark setting via winit and maps it to our palette,
  live-updating on change.
- **`error`**: the typed error set for the app.

Planned:

- **`ui`**: the `egui` integration for the main UI overlay (left-aligned floating
  toolbar with Hand Tool and Crop Tool, toasts), drawn on our wgpu pipeline, with
  accessibility via `accesskit`.
- **`curate`**: delete, flag, and undo logic over the `trash` crate.
- **`input`**: key and mouse bindings mapped to messages; the one place shortcuts
  live.

## The hot path: opening and flipping through images

This is the experience that makes or breaks the app, so it gets first-class
treatment.

1. **On open**, `fs` scans the containing folder once, off-thread, and returns the
   ordered `entries`. Meanwhile `decode` is already working on the requested file —
   we do not wait for the scan to show the first image.
2. **Decode is prioritized:** the *current* image is decoded at highest priority.
   Async image decoding runs on a background thread using `std::sync::mpsc` so
   left/right navigation and trashing/undoing trash is perfectly snappy and no
   longer freezes the UI. As soon as it's ready it's uploaded to a GPU texture
   and drawn. First pixels appear as fast as decode allows.
3. **Neighbors are prefetched.** After the current image, `decode` speculatively
   decodes `current ± 1` (then `± 2`) in the background via the mpsc channel and parks
   them in the GPU cache. Pressing → then shows an already-decoded, already-uploaded
   frame — effectively instant.
4. **The cache is bounded (LRU).** Only a window of frames lives in GPU memory;
   far-away images are evicted. Memory stays flat whether the folder has 10 images
   or 100,000.
5. **Panning/zooming is pure GPU.** The decoded frame is a texture; pan and zoom
   are just changes to the sampling transform. No re-decode, no CPU work per frame.

The rule: **the UI thread only ever draws things that are already ready.** All
"getting ready" happens off-thread and arrives as messages.

## The cull workflow (why deleting feels right)

Two modes, because two kinds of users:

- **Instant delete** — `Delete` moves the current file to the OS trash via `trash`,
  records an `UndoAction`, shows a non-blocking toast ("Moved to Trash · Undo"),
  and **advances to the image that took its place** (new `current`, not a jump to
  the top). No modal — modals destroy culling speed. `Ctrl+Z` restores from trash.
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
│   │  Linux additionally denies network syscalls     │     bytes    │
│   └─────────────────────────────────────────────────┘              │
└──────────────────────────────────────────────────────────────────────┘
        OS packaging profiles add a second network-denial boundary when used.
```

- **No application networking stack exists.** No socket/HTTP client crate is
  linked, and CI enforces the dependency policy (see `PRIVACY.md`). Syscall-level
  denial comes from Linux worker seccomp and the enclosing OS package profiles.
- **C-backed decoding is process-isolated.** The daemon receives one versioned native path request and returns a validated, exact-length RGBA8 stream over its existing pipe. This avoids trusting worker-owned mapping metadata or mutable backing storage. Linux denies classic and io_uring network paths. Windows constrains the Job Object to one process and 1.5 GiB aggregate memory; Linux denies process-creating syscalls while allowing same-process decoder threads; supported non-Linux Unix targets create a private session and apply a one-process resource limit. All workers have containment lifetime controls, typed bounded responses, and a hard request deadline covering both send and receive. The worker currently receives a path and therefore retains the filesystem access granted by its enclosing OS package; Phase 7 packaging profiles are responsible for narrowing that ambient access. Pure-Rust formats remain in the main process but decode off the UI thread under the same dimension, allocation, and aggregate concurrency limits.
- **Trash, not unlink**, by default — the filesystem is treated as precious.

## What is intentionally absent

No account layer. No sync engine. No network client. No telemetry/analytics/crash
pipeline. No plugin host. No embedded database of your library. No background
service or auto-updater that runs when the app is closed. Each of these is a
common source of the bloat, the privacy leaks, and the attack surface we exist to
avoid — leaving them out is an architectural decision, not an omission.
