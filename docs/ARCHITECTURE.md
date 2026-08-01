# Architecture

How viewr is put together, and how a keystroke becomes a pixel. This describes the
current implementation and names the seams that still need simplification.

## Design goals that shape the architecture

1. **Instant.** The first frame of an image must appear as fast as the disk and
   decoder allow, never blocked on anything else.
2. **Never janky.** The UI thread never decodes or performs a bulk image read.
   Heavy image work is off-thread; narrow native dialogs and Trash calls remain
   synchronous user-triggered boundaries and are tracked explicitly below.
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
setup does not unnecessarily serialize first-pixel work. Native dialogs and Trash
or permanent-delete calls for one current file remain synchronous,
user-triggered platform calls. Trash restore uses one typed worker, a one-result
channel, and `EventLoopProxy` wake; only the event thread reconciles receipts,
playlist scope, and visible results. The general performance probe does not
exercise native Trash operations, so their latency is not claimed by that gate.

An explicit developer/CI probe uses the same application loop, records the first
successfully presented window frame and image, samples bounded folder positions,
observes a settled idle interval, reads the process peak resident set, and exits.
Normal GUI launches do not construct this probe or collect performance data.

```rust
struct App {
    // Session state handles the selected path, presented path, load errors,
    // and generation-tracking for foreground decode supersession.
    session: crate::session::Session,

    // File list state tracking.
    playlist: Option<crate::playlist::Playlist>,

    // Exact source/pixel match on screen plus a bounded neighbor cache.
    current_image: Option<Arc<DecodedImage>>,
    current_source: Option<Arc<ImageSource>>,
    current_image_reuse: ImageReuseEligibility,
    prefetch: PrefetchCache,
    prefetch_sources: HashMap<PathBuf, Arc<ImageSource>>,

    // View, crop, animation, details, and in-memory pixel edit state.
    transform: Transform,
    animation: Option<AnimationPlayback>,
    image_details: Option<ImageDetails>,
    heal: HealTool,

    // Auxiliary, Save As, crop, and display-preview work each has a one-result,
    // event-loop-owned job.
    auxiliary_job: Option<OneShotJob<AuxiliaryLoadContext, AuxiliaryLoadResult>>,
    save_job: Option<OneShotJob<(), SaveResult>>,
    close_after_save: bool,
    save_recovery_unsettled: bool,
    crop_job: Option<OneShotJob<CropJobContext, CropJobResult>>,
    crop_recovery_unsettled: bool,
    preview_job: Option<OneShotJob<PreviewJobContext, PreviewJobResult>>,
    preview_recovery_unsettled: bool,

    // Docked chrome and the session-only export-privacy choice.
    show_tools_panel: bool,
    tools_panel_open: bool,
    tools_panel_side: DockSide,
    show_filmstrip_panel: bool,
    filmstrip_panel_open: bool,
    show_image_info: bool,
    image_info_side: DockSide,
    retain_exif: bool,

    // Latest safely recoverable Trash action.
    last_trashed: Vec<TrashedFile>,
}
```

There is no document database, plugin registry, or scene graph. The state is
intentionally centralized so the winit event loop has one owner. The `session`,
`crop`, `playlist`, `performance`, and `job` modules establish smaller state and
logic seams without introducing a second mutable store. `App` remains a large
orchestrator. Extending bounded job ownership to the remaining worker surfaces,
plus extracting dock/menu view models, is explicit roadmap work.

## Modules

Shipped:

- **`app`**: the winit application handler and centralized state/input dispatch.
  It opens command-line and native file requests, schedules background work, and
  feeds one immutable frame snapshot to the UI.
- **`job`**: the bounded, one-result ownership boundary used by current-image
  details, animation discovery, rating observation, Save As, crop, and over-limit
  display previews. The event loop retains operation context while the worker
  receives one non-cloneable completion endpoint. Replaced work cannot publish, a
  context-owned cancellation flag can stop obsolete crop rows, and an endpoint
  that closes without a result wakes the event loop so it becomes a terminal state
  instead of permanent false busy state. The interface requires a restart after
  an unexpected executor failure; release builds do not claim thread-panic
  recovery or promise that the same failing input will succeed after restart.
- **`gpu`**: the wgpu pipeline. It clears to the image background and draws one
  textured quad, scissored to the physical-pixel viewport left after docked
  chrome. Egui draws in a separate full-window pass. The current image is an sRGB
  texture with a complete GPU-generated mip chain and trilinear sampling. A source
  larger than the adapter texture or pixel limit receives an aspect-preserving
  preview prepared by a dedicated replace-latest background worker. Its bounded,
  fallible area resampler works in linear light and premultiplied alpha; the winit
  thread performs only the validated texture upload. The event loop owns the
  exact preview path, generation, presentation kind, source, and crop recovery
  context through one bounded completion slot. Superseded rows cancel by image
  generation and late completion cannot publish after owner replacement, while
  the full decoded image remains available for export. Typed failures remain
  retryable. If the presentation worker exits in an unwind build, a queue guard
  closes scheduling and drops pending work so its completion loss remains
  observable even after owner replacement. Endpoint loss becomes persistent,
  visible recovery state rather than false busy state or scheduling into a dead
  executor. Retry is disabled only while the current load specifically requires
  that lost executor; a later ordinary decode failure remains retryable. A typed
  output contract admits only matching RGBA8 sRGB working pixels and an sRGB
  presentation surface; unsupported surface formats fail explicitly instead of
  changing the transfer function. A Spot Heal patch updates the base level and
  regenerates dependent mips. No scene graph.
- **`color`**: the narrow color contract shared by decode, edits, previews, and
  presentation. It names the working color space and pixel format separately from
  the renderer-owned output transform. The only shipping contract is RGBA8 sRGB
  to sRGB. This deliberately prevents a future higher-precision or wide-gamut
  decoder result from entering the old upload path until a compatible output
  transform exists. Preview generation, thumbnail upload, and export enforce the
  same boundary rather than silently reinterpreting unfamiliar pixels.
- **`decode`**: opens one source object and turns that exact handle into RGBA
  pixels. The live handle and native object identity travel with every accepted
  foreground or speculative result instead of being reconstructed from its path.
  Pure-Rust formats decode the duplicated handle on background threads via the
  `image` crate. For complex C-backed formats, the main process reads bounded
  encoded bytes from the same handle and delegates them to an
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
  superseded work exits between frames, and replacement frames duplicate the
  already accepted source handle rather than reopening its path.
- **`image_info`**: best-effort local format, size, camera, lens, exposure,
  aperture, ISO, focal length, date, and privacy-category inspection. Auxiliary
  work duplicates the retained `ImageSource` handle that supplied accepted pixels
  instead of reopening the pathname. The UI publishes bounded supported-EXIF tag
  count plus location, authorship, unique identifier, description, software,
  thumbnail, and maker-data presence without retaining or exposing the raw values
  behind those presence-only categories. It also states that this is not proof against other metadata or hidden
  pixel data. Container extraction, TIFF
  directory count, component sizes, offsets, recursion, thumbnails, and aggregate
  allocations are preflighted under fixed limits before EXIF decoding. TIFF IFDs
  may live anywhere in the source file: the reader seeks only bounded metadata,
  removes pixel-strip locators, and compacts the result before parsing.
- **`view`**: pure geometry for placing an image in the viewport, including
  physical-pixel insets reserved by docked chrome, fit math, zoom, and pan.
  Unit-tested without a GPU.
- **`edit`**: exact pixel transforms, crop, and save-as/convert. Export re-encodes
  from the visible pixels and strips metadata by default without cloning a second
  RGBA buffer. Crop validates source shape, uses fallible output allocation, and
  checks cooperative cancellation before allocation and between copied rows. Its
  event-loop-owned job retains the exact source generation, selected and
  presented path, decoded allocation, edit transform, animation, and auxiliary
  work needed to reject stale completion or restore a retryable selection.
  Discarding the job rejects late publication. A typed computation failure keeps
  direct retry available, while endpoint loss persistently disables another crop
  until restart instead of promising recovery from an unsupervised thread.
  Save As rejects a destination that aliases the open source before encoding and
  builds the complete output in a sibling temporary file before one atomic
  replacement attempt. Explicit session-only EXIF retention is content-driven,
  supports JPEG, PNG, and WebP destinations, uses the bounded metadata reader,
  and normalizes orientation, output dimensions, and stale thumbnail offsets
  before writing supported tags. The event loop owns one bounded Save As result
  slot while the worker receives a non-cloneable consuming completion endpoint.
  Rating replacement and Save As exclude each other at their method boundaries.
  A normal close waits for a successful captured output transaction; failure
  cancels deferred close so its guidance remains visible. Completion updates only
  path-free status for the captured image snapshot, not foreground image state,
  so changing the current selection cannot admit a stale pixel mutation. Endpoint
  loss clears busy ownership, retains a persistent recovery state that disables
  another export, and requires a process restart; release builds do not claim
  recovery from an in-process thread panic.
- **`heal`**: pure-Rust spot-heal preparation, bounded region extraction,
  deterministic edge-aware ranking of up to eight spatially distinct patches,
  robust boundary tone adaptation, adjustable feathered compositing,
  distance-ordered directional fallback inpainting, and byte-bounded in-memory
  pixel-patch history. Refresh Source reuses the retained bounded job, replaces
  the latest result without creating another undo step, and never retains the
  full decoded image. A path-free worker shares immutable decoded pixels just
  long enough to copy the bounded working region, drops the full image, and then
  computes the repair. Apply, refresh, undo, and redo treat decoded pixels,
  bounded history, GPU presentation, and success copy as one transaction. They
  upload only the changed base-texture rectangle before dependent mips are
  regenerated, fall back to full-texture presentation, and restore exact pixels
  plus history when presentation fails. The tool is
  unavailable if the adapter cannot
  represent the complete decoded image in one texture, preventing an ambiguous
  source-to-display coordinate mapping.
- **`curate`**: move to the OS trash or recycle bin and restore for undo. On
  macOS, a native receipt retains the exact resulting trash URL so restoration
  does not depend on an unsupported global trash listing. Windows and Linux take
  an in-memory snapshot of existing Trash identifiers before moving, then accept
  exactly one new same-origin identifier only when the Trash object's native file
  identity matches the retained accepted-source handle. The receipt keeps that
  handle open to prevent identity reuse, and restore repeats the identity check
  before selecting the identifier. A Windows or Linux batch restore enumerates
  Trash once and consumes each exact identifier from that shared snapshot. It
  never falls back to pathname matching.
  External platform errors are mapped to fixed path-free user categories and
  retry dispositions before they leave this boundary. Identifiers are not logged
  or persisted.
- **`macos`**: the narrow native bridge for Finder and Open With requests plus
  recoverable `NSFileManager` trash operations. It is absent from other builds.
- **Windows Open With boundary**: the File and image context actions verify that
  the current pathname still resolves to the retained accepted source, then call
  `SHOpenWithDialog` with `OAIF_EXEC`, a parent HWND, and one NUL-terminated UTF-16
  path. No shell command, editor preference, path log, or completion inference is
  introduced. Successful delegation sets only a session-local, path-free `F5`
  reminder that clears when a new load starts.
- **`sandbox` / `worker_limit`**: spawn and pool `viewr-decode`; platform-specific
  Job Object, process-group, seccomp, package-sandbox, memory, hard-deadline, and
  generation-cancellation controls for helpers.
- **`fs`**: recognizing regular image files (core and worker extensions), excluding
  symlinks from automatic scans, natural-sort ordering (`img2` before `img10`),
  and versioned native file identity used to bind a displayed source to guarded
  mutation.
- **`ratings`**: bounded JPEG header, XMP, and existing IFD0 `0x4746` parsing;
  complete rating-state reconciliation; and the narrow Windows source-write
  transaction. XMP is canonical. An existing valid SimpleRating mirror is updated
  without growing or relocating the TIFF directory. The transaction snapshots the
  retained accepted source, stages beside it under source-equivalent security,
  revalidates identity and bytes, calls `ReplaceFileW`, reopens and verifies the
  candidate, and either removes the retained original or reconciles it. Other
  platforms expose the reader and remain read-only.
- **`playlist`**: one canonical naturally ordered folder catalog plus rating state
  and a derived minimum-rating projection. Navigation, Home and End, Folder
  Previews, and prefetch consume projected canonical indices. Trash and Undo retain
  canonical positions. A just-rated image can remain explicitly outside the
  active filter until the next navigation action without creating a second list.
- **`prefetch`**: an in-memory LRU bounded to five decoded neighbors and 256 MiB,
  whichever limit is reached first, plus generation-tagged scheduling that makes
  failed or uncacheable outcomes terminal for the current playlist. Entries use
  shared immutable decoded ownership plus its paired source handle so a nearby
  just-left pristine frame can
  enter the cache without copying pixels. Entries and scheduling state are never
  persisted.
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
  inspector, accessible rating and threshold radio groups, first-write disclosure,
  filtered-empty recovery, About modal, appearance picker, load/retry state, and
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
   reads and decode do not freeze navigation. When navigation reaches a neighbor
   already decoding speculatively, the first valid selected-path completion is
   uploaded and the redundant request is cancelled. First pixels appear as fast
   as either accepted path allows.
3. **The last good frame stays presented.** A cache miss, explicit reload, or
   decode failure does not clear the current GPU texture. Results replace it only
   after successful decode and only when their path/generation still matches the
   selected source. If navigation returns to that still-presented frame while it
   remains a pristine source decode, the abandoned generation is cancelled and
   the session settles without decode or upload. Load errors remain actionable
   through Retry. The UI derives a genuine-open signal separately from its broader
   image-preparation busy state. Its target status uses a bounded, path-free
   filename from the current selection, while displayed metadata continues to
   identify the presented pixels; derived crop preview work is labeled separately.
   Crop commands independently require a settled successful source load, including
   keyboard entry points, so retained last-good pixels after a failed Reload cannot
   be edited as if they belonged to the selected generation.
   Source-load preview queue, preparation, upload, and channel failures enter the
   durable selected-file error state. Crop compute, preview, channel, or renderer
   failure stays edit-specific, leaves the original presentation unchanged, and
   restores its exact current-source selection. Crop recovery requires matching
   generation, selected and presented paths, and decoded Arc identity.
4. **Neighbors are prefetched.** After the current image, `decode` speculatively
   decodes nearby entries in the background and parks a bounded set of RGBA
   results in RAM. A terminal outcome is attempted once until the speculative
   generation changes or a successful foreground open makes the path eligible
   again. Per-job cancellation stops superseded reads after playlist replacement,
   explicit Reload, or a same-path foreground win; old-generation completions are
   ignored. After moves within two positions, a just-left pristine source decode
   is shared into this same LRU after replacement, while larger jumps, edits,
   playback frames, explicit-Reload state, and oversized images remain excluded.
   Navigation can upload an already-decoded frame immediately.
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

Trash restore retains an explicit event-loop ownership boundary. The worker owns
cloned exact receipts; the event loop owns captured playlist scope, indices, prior
Undo state, and the only commit. Active or indeterminate restore ownership
suppresses settled UI claims, so the interface cannot become a second recovery
state owner. Conflicting mutations wait while zoom, pan, panels, and appearance
remain responsive. Normal close defers through terminal reconciliation and join.
The UI exposes an indeterminate worker-loss route instead of cancellation or
fractional progress. An indeterminate restore retains its receipt and blocks the
new Trash action that could replace it. `U` remains the typed reconciliation path;
permanent delete does not create a second Undo owner.

The correction is not a framework rewrite. Extract pure state transitions with
explicit inputs and outputs, keep `App` as the sole runtime owner, and make winit,
wgpu, dialogs, and native accessibility thin adapters. Each extraction must land
with behavior-preserving tests and no second source of truth.

## The Trash workflow

- `Delete` moves the current file to the OS Trash through the supported native
  integration, records an exact recoverable receipt when the platform exposes one,
  shows a non-blocking result toast, and **advances to the image that took its
  place** rather than jumping to the top. Normal Trash has no modal. `U` restores
  the latest safely recoverable action. A receiptless successful move routes
  recovery to system Trash and preserves an older valid `U` action.
- There is no bare-letter Trash shortcut and no mark, review, or batch mode. The
  destructive target is always the currently visible image. The accepted-source
  handle supplies an identity proof immediately before the pathname Trash sink.
  Missing, replaced, linked, and identity-unavailable entries never reach that
  sink. The comparison narrows accidental replacement risk but remains a
  documented non-atomic boundary because desktop Trash integrations consume a
  pathname.
- Each successful Trash action retains its exact in-memory playlist identity, so
  Undo after a folder change restores on disk without inserting a source-folder
  path into the unrelated current view. Restore work runs on the typed worker. A
  fixed top status names the operation; the event loop reconciles the result once
  against the captured playlist scope. Normal close waits for reconciliation, and
  worker loss preserves the receipt with durable polite recovery guidance.

`Shift+Delete` is the only permanent delete, and it's the only action that shows a
confirmation. The dialog names only a bounded, quote-safe filename and exposes
Delete permanently and Cancel as its two actions. Single Trash and permanent
delete reuse the retained source handle that supplied accepted pixels. Single
Trash performs an immediate no-follow native-identity comparison. Permanent
delete performs the comparison before opening confirmation and repeats it after
confirmation immediately before `remove_file`. Entries detected as changed,
missing, linked, or identity-unavailable do not reach either sink. The final
platform calls still consume a pathname, so the narrower post-comparison race
remains explicit.
Successful permanent deletion preserves an older valid Trash receipt and reports
that `U` refers only to that prior action. Restore failures are typed as immediate
retry, resolve-then-retry, manual system Trash review, or terminal. Only the first
two retain in-app retry ownership. Restore repeats the source-identity check and
then asks the platform to move the checked identifier on Windows and Linux or the
checked Trash URL on macOS in a later call. This remains a narrow non-atomic
boundary, not a handle-bound restore transaction.
Foreground load or preview work, crop, Save As, an active heal stroke, and a heal
worker own current state, so delete and restore shortcuts wait with a specific
reason instead of racing them. Windows embeds the Common Controls v6 activation
manifest required by the native custom-button dialog and requests only the
caller's current privilege. Windows and Linux receipt capture synchronously list
Trash once before the move and once afterward. The moved item receives one fixed
path-private capture result: bound, baseline
listing failed, final listing failed, no new same-origin candidate, ambiguous
identity-bound candidates, or retained-source identity mismatch. The matcher
still accepts exactly one new same-origin item only after native identity matches;
classification adds evidence, not a fallback.

With opt-in `viewr=info` logging, Undo reports submitted, restored, failure, and
total native restore values. Fixed
start, deferred-close, reconciliation, and disconnect records identify only the
operation category and submitted count. These records contain no paths,
filenames, Trash identifiers, native identities, or raw platform errors, and they
are neither persisted nor sent off-machine. The timing is local diagnostic
evidence, not a CI budget or cross-platform latency claim. The active state
deliberately has no percentage, estimate, or cancellation claim.

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
  Job Object to one process and 1.5 GiB aggregate memory. macOS workers use a
  private session, while signed helpers inherit the application's network-denied
  App Sandbox. Supported BSD targets additionally apply address-space and
  one-process resource limits. All workers have containment lifetime controls,
  typed bounded responses, a hard request deadline covering send and receive,
  and foreground-generation cancellation that terminates stale blocked requests.
  Pure-Rust formats remain in the main process but decode off the UI thread under
  the same dimension, allocation, and aggregate concurrency limits.
- **Trash, not unlink**, by default: the filesystem is treated as precious.

## What is intentionally absent

No account layer. No sync engine. No network client. No telemetry/analytics/crash
pipeline. No plugin host. No embedded database of your library. No background
service or auto-updater that runs when the app is closed. Each of these is a
common source of the bloat, the privacy leaks, and the attack surface we exist to
avoid. Leaving them out is an architectural decision, not an omission.
