# Supported formats

Honest capability list for viewr. Breadth never lowers the safety bar: the main
process stays pure-Rust; formats that need C libraries run only in the
`viewr-decode` worker.

## Core (in-process, pure Rust)

Always available in the default build. Decoded with `image`, `jxl-oxide`, or
`resvg` (SVG shapes/paths; text shaping features are off to keep the trusted
dependency set lean).

| Family | Extensions |
|--------|------------|
| JPEG | `.jpg`, `.jpeg` |
| PNG / APNG | `.png` (APNG plays when multiple frames are present) |
| GIF | `.gif` (including animation frames when present) |
| WebP | `.webp` (including animation frames when present) |
| BMP | `.bmp` |
| TIFF | `.tif`, `.tiff` (one decoded image; no page navigator yet) |
| ICO | `.ico` (one decoded image; no frame navigator yet) |
| QOI | `.qoi` |
| TGA | `.tga` |
| PNM | `.ppm`, `.pgm`, `.pbm`, `.pnm` |
| HDR / EXR | `.hdr`, `.exr` |
| farbfeld | `.ff` |
| DDS | `.dds` |
| JPEG XL | `.jxl` |
| SVG | `.svg` (bounded vector shapes and paths rasterized to RGBA; resource and unbounded-scratch features rejected) |

Golden-file style coverage for many of these lives in `crates/viewr/tests/corpus.rs`
and unit tests under `decode` / `edit`.

Every accepted encoded source is capped at 512 MiB before decode or Windows
content-witness work. A larger file fails with an explicit load error. This is an
encoded-file ceiling, independent of the stricter decoded-pixel, dimension,
animation, SVG, metadata, and worker-response limits below. Superseded decode and
full content-witness work stops between fixed 64 KiB chunks rather than reading
the remainder of a bounded but obsolete source. The replace-latest animation,
details, and rating task also exits between stages after navigation, and all full
comparisons run off the UI thread. Folder-rating discovery does not
hash each complete image. Its read-only seam checks native identity and version
around two JPEG header reads capped at 16 MiB each, requires the exact consumed
bytes and parsed results to match, polls cancellation between segments, and
returns no authority for file mutation.

## Container behavior

- GIF, animated WebP, and APNG decode off the UI thread and play with bounded
  frame count, bounded decoded bytes, container delay, pause/resume, and loop
  behavior. Detection follows file content rather than the extension, and
  superseded animation work stops between frames. A still container remains a
  still image.
- TIFF and ICO currently expose one decoded image. Listing the container as
  supported does not claim multi-page or icon-frame navigation.
- SVG decoding accepts bounded vector markup but rejects both embedded raster
  image data and external image hrefs with an explicit error. A selected SVG can
  neither read another local image nor expand its resource budget through a
  resolver. Gzip-compressed SVG is rejected before parsing so decompression cannot
  bypass the 64 MiB input ceiling. Document type declarations are also rejected
  before parsing so entity expansion cannot outgrow that ceiling. Markup is capped
  at 100,000 elements, 100,000 attributes, 4 MiB of cumulative attribute data,
  and 128 levels, while simultaneously live opacity, blend, and isolation layers
  have a 256 MiB intermediate-pixel budget. Cumulative path geometry is capped at
  512 KiB and 100,000 lexical tokens. Parsed nodes, path
  segments, tile-amplified edge work, and cumulative paint/layer area have
  independent ceilings. `use`, marker, stylesheet, inline-style, text, gradient, stroke,
  filter, mask, clipping-path, and paint-pattern features fail closed because
  their parser expansion, subdivision, or renderer scratch allocation is not
  independently bounded. Solid-filled shapes, bounded paths, transforms, and
  bounded opacity groups remain supported.
- All eight EXIF orientation values are normalized into displayed pixels when the
  decoder exposes orientation metadata. Rotation, flips, and crop are exported in
  their visible orientation rather than copied as a stale orientation tag.

## Color behavior

- A bounded, valid embedded RGB ICC profile is converted to the current sRGB
  working path before GPU upload. The same conversion applies to supported
  animated frames. Missing profiles are treated as sRGB; invalid, oversized, or
  unsupported profiles produce an explicit fallback status in Image Information.
  PNG and WebP containers are preflighted before decoder allocation. JPEG XL's
  locally reviewed `jxl-color` boundary rejects encoded, declared, or amplified
  ICC output beyond the same 10 MiB ceiling. The reviewed `jxl-render` boundary
  also skips an unreferenced LF-frame level before table lookup so malformed
  input returns through the decode path instead of panicking. Its composition
  state now always becomes terminal and wakes waiters after an error. The
  reviewed `jxl-color` boundary requires the exact 12-byte CICP tag layout with
  zero reserved bytes, reads payload fields at their specified tag offset,
  round-trips PQ and HLG transfer evidence, requires AVX2 plus FMA before AVX2
  conversion entry points, and preserves the first parallel transform error.
- Source pixels, normalized working pixels, and presentation are separate typed
  stages. Successful normalization produces only validated RGBA8 sRGB working
  pixels. Crop and pixel transforms preserve that encoding; preview generation,
  thumbnails, export, and renderer upload reject an incompatible encoding before
  applying sRGB math or touching an output destination. The renderer accepts the
  matching sRGB-to-sRGB output contract and refuses a surface without an sRGB
  format. This is an explicit SDR limit, not a wide-gamut or HDR claim.
- SVG currently enters the sRGB path without a complete source-to-output color
  description. Worker protocol V2 carries either a bounded ICC profile, H.273
  CICP values, or an explicit unknown status with every RGBA8 stream. AVIF keeps
  trustworthy ICC or CICP evidence from libavif. HEIC/HEIF checks source ICC size
  before fallible allocation and explicitly preserves source NCLX when running
  on libheif 1.21 or newer. Libheif 1.23 additionally passes through HEVC VUI
  color when no container NCLX exists. Decoded output CICP wins when source and
  output primaries or transfer demonstrate that a requested transform changed the
  pixel encoding. ICC remains authoritative when version-10 passthrough performs
  no extra gamut conversion, including when matching bitstream-only NCLX evidence
  coexists. Version 8 and 9 runtimes expose decoded output CICP, or unknown if the
  decoder supplies none, rather than retaining ICC after their implicit sRGB
  target. The embedded 1.23 CI lane requires libde265 1.0.7 or newer because
  earlier adapters do not propagate HEVC VUI color. Untagged output follows
  libheif's deterministic sRGB decode fallback. ICC input is normalized to sRGB,
  explicit sRGB CICP is accepted as tagged sRGB, Display P3 CICP is converted
  into the sRGB working path, and color spaces that the current working path
  cannot convert remain visible as a fallback status rather than being silently
  relabeled.
- Core and worker dimensions and declared RGBA output sizes are validated before
  parent pixel allocation. Superseded worker reads and IPC requests stop and
  terminate their contained helper instead of occupying a decode slot until the
  hard deadline.
- Unmanaged Windows-legacy and real X11 convert working sRGB into the admitted
  display ICC before presentation and refresh that conversion when the window
  changes monitor. Managed compositors stay tagged sRGB. CMYK profile
  conversion, wide-gamut preservation, and HDR presentation are not yet
  claimed. `ROADMAP.md` defines the acceptance work required before those
  claims can be made.

## Metadata export behavior

- Save As strips metadata by default. Optional session-only EXIF retention is
  available for JPEG, PNG, and WebP destinations.
- Source metadata is detected from file content, not its extension. The bounded
  reader supports JPEG, PNG text/eXIf variants, WebP, and TIFF metadata whose IFD
  is located beyond the image prefix.
- Export validates every option before touching the destination. Pixels and
  optional metadata are written through one retained sibling temporary-file
  handle, whose pathname identity is checked before commit. Existing destinations
  receive an app-owned, identity-bound overwrite prompt after the native dialog
  and are replaced only after the complete output succeeds. Destinations confirmed
  absent use a no-clobber install so a concurrently created file survives.

The Linux desktop entry, macOS application bundle, and Windows AppContainer
manifest advertise exactly this core extension set. They intentionally omit the
worker formats below because default release artifacts do not enable their C
backends. Exact-set repository tests fail if those declarations drift.

## Worker (out-of-process)

Listed in folder navigation so users can step onto them. Decode requires the
`viewr-decode` binary co-located with `viewr` (same directory after
`cargo build --workspace`).

`viewr` starts that worker and sends it encoded bytes over a private pipe. It is
not a command-line tool: run by hand, it prints a short explanation of what it
is and exits, because there is no protocol frame on a terminal to answer.

| Format | Extensions | Default worker build | Full decode |
|--------|------------|----------------------|-------------|
| AVIF | `.avif` | Error explaining missing feature | `--features avif` (+ system libavif) |
| HEIC / HEIF | `.heic`, `.heif` | Error explaining missing feature | `--features heic` (+ system libheif) |
| Camera RAW | `.cr2`, `.nef`, `.arw`, `.dng`, `.rw2`, `.orf`, `.raf` | **Deferred** honest error | Feature `raw` reserved; implementation not shipped |

### Building the worker with C backends

```text
# Default (CI-safe, no system C libraries):
cargo build -p viewr-decode

# With optional backends (requires local native deps):
cargo build -p viewr-decode --features avif,heic
```

The main `viewr` binary never links those C libraries.

## Explicit non-support (for now)

Anything not listed above is not claimed. Opening an unknown extension fails
cleanly without panicking.

## Privacy and safety notes

- No format path opens a network connection.
- The main process opens optional C-backed inputs and sends bounded encoded bytes
  over versioned worker IPC; the worker receives no filesystem path. Linux
  applies a fail-closed network-denying policy to every worker and a tested
  default-deny syscall allowlist when AVIF or HEIC is enabled. Exact-set tests
  cover the Flatpak, App Sandbox, and AppContainer profiles, and platform CI
  builds or validates their native sandbox form. Pure-Rust formats decode in the
  main process under the same shape and aggregate concurrency caps.
