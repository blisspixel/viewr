# Product quality

**Status:** open for v0.6. This matrix is the executable contract for first-time,
power-user, admin, failure-recovery, and visual-polish paths. It does not close v0.6.
Representative Windows, macOS, and Linux hardware still have to pass the same rows
using the checksummed archives and synthetic fixture artifact from one retained
candidate workflow run.

v0.6 adds no new feature category. Clipboard open/copy and touch gestures stay
behind this work.

## Source of truth

Help, the first-run card, README essential controls, the Windows accessibility
smoke test, and this matrix quote one covered catalog:

- `crates/viewr/src/shortcuts.rs` owns empty-state copy and About shortcut groups.
- About lists Open, Browse, View, and Edit keys, including `[` / `]`, `F5`,
  `T` `G` `I`, Space-to-fit, Save As, and Undo Trash.
- The empty card heading distinguishes first-run, opening, and failure. First-run
  copy is:

  `Open a file to start, or drop a file or folder. Its folder is browsed when access allows. Open Folder selects it explicitly for this session.`

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

1. Put the intended v0.6 runtime, package contents, version, changelog, and
   reviewed release notes on `main`. Confirm the normal CI and fuzz workflows are
   green for that exact commit.
2. Dispatch `Release artifacts` on `main` once. A branch dispatch repeats CI and
   fuzzing, builds all four target archives, verifies them, generates one
   deterministic synthetic fixture artifact, and retains all five artifacts for
   30 days without creating a GitHub release:

   ```text
   gh workflow run release.yml --repo blisspixel/viewr --ref main
   gh run list --repo blisspixel/viewr --workflow release.yml --event workflow_dispatch
   gh run download <run-id> --repo blisspixel/viewr \
     --dir .agent/product-quality/<run-id>/artifacts
   python -B scripts/release_artifact.py verify \
     .agent/product-quality/<run-id>/artifacts/viewr-<target>/<archive>
   ```

3. Record the run URL and its exact `headSha`. Windows, macOS, and Linux must use
   their platform archive and `product-quality-fixtures` from that one run. Each
   platform records its own archive filename and SHA-256 because the target bytes
   differ.
4. If application source, dependencies, workflows, packaging, or user-facing
   behavior instructions change after the candidate build, discard every result
   and start with a new run. Committing completed evidence and updating release
   status do not change the candidate application that was exercised. The final
   tag workflow still reruns the complete automated release gate and its published
   assets receive their own checksum and provenance verification.

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
| macOS | Retina laptop plus an external display, including a live move between them | Not yet recorded |
| Linux | Native Wayland and X11 or Xwayland sessions at a typical desktop scale, plus a Mesa software-GPU session | Not yet recorded |

A platform record may combine explicitly named hosts or sessions when one machine
cannot cover the row. Its metadata and observations must say which host or
session produced each result. Lack of suitable hardware is not a pass; keep the
row open until the coverage exists.

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
cargo run --release --locked -p viewr \
  --example gen_product_quality_fixtures -- <new-output-directory>
python -B scripts/product_quality_evidence.py fixture-manifest \
  <new-output-directory>
```

Use `browse/1-red.png`, `browse/2-green.png`, and `browse/10-blue.png` for open,
drop, folder, and natural-order navigation. Use `sequences/` for page and
animation checks, `visual/` for fit and scale, and `failure/` for rejected
input. For reload, deletion, rename, Save As, and Trash checks, first copy
`editing/` to a disposable working directory. Replace the working source with
the provided replacement; never mutate the retained artifact.

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
| PQ-FT-02 | Open `browse/1-red.png` | The image appears without changing window size. When access allows, Left and Right browse its folder in natural order. The last good frame stays visible during a later miss. |
| PQ-FT-03 | Drop `browse/1-red.png`, then the `browse/` folder | The drop is classified the same way as a command-line path: a folder starts the Open Folder browse; a file opens as a file. |
| PQ-FT-04 | Open Folder | The session browses that folder explicitly. Sandboxed file-only grants remain one image until this consent exists. |
| PQ-FT-05 | Open Help > About viewr | Version, platform, license, privacy, and the grouped shortcut catalog are readable. Background controls cannot activate. Close and Escape dismiss it. On the 640 by 480 minimum window, Close stays reachable. |
| PQ-FT-06 | Open both files under `failure/` | The card heading names the selected file, the error is one short line, Retry is present, and menus stay usable. |

### Power-user

| ID | Action | Required result |
| --- | --- | --- |
| PQ-PW-01 | Keyboard-only first image | `O` or `Ctrl/Cmd+O` opens a file; `Ctrl/Cmd+Shift+O` opens a folder; Left/Right, Home/End, Page Up/Page Down browse. |
| PQ-PW-02 | Open `sequences/two-page.tiff` and `sequences/two-size.ico` | `[` and `]` plus Image Information and View step one page. Documents do not play. Crop and Spot Heal block a step. |
| PQ-PW-03 | Open all three `sequences/two-frame.*` animations | GIF, WebP, and APNG play in bounds. While paused, `[` and `]` step frames. |
| PQ-PW-04 | Open `visual/small.png` and `visual/large.png` | Space tap fits. Hold Space to pan. `Ctrl/Cmd+0` fits, `Ctrl/Cmd+1` is actual size, `+` / `-` zoom at the cursor. The small source rests at 100 percent. |
| PQ-PW-05 | Panels | `T`, `G`, and `I` show Tools, Folder Previews, and Image Information. Persistent chrome never covers the photo. |
| PQ-PW-06 | Replace a disposable copy of `editing/source.png` with `editing/replacement.png` | `F5` reloads without blanking. An external change with unsaved edits keeps the last good frame and asks for F5. |
| PQ-PW-07 | Save As and Trash a disposable `editing/source.png` | `Ctrl/Cmd+Shift+S` exports. Delete moves the visible image to Trash. `U` restores the recoverable receipt when the platform can prove it. |

### Admin

| ID | Action | Required result |
| --- | --- | --- |
| PQ-AD-01 | `viewr doctor` | Reports binaries, worker protocol, windowing libraries, and graphics runtimes. A passing last line is not proof that a window opened. |
| PQ-AD-02 | Clean install of the current preview | The published installer command uses the immutable v0.5.0 asset until v0.6 is tagged. No background updater runs. |
| PQ-AD-03 | Help > Get latest release | The Update modal names the running version, refuses to check a network, and only the explicit button hands the release URL to the browser. |
| PQ-AD-04 | Unsigned preview | OS trust warnings may appear. Docs do not tell anyone to disable platform security. |

### Failure recovery

| ID | Action | Required result |
| --- | --- | --- |
| PQ-RC-01 | Launch directly with `failure/malformed.png` | The empty canvas does not claim a previous image is visible. Retry stays available. |
| PQ-RC-02 | Delete a disposable `editing/source.png` outside viewr | The last good frame stays. A polite status says the selected path no longer names that file. |
| PQ-RC-03 | Worker or preview loss | Named recovery status, not a silent busy state. Crop or preview work that cannot continue says to close and reopen before trying that path again. |
| PQ-RC-04 | Restore ownership unsettled | Undo Trash does not claim a settled receipt. A new Trash move waits. |
| PQ-RC-05 | Launch missing a windowing library or GPU runtime | The process prints an actionable message and exits non-zero. `doctor` names the same gap. |

### Visual polish and budgets

| ID | Action | Required result |
| --- | --- | --- |
| PQ-VS-01 | Empty, opening, and error cards | Opaque themed surfaces, AA text, stable geometry across unchanged frames, no vertical drift. |
| PQ-VS-02 | Light, Dark, Console, and inspection backgrounds | Chrome tokens stay AA. Image pixels are unchanged by Appearance. |
| PQ-VS-03 | Mixed DPI and multi-monitor | Text, focus rings, and panels stay usable at 100%, 150%, and 200%. Moving the window between monitors refreshes the display profile on unmanaged Windows-legacy and real X11. |
| PQ-VS-04 | Startup and navigation | Window-ready, first-pixel, navigation, idle-redraw, and memory numbers meet [PERFORMANCE.md](PERFORMANCE.md) on CI and do not regress on representative hardware. |

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
with Pass, Fail, or Approved exception. Every observation is substantive. An
approved exception links the reviewed GitHub issue that owns it.

Validate each completed record and then the three-platform gate:

```text
python -B scripts/product_quality_evidence.py check <platform-record.md>
python -B scripts/product_quality_evidence.py gate \
  docs/release-evidence/product-quality/v0.6.0 \
  --artifacts .agent/product-quality/<run-id>/artifacts
```

The gate rejects missing or duplicate rows, placeholder observations, invalid
archive provenance, mixed candidate runs, and every recorded failure. It also
uses GitHub CLI to prove that the recorded URL is a successful manual `Release
artifacts` run on `main` at the recorded commit, then compares every recorded
digest with the downloaded archive and its sidecar. The check command accepts a
complete failing record so a defect can be preserved honestly before it is
fixed. The gate also requires the exact nonempty synthetic fixture set generated
by that workflow run, verifies its per-file checksums, and requires every platform
to record the same checksum-manifest digest.

Use only synthetic filenames and fixtures. Do not include a personal image,
private path, raw metadata, or unrelated screen content. If the tested artifact
bytes change, the record no longer closes the gate.

Do not tag v0.6.0 while any required platform row is unrecorded or any
high-severity product-quality issue remains.
