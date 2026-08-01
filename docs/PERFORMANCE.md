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
- **Idle redraws:** delivered redraw events during a settled 500 ms observation
  window.
- **Settled filmstrip:** every visible cell has either its bounded GPU thumbnail
  or a terminal placeholder for the current visible-window generation. A corrupt
  optional thumbnail cannot turn a stable viewer into a probe timeout.
- **Peak resident memory:** the process peak resident set after decode, thumbnail,
  and navigation work has settled.
- **Folder scaling:** resident-memory growth between 16-image and 50,000-image
  folders, plus exact bounds on decoded and GPU thumbnail caches.

Separate unit contracts cap the current base GPU image texture at 64 Mi pixels.
RGBA8 therefore uses at most 256 MiB for level zero and about 341 MiB for its
complete mip chain. Sources above the adapter dimension or pixel budget receive an
aspect-preserving preview; full decoded pixels remain available for export. The
preview worker borrows the source buffer and fallibly allocates only its bounded
output, avoiding a second full-resolution RGBA copy. Its linear-light,
alpha-correct area resampling is generation-cancellable between output rows. The
window thread never performs the resize.

Spot Heal copies at most 4 Mi working pixels and rejects strokes whose raster work
would exceed 16 Mi pixel visits. Candidate matching retains at most 2,048 boundary
samples and eight distinct sources. Median tone estimation uses fixed histograms,
not one heap allocation per candidate. The refresh job keeps only the bounded
working region so it can try another source without retaining a second full image.
The work remains asynchronous and cancellable. A ground-truth repair corpus and
dedicated heal latency gate are tracked in `ROADMAP.md`; the GUI navigation probe
does not claim to measure repair quality or latency.

The probe emits one path-free JSON record to its caller and exits. Alongside the
enforced delivered-redraw count, that record carries fixed diagnostic counts for
non-redraw window events, event-driven egui repaint requests, and scheduled egui
repaints, plus final window-focus and pointer-inside booleans. These fields identify
correlation, not causation, and carry no event payload, coordinate, path, or input.
The harness preserves every completed run in order when `--idle-diagnostics` is
requested and automatically prints the same fixed evidence when completed reports
violate a gate. Normal viewer launches never emit it. The probe has a one-minute
internal deadline, and the outer harness has a 90-second process timeout. A hang
therefore fails with a bounded diagnostic instead of stalling CI.

## Local Trash timing evidence

Current-image Trash and Undo use native platform services and are not part of the
GUI performance probe. Trash is a synchronous user-triggered call. Restore runs
through a typed worker, allowing the event loop to repaint a fixed operation
status and non-mutating view controls while conflicting playlist, edit, and
destructive actions wait.

When a developer explicitly enables `RUST_LOG=viewr=info` or
`VIEWR_LOG=info`, Undo reports submitted, restored, failed, and total native
restore values from the local monotonic clock. Fixed worker lifecycle records add
only operation category and submitted count. Normal launches remain silent. These
records are not retained and contain no path, filename, receipt identifier, native
identity, or raw platform error. They are local diagnostic evidence, not telemetry
or a cross-platform latency claim. The active restore state makes no percentage,
estimate, or cancellation claim. No Trash or restore latency release budget is
currently claimed.

## CI budgets

The Ubuntu 24.04 GUI performance job builds the locked release workspace and runs
the probe under Xvfb with Mesa's software GPU. The harness creates deterministic
PNGs, hard-links or copies them into temporary corpora, and deletes the entire
temporary workspace on exit. The 1920x1080 timing and folder-scaling corpora run
three times;
window/first-image timings use the median, while navigation, absolute resources,
and capacity measurements use the worst result. Folder-growth RSS uses the
highest large-folder peak minus the lowest small-folder peak so a noisy small run
cannot hide growth. A separate eight-image 4096x4096 corpus runs once. Each decoded
neighbor is 64 MiB, so retaining five would require 320 MiB; the gate requires real
neighbor retention, exact byte accounting, and eviction to at most four entries
under the 256 MiB budget. This prevents the byte-limit assertion from passing on a
corpus that only exercises the five-entry limit.

| Measurement | Enforced limit |
|---|---:|
| First presented window frame | 3,000 ms |
| First presented image | 5,000 ms |
| Slowest sampled navigation | 500 ms |
| Settled idle redraws in 500 ms | 2 |
| 50,000-file peak resident set | 768 MiB |
| 16-to-50,000-file resident growth | 96 MiB |
| Retained decoded neighbors | 5 |
| Retained decoded-neighbor pixels | 256 MiB |
| Retained folder-preview textures | 9 |
| Current base GPU image texture | 64 Mi pixels |

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
is needed from a local developer build. Add `--idle-diagnostics` to retain the
fixed per-run attribution above even when the gate is green. The normal release
GUI remains a windowed application without a console.

Decode-only measurements remain available separately:

```text
cargo run --release --example gen_corpus -- corpus
cargo run --release --example bench_decode -- corpus
```

`bench_decode` reports median decode milliseconds and megapixels per second. It
and the GUI gate are dependency-free beyond the application itself, keeping the
measurement surface small and auditable.

## Current evidence

On the Windows development host on 2026-07-29, the final three-run optimized
rating-enabled probe met every startup, navigation, memory, folder-scaling, cache,
and idle budget: 786.17 ms median first frame, 830.02 ms median first image,
65.01 ms slowest sampled navigation, 246.88 MiB small-folder peak resident set,
261.55 MiB large-folder peak resident set, four decoded cache entries at the exact
256 MiB byte budget, and at most one delivered redraw in every measured idle
window. All completed idle windows reported zero non-redraw events, zero
event-driven repaint requests, and zero scheduled egui repaints. The synthetic
50,000-image folder included the session rating scan and filtered-playlist state;
those processes were unfocused with the pointer outside at measurement completion.

The probe now treats an outstanding egui repaint deadline as unsettled UI work. A
500 ms idle observation starts or restarts only after delayed hover and activation
work is quiet. This preserves the limit of two delivered redraws and the existing
hard timeout. It does not hide a continuous repaint loop: one prevents the idle
window from settling and fails at the deadline.

A controlled Windows release run reproduced elevated redraws during initial window
activation and exposed an outstanding egui repaint deadline as the missing settled
state. After the probe began waiting for that deadline to become quiet, every
focused and pointer-inside window completed at zero or one measured redraw without
changing application scheduling or weakening the limit of two.

The canonical release-mode Ubuntu performance job passed in
[CI run 30592874307](https://github.com/blisspixel/viewr/actions/runs/30592874307)
on 2026-07-30. It proves the regression budgets under the documented Ubuntu, Xvfb,
and software-GPU environment. It does not replace the open target-hardware checks
for Windows, macOS, representative Linux desktops, mixed-DPI displays, or profiled
monitors.

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

Focused unit tests cover the immediate reverse decision and shared cache identity.
If the requested pristine source frame is still presented, the runtime path
cancels the abandoned generation and settles without another decode or texture
upload. After a move within two positions completes, the just-left pristine decode
can share its allocation with the existing LRU; its RGBA length still counts
against the same entry and 256 MiB limits. Larger jumps, derived edit results,
animation playback frames, explicit-Reload state, and over-budget or evicted
images use the normal loading path. This is not a measured end-to-end latency
claim.

Save As borrows RGBA pixels directly for alpha-capable encoders. Formats that
require RGB allocate one fallible output buffer rather than cloning RGBA first.
Animated images have independent 256 MiB and 1,000-frame caps and exist only for
the current image.

## Known limits

- Xvfb and a software GPU make the CI test reproducible but do not represent
  platform compositor or discrete-GPU performance.
- Peak resident set is a process-level high-water mark, not a heap allocation
  profile. The separate small and large processes make folder-growth comparison
  useful while avoiding accumulated state from an earlier run.
- Process resident set does not reliably report dedicated GPU memory. The exact
  texture-shape contract bounds viewr's allocation, but target-hardware GPU traces
  remain part of release acceptance.
- Manual cold-launch and interaction checks on Windows, macOS, and representative
  Linux desktops remain release acceptance work.
