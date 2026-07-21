# Privacy

This is both viewr's design contract and the plain-language statement users will
eventually read. Privacy in viewr is not a policy promise — it's a property of the
code, and where possible it's enforced in CI so it can't quietly regress.

## The guarantee, plainly

- viewr **never connects to the internet.** It has no networking code.
- viewr **collects nothing.** No telemetry, no analytics, no usage metrics, no
  "help us improve" data, no crash reports sent anywhere.
- viewr **keeps no logs of your activity.** Which files you open, which folders you
  browse, and which images you delete are never recorded to a server, and not
  written to any analytics store on disk.
- viewr **has no account and no cloud.** There is nothing to sign into and nothing
  to sync.
- Your photos, filenames, and folder structure **never leave your machine.**

There is no setting to turn any of this off, because none of the corresponding
machinery exists in the first place.

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
   - **Linux** — Flatpak with **no** `--share=network`.
   Even a hypothetically compromised build cannot open a connection.
3. **No analytics/telemetry SDKs, ever.** There is no analytics dependency to
   configure. This is also enforced by the dependency audit above.
4. **Isolated decoding.** Untrusted image bytes are parsed in a restricted worker
   with no filesystem or network access, so a malicious file cannot exfiltrate
   anything even in the worst case.

## Local data: what viewr does and doesn't write

- viewr stores **only** a tiny settings file (e.g. your window size and whether
  permanent-delete asks for confirmation). It contains no history of what you
  viewed.
- **No thumbnail database, no recently-opened list, no search index** of your
  library is built or retained by default. If a convenience feature like "recent
  folders" is ever added, it will be **opt-in, local-only, and clearable in one
  click** — and it will still never be transmitted anywhere.
- Deletes go to the **system trash**, so your OS (not viewr) holds the recoverable
  copy under its normal rules.

## Metadata is yours

Images carry EXIF metadata — often including **GPS coordinates**, camera serial
numbers, and timestamps. Bloated apps silently preserve all of it when you export.

viewr does the opposite: on **Save As / convert**, the app re-encodes the raw image pixels directly and drops EXIF, GPS, and all other metadata entirely by construction. Your address and identifying fields never ride along inside a photo you share.

## Updates

viewr does **not** check for updates in the background or contact any server on
launch. Updates are delivered through the platform's normal channels (your package
manager, store, or a manual download you initiate). The app closed is the app
doing nothing at all.

## Summary

Most apps ask you to trust a privacy policy. viewr is built so there is nothing to
trust: no network code to leak through, no telemetry to disable, no account to
compromise, and CI that fails if any of that sneaks in. Privacy here is the
absence of the machinery that violates it.
