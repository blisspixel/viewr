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
- **Zero product temp debris.** The GUI never writes under the system temp folder
  for probes. `viewr doctor` and `viewr benchmark` (without a directory) run
  fully **in memory**. On launch, viewr also scrubs any leftover `viewr_*` names
  it may have left under temp from older builds or crashes. Unit tests use a
  RAII temp workspace that deletes itself on drop.
- The developer/CI performance probe runs only when explicitly invoked, emits one
  path-free measurement record to its caller, and retains no history. Its Python
  harness owns and removes the temporary deterministic image corpus it creates.

There is no setting to turn any of this off, because none of the corresponding
machinery exists in the first place.

## Logging is opt-in (stderr only, never a log file)

By default the process is silent: no `log` output, **no log files on disk**.

If you want diagnostics while developing, set an environment variable yourself.
Output goes to **stderr only**; viewr never opens a `.log` file:

```text
RUST_LOG=viewr=debug
# or
VIEWR_LOG=info
```

Even when logging is on, viewr avoids writing full filesystem paths into log
lines. Nothing is ever sent off-machine.

## How the code enforces it

A promise you can verify beats a promise you have to trust.

1. **Remote clients are absent and local Linux IPC is confined.** A CI dependency
   audit **fails the build** if an HTTP, TLS, websocket, QUIC, or remote-service
   client appears. Windows and macOS use AccessKit's platform adapters with default
   features disabled. Linux's upstream AccessKit/AT-SPI adapter needs a generic
   D-Bus implementation; cargo-deny permits that implementation and its process
   helper only behind the reviewed AccessKit dependency path. Before logging,
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
   File and folder access remains user-directed: **Open File** grants one selected
   item, while **Open Folder** is the explicit consent path for sibling navigation.
   viewr does not request broad photo-library access or persist a folder grant.
3. **No analytics/telemetry SDKs, ever.** There is no analytics dependency to
   configure. This is also enforced by the dependency audit above.
4. **Split decode boundary.** Common pure-Rust formats decode in the main process
   under shape, allocation, and concurrency limits, with a pre-parse input cap for
   SVG. PNG text, EXIF, and ICC fields plus WebP EXIF and ICC fields are bounded
   before their decoders allocate them. JPEG XL initialization rejects encoded,
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
  Linux uses `XDG_CONFIG_HOME` or `.config`. Deleting that file restores System.
- viewr **does not** write history, a recently-opened list, thumbnail database,
  photo-library search index, or general settings database. Flags, picks,
  filmstrip thumbs, panel visibility/position, and neighbor **prefetch** live
  **only in RAM for the current session** and disappear when the app closes
  (never under temp or beside your photos).
- viewr **does not** create companion files next to your photos (no `_picks.txt`,
  no sidecar caches).
- Spot-heal strokes, repair regions, and undo/redo pixel patches exist only in
  bounded RAM. Navigation clears them. The source file is never edited in place.
- **Save As / convert** only writes the file path you choose in the save dialog.
  It validates the source, destination format, pixels, and metadata option first,
  builds a sibling temporary output, and replaces the destination only after both
  pixel encoding and optional metadata writing succeed.
- Deletes go to the **system trash**, so your OS (not viewr) holds the recoverable
  copy under its normal rules. Permanent delete requires an explicit confirmation
  dialog and skips the trash.

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
It limits container chunks, payload bytes, TIFF directories and tags, recursive
offsets, embedded thumbnails, and allocation-driving component counts before the
metadata library parses the payload. Malformed or over-limit metadata is ignored;
it never prevents the pixels from opening or forces an unbounded allocation.
TIFF inspection seeks to bounded metadata wherever its IFD lives and compacts out
pixel-strip locators, so large ordinary TIFF pixel data does not need to enter the
metadata parser. Retention is content-driven and writes only JPEG, PNG, or WebP
destinations supported by the transactional export path.

## Updates

viewr does **not** check for updates in the background or contact any server on
launch. The `viewr update` CLI command only prints how to rebuild or replace the
binary locally; it never downloads anything. Updates are delivered through the
platform's normal channels (your package manager, store, or a manual download you
initiate). A future graphical check is eligible only after a canonical signed
release source exists, and it must run solely on an explicit user command. The app
closed is the app doing nothing at all.

## Freedom

Everything above is default behavior with no account, no phone-home, and no
dark-pattern "consent" banner. You own the binary and the photos. Logging,
diagnostics, and any future local preferences stay under your control.

## Summary

Most apps ask you to trust a privacy policy. viewr is built so there is nothing to
trust: no network code to leak through, no telemetry to disable, no account to
compromise, no activity log by default, no leftover temp debris, and CI that fails
if networking sneaks in. Privacy here is the absence of the machinery that
violates it.
