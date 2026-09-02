# Product quality

**Status:** open, and still open after the v0.6.1 tag. This matrix is the
executable contract for first-time, power-user, admin, failure-recovery, and
visual-polish paths. It does not close v0.6.
Representative Windows, macOS, and Linux hardware still have to pass the same rows
using the checksummed archives and synthetic fixture artifact from one retained
candidate workflow run.

v0.6.0 and v0.6.1 were published before any of those rows were recorded. The
released archives therefore carry no representative-hardware evidence, the
release notes state that limit, and this matrix is carried forward as open work
that blocks the v0.7.0 tag.

v0.6 broadens the core viewing surface with a transient full-image collage. It
does not add a library, catalog, thumbnail mode, or durable album state. Clipboard
open/copy and touch gestures stay behind this work.

## Source of truth

Help, the first-run card, README essential controls, the Windows accessibility
smoke test, and this matrix quote one covered catalog:

- `crates/viewr/src/shortcuts.rs` owns empty-state copy and About shortcut groups.
- About lists Open, Browse, View, and Edit keys, including `[` / `]`, `F5`,
  `T` `G` `I`, `Shift+G`, Space-to-fit, `F` / `F11`, Escape, Save As, and Undo
  Trash.
- The empty card heading distinguishes first-run, opening, and failure. First-run
  copy is:

  `Open File, Open Folder, or drop a file or folder. A dropped file also browses its folder when access allows. Open Folder selects the folder for this session.`

- Decoder and I/O errors on that card stay one short line. Retry remains named.
- `A` / `D` are not navigation keys. Left and Right, Home and End, and Page Up
  and Page Down own folder browsing.

In-process tests prove the catalog, AccessKit names, bounded errors, and that
About Close stays inside the 640 by 480 minimum window. They do not prove
feel, mixed-DPI layout, or multi-monitor behavior on real hardware.

## In-process evidence versus hardware

| Kind | What it proves | What it does not prove |
| --- | --- | --- |
| Covered policy and UI tests | Copy, shortcut identity, empty/opening/error states, About overflow, contrast tokens | Pointer feel, real type rendering at 150% or 200% scale, monitor-move smoothness |
| Windows UI Automation smoke | Native first-run scope, Open File/Folder, About window and shortcut text, Close | Speech quality or non-Windows first-run |
| Performance probe in [PERFORMANCE.md](PERFORMANCE.md) | Startup, navigation, idle redraw, and memory budgets on the CI runner | Representative-GPU latency |
| This matrix on retained candidate archives | The v0.6 gate | Nothing until every required platform row is recorded |

## Candidate artifact contract

Run the manual matrix against archives built once from one candidate commit. Do
not test a local developer build or combine archives from different workflow
runs.

1. Put the intended v0.6 runtime, package contents, workspace version, compiled
   installer commands, unreleased changelog, and reviewed prospective release
   notes on `main`. Keep README and INSTALL accurate about which immutable
   release is the current public download. Confirm the normal CI and fuzz
   workflows are green for that exact commit.
2. Dispatch `Release artifacts` on `main` once. A branch dispatch repeats CI and
   fuzzing, builds all four target archives, verifies them, generates one
   deterministic synthetic fixture artifact, and retains all five artifacts for
   30 days without creating a GitHub release:

   ```text
   gh workflow run release.yml --repo blisspixel/viewr --ref main
   gh run list --repo blisspixel/viewr --workflow release.yml --event workflow_dispatch
   gh run download <run-id> --repo blisspixel/viewr --dir .agent/product-quality/<run-id>/artifacts
   python -B scripts/release_artifact.py verify .agent/product-quality/<run-id>/artifacts/viewr-<target>/<archive>
   ```

3. Record the run URL and its exact `headSha`. Windows, macOS, and Linux must use
   their platform archive and `product-quality-fixtures` from that one run. Each
   platform records its own archive filename and SHA-256 because the target bytes
   differ.
4. If application source, dependencies, workflows, packaging, or user-facing
   behavior instructions change after the candidate build, discard every result
   and start with a new run. Only completed evidence, the release date, and public
   release-status or immutable-download links may change without resetting the
   candidate. Those status-only changes do not alter application behavior. The
   final tag workflow still reruns the complete automated release gate, rebuilds
   the archives with the final public documents, and gives every published asset
   its own checksum and provenance verification.

This separates two claims cleanly. v0.6 proves the integrated product on exact
candidate bytes before tagging. v0.8 later proves clean install, update,
uninstall, rollback, and final artifact acceptance as a separate release-readiness
gate.

## Manual matrix

Use disposable copies, never personal photos. Record operating-system version,
display scale, graphics adapter, package type, artifact filename, and SHA-256
for every platform. Do not mark the roadmap product-quality gate complete until
Windows, macOS, and Linux each have a complete record from one candidate commit
and workflow run.

| Platform | Representative hardware | Status |
| --- | --- | --- |
| Windows | Typical laptop or desktop at 100%, 150%, and 200% scale, including a move between displays with different scale factors | Not yet recorded |
| macOS | Apple Silicon Retina laptop plus an external display, including a live move between them | Not yet recorded |
| Linux | Native Wayland and X11 or Xwayland sessions at a typical desktop scale, plus a Mesa software-GPU session | Not yet recorded |

A platform record may combine explicitly named hosts or sessions when one machine
cannot cover the row. Its metadata and observations must say which host or
session produced each result. Lack of suitable hardware is not a pass; keep the
row open until the coverage exists.

The v0.6 macOS hardware claim is explicitly Apple Silicon. The aarch64 archive
must be the macOS record's artifact. The x86_64 macOS archive remains covered by
native hosted build, tests, archive verification, checksums, and final release
attestation, but Intel hardware acceptance is deferred to the v0.8 complete
artifact matrix. Do not imply that the v0.6 record tested Intel hardware.

## Candidate fixture artifact

The manual run produces `product-quality-fixtures` from the candidate source and
uploads it beside the application archives. It contains only deterministic,
synthetic files, a role manifest, and a canonical SHA-256 manifest; it contains
no personal metadata. Use the downloaded copy unchanged. Every platform record
captures the checksum-manifest digest. The evidence gate rejects a missing,
empty, linked, incomplete, extended, or byte-changed fixture set.

The generator refuses to overwrite an existing directory. To reproduce the
same fixture roles from the candidate checkout without treating a developer
build as the application under test:

```text
cargo run --release --locked -p viewr --example gen_product_quality_fixtures -- <new-output-directory>
python -B scripts/product_quality_evidence.py fixture-manifest <new-output-directory>
```

Use `browse/1-red.png`, `browse/2-green.png`, and `browse/10-blue.png` for open,
drop, folder, latest-first selection, and natural-order navigation. Use `sequences/` for page and
animation checks, `mosaic/` for the three-group full-image collage, `visual/` for
fit and scale, and `failure/` for rejected input. Every collage photo has four
distinct corner markers, mixed aspect ratios, and a deterministic natural-order
name so cropping, order, and page boundaries are visible. For reload, deletion,
rename, Save As, and Trash checks, first copy `editing/` to a disposable working
directory. Replace the working source with the provided replacement; never
mutate the retained artifact.

The display transitions are evidence targets, not arbitrary permutations.
Microsoft's [desktop high-DPI test guidance](https://learn.microsoft.com/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows#testing-your-changes)
calls for starting and moving a window across different-DPI displays and changing
scale while it runs. Apple's [high-resolution guidance](https://developer.apple.com/library/archive/documentation/GraphicsAnimation/Conceptual/HighResolutionOSX/Explained/Explained.html)
documents that a window's backing resolution changes dynamically as it moves
between displays. Native Wayland and X11 through Xwayland remain distinct display
paths, as described by the [Wayland protocol documentation](https://wayland.freedesktop.org/docs/book/Xwayland.html),
so one Linux session cannot stand in for the other.

Run the same workflow on every platform.

### First-time

| ID | Action | Required result |
| --- | --- | --- |
| PQ-FT-01 | Launch with no path | The window opens at the documented monitor-bounded size. The empty card names Open File, Open Folder, drop, sibling browsing versus explicit folder consent, and the local-only privacy line. No confirmation step is added. |
| PQ-FT-02 | Copy `browse/` to a disposable directory, give its three files distinct modification times, open that directory, confirm Latest First, switch File > Preferences > Default folder sort to Name, browse all three, restart viewr, reopen the directory, then open a disposable copy of `editing/source.png`, overwrite that copy with `failure/malformed.png`, and press F5 | The retained fixture stays unchanged and the newest modified disposable copy opens first. Exactly one folder-sort radio is selected, and its help states that the current association path does not provide the manager's sort. Changing to Name preserves the current image and produces red, green, blue order before and after restart. Each image appears without changing window size. The forced later miss keeps the last good frame visible and reports the reload failure. |
| PQ-FT-03 | Drop `browse/1-red.png`, then the `browse/` folder | The drop is classified the same way as a command-line path: a folder starts the Open Folder browse; a file opens as a file. |
| PQ-FT-04 | Choose `browse/` with Open Folder, then cite the candidate workflow's `tests/sandbox_profiles.rs` job and `selected_file_scan_outcomes_cover_success_and_limits` in the observation | The portable archive browses the selected folder explicitly. The package-profile checks prove the sandbox grants, while the exact entry-state test proves that a failed sibling scan retains only the selected image until explicit folder consent exists. The portable archive is not misrepresented as a sandbox package. |
| PQ-FT-05 | Open Help > About viewr | Version, platform, license, privacy, and the grouped shortcut catalog are readable. Background controls cannot activate. Close and Escape dismiss it. On the 640 by 480 minimum window, Close stays reachable. |
| PQ-FT-06 | Open both files under `failure/` | The card heading names the selected file, the error is one short line, Retry is present, and menus stay usable. |
| PQ-FT-07 | In File > Preferences select System, English, Spanish, French, and German, restarting after each explicit selection | Exactly one language radio is selected. Primary cataloged menus, actions, headings, and their accessible names change immediately, the explicit choice survives restart, accented glyphs render, and uncataloged advanced copy remains readable through the documented English fallback. |

### Power-user

| ID | Action | Required result |
| --- | --- | --- |
| PQ-PW-01 | Keyboard-only first image | `O` or `Ctrl/Cmd+O` opens a file; `Ctrl/Cmd+Shift+O` opens a folder; Left/Right, Home/End, Page Up/Page Down browse. |
| PQ-PW-02 | Open `sequences/two-page.tiff` and `sequences/two-size.ico` | `[` and `]` plus Image Information and View step one page. Documents do not play. Crop and Spot Heal block a step. |
| PQ-PW-03 | Open all three `sequences/two-frame.*` animations | GIF, WebP, and APNG play in bounds. While paused, `[` and `]` step frames. |
| PQ-PW-04 | Open `visual/small.png` and `visual/large.png` | Space tap fits. Hold Space to pan. `Ctrl/Cmd+0` fits, `Ctrl/Cmd+1` is actual size, and `+` / `-` zoom around the viewport center. Wheel or trackpad zoom keeps the pointer location fixed. The small source rests at 100 percent. |
| PQ-PW-05 | Panels | `T`, `G`, and `I` show Tools, Folder Previews, and Image Information. Persistent chrome never covers the photo. |
| PQ-PW-06 | Replace a disposable copy of `editing/source.png` with `editing/replacement.png` and press F5. Repeat after making an unsaved crop before the external replacement | The clean case reloads without blanking. The unsaved-edit case keeps the last good frame, does not discard the crop, and asks for F5. |
| PQ-PW-07 | Spot Heal a disposable `editing/source.png`, finish the tool, use Save As, then Trash the source | Heal success and the continuing top cue identify the in-memory Save As boundary. `Ctrl/Cmd+Shift+S` exports a copy containing the repaired pixels while the source stays unchanged. Delete moves the visible image to Trash, and its routine result stays in top chrome instead of covering the photo. `U` restores the recoverable receipt when the platform can prove it. |
| PQ-PW-08 | Open `mosaic/01-wide.png`; press Up to enter Full-Image Collage; wait for the group to settle; traverse it with Left and Right; resize between landscape and portrait; toggle fullscreen; use Page Down and Page Up; open one photo with Down, another with Enter, and another with a click; repeat entry with `Shift+G`; then cite `twelve_landscape_photos_fill_the_screen_in_justified_rows`, `source_aspects_define_tiles_without_equal_cell_letterboxing`, `collage_accepts_twelve_photos_and_tiny_views_stay_safe`, `collage_tile_enlarges_a_complete_small_image_without_changing_its_aspect`, `no_eviction_insert_rejects_pressure_without_displacing_existing_images`, and `mosaic_loading_announcement_is_stable_until_the_terminal_count` from the candidate workflow | The first and second groups contain 12 full photos each and the third contains two in natural order. Every photo preserves all four corner markers and its aspect ratio, with no crop or thumbnail substitution. Actual aspect ratios define dense justified rows with narrow gutters, including 3:4 portrait tiles and panoramic tiles, rather than equal empty cells. Complete small sources enlarge inside their exact tiles while ordinary single-photo Fit remains capped at 100 percent. The collage reflows only when another complete photo becomes ready. Selection, position-only accessible names, Up entry, Down, Enter, click, groups, fullscreen, and Escape remain coherent. The cited tests prove dense aspect-aware geometry, complete tile enlargement, bounded no-eviction admission, and one stable loading announcement followed by a terminal count; if a real host reaches a memory or display limit, it shows fewer complete photos with the matching explicit status. |
| PQ-PW-09 | Copy at least 20 images to one disposable folder, open it, then press Delete on each fully presented image as quickly as presentation permits while earlier Trash moves remain active. Press Delete once before a deliberately slow image finishes opening | Every fully presented image is accepted without waiting for the prior platform move and the next neighbor begins presentation immediately. The active count remains visible in top chrome without covering the photo. Platform moves drain in order, the loading image is not admitted and gets an explicit wait reason, a failure stops unsubmitted requests with a path-free count, and `U` refers only to the latest safely recoverable completed move. |

### Admin

| ID | Action | Required result |
| --- | --- | --- |
| PQ-AD-01 | `viewr doctor` | Reports binaries, worker protocol, windowing libraries, and graphics runtimes. A passing last line is not proof that a window opened. |
| PQ-AD-02 | Inspect the installation contract in the candidate archive and open File > Default Image Viewer | README and INSTALL identify v0.6.1 as the current immutable public download. The named modal states that file associations are opt in, provides the platform-specific PNG and JPEG route, and blocks background actions. No background updater runs, and no instruction disables platform security. Clean install, update, uninstall, association, and rollback acceptance remain the v0.8 gate. |
| PQ-AD-03 | Help > Get latest release | The Update modal names the running version, refuses to check a network, and only the explicit button hands the release URL to the browser. |
| PQ-AD-04 | Unsigned preview | OS trust warnings may appear. Docs do not tell anyone to disable platform security. |

### Failure recovery

| ID | Action | Required result |
| --- | --- | --- |
| PQ-RC-01 | Launch directly with `failure/malformed.png` | The empty canvas does not claim a previous image is visible. Retry stays available. |
| PQ-RC-02 | Delete a disposable `editing/source.png` outside viewr | The last good frame stays. A polite status says the selected path no longer names that file. |

### Candidate automated prerequisites

The following fault states have no safe deterministic trigger in the production
interface on every platform. They are candidate-wide automated prerequisites,
not instructions to add a hidden release fault mode. Each platform record still
contains these IDs, but its observation cites the exact candidate CI run and the
named test or controlled job. The same provenance is expected in all three
records.

| ID | Automated procedure | Required result |
| --- | --- | --- |
| PQ-RC-03 | In the exact candidate CI test job, run the workspace suite containing `crop_preview_disconnect_copy_and_recovery_priority_are_truthful`, `dropped_and_panicking_workers_have_stable_terminal_failures`, and `dropped_worker_and_panicking_worker_are_observable_terminal_failures`. | Executor loss becomes a named terminal recovery state. Crop or preview work that cannot continue tells the user to close and reopen before retrying that path. |
| PQ-RC-04 | In the exact candidate CI test job, run the workspace suite containing `recovery_blocks_only_actions_that_need_the_unsettled_owner` and its Undo Trash unsettled-state assertions. | Undo Trash does not claim a settled receipt, and another Trash move stays blocked while restore ownership is unsettled. |
| PQ-RC-05 | In the exact candidate CI test and GUI-performance jobs, run the startup support matrix, missing-library copy tests, doctor tests, and Linux Xvfb software-Mesa presentation probe. | Missing presentation dependencies produce actionable non-zero launch diagnostics, `doctor` names the same class of gap, and the controlled software-GPU environment presents successfully. |

### Visual polish and budgets

| ID | Action | Required result |
| --- | --- | --- |
| PQ-VS-01 | Empty, opening, and error cards | Opaque themed surfaces, AA text, stable geometry across unchanged frames, no vertical drift. |
| PQ-VS-02 | Light, Dark, Console, and inspection backgrounds | Chrome tokens stay AA. Image pixels are unchanged by Appearance. |
| PQ-VS-03 | Move the window repeatedly between the differently scaled displays or sessions named in the platform metadata, then cite `profile_refresh_follows_monitor_identity_changes` and `returning_to_a_prior_monitor_is_a_new_identity` from the candidate workflow | Text, focus rings, and panels stay usable throughout the required platform scale coverage. The manual result records every move. The named automated checks prove display-profile refresh on monitor identity changes without pretending a portable archive exposes internal ICC-refresh state. |
| PQ-VS-04 | Run the candidate-binary performance procedure below | Window-ready, first-pixel, navigation, idle-redraw, memory, 50,000-file, and decoded-cache numbers meet [PERFORMANCE.md](PERFORMANCE.md). The observation records every required rollup value and both tested executable SHA-256 values. |

## Candidate-binary performance procedure

Use the extracted, archive-verified candidate binary, not a local build. Run the
harness from a clean checkout of the recorded candidate commit. It creates and
deletes its own deterministic 16-image, 50,000-image, and 4096-square cache-stress
corpora. The fixture artifact alone is intentionally small and is not performance
evidence.

First verify the archive and sidecar as described above. The archive extracts into
an archive-prefix directory, and both `viewr` and `viewr-decode` are under `bin/`.
Run the required session commands from the candidate checkout. The harness removes
an inherited `VIEWR_DECODE_BIN`, requires the colocated worker, copies both
verified executables into a private harness directory, and hashes those copies
before and after the complete run. On Windows, make the tested display primary
and run this one-line PowerShell command first at 100% scale. Repeat it at 150%
and 200% after changing the primary display scale, session label, and filename to
`windows-150` and `windows-200`:

```text
python -B scripts/performance_gate.py --binary <extracted-directory>/<archive-prefix>/bin/viewr.exe --no-xvfb --idle-diagnostics --session-label windows-100 --report-file docs/release-evidence/product-quality/v0.6.1/performance/windows-100.json
```

On macOS, make the built-in Retina display the main display and run once with
`macos-retina`, then make the external display the main display and run once with
`macos-external`. Use the same value for the session label and filename. The
report records a one-way SHA-256 identity, built-in and Retina flags, and measured
scale for the main display. On Linux, run in the native sessions named below:

```text
python -B scripts/performance_gate.py --binary <extracted-directory>/<archive-prefix>/bin/viewr --no-xvfb --idle-diagnostics --session-label <session> --report-file docs/release-evidence/product-quality/v0.6.1/performance/<session>.json
```

Use `linux-wayland` and `linux-x11` in the corresponding native sessions. For the
Xwayland variant, also set `WINIT_UNIX_BACKEND=x11` so the measured report records
that explicit backend selection. The Wayland session does not require Xwayland or
`DISPLAY`; its actual renderer identity comes from viewr's wgpu adapter. For the
required software renderer, use an X11 or Xwayland session with `DISPLAY`, install
`glxinfo`, confirm that `glxinfo -B` names Mesa llvmpipe or softpipe, then run:

```text
WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=1 python -B scripts/performance_gate.py --binary <extracted-directory>/<archive-prefix>/bin/viewr --no-xvfb --idle-diagnostics --session-label linux-mesa-software --report-file docs/release-evidence/product-quality/v0.6.1/performance/linux-mesa-software.json
```

The complete committed report set is exactly `windows-100`, `windows-150`,
`windows-200`, `macos-retina`, `macos-external`, `linux-wayland`, `linux-x11`, and
`linux-mesa-software`, each with the `.json` suffix. Each report contains no path,
refuses to overwrite earlier evidence, includes both candidate executable SHA-256
values, measured display or session evidence, sanitized renderer controls, and all
raw probe records, enforced budgets, exact folder-growth value, summary, and pass
or fail state. Each raw probe records the backend, name, device type, and driver
of the wgpu adapter that actually presented its frames. Only the software-Mesa
session also records the environment's OpenGL query. The gate rejects impossible
platform backends, adapter changes within one session, and raw runs copied between
any required sessions. Copy the
worst-case rollup into PQ-VS-04: window ready ms, first pixel ms, navigation
maximum ms, idle redraws, small RSS MiB, large RSS MiB, folder growth MiB,
large-folder image count, cache-stress entry count and MiB, `viewr` SHA-256,
`viewr-decode` SHA-256, and every report path. A generated failed report remains defect evidence. If the
harness fails before it can create a report, record the console diagnostic as a
Fail observation and do not invent numeric values. A failed or missing report
cannot close the gate.

## Recording a result

Create no result file until one platform run is complete. Store a completed run at
`docs/release-evidence/product-quality/<version>/<platform>.md`, using `windows`,
`macos`, or `linux` for the platform name. Its first line is
`# Product quality evidence: <platform>`. A two-column metadata table must contain
exactly Version, Candidate commit, Candidate workflow run, Fixture artifact,
Fixture manifest SHA-256, Artifact filename, Artifact SHA-256, Package type,
Operating system, Display scale, Graphics adapter, and Run date. Fixture artifact
is `product-quality-fixtures`; package type is `portable archive`. A second table
contains Check, Result, and Observation. It records every ID above exactly once
with Pass, Fail, or Approved exception. Every observation is substantive and
names what was exercised, what was observed, and which named host or session
produced it. Generic text such as `Observed expected behavior`, `Pass`, or `OK`
is rejected. An approved exception links the reviewed GitHub issue that owns it
and starts with `Low severity:` or `Medium severity:`. Critical and high-severity
exceptions cannot pass. The validator requires each manual row to name its
row-specific controls and outcomes plus the host or session. PQ-FT-04 and
PQ-VS-03 also cite their named candidate-workflow checks. PQ-RC-03 through
PQ-RC-05 are hard prerequisites: all three records repeat the same exact
candidate-run observation, and no exception can close them. PQ-VS-04 is also a
hard prerequisite and cannot close through an exception.

Validate each completed record and then the three-platform gate:

```text
python -B scripts/product_quality_evidence.py check <platform-record.md>
python -B scripts/product_quality_evidence.py gate docs/release-evidence/product-quality/v0.6.1
```

The gate rejects missing or duplicate rows, generic observations, invalid archive
provenance, mixed candidate runs, incomplete performance sessions, and every
recorded failure. It uses GitHub CLI to prove that the recorded URL is a successful
manual `Release artifacts` run on `main` at the recorded commit and downloads that
run into a fresh temporary directory itself. It verifies all four archives with
the canonical archive verifier, including automated Intel macOS coverage, checks
the recorded archive digests, and compares every performance report's two
executable SHA-256 values with `bin/viewr[.exe]` and `bin/viewr-decode[.exe]` in
the applicable archive manifest. It
also recomputes every report summary from its retained raw runs and rejects hidden
playlist, cache, thumbnail, idle, adapter, or budget failures. The software-Mesa
session must prove that the actual viewr adapter is the controlled GL software
adapter, not merely that an unrelated graphics query found one. A caller-supplied
local artifact directory is never treated as run provenance. Manual candidate
runs store retained artifacts without compression so GitHub's reported artifact
sizes bound the bytes extracted by the CLI; the gate checks those sizes before
download and checks file and aggregate limits again afterward. Tag runs may use
normal artifact compression because they are not accepted as candidate evidence.

The check command accepts a complete failing record so a defect can be preserved
honestly before it is fixed. The gate also requires the exact nonempty synthetic
fixture set downloaded from that workflow run, verifies its per-file checksums,
and requires every platform to record the same checksum-manifest digest.

Use only synthetic filenames and fixtures. Do not include a personal image,
private path, raw metadata, or unrelated screen content. If the tested artifact
bytes change, the record no longer closes the gate.

Do not tag v0.7.0 while any required platform row is unrecorded or any
high-severity product-quality issue remains. v0.6.0 and v0.6.1 were tagged in
exactly that state as explicit, documented exceptions; do not treat automated
gates as a substitute for these hardware records.
