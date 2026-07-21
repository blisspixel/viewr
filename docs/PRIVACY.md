# Privacy

This is both viewr's design contract and the plain-language statement users will
eventually read. Privacy in viewr is not a policy promise — it's a property of the
code, and where possible it's enforced in CI so it can't quietly regress.

## The guarantee, plainly

- viewr **never connects to the internet.** It has no networking code.
- viewr **collects nothing.** No telemetry, no analytics, no usage metrics, no
  "help us improve" data, no crash reports sent anywhere.
- viewr **keeps no logs of your activity.** Which files you open, which folders you
  browse, and which images you delete are never recorded to a server, and **not
  written to any log or side-file on disk**. There is **no log file** — not even
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

There is no setting to turn any of this off, because none of the corresponding
machinery exists in the first place.

## Logging is opt-in (stderr only — never a log file)

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

1. **No network dependency, checked in CI.** viewr links no HTTP/socket crate. A CI
   job audits the full dependency tree (e.g. `cargo-deny` / a dependency check) and
   **fails the build** if a networking-capable crate appears. The absence of
   networking is a tested invariant, not a habit.
2. **Network-denied packaging profiles.** The repository contains local packaging
   profiles with no network entitlement:
   - **macOS** — App Sandbox, no `com.apple.security.network.*` entitlement.
   - **Windows** — AppContainer without the internet capability.
   - **Linux** — Flatpak with **no** `--share=network`; the decode worker also
     installs a seccomp filter that returns `EPERM` for classic socket and
     io_uring networking paths.
   A bare `cargo build` does not automatically apply App Sandbox, AppContainer, or
   Flatpak confinement. Phase 7 tracks runtime verification of those profiles.
   Independently, the dependency ban applies to every build, and Linux worker
   spawn fails if its network-denying seccomp filter cannot be installed.
3. **No analytics/telemetry SDKs, ever.** There is no analytics dependency to
   configure. This is also enforced by the dependency audit above.
4. **Split decode boundary.** Common pure-Rust formats decode in the main process
   under shape, allocation, and concurrency limits, with a pre-parse input cap for
   SVG. Optional C-backed formats decode from bounded inputs in a worker. Linux
   denies that worker's classic socket and io_uring network paths; the documented
   OS packages constrain the whole app.
   Bare Windows and macOS Cargo builds do not claim that package-level boundary.

## Local data: what viewr does and doesn't write

- viewr **does not** write a settings file, history, recently-opened list,
  thumbnail database, or search index of your library. Flags, picks, filmstrip
  thumbs, and neighbor **prefetch** live **only in RAM for the current session**
  and disappear when the app closes (never under temp or beside your photos).
- viewr **does not** create companion files next to your photos (no `_picks.txt`,
  no sidecar caches).
- **Save As / convert** only writes the file path you choose in the save dialog.
- Deletes go to the **system trash**, so your OS (not viewr) holds the recoverable
  copy under its normal rules. Permanent delete requires an explicit confirmation
  dialog and skips the trash.

If a convenience feature like "remember window size" is ever added, it will be
**opt-in, local-only, and clearable in one click** — and it will still never be
transmitted anywhere.

## Metadata is yours

Images carry EXIF metadata — often including **GPS coordinates**, camera serial
numbers, and timestamps. Bloated apps silently preserve all of it when you export.

viewr does the opposite by default: on **Save As / convert**, the app re-encodes
the raw image pixels and **drops EXIF, GPS, and all other metadata**. Your address
and identifying fields do not ride along inside a photo you share unless you ask.

**Retain EXIF on Save As** is an explicit session option (File menu or Image Info
panel). It defaults to **off**. Turning it on keeps EXIF tags for the rest of that
session only — the choice is never written to a settings file.

## Updates

viewr does **not** check for updates in the background or contact any server on
launch. The `viewr update` CLI command only prints how to rebuild or replace the
binary locally; it never downloads anything. Updates are delivered through the
platform's normal channels (your package manager, store, or a manual download you
initiate). The app closed is the app doing nothing at all.

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
