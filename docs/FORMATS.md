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
| PNG | `.png` |
| GIF | `.gif` (including animation frames when present) |
| WebP | `.webp` |
| BMP | `.bmp` |
| TIFF | `.tif`, `.tiff` |
| ICO | `.ico` |
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
  over versioned worker IPC; the worker receives no filesystem path. Linux applies a
  fail-closed network-denying seccomp filter. Exact-set tests cover the Flatpak,
  App Sandbox, and AppContainer profiles, and platform CI builds or validates
  their native sandbox form. Pure-Rust formats decode in the main
  process under the same shape and aggregate concurrency caps.
