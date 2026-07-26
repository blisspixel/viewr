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
| SVG | `.svg` (vector rasterized to RGBA) |

Golden-file style coverage for many of these lives in `crates/viewr/tests/corpus.rs`
and unit tests under `decode` / `edit`.

## Container behavior

- GIF, animated WebP, and APNG decode off the UI thread and play with bounded
  frame count, bounded decoded bytes, container delay, pause/resume, and loop
  behavior. Detection follows file content rather than the extension, and
  superseded animation work stops between frames. A still container remains a
  still image.
- TIFF and ICO currently expose one decoded image. Listing the container as
  supported does not claim multi-page or icon-frame navigation.
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
  ICC output beyond the same 10 MiB ceiling.
- SVG and optional worker output currently enter the sRGB path without a complete
  source-to-output color description. The worker protocol does not yet carry ICC
  or equivalent AVIF/HEIC color metadata.
- Core and worker dimensions and declared RGBA output sizes are validated before
  parent pixel allocation. Superseded worker reads and IPC requests stop and
  terminate their contained helper instead of occupying a decode slot until the
  hard deadline.
- CMYK profile conversion, per-display output transforms, wide-gamut preservation,
  and HDR presentation are not yet claimed. `ROADMAP.md` defines the acceptance
  work required before those claims can be made.

## Metadata export behavior

- Save As strips metadata by default. Optional session-only EXIF retention is
  available for JPEG, PNG, and WebP destinations.
- Source metadata is detected from file content, not its extension. The bounded
  reader supports JPEG, PNG text/eXIf variants, WebP, and TIFF metadata whose IFD
  is located beyond the image prefix.
- Export validates every option before touching the destination, assembles pixels
  and metadata in a sibling temporary file, and replaces the selected destination
  only after the complete output succeeds.

The Linux desktop entry, macOS application bundle, and Windows AppContainer
manifest advertise exactly this core extension set. They intentionally omit the
worker formats below because default release artifacts do not enable their C
backends. Exact-set repository tests fail if those declarations drift.

## Worker (out-of-process)

Listed in folder navigation so users can step onto them. Decode requires the
`viewr-decode` binary co-located with `viewr` (same directory after
`cargo build --workspace`).

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
