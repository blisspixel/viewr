# Ratings and folder filtering

Status: approved product contract, not implemented.

viewr will support durable integer ratings because they improve a real folder
workflow: rate the image in front of you, then narrow navigation to the strongest
images. Ratings are not a library, activity history, synchronization service, or
reason to introduce background indexing.

## Storage and privacy contract

- The canonical value is `xmp:Rating`. Supported viewr ratings are integers from
  1 through 5. Missing or zero means Unrated. The external value `-1` means
  Rejected and remains distinct from Unrated.
- JPEG, and later TIFF after dedicated multi-page preservation proof, also mirrors
  the same integer in IFD0 tag `0x4746` as an unsigned SHORT. This is the Windows
  `System.SimpleRating` 0-to-5 field.
- viewr never writes IFD tag `0x4749` or `System.Rating`. That separate Windows
  property uses a 0-to-99 scale, where a literal value of 4 does not mean four
  stars.
- Clearing a rating removes viewr-supported rating fields instead of writing
  history. If `xmp:MetadataDate` already exists it may be updated consistently;
  viewr does not add creator-tool, history, identifier, timestamp, or activity
  fields merely to store a rating.
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
- The initial writable scope is ordinary content-detected JPEG. TIFF remains
  read-only until endian, BigTIFF, multi-page, strip and tile offsets, and unknown
  metadata preservation are proven. PNG, WebP, HEIF, AVIF, JPEG XL, camera RAW,
  GIF, SVG, BMP, and other formats remain visibly unsupported for rating writes
  until each container has equivalent proof. There is no persistence fallback.

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
- An active filter is persistent and explicit, for example `Image 2 of 7 matching
  rating 4 or higher; 42 images in folder`.
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

## Source-write safety gate

No rating UI ships until all of these conditions are proven:

1. A safe XMP reader and writer has fixed size, depth, attribute, namespace, and
   time limits and no ignored runtime advisory on the untrusted metadata path.
2. A write binds to the exact accepted source identity and version, rejects links
   and unsupported filesystems, and revalidates immediately before commit.
3. The implementation builds a private same-directory temporary, changes only the
   rating fields, syncs it, and performs a failure-atomic platform replacement.
   Windows uses `ReplaceFileW` without either ignore-ACL flag. Other platforms
   must preserve their equivalent permissions, ownership, extended attributes,
   resource forks, and security state or fail closed.
4. Post-commit verification reopens the source, confirms both rating fields, and
   proves image payload plus unrelated EXIF, XMP, ICC, IPTC, comments, thumbnails,
   and unknown metadata were preserved. A failed verification restores the
   original or exposes fixed recovery guidance if restoration itself fails.
5. Fixtures cover absent, zero, 1 through 5, Rejected, fractional, malformed,
   duplicate, conflicting, oversized, and permission-denied metadata. Fault
   injection covers every transaction phase. Cross-tool JPEG checks include
   Windows WIC and an XMP-aware application.
6. Filter, shortcut, accessibility, prefetch, Trash, Undo, F5, external-edit,
   privacy, coverage, and 50,000-file performance gates all pass.

The current `little_exif` dependency reaches `quick-xml 0.37.5`, which is covered
by narrow advisory exceptions only because viewr does not parse or rewrite
untrusted XMP. Those exceptions cannot be broadened to implement ratings. A
patched bounded metadata path and the preservation evidence above are required
before source-mutating code is acceptable.
