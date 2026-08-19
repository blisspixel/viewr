# Privacy

This is both viewr's design contract and the plain-language statement users will
eventually read. Privacy in viewr is not a policy promise. It is a property of the
code, and where possible it's enforced in CI so it can't quietly regress.

## The guarantee, plainly

- viewr **never connects to the internet.** It has no HTTP, TLS, remote-service,
  telemetry, or update client. Linux permits local Unix-domain IPC for desktop and
  accessibility integration, while an early kernel policy denies Internet socket
  creation and io_uring.
- viewr **collects nothing.** No telemetry, no analytics, no usage metrics, no
  "help us improve" data, no crash reports sent anywhere.
- viewr **keeps no logs of your activity.** Which files you open, which folders you
  browse, and which images you delete are never recorded to a server, and **not
  written to any log or side-file on disk**. There is **no log file**, not even
  an empty one.
- viewr **has no account and no cloud.** There is nothing to sign into and nothing
  to sync.
- Your photos, filenames, and folder structure **never leave your machine.**
  viewr does **not** build or retain a library index, thumbnail database, or
  "recent folders" list of your collection.
- **Native dialog and external-app boundary.** Open, Open Folder, Save As path
  choice, and permanent-delete confirmation use native operating-system dialogs.
  When a captured Save As destination already exists, viewr adds an app-owned,
  identity-bound overwrite confirmation. Open With also delegates the
  exact current source to an application the user
  explicitly selects. That application receives the original file, path, and
  embedded metadata and may read, transmit, modify, or replace it under its own
  privacy and security rules. viewr constructs no shell command, remembers no
  editor choice, and retains no handoff history. The operating system may record
  paths in its recent-item, quick-access, or jump-list data according to policy.
- **Memory boundary.** Decoded pixels and thumbnails stay in bounded process and
  GPU memory rather than a viewr disk cache. The operating system may page process
  memory, GPU drivers may retain copies, and viewr does not claim secure erasure
  against a live-system or forensic adversary. Full-disk encryption and operating-
  system controls are the relevant at-rest protections for that threat model.
- **Zero routine product temp debris.** The GUI never writes under the system
  temp folder for probes. `viewr doctor` and `viewr benchmark` (without a
  directory) run fully **in memory**. viewr never sweeps matching names from the
  shared temporary directory because name patterns cannot prove ownership.
  Current verification workspaces are uniquely named, atomically created,
  RAII-owned directories that are removed when their owner exits normally.
- The developer/CI performance probe runs only when explicitly invoked, emits one
  path-free measurement record to its caller, and retains no history. Its Python
  harness owns and removes the temporary deterministic image corpus it creates.

There is no setting to turn any of this off, because none of the corresponding
machinery exists in the first place.

## Logging is opt-in (stderr only, never a log file)

By default the process is silent: no `log` output, **no log files on disk**.
Silence covers diagnostics, not failure. A launch that cannot open a window or
create a GPU surface prints one actionable message on stderr and exits non-zero
without any logging variable set.

If you want diagnostics while developing, set an environment variable yourself.
Output goes to **stderr only**; viewr never opens a `.log` file:

```text
RUST_LOG=viewr=debug
# or
VIEWR_LOG=info
```

Even when logging is on, viewr avoids writing full filesystem paths into log
lines. Display chrome, filmstrip labels, Image Information basenames, status
text, and in-process texture debug names use the same privacy-safe filename
helper, so full directory paths do not enter those surfaces. Bare levels such as
`info` apply only to viewr. Only the exact `viewr`
target or a `viewr::` descendant can emit records; prefix lookalikes and
dependency-target directives are rejected because external payloads do not share
viewr's path-private logging contract. Nothing is ever sent off-machine.
Behavior tests prove that no logger is constructed when both logging variables are
absent, that `RUST_LOG` retains precedence over `VIEWR_LOG`, and that an
external-target-only directive constructs no logger. The repository privacy
wrappers add narrow source tripwires around reviewed orchestration and ephemeral
contracts; they are not described as a general Rust write-path analyzer.
Batch curation records use fixed counts and monotonic elapsed milliseconds only.
Receipt-capture evidence distinguishes fixed listing, candidate, ambiguity, and
identity categories without logging original paths, filenames, Trash identifiers,
native identities, or raw platform errors. The records are not retained after
stderr is consumed. The serialized batch worker adds only fixed start,
deferred-close, reconciliation, or disconnect categories with operation type and
submitted count. Exact source handles, paths, receipts, and playlist scope stay in
process memory and never enter those records.

## How the code enforces it

A promise you can verify beats a promise you have to trust.

1. **Remote clients are absent and local Linux IPC is confined.** A CI dependency
   audit **fails the build** if an HTTP, TLS, websocket, QUIC, or remote-service
   client appears. Windows and macOS use AccessKit's platform adapters with default
   features disabled. Linux's upstream AccessKit/AT-SPI adapter needs a generic
   D-Bus implementation; cargo-deny permits that implementation and its process
   helper only behind the reviewed AccessKit dependency path and viewr's
   OpenURI Open With chooser. Before logging,
   workers, GUI initialization, or application threads, Linux rejects configured
   D-Bus addresses that are not `unix:` transports, installs `no_new_privs`, denies
   non-Unix socket families and io_uring with seccomp, mirrors those denials onto
   x32 syscall aliases on x86-64, and verifies that an Internet socket fails with
   `EPERM`. Startup fails closed if any step fails.
2. **Network-denied packaging profiles.** The repository contains local packaging
   profiles with no network entitlement:
   - **macOS**: App Sandbox, no `com.apple.security.network.*` entitlement.
   - **Windows**: AppContainer without the internet capability.
   - **Linux**: Flatpak with **no** `--share=network`; the decode worker also
     installs a seccomp filter that returns `EPERM` for classic socket and
     io_uring networking paths, including x32 aliases on x86-64.
   The test suite checks each profile as an exact allowlist. Linux CI performs a
   clean Flatpak build from checksum-pinned Cargo sources and runs the worker IPC
   probe in its build sandbox. macOS CI ad-hoc signs an App Sandbox bundle and
   runs the same main-to-worker probe. Windows CI makes the SDK validate an
   unsigned AppContainer MSIX containing both binaries. A bare `cargo build`
   does not apply those package boundaries, and schema/signature checks are not
   evidence that an unsigned package was installed. Independently, the
   dependency ban applies to every build. Bare Linux builds still receive the
   application startup policy described above, and Linux worker spawn fails if its
   stricter network-denying seccomp filter cannot be installed. AVIF/HEIC Linux workers
   additionally install a default-deny syscall allowlist before reading IPC;
   Ubuntu 24.04 CI decodes generated AVIF and HEIC inputs through the release-mode
   worker and fails if that policy blocks required decoder behavior.
   File and folder access remains session-scoped. In a sandbox, **Open File** may
   grant only the selected item, while **Open Folder** is the explicit consent path
   for sibling navigation. Outside a file-access sandbox, opening one file also
   scans its containing folder for navigation when normal operating-system
   permissions allow it. Automatic scans include only regular directory entries
   and do not follow sibling symlinks. Enumeration is checked against a retained
   directory handle, and automatic entries carry the regular object's native
   identity and version into decode, prefetch, thumbnails, and rating inspection.
   Each markable accepted source also retains its selected pathname only in
   memory. Version checks reopen that final entry without following links and
   require the same identity and version, but all image and metadata bytes still
   come from the accepted handle. Windows additionally keeps a SHA-256 witness of
   those bytes only in memory because same-length rewrites can preserve every
   writable timestamp. The witness is never persisted, logged, or exposed.
   Accepted encoded sources are limited to 512 MiB before witness work, and
   superseded decode work stops between fixed 64 KiB chunks. Folder-rating scans
   use a separate read-only, native-version-bound header reader. It is capped at
   16 MiB per pass, requires the exact consumed bytes and parsed results from two
   reads to match, stops between segments, and cannot authorize file mutation.
   A symlink chosen directly through Open File remains the explicit selected path
   and does not expand automatic discovery. F5 can explicitly adopt a replacement
   ordinary file but does not follow a link substituted for an automatic entry.
   After sibling access succeeds, prefetch selects at most four nearby paths for
   each current position. The shared executor separately caps queued and
   concurrent decoding, and decoded neighbors remain only in memory; viewr writes
   none of them to disk. Failed or over-budget speculative results remain
   suppressed until the playlist generation changes or a successful foreground
   presentation reopens the path. Stale generations cannot populate the cache,
   and their queued or active reads observe cooperative cancellation. With
   diagnostics explicitly enabled, an effective terminal outcome names only a
   control-safe, bounded filename, never its directory. viewr does not request a
   whole-library capability or persist a folder grant.
3. **No analytics/telemetry SDKs, ever.** There is no analytics dependency to
   configure. This is also enforced by the dependency audit above.
4. **Split decode boundary.** Common pure-Rust formats decode in the main process
   under shape, allocation, and concurrency limits, with a pre-parse input cap for
   SVG. SVG image hrefs are rejected, including embedded raster data and external
   file references, so selected markup cannot read another local image through
   the renderer. DTD expansion, expansion-heavy references, markers, stylesheets,
   inline styles, text references, gradients, strokes, excessive path geometry, unbounded
   renderer scratch features, excessive tree depth, excessive element or attribute count,
   excessive cumulative attribute bytes,
   excessive paint work, and excessive simultaneously live layers also fail closed
   before raster allocation. PNG text, EXIF, and ICC fields plus WebP EXIF and ICC
   fields are bounded before their decoders allocate them. JPEG XL initialization
   rejects encoded,
   declared, or command-amplified ICC output beyond 10 MiB. For optional C-backed
   formats, the main process opens the selected file
   and sends bounded encoded bytes to the worker. The worker receives no path and
   needs no dynamic filesystem grant. Linux denies that worker's classic socket
   and io_uring network paths. Feature-gated C workers further deny every syscall
   outside a reviewed runtime allowlist, while permitting libheif plugin discovery
   only through argument-filtered read-only opens. The documented OS packages
   constrain the whole app. Foreground worker-file reads check the current image
   generation after every bounded chunk. A superseded pipe request is polled at a
   fixed interval and its containment unit is terminated, so stale work cannot
   retain a decode slot for the full deadline.
   Bare Windows and macOS Cargo builds do not claim that package-level boundary.

## Local data: what viewr does and doesn't write

- viewr writes exactly one optional UI preference: the validated word `system`,
  `light`, `dark`, or `console` in the platform configuration directory under
  `viewr/appearance`. It contains no path, timestamp, device identifier, or image
  data. Windows uses `%APPDATA%`, macOS uses `Library/Application Support`, and
  Linux uses `XDG_CONFIG_HOME` or `.config`. Deleting that file quietly restores
  System. Invalid, oversized, unreadable, or unavailable state also uses System
  and shows one fixed recovery notice without the path, raw error, or stored
  content. A failed explicit save keeps the selected appearance for the current
  session and shows fixed recovery guidance. Explicitly enabled diagnostics
  receive only a fixed load reason or failed save phase, never a raw error or
  configuration path.
- viewr **does not** write history, a recently-opened list, thumbnail database,
  photo-library search index, rating/flag/pick database, or general settings
  database. Filmstrip thumbnails, panel visibility/position, and neighbor
  **prefetch** live **only in RAM for the current session** and disappear when the
  app closes (never under temp or beside your photos). The accepted-source native
  identity used for guarded Trash and permanent-delete checks is never persisted,
  displayed, or logged. Replacement, missing, unsupported, unavailable, and
  platform Trash outcomes enter diagnostics only as fixed categories.
- viewr **does not use companion files as product state** (no `_picks.txt`, XMP
  sidecar, or thumbnail cache). Explicit Save As and Windows JPEG rating writes
  do use private same-directory transaction files so the destination can be
  replaced atomically. They are not an index or durable metadata store.
- Durable ratings are the one narrow, explicit exception to the normal
  no-source-mutation viewer behavior. On Windows, ordinary identity-bound JPEGs
  with supported metadata can store a disclosed 0-to-5 preference in standard
  embedded `xmp:Rating`. Other formats and platforms remain read-only. viewr
  never creates a rating database, companion file, alternate stream, metadata
  timestamp field, separate timestamp record, or viewing-history record. Rating
  an image intentionally replaces that source
  file after exact-source checks, bounded parsing, same-directory staging,
  verification, and failure reconciliation. The small preference becomes visible
  to metadata-aware apps, but it records no user identity, assignment time, or
  viewing history. Consent is requested before the first write in each session
  and is never persisted.
- Windows rating snapshots and candidates receive the accepted source's owner,
  group, and discretionary access-control list at creation. The pristine snapshot
  is delete-on-close and immediately unlinked. Normal completion removes the work
  file and retained original. A process or power loss in the narrow interval after
  replacement can leave a source-protected `.viewr-rating-backup-*` original; an
  unreconciled failure can retain a protected work copy for manual recovery.
  viewr does not broadly delete these names on startup because another process may
  own one and a random file must never be mistaken for safe debris.
- Spot-heal strokes, repair regions, and undo/redo pixel patches exist only in
  bounded RAM. Navigation clears them. Decoded pixels, bounded history, and GPU
  presentation commit together; presentation failure restores exact pixels and
  history before reporting failure. The source file is never edited in place.
- A pending crop keeps only its source decode, exact in-memory selection, paused
  animation ownership, and generation token. It writes nothing. Navigation sets
  cooperative cancellation before dropping that state; a current-source failure
  restores the selection, while a stale generation cannot present or restore it.
- **Save As / convert** only writes the file path you choose in the save dialog.
  It validates the source, destination format, pixels, and metadata option first,
  retains the canonical destination parent identity, captures the confirmed
  destination's absence or exact native identity and version, and presents an
  app-owned overwrite prompt for every captured existing file. That event-loop
  capability exposes no reader and performs no full-file hash. After consent, it rechecks
  that exact file before starting work, during staging, and immediately before
  replacement. Pixel encoding and optional EXIF insertion use
  the retained temporary-file handle, and the temporary pathname must still name
  that handle before commit. A destination confirmed absent is installed with a
  no-clobber operation. If a recheck detects that another process created,
  replaced, or changed either boundary, viewr leaves the destination untouched
  and reports failure. A narrow final pathname and parent-resolution race remains
  between that recheck and the operating-system replacement call.
- Deletes go to the **system trash**, so your OS (not viewr) holds the recoverable
  copy under its normal rules. Permanent delete requires an explicit confirmation
  dialog with Delete permanently and Cancel actions, then skips the trash. Its
  filename is bounded, path-free, control-safe, bidi-safe, and quote-safe.
  Single Trash verifies the retained source that supplied accepted pixels on its
  worker before calling the platform. Permanent delete applies a bounded native
  check before confirmation, then its worker applies the full check immediately
  before deletion. Missing, replaced,
  linked, and identity-unavailable entries remain untouched. Fixed rejection
  copy and diagnostics contain neither paths nor native identifiers.
  External Trash errors can contain paths and arbitrary platform descriptions,
  so viewr maps them, including macOS destination inspection failures, to fixed
  categories before showing or logging them. Identity-rejected and failed platform
  Trash attempts leave the current image and playlist unchanged, while retryable
  restore receipts remain only in session memory for explicit retry.
  Windows and Linux receipts contain a new platform Trash item identifier only
  after its native file identity matches the retained accepted-source handle;
  macOS receipts contain the exact resulting URL and the same handle. The handle
  prevents identity reuse, and restore repeats the identity check. These values
  are never logged, displayed, or persisted, and restore never falls back to
  matching an original pathname. After restore, viewr reopens the ordinary final
  path without following a link, requires the same retained identity, and captures
  a fresh version before rating or automatic navigation. Missing items end the
  in-app retry, while
  ambiguous or invalid receipts direct manual system Trash review.
  The File menu exposes only whether Undo Trash is available or unsettled, never a
  filename, original path, Trash identifier, native identity, or history count.
  A successful receiptless move and a permanent delete both preserve any earlier
  valid `U` action; neither claims the new action can be restored in-app. The final
  Trash and permanent-delete operations remain pathname based. Restore also
  makes a later platform call with the checked Trash identifier on Windows and
  Linux or the checked Trash URL on macOS. The final source and receipt checks narrow
  but do not eliminate a hostile final swap before the operating system consumes
  that path, identifier, or URL.
  The Windows dialog manifest requests the current user's privilege, not elevation.

Any future convenience preference such as remembered window size must be
local-only, plainly documented, and easy to clear. It will never be transmitted.

## Optional local models

No model runtime or model weights currently ship with viewr. Any future Describe
Image or advanced repair model must remain separately installed and explicitly
invoked. It may not use localhost HTTP, receive a photo path, write a prompt,
response, history, cache, or log, or run automatically. **It must never silently
write generated tags or descriptions back into the file's EXIF data.** The
process boundary, network denial, zero-write contract, model bake-off, and release
acceptance gate are specified in `docs/LOCAL-INTELLIGENCE.md`.

## Metadata is yours

Images carry EXIF metadata, often including **GPS coordinates**, camera serial
numbers, and timestamps. Bloated apps silently preserve all of it when you export.

Image Information performs a bounded supported-EXIF scan through a duplicate of
the accepted source handle that supplied the displayed pixels. It reports the tag
count and only the presence of location-related data, owner or author fields,
camera, lens, or image identifiers, descriptions or comments, software history,
embedded thumbnails, and opaque maker-specific data. Raw coordinates, serials,
owner names, comments, and maker data are not displayed or logged. `No supported
EXIF detected` is deliberately not called clean or metadata-free: XMP, IPTC,
format-specific fields, filenames, filesystem attributes, application history,
and data hidden in image pixels may still exist. viewr does not claim to detect
steganography or prove that a source is safe to share.

viewr does the opposite by default: on **Save As / convert**, the app re-encodes
the raw image pixels and **drops EXIF, GPS, and all other metadata**. Your address
and identifying fields do not ride along inside a photo you share unless you ask.

**Keep camera metadata when saving** is an explicit checkbox in the Image
Information panel. It defaults to **off**. Turning it on keeps supported EXIF tags
for the rest of that session only; the choice is never written to a settings file.
That opt-in can retain descriptive, camera, timestamp, serial-number, and GPS tags,
so the panel states the privacy consequence directly. Before writing the new copy,
viewr sets Orientation to 1, updates image dimensions, and removes stale thumbnail
offset/length tags so retained metadata cannot contradict the exported pixels.
Automatic inspection and opt-in retention share the same bounded EXIF extractor.
Reads through the accepted source handle are serialized because duplicate file
handles can share a cursor. Accepted length, modification time, and operating-
system change-time evidence are checked before and after extraction. A no-follow
pathname reopen also requires the accepted identity and version, so Windows
handle metadata caching cannot hide a pathname replacement. A detected rewrite
or rename fails closed instead of retaining metadata from uncertain bytes. The
in-memory Windows SHA-256 witness also rejects an in-place rewrite whose length
and writable timestamps were restored. The shared 512 MiB encoded-input limit is
enforced before hashing, and generation cancellation is checked throughout the
bounded stream. Replace-latest animation, details, and rating inspection exits
between stages and propagates that cancellation through every full comparison.
Full content comparisons run on owned background work, including the Windows Open
With verification and final destructive-action checks. Folder-rating discovery
reads only the bounded JPEG header twice through a retained source, requires the
exact consumed bytes and parsed result to match, and
rechecks native identity and version afterward. Rating writes always return to
the full content-witness boundary.
Main and worker decode publication, animation discovery, and rating inspection
apply the same before-and-after version contract before publishing accepted state.
It limits container chunks, payload bytes, TIFF directories and tags, recursive
offsets, embedded thumbnails, and allocation-driving component counts before the
metadata library parses the payload. Malformed or over-limit metadata is ignored;
it never prevents the pixels from opening or forces an unbounded allocation.
TIFF inspection seeks to bounded metadata wherever its IFD lives and compacts out
pixel-strip locators, so large ordinary TIFF pixel data does not need to enter the
metadata parser. Retention is content-driven and writes only JPEG, PNG, or WebP
destinations supported by the transactional export path.

Open With first verifies the unmodified original source on a
generation-cancellable background job, then passes that exact path to a native
chooser: Windows `SHOpenWithDialog`, macOS application picker plus
`NSWorkspace`, or the Linux desktop-portal `OpenURI` method with `ask`.
Navigation cancels obsolete verification. It does not pass viewr's
unsaved crop, rotation, flip, or Spot Heal state and does not sanitize metadata
first. After a successful handoff, a session watcher reloads the source when
those edits are idle. If a silent reload would destroy unsaved work, a
path-private `F5` reminder stays owned by the last-good frame. viewr does not
infer edit completion from another process lifetime, write handoff history, or
launch a default application without the chooser.

## Updates

viewr does **not** check for updates in the background or contact any server on
launch. The `viewr update` CLI command prints the official release URL, explicit
installer commands, and source-build guidance but never downloads anything. Help >
Get latest release presents one explicit action. Activating it asks the operating
system to open the official stable release in an external browser; the browser's
network and history behavior is outside viewr.

The separately downloaded installer scripts perform foreground HTTPS requests to
`github.com` and `api.github.com` only after the user
runs them. They verify the selected release checksum and manifest, install for the
current user, and exit. They do not grant network capability to the installed app,
create an updater service, schedule a task, or enable background checks. The app
closed is the app doing nothing at all.

## Freedom

Everything above is default behavior with no account, no phone-home, and no
dark-pattern "consent" banner. You own the binary and the photos. Logging,
diagnostics, and any future local preferences stay under your control.

## Summary

Most apps ask you to trust a privacy policy. viewr removes the main product-owned
collection paths: no remote client, telemetry, account, activity log, persistent
library index, or product temp cache. Native dialogs, operating-system paging,
system trash, GPU memory, explicitly selected external applications, and optional
package boundaries remain platform-owned surfaces and are stated above instead of
being presented as guarantees viewr does not control.
