# Performance

Performance is a tested property, not a hope. This document records what viewr
measures, how the regression gate works, and which claims the evidence does and
does not support.

## Enforced measurements

The explicit `performance-probe` process exercises the real window, renderer,
image loader, folder scanner, neighbor prefetch, and folder-preview path. Normal
viewer launches never collect these measurements.

- **Window ready:** process entry to the first successful frame presentation
  after the window is made visible.
- **First pixel:** process entry to the first successfully presented image frame.
- **Navigation:** the slowest request-to-present interval across distinct sampled
  positions in the folder.
- **Idle redraws:** redraw requests during a settled 500 ms observation window.
- **Peak resident memory:** the process peak resident set after decode, thumbnail,
  and navigation work has settled.
- **Folder scaling:** resident-memory growth between 16-image and 50,000-image
  folders, plus exact bounds on decoded and GPU thumbnail caches.

The probe emits one path-free JSON record to its caller and exits. It has a
one-minute internal deadline, and the outer harness has a 90-second process
timeout. A hang therefore fails with a bounded diagnostic instead of stalling CI.

## CI budgets

The Ubuntu 24.04 GUI performance job builds the locked release workspace and runs
the probe under Xvfb with Mesa's software GPU. The harness creates a deterministic
1920x1080 PNG, hard-links or copies it into temporary corpora, and deletes the
entire temporary workspace on exit. Both corpora run three times;
window/first-image timings use the median, while navigation, absolute resources,
and capacity measurements use the worst result. Folder-growth RSS uses the
highest large-folder peak minus the lowest small-folder peak so a noisy small run
cannot hide growth.

| Measurement | Enforced limit |
|---|---:|
| First presented window frame | 3,000 ms |
| First presented image | 5,000 ms |
| Slowest sampled navigation | 500 ms |
| Settled idle redraws in 500 ms | 2 |
| 50,000-file peak resident set | 768 MiB |
| 16-to-50,000-file resident growth | 96 MiB |
| Retained decoded neighbors | 5 |
| Retained folder-preview textures | 9 |

These limits are deliberately looser than a fast development machine. They are
stable regression tripwires for a shared virtual runner, not universal latency
promises or a substitute for profiling target hardware.

## How to reproduce

On Linux:

```text
cargo build --release --workspace --locked
python -B scripts/performance_gate.py --binary target/release/viewr
```

On Windows, pass `--no-xvfb` and use `target/debug/viewr.exe` when console output
is needed from a local developer build. The normal release GUI remains a
windowed application without a console.

Decode-only measurements remain available separately:

```text
cargo run --release --example gen_corpus -- corpus
cargo run --release --example bench_decode -- corpus
```

`bench_decode` reports median decode milliseconds and megapixels per second. It
and the GUI gate are dependency-free beyond the application itself, keeping the
measurement surface small and auditable.

## Current local evidence

On the Windows development host on 2026-07-22, the latest three-run debug GUI
probe passed the 50,000-file contract with a 1,063 ms median first frame and first
image, 265 ms slowest sampled navigation, two settled redraws, 247.41 MiB
small-folder peak resident set, 256.36 MiB large-folder peak resident set, and
8.95 MiB displayed peak difference. The idle-aware gate first exposed and then
verified removal of a self-sustaining egui event-loop repaint cycle and
paint-dependent background-work starvation.

Those numbers validate the local harness and provide a reference observation.
Only the release-mode Ubuntu job enforces the canonical CI result. No hosted run
is claimed until a Git remote and runner are available.

## Decode reference

Historical release measurements on the same development class of machine used
the pure-Rust decoders:

| Image | Pixels | Median decode |
|---|---:|---:|
| 1920x1080 JPEG | 2.1 MP | about 7 ms |
| 1920x1080 PNG | 2.1 MP | about 8 ms |
| 4000x3000 JPEG | 12 MP | about 38 ms |
| 4000x3000 PNG | 12 MP | about 36 ms |
| 8000x6000 PNG | 48 MP | about 131 ms |

Prefetched navigation can avoid decode latency entirely. A cold cache miss still
depends on storage, decoder, image dimensions, GPU initialization, display stack,
and host load, so viewr does not turn these reference numbers into a blanket
"instant" guarantee.

## Known limits

- Xvfb and a software GPU make the CI test reproducible but do not represent
  platform compositor or discrete-GPU performance.
- Peak resident set is a process-level high-water mark, not a heap allocation
  profile. The separate small and large processes make folder-growth comparison
  useful while avoiding accumulated state from an earlier run.
- Manual cold-launch and interaction checks on Windows, macOS, and representative
  Linux desktops remain release acceptance work.
