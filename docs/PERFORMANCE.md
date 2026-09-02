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

Full-Image Collage uses the same complete decoded RGBA images and color-managed,
mipmapped texture path. It never calls the Folder Previews thumbnail generator.
The current decode counts first against the existing 256 MiB decoded-pixel
budget; only the remaining bytes can admit up to 11 neighboring photos in its
group, or up to 12 when the retained current photo is outside that group. The
current GPU texture is reused when it belongs to the group. Other textures are
uploaded at most one per redraw, so entering the view does not issue one burst of
12 event-loop uploads. A completion that cannot fit does not evict an already
accepted group photo and trigger decode churn. Level-zero mosaic texture bytes
therefore follow the same aggregate decoded-pixel bound, with the documented mip
chain overhead, while an inactive current texture remains available for returning
to single-photo view.

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

## Local curation timing evidence

Current-image Trash and Undo use native platform services and are not part of the
GUI performance probe. Trash, permanent delete after confirmation, and restore run
through one typed worker. Repeated Delete submissions for fully presented images
enter a bounded application queue, advance immediately, and drain serially through
that worker. This keeps platform Trash receipt capture ordered while removing the
previous wait between one completed move and acceptance of the next. A selected
image must finish presentation before it can enter the queue. Strong
accepted-source comparison and restored rating inspection remain off the event
loop, allowing it to repaint a fixed operation
status and non-mutating view controls while conflicting playlist, edit, and
destructive actions wait. Trash receipt capture lists system Trash once after the
move and binds by original path plus retained object identity, rather than listing
before and after. After a successful removal, surviving neighbor prefetches stay
in memory so advancing to the next image does not cold-decode by default.

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
three times. Window and first-image timings use the slower of the small-folder and
large-folder medians. Navigation and idle redraws use the worst result across all
six timed runs and the cache-stress process. Folder-growth RSS uses the highest
large-folder peak minus the lowest small-folder peak so a noisy small run cannot
hide growth. A separate eight-image 4096x4096 corpus runs once. Each decoded
neighbor is 64 MiB, so retaining five would require 320 MiB; the gate requires real
neighbor retention, exact byte accounting, and eviction to at most four entries
under the 256 MiB budget. This prevents the byte-limit assertion from passing on a
corpus that only exercises the five-entry limit. Cache-stress RSS is retained as a
diagnostic but is not compared with the 768 MiB limit, which applies to the
50,000-file corpus.

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
| Full-image collage group | Up to 12 complete photos, 256 MiB current plus neighbors |
| Retained folder-preview textures | 9 |
| Current base GPU image texture | 64 Mi pixels |

These limits are deliberately looser than a fast development machine. They are
stable regression tripwires for a shared virtual runner, not universal latency
promises or a substitute for profiling target hardware.

File decode uses extra cores without growing those memory caps. Concurrent file
decodes are `logical_cpus - 1`, at least two and at most six. Foreground opens
still outrank prefetch and thumbnails. The GUI probe continues to measure
latency and resident set, not core utilization.

## How to reproduce

On Linux:

```text
cargo build --release --workspace --locked
python -B scripts/performance_gate.py --binary target/release/viewr
```

On Windows, pass `--no-xvfb` and use `target/debug/viewr.exe` when console output
is needed from a local developer build. Add `--idle-diagnostics` to print the
fixed per-run attribution above even when the gate is green. Add
`--session-label <label> --report-file <new-path>.json` to retain a path-free,
binary-digest-bound record of every run, the aggregate values, exact enforced
folder growth, sanitized renderer controls, actual wgpu adapter identity, budgets,
and pass or fail state. The harness copies the verified application and decoder
executables into a private directory, runs only those copies, and rejects any
byte change. The lowercase session label must match the report filename stem, and
the command refuses to overwrite an existing report. The normal release GUI
remains a windowed application without a console. A launch, timeout, or parse
failure can occur before a structured report exists; preserve that diagnostic as
a failed quality observation instead of inventing measurements.

Decode-only measurements remain available separately:

```text
cargo run --release --example gen_corpus -- corpus
cargo run --release --example bench_decode -- corpus
```

`bench_decode` reports median decode milliseconds and megapixels per second. It
and the GUI gate are dependency-free beyond the application itself, keeping the
measurement surface small and auditable.

## Current evidence

On the Windows development host on 2026-08-01, the final three-run optimized
rating-enabled probe met every startup, navigation, memory, folder-scaling, cache,
and idle budget after full accepted-source comparisons were confined to background
work: 866.91 ms median first frame, 918.70 ms median first image, 74.57 ms slowest
sampled navigation, 253.76 MiB small-folder peak resident set, 277.88 MiB large-folder peak
resident set, four decoded cache entries at the exact 256 MiB byte budget, and at
most one delivered redraw in every measured idle window. All completed idle
windows reported zero non-redraw events, zero event-driven repaint requests, and
zero scheduled egui repaints. The synthetic 50,000-image folder included the
session rating scan and filtered-playlist state; those processes were unfocused
with the pointer outside at measurement completion.

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

The last audited maintenance baseline passed its release-mode Ubuntu performance
job in [CI run 32419502385](https://github.com/blisspixel/viewr/actions/runs/32419502385)
on 2026-08-20 at commit `8cbe724`: 108.94 ms first window, 257.93 ms first pixel,
199.20 ms slowest sampled navigation, zero settled idle redraws, 313.16 MiB
small-folder peak resident set, 329.70 MiB large-folder peak resident set, 50,000
images, and four decoded cache entries at the exact 256 MiB byte budget. It proves
the regression budgets under the documented Ubuntu, Xvfb, and software-GPU
environment. The README badge and GitHub branch page are the live status. This
immutable baseline does not replace the open target-hardware checks for Windows,
macOS, representative Linux desktops, mixed-DPI displays, or profiled monitors.
The CI software-GL presentation probe also parses the exact performance report
schema and accepts success only when viewr's measured adapter is an identified
OpenGL software renderer, rather than trusting the ambient environment alone.

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
- The automated GUI timing probe does not enter Full-Image Collage. Pure collage,
  admission, texture-reuse, and accessibility tests enforce its structural
  bounds; PQ-PW-08 remains the candidate-binary interaction and visual gate on
  representative hardware.
- Manual cold-launch and interaction checks on Windows, macOS, and representative
  Linux desktops remain release acceptance work.
