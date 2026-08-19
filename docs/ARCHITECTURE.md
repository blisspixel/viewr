# Architecture

How viewr is put together, and how a keystroke becomes a pixel. This describes the
current implementation and names the seams that still need simplification.

## Design goals that shape the architecture

1. **Instant.** The first frame of an image must appear as fast as the disk and
   decoder allow, never blocked on anything else.
2. **Never janky.** The UI thread never decodes or performs a bulk image read.
   Heavy image work and destructive platform calls are off-thread; narrow native
   dialogs and permanent-delete confirmation remain synchronous user-triggered
   boundaries and are tracked explicitly below.
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

Image decoding and folder scanning run off the event thread. Folder scans use one
event-loop-owned completion and cooperative cancellation, reject more than
100,000 supported entries or 64 MiB of cumulative encoded path storage instead of
truncating, and surface endpoint loss. Enumeration and child opens are checked
against one retained directory identity. Each admitted child is opened relative
to that handle without following its final component, and its native identity
plus version travels with the playlist entry. Automatic decode, prefetch,
thumbnail, and rating work must match both fields. An explicit F5 refresh may
adopt a new ordinary file at the same path, but it still does not follow a newly
substituted link.
Results wake the event loop through a typed `UserEvent`, request a redraw, and are
applied only if they still match the current path or generation.
The initial decode and folder scan start before renderer initialization, so GPU
setup does not unnecessarily serialize first-pixel work. Native dialogs,
including permanent-delete confirmation, remain synchronous and user-triggered.
The Trash, permanent-delete, and restore platform calls use one typed curation worker,
a one-result channel, and an `EventLoopProxy` wake; only the event thread
reconciles receipts, playlist scope, and visible results. The general performance
probe does not exercise native Trash operations, so their latency is not claimed
by that gate.

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
    // At most four event-loop-owned speculative decode jobs across generations.
    prefetch_schedule: PrefetchSchedule,

    // View, crop, animation, details, and in-memory pixel edit state.
    transform: Transform,
    animation: Option<AnimationPlayback>,
    image_details: Option<ImageDetails>,
    heal: HealTool,

    // Folder scan, auxiliary, Save As, crop, and display-preview work each has a
    // one-result, event-loop-owned job.
    folder_scan_job: Option<OneShotJob<FolderScanContext, FolderScanResult>>,
    auxiliary_job: Option<OneShotJob<AuxiliaryLoadContext, AuxiliaryLoadResult>>,
    save_job: Option<OneShotJob<(), SaveResult>>,
    close_after_save: bool,
    save_recovery_unsettled: bool,
    crop_job: Option<OneShotJob<CropJobContext, CropJobResult>>,
    crop_recovery_unsettled: bool,
    preview_job: Option<OneShotJob<PreviewJobContext, PreviewJobResult>>,
    preview_recovery_unsettled: bool,

    // At most nine event-loop-owned filmstrip jobs and GPU textures.
    thumbnail_schedule: ThumbnailSchedule,
    thumb_textures: HashMap<PathBuf, TextureHandle>,

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
`crop`, `playlist`, `performance`, `job`, `prefetch`, `thumbs`, and `chrome`
modules establish smaller state and logic seams without introducing a second
mutable store. `App` remains a large orchestrator. Narrowing native event
plumbing is explicit roadmap work.

## Modules

Shipped:

- **`app`**: the winit application handler and centralized state/input dispatch.
  It opens command-line and native file requests, schedules background work, and
  feeds one immutable frame snapshot to the UI. Every path that arrives from
  outside the window, whether from the command line, a drop, a desktop Open
  With, or the macOS open-file event, is classified once by `entry_state`: a
  folder starts the same browse the Open Folder button starts, and everything
  else is presented as a file so a missing path still names itself.
- **`job`**: the bounded, one-result ownership boundary used by current-image
  details, animation discovery, rating observation, folder scans, Save As, crop,
  over-limit display previews, each active filmstrip thumbnail, and each
  speculative neighbor decode. The event loop retains operation context while the worker
  receives one non-cloneable completion endpoint. Replaced work cannot publish,
  context-owned cancellation flags stop obsolete crop or prefetch work, and an
  endpoint that closes without a result wakes the event loop so it becomes a
  terminal state instead of permanent false busy state. An acceptance-armed wake
  handshake installs every accepted owner before notification and keeps rejected
  work silent and unowned. The interface requires a restart after an unexpected
  required executor failure; release builds do not claim general thread-panic
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
- **`display_state`**: covered monitor-identity comparison, display-color
  policy, and display-ICC admission. `App` supplies the name, origin, size, and
  scale winit already reports, plus the window-system class launch already
  resolved. A display ICC is fetched only when the policy would apply it:
  unmanaged Windows-legacy and real X11. `display_probe` reads Windows ICM
  profile bytes under the protocol size cap, asks DisplayConfig whether
  Advanced Color is enabled, and on real X11 reads the root-window
  `_ICC_PROFILE` atom through libX11. `display_output` then converts working
  sRGB into that admitted encoding at GPU upload. Managed compositors stay
  tagged sRGB, unmanaged X11 without an admitted profile reports the
  deterministic sRGB fallback, and moving the window between unmanaged
  displays rebuilds the transform and re-uploads the current picture.
  Export, edits, and thumbnails stay in working sRGB.
- **`file_coherence`**: covered session policy for external file and folder
  changes. It maps source identity observations and directory stamps onto
  reload, F5 reminder, current-gone, and folder-rescan actions, coalesces
  noisy bursts, and blocks silent reload while crop, heal, rotate, flip, or
  other work owns the current source. `App` owns the watcher thread and
  playlist mutation. A pending observation cannot act unless the presented
  file is still the path that thread was started against. Open With
  availability is a native chooser on every shipping host.
- **`decode`**: opens one source object and turns that exact handle into RGBA
  pixels. The live handle and native object identity travel with every accepted
  foreground or speculative result instead of being reconstructed from its path.
  Identity and version are captured together. The retained handle remains the
  only byte source. For a markable source, its selected pathname is also retained
  in memory and reopened without following the final component only to prove that
  it still resolves to the accepted identity and version. A successful decoder
  result is discarded if either the retained object changed or its pathname was
  replaced before publication, including an in-place rewrite of the same object
  while decoding. Because Windows can cache or preserve every writable timestamp
  across a same-length rewrite, each Windows source also retains an in-memory
  SHA-256 content witness. It is checked through serialized handle reads and is
  never persisted, exposed, or used as an identifier. Source acceptance enforces
  the shared 512 MiB encoded-input ceiling before that streaming work. Decode
  generation cancellation is checked between fixed 64 KiB chunks, including
  later witness comparisons. The replace-latest animation, details, and rating
  task propagates the same generation check through every comparison and exits
  between stages, so obsolete inspection cannot monopolize its sole executor.
  Each replace-latest queue is supervised: a worker that stops, including one
  that unwinds, decrements a live-worker count, and the last one out closes the
  queue. A queue served by several workers therefore survives losing one, and a
  queue with none left rejects further work instead of accepting jobs no thread
  will run, which would leave the event loop waiting for a completion that
  cannot arrive. `schedule_foreground_decode`, `schedule_current_image_details`,
  and `schedule_image_preview` turn that rejection into a named error the viewer
  reports.
  Folder-rating discovery cannot grant write capability: its separate read-only
  source checks native scan identity and version around two reads of at most the
  16 MiB JPEG header. The exact bytes
  consumed by the parser and the parsed result must match, including parse-failure
  paths, with cancellation between segments.
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
  SVG installs a fail-closed href resolver: vector shapes and paths remain
  supported, while embedded raster data and external file references return one
  fixed error before rendering. A streaming preflight also rejects image elements
  before URL decoding and rejects document type declarations before entity
  expansion, caps markup depth, element count, cumulative attribute count, and
  cumulative parsed-attribute bytes, and excludes expansion-heavy `use` and
  marker nodes. Cumulative `d` and `points` payloads have byte and token ceilings
  before usvg can materialize path segments. A parsed-tree walk bounds
  simultaneously live opacity, blend, and isolation layers, total painted and
  layer area, parsed nodes, path segments, and tile-amplified edge work.
  Stylesheets, inline styles, text, gradients, strokes, filters, masks, clipping paths,
  and paint patterns fail closed because their expansion, subdivision, or scratch
  allocation is not independently fallible and bounded. The local JPEG XL color
  patch requires the exact 12-byte CICP tag layout, including zero reserved
  bytes, preserves PQ and HLG transfer evidence without relying on LUT parsing,
  checks exact AVX2 dispatch requirements, and gives transform errors precedence
  over successful parallel chunks. Composition errors publish a terminal render
  state and notify waiters.
- **`animated`**: bounded GIF, WebP, and APNG frame decode plus deterministic
  deadline, pause/resume, and finite/infinite loop state. It is current-image-only
  RAM state and never a disk cache. Containers are identified by content after
  any misleading rename, APNG receives allocation limits at decoder construction,
  superseded work exits between frames, and frame decoding duplicates the already
  accepted source handle rather than reopening its path. Source version is
  checked before and after the full frame decode; a rewrite or rename discards
  the animation instead of pairing it with stale accepted pixels.
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
  replacement attempt. The event loop retains the canonical destination parent
  identity plus whether the destination was absent or the exact native identity
  and version present after the native dialog. The destination capability exposes
  no reader, performs no full-file content witness on the event loop, and accepts
  an existing destination independently of the encoded-source size ceiling. Every
  captured existing file receives
  a second, app-owned overwrite prompt, and its identity and version are checked
  after that consent, during staging, and immediately before commit. The prompt
  exclusively owns UI and assistive-technology actions. Every source transition,
  including an operating-system open, drag and drop, completed explicit folder
  open, or rating-filter result that replaces or clears the selection, cancels it
  before changing the selected source. Save As is unavailable while an explicit
  folder-open scan can still install a new source. Explicit
  session-only EXIF retention reads through a
  serialized duplicate of the accepted source handle
  that supplied the displayed pixels and rejects any available length,
  modification-time, or change-time evidence of a rewrite or rename before or
  during extraction. It supports JPEG, PNG, and WebP destinations, uses the bounded
  metadata reader, and normalizes orientation, output dimensions, and stale
  thumbnail offsets before writing supported tags. Pixel encoding and optional
  EXIF insertion both write through the retained temporary-file handle. Its final
  pathname must still identify that handle before commit, and an absent confirmed
  destination installs with a no-clobber primitive so a concurrently created file
  is never overwritten. The event loop owns one bounded
  Save As result slot while the worker receives a non-cloneable consuming
  completion endpoint.
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
  never falls back to pathname matching. After success, the restored pathname is
  reopened without following its final component and must match the retained
  object identity. That fresh handle supplies rating observation and new
  identity-plus-version playlist provenance on the curation worker before the
  event loop commits restored entries.
  External platform errors are mapped to fixed path-free user categories and
  retry dispositions before they leave this boundary. Identifiers are not logged
  or persisted.
- **`macos`**: the narrow native bridge for Finder open-file delivery, the
  Open With application picker plus NSWorkspace handoff, and recoverable
  `NSFileManager` trash operations. It is absent from other builds.
- **Open With boundary**: File and the image context action verify that the
  current pathname still resolves to the retained accepted source on one
  generation-cancellable background job, then present a native user-mediated
  chooser on the event loop: Windows `SHOpenWithDialog`, macOS NSOpenPanel
  plus NSWorkspace, or Linux desktop-portal OpenURI with `ask`. Navigation
  cancels and discards obsolete verification. No shell command, editor
  preference, path log, or completion inference is introduced. A session
  watcher reloads a changed source when edits are safe; otherwise a
  path-free `F5` reminder stays attached to the last-good frame.
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
  whichever limit is reached first, plus at most four event-loop-owned one-result
  decode jobs across current and cancelled generations. Owner context retains the
  only publishable path, generation, cancellation state, and foreground-win bit.
  Stale work cannot publish, queue rejection owns nothing, and typed decode or
  endpoint failures become terminal for the current playlist without including a
  path in the worker result. Entries use shared immutable decoded ownership plus
  its paired source handle so a nearby just-left pristine frame can enter the
  cache without copying pixels. Entries and scheduling state are never persisted.
- **`thumbs`**: one event-loop-owned schedule capped at nine active folder-preview
  jobs and nine retained GPU textures for the visible filmstrip window. Exact
  generation and visibility gate publication. Workers return only structurally
  valid sRGB RGBA8 pixels or a stable path-free failure category. Typed failure
  and closed completion are terminal only while the path stays visible; unwind
  builds also contain worker panic at this boundary. Leaving the window or
  resetting the generation permits an explicit retry. Executor saturation owns
  nothing and stays retryable. An acceptance-armed wake handshake makes fast or
  disconnected accepted work observable without letting rejected work spin the
  event loop.
- **`theme`**: resolves System, Light, Dark, or Console against winit's native
  decoration and theme signal, supplies complete GPU and chrome color tokens,
  and reads or writes one validated appearance word in the platform configuration
  directory. The bounded read rejects oversized and unknown values.
- **`performance`**: stable, path-free probe output and narrow platform peak-RSS
  readers used only by the explicit developer/CI performance command.
- **`startup`**: launch prerequisites and first-window geometry. Session
  detection, the dynamically loaded library tables, backend selection, graphics
  runtime reporting, every launch message, and the monitor-bounded default window
  size are pure. Only the loader probe and the environment reads touch the
  platform, and `app` supplies the monitor extent winit reports.
- **`error`**: the typed error set for the app.
- **`chrome`**: the pure, immutable projection from one event-loop-owned frame
  snapshot to dock layout and control presentation. It derives applicability,
  readiness, selected state, labels, shortcuts, and accessibility copy without a
  window or a second mutable store. The same captured dock facts reserve the GPU
  viewport and drive the painted panels, preventing layout and visible chrome
  from observing different state.
- **`ui`**: the thin `egui` and AccessKit adapter. It paints the conventional menu
  bar, fully hideable and collapsible docked tools and folder previews, left/right
  Image Information,
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
   Starting another scan cancels ownership of the previous scan. Enumeration is
   nonrecursive and capped at 100,000 supported regular files plus 64 MiB of
   cumulative encoded path storage. Cancellation is observed during enumeration
   and natural-sort comparisons. Exceeding either cap is visible and never
   installs a silently truncated playlist.
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
   results in RAM. Each of at most four accepted jobs owns one result endpoint;
   cancelled old-generation owners remain bounded and observable until they
   finish, but their pixels cannot publish. A terminal outcome is attempted once
   until the speculative generation changes or a successful foreground open
   makes the path eligible again. Per-job cancellation stops superseded reads
   after playlist replacement, explicit Reload, or a same-path foreground win.
   After moves within two positions, a just-left pristine source decode is shared
   into this same LRU after replacement, while larger jumps, edits, playback
   frames, explicit-Reload state, and oversized images remain excluded. Navigation
   can upload an already-decoded frame immediately.
5. **Caches are bounded.** Full decoded neighbors are capped by both count and
   decoded bytes, speculative neighbor work is capped at four accepted owners,
   thumbnail work and visible GPU textures are each capped at nine, and the
   renderer owns one current image texture plus its mip levels. Image-cache memory
   does not grow with folder length; the lightweight playlist path index
   necessarily does.
6. **Panning/zooming is pure GPU.** The decoded frame is a texture; pan and zoom
   are changes to the sampling transform, with mip selection handled by the
   sampler. No re-decode and no CPU resampling occur per frame.

The rule: **the UI thread only ever draws things that are already ready.** All
"getting ready" happens off-thread and arrives as messages.

## Current architecture pressure

`app.rs` owns the event loop, input table, asynchronous job lifecycle, crop
geometry, curation, and frame assembly. `ui.rs` paints an immutable frame through
the covered `chrome` policy, so dock layout, menu enablement, selected state, and
accessibility presentation no longer live in native paint code. The UI adapter is
inside the 85 percent CI coverage floor because its egui and AccessKit output is
exercised without a native window.

The covered `presentation` seam owns immutable loaded-versus-cropped identity,
pristine-pixel reuse eligibility, selected-versus-presented navigation planning,
opening-state classification, durable load-error selection, and preview result
identity. It owns no path, image, cache, job, playlist, session, renderer, window,
or event-loop state. `App` supplies one snapshot of those facts and remains the
only owner that applies the decision, mutates selection, schedules work, or
publishes pixels.

The covered `curation_state` seam defines the value state for Trash, permanent
delete, and restore lifecycle policy. It derives fixed recovery priority and
guidance, source-removal preflight, operation status, count grammar, and every
deferred-close disposition from explicit inputs. `App` stores that value, owns
the worker and all paths, receipts, playlist changes, and visible recovery
application, and remains the only mutable curation owner.

The covered `rating_state` seam defines presented-rating, recovery, discovery,
terminal-write, and deferred-close policy. It receives immutable facts after the
event loop has observed a worker channel state and joined the terminal writer.
`App` remains the only owner of accepted sources, paths, workers, disclosure,
playlist mutation, visible recovery, UI dispatch, and close application. A
worker panic or disconnected terminal channel therefore becomes the same fixed
indeterminate recovery result without creating another mutable rating store.

The covered `save_state` seam defines Save As start blockers, folder-scan save
gates, terminal close disposition, and app close wait coordination. `App` remains
the only owner of destinations, image buffers, Save As workers, native dialogs,
and close application. Recovery status copy lives beside the other recovery
strings so chrome and Save As preflight share one fixed message.

The covered `gpu_image` seam owns CPU-only texture sizing, mip planning,
linear-light alpha-correct preview preparation, and upload selection. The covered
`gpu_policy` seam owns first-supported sRGB surface selection, full-resolution
patch geometry validation, exact placement-buffer packing, and clear-color
mapping. Neither creates a device resource or owns event-loop state. `gpu.rs`
keeps the device, queue, surface, texture, pipeline, sampler, mip generation, and
frame lifecycle, consuming the validated decisions while preserving one renderer
owner and the existing asynchronous preview contract.

The exact-path exclusions remain because the application and renderer must
exercise native event-loop and wgpu outcomes. Remaining renderer helpers are small
and coupled to that adapter rather than hidden domain policy. Concentrated
shortcut, event, remaining crop recovery, rating, save, and curation integration
transitions in `app.rs`, plus pure entry parsing, remain explicit v0.2 extraction
debt.
Meaningful decisions must keep moving behind narrow covered seams before later
page-state work expands them. File-coherence policy, including reload, F5
reminder, gone, rename, and folder-rescan decisions, already lives in the
covered `file_coherence` module.

Source removal and Trash restore retain an explicit event-loop ownership boundary.
The worker owns strong accepted-source validation, the Trash or permanent-delete
platform call, cloned exact restore receipts, and restored rating/provenance
inspection. The event loop owns captured playlist scope, indices, prior Undo
state, and the only commit. Active or indeterminate curation ownership
suppresses settled UI claims, so the interface cannot become a second recovery
state owner. Conflicting mutations wait while zoom, pan, panels, and appearance
remain responsive. Normal close defers through terminal reconciliation and join.
Failure or a partial restore cancels deferred close so visible recovery guidance
is not lost. The UI exposes an indeterminate worker-loss route instead of
cancellation or fractional progress. An indeterminate Trash or permanent-delete
operation blocks another destructive action for the rest of the process and
explicitly requires closing and reopening viewr after inspecting the filesystem.
An indeterminate restore retains its receipt and blocks the new Trash action that
could replace it. `U` remains the typed reconciliation path; permanent delete
does not create a second Undo owner.

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
  handle supplies a strong identity and content proof on the curation worker
  immediately before the pathname Trash sink.
  Missing, replaced, linked, and identity-unavailable entries never reach that
  sink. The comparison narrows accidental replacement risk but remains a
  documented non-atomic boundary because desktop Trash integrations consume a
  pathname.
- Each successful Trash action retains its exact in-memory playlist identity, so
  Undo after a folder change restores on disk without inserting a source-folder
  path into the unrelated current view. Trash and restore work run on the typed
  worker. A fixed top status names the operation; the event loop reconciles the
  result once against the captured playlist scope. Normal close waits only for a
  successful reconciliation. A failure stays visible, and worker loss preserves
  durable polite recovery guidance.

`Shift+Delete` is the only permanent delete, and it's the only action that shows a
confirmation. The dialog names only a bounded, quote-safe filename and exposes
Delete permanently and Cancel as its two actions. Single Trash and permanent
delete reuse the retained source handle that supplied accepted pixels. Single
Trash performs the full comparison on its worker. Permanent delete performs a
bounded native-only comparison before opening confirmation, then its worker
repeats the full identity and content comparison immediately before `remove_file`.
Entries detected as changed,
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
