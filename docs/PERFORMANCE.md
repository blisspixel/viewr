# Performance

Performance is a tested property, not a hope. This document records what we
measure, how, and the current numbers, so a regression is visible rather than felt.

## What we measure

- Decode time: how long `DecodedImage::load` takes per format and size. This is
  the CPU work of opening a file, separate from GPU upload and drawing.
- (Planned) First-pixel latency: cold start to the first frame on screen.
- (Planned) Navigation latency: time from an arrow press to the next image drawn,
  which the neighbor prefetch cache is designed to keep near zero.
- (Planned) Steady-state memory across a large folder, which the bounded texture
  cache is designed to keep flat.

## How to reproduce

```
cargo run --release --example gen_corpus -- corpus
cargo run --release --example bench_decode -- corpus
```

`bench_decode` reports the median of several runs per file, in milliseconds and
megapixels per second. It is dependency-free (no benchmark framework) on purpose,
which keeps the tool honest and the dependency tree small.

## Current decode numbers

Measured on the reference development machine (Windows, release build, `image`
pure-Rust decoders). Absolute times vary by hardware; the point is the shape and
the regression baseline.

| Image | Pixels | Median decode |
|---|---|---|
| 1920x1080 JPEG | 2.1 MP | ~7 ms |
| 1920x1080 PNG | 2.1 MP | ~8 ms |
| 4000x3000 JPEG | 12 MP | ~38 ms |
| 4000x3000 PNG | 12 MP | ~36 ms |
| 8000x6000 PNG | 48 MP | ~131 ms |

Across the full corpus (8 formats at 16x16, 256x171, 1920x1080, 4000x3000, plus a
48 MP image), every file decoded with no failures. Throughput on large images
lands in the 300 to 460 MP/s range depending on format, with PPM and TIFF fastest
and TGA/BMP (uncompressed, IO-bound) slowest.

## What the numbers mean for the experience

A 12 MP photo (a typical modern camera image) opens in well under one frame at
60 Hz once decode is off the UI thread and neighbors are prefetched. Even a 48 MP
image decodes in ~130 ms, which the prefetch cache turns into an instant swap when
paging through a folder because the work happens before you ask for the image.

## Guardrails (planned)

CI will run the decode benchmark on a fixed corpus and fail if median decode time
regresses beyond a set threshold, so "it got slow" fails a check rather than a
user's patience. Until that gate lands, the numbers here are the manual baseline.
