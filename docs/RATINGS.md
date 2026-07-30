# Ratings and folder filtering

Status: implemented for bounded rating discovery and filtering, with source writes
limited to ordinary supported JPEG files on Windows.

viewr supports durable integer ratings because they improve a real folder
workflow: rate the image in front of you, then narrow navigation to the strongest
images. Ratings are not a library, activity history, synchronization service, or
reason to introduce background indexing.

## Storage and privacy contract

- The canonical value is `xmp:Rating`. Supported viewr ratings are integers from
  1 through 5. Missing or zero means Unrated. The external value `-1` means
  Rejected and remains distinct from Unrated.
- When a writable JPEG already contains a valid IFD0 tag `0x4746`, viewr updates
  the same unsigned SHORT in place. This is the Windows `System.SimpleRating`
  0-to-5 field. viewr does not insert an absent TIFF entry because growing or
  relocating an unknown IFD could invalidate MakerNote or other offset-bearing
  metadata. `xmp:Rating` remains the canonical interoperable value.
- viewr never writes IFD tag `0x4749` or `System.Rating`. That separate Windows
  property uses a 0-to-99 scale, where a literal value of 4 does not mean four
  stars.
- Clearing removes `xmp:Rating` and writes zero into an existing valid `0x4746`
  mirror. viewr does not add or update creator-tool, metadata-date, history,
  identifier, timestamp, or activity fields merely to store a rating.
- There is no global rating database, folder manifest, companion sidecar,
  alternate data stream, extended attribute, recent-file record, or disk cache.
  The source image is the durable record. A rating therefore follows a normal
  rename, move to Trash, and exact Undo restoration.
- Before the first rating write in each session, viewr must explain: `Ratings are
  written into this image file and may be visible to other apps.` The intended
  rating remains pending until the user confirms. Consent is not persisted.
- Diagnostics use fixed outcome categories only. They do not contain a path,
  filename, rating history, raw metadata, or native error text.

The small rating value discloses a preference to anyone who receives the file. It
does not reveal viewing history, when the rating was assigned, or which viewr user
assigned it. This is an intentional and visible privacy tradeoff, not hidden
state.

## Interoperability and malformed state

- If only `xmp:Rating` or `0x4746` exists, viewr reads that supported value.
- If both exist and agree, viewr exposes the shared value.
- Conflicting fields remain Conflict. Fractional, duplicated, malformed, or
  out-of-range values remain Unsupported. They are never rounded or silently
  repaired.
- Rejected, Conflict, Unsupported, Unreadable, Unrated, and ratings 1 through 5
  are distinct states.
- An explicit new rating may reconcile otherwise valid conflicting rating fields,
  but only after the normal first-write disclosure and source-safety checks.
- The initial writable scope is ordinary content-detected JPEG on Windows. TIFF
  remains read-only until endian, BigTIFF, multi-page, strip and tile offsets, and
  unknown metadata preservation are proven. PNG, WebP, HEIF, AVIF, JPEG XL,
  camera RAW, GIF, SVG, BMP, and other formats remain visibly unsupported for
  rating writes until each container has equivalent proof. Non-Windows builds
  currently read supported JPEG ratings and filter in memory but do not write
  ratings. There is no persistence fallback.

Adobe defines `xmp:Rating` as `-1` for Rejected and `0` through `5` for ratings.
Microsoft maps `System.SimpleRating` to `xmp:Rating` and IFD tag `18246`, or
`0x4746`, for JPEG and TIFF. The design follows those interoperable meanings:

- [Adobe XMP Basic namespace](https://developer.adobe.com/xmp/docs/xmp-namespaces/xmp/)
- [Adobe XMP specifications](https://developer.adobe.com/xmp/docs/xmp-specifications/)
- [Microsoft System.SimpleRating](https://learn.microsoft.com/en-us/windows/win32/wic/-wic-photoprop-system-simplerating)
- [Microsoft System.Rating](https://learn.microsoft.com/en-us/windows/win32/wic/-wic-photoprop-system-rating)
- [CIPA Exif and metadata standards](https://www.cipa.jp/e/std/std-sec.html)

Sources reviewed 2026-07-29.

## Interaction contract

- `0` clears the rating after source writes have been disclosed for the session.
  `1` through `5` assign that exact rating. Key repeat is ignored.
- Rating keys work only in normal image-viewing mode when no text or numeric field,
  modal, menu, popup, Crop, Spot Heal, or destructive action owns input.
- Fit Image to View moves to the primary modifier plus `0`. Actual Size moves to
  the primary modifier plus `1`. Both remain visible in View.
- Edit > Rating exposes Unrated and 1 through 5 for discovery and pointer or
  keyboard access. The current state is always visible as text, such as
  `Rating: 4 of 5`, without relying on color or a star glyph.
- View > Rating Filter exposes All images and At least 1 through At least 5. The
  filter is session-only, resets to All on a folder change, and writes nothing.
- The top status keeps filename, dimensions, zoom, rating, and folder position in
  distinct readable groups. When width is constrained it removes lower-priority
  detail instead of collapsing all values together.
- An active filter is persistent and explicit, for example
  `3 / 3 rated 4+ · 12 total`.
- If no image matches, the loaded-folder surface says `No images are rated 4 or
  higher.` and exposes `Show all images`. It never falls back to the first-run Open
  File card.

## Navigation and state ownership

The folder catalog remains one canonical natural-order list. A rating filter is a
derived projection of indices, never a second mutable playlist.

- Next, Previous, Home, End, Page Up, Page Down, the counter, Folder Previews,
  thumbnail selection, and neighbor prefetch operate on the visible projection.
- Applying a filter keeps the current image if it matches. Otherwise it selects
  the next natural-order match, then the previous match. Empty results use the
  dedicated no-match state.
- A rating write commits before filter membership changes. If the new value falls
  outside the active filter, the image stays visible with `Outside current filter`
  until the next navigation action. This avoids a successful keypress immediately
  moving the target away.
- Trash receipts and Undo use canonical folder positions, never filtered
  positions. Embedded ratings move and restore with the exact file.
- Rating discovery is in-memory, bounded, cancellable, generation-tagged, and
  limited to the explicitly opened folder. It creates no watcher or persistent
  index. A 50,000-file folder must remain within the existing performance and
  memory budgets.

## Source-write safety boundary

The Windows writer is enabled only for a source that passes all of these checks:

1. The rating path uses `quick-xml` 0.41 directly, outside the advisory-affected
   transitive versions. JPEG header bytes, segment count, XMP packet size, XML
   events, depth, attributes, namespace declarations, rating text, TIFF entry
   count, and total encoded bytes are all bounded. DTDs, extended XMP, duplicate
   packets, unknown namespace prefixes, ambiguous RDF subjects, nested rating
   properties, and metadata signatures hidden after the JPEG scan start fail
   closed.
2. A write retains the exact source handle that supplied displayed pixels, checks
   native identity plus file length and change timestamps, rejects links and
   reparse points, snapshots from that handle, compares the complete snapshot,
   and revalidates immediately before replacement.
3. The pristine snapshot and candidate are created beside the source with its
   owner, group, and discretionary access-control list applied at `CreateFileW`.
   The pristine copy is delete-on-close and immediately unlinked. The complete
   candidate is synced and verified before `ReplaceFileW` runs with neither
   ignore-ACL flag.
4. Replacement retains the exact original under a private transaction name.
   Post-commit verification reopens the candidate, checks its rating and complete
   image tail, rechecks candidate and backup path binding, and only then removes
   the original. Verification failure restores the retained original when that can
   still be proven. An uncertain restore keeps the application open, disables
   further writes, and exposes fixed recovery guidance instead of claiming the
   previous rating is safe.
5. The rewriter changes only `xmp:Rating` and an existing valid `0x4746` value.
   Image payload, segment order, comments, ICC, IPTC, EXIF, XMP subject, unknown
   metadata, and all other bytes remain preserved. It never adds creator, tool,
   date, identifier, history, or `0x4749` fields.

The old `little_exif` and build-time Wayland paths still resolve advisory-affected
`quick-xml` versions under their existing narrow exceptions. They never receive
raw user XMP in viewr's reviewed paths. Ratings use the separate patched 0.41
dependency, so the new untrusted parser does not broaden either exception.

## Validation contract and known boundary

Every release runs malformed and oversized metadata fixtures; rating values from
absent through 5 plus Rejected, duplicates, conflicts, and invalid values; exact
payload and unrelated-segment preservation; actual Windows success, stale-source,
partial replacement, rollback, path-binding, security-descriptor, and transaction-
cleanup tests; playlist and worker race tests; native UI Automation; Windows Shell
Property System interoperability; local GExiv2 interoperability through a supplied
Python executable or the default GIMP 3 Python when present; privacy and dependency
gates; meaningful coverage; and the 50,000-file performance corpus.

`ReplaceFileW` is the narrow final pathname operation. Another process can still
replace a path after the last check. Abrupt process or power loss after replacement
but before cleanup can also leave a source-protected `.viewr-rating-backup-*`
original, and an unreconciled failure may retain a protected work copy for manual
recovery. viewr will not broadly delete such names at startup because it cannot
safely infer ownership across concurrent processes. These are explicit recovery
boundaries, not hidden rating storage.
