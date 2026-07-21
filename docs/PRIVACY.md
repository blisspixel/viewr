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
  written to any log or side-file on disk by default**.
- viewr **has no account and no cloud.** There is nothing to sign into and nothing
  to sync.
- Your photos, filenames, and folder structure **never leave your machine.**
- **Temp probes are cleaned.** Doctor and benchmark may create short-lived files
  under the system temp directory; they are removed when the command finishes
  (including on error). Unit tests use the same pattern.

There is no setting to turn any of this off, because none of the corresponding
machinery exists in the first place.

## Logging is opt-in

By default the process is silent: no `log` output, no log files.

If you want diagnostics while developing, set an environment variable yourself:

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
2. **Sandboxed with the network denied.** Release builds ship inside the OS sandbox
   with no network entitlement:
   - **macOS** — App Sandbox, no `com.apple.security.network.*` entitlement.
   - **Windows** — AppContainer without the internet capability.
   - **Linux** — Flatpak with **no** `--share=network`; the decode worker also
     installs a seccomp filter that returns `EPERM` for network syscalls.
   Even a hypothetically compromised build cannot open a connection.
3. **No analytics/telemetry SDKs, ever.** There is no analytics dependency to
   configure. This is also enforced by the dependency audit above.
4. **Isolated decoding.** Untrusted image bytes are parsed in a restricted worker
   with no network access (and platform process limits), so a malicious file
   cannot exfiltrate anything over the network even in the worst case.

## Local data: what viewr does and doesn't write

- viewr **does not** write a settings file, history, recently-opened list,
  thumbnail database, or search index of your library. Flags and picks live
  **only in memory for the current session** and disappear when the app closes.
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
