# Product quality

**Status:** open for v0.6. This matrix is the executable contract for first-time,
power-user, admin, failure-recovery, and visual-polish paths. It does not close v0.6.
Representative Windows, macOS, and Linux hardware still have to pass the same rows
on published artifacts.

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
| This matrix on published zips | The v0.6 gate | Nothing until every required platform row is recorded |

## Manual matrix

Use disposable copies, never personal photos. Record operating-system version,
display scale, graphics adapter, package type, artifact filename, and SHA-256
for every platform. Do not mark the roadmap product-quality gate complete until
Windows, macOS, and Linux each have a complete record against one artifact hash.

| Platform | Representative hardware | Status |
| --- | --- | --- |
| Windows | Typical laptop or desktop at 100%, 150%, and 200% scale, including a mixed-DPI move when available | Not yet recorded |
| macOS | Retina laptop, including a move between built-in and external displays when available | Not yet recorded |
| Linux | Wayland and X11 sessions on a typical desktop scale, including a software-GPU path when that is the host | Not yet recorded |

Run the same workflow on every platform.

### First-time

| Action | Required result |
| --- | --- |
| Launch with no path | The window opens at the documented monitor-bounded size. The empty card names Open File, Open Folder, drop, sibling browsing versus explicit folder consent, and the local-only privacy line. No confirmation step is added. |
| Open a file | The image appears without changing window size. When access allows, Left and Right browse its folder. The last good frame stays visible during a later miss. |
| Drop a file or folder | The drop is classified the same way as a command-line path: a folder starts the Open Folder browse; a file opens as a file. |
| Open Folder | The session browses that folder explicitly. Sandboxed file-only grants remain one image until this consent exists. |
| Open Help > About viewr | Version, platform, license, privacy, and the grouped shortcut catalog are readable. Background controls cannot activate. Close and Escape dismiss it. On the 640 by 480 minimum window, Close stays reachable. |
| Unsupported or malformed file | The card heading names the selected file, the error is one short line, Retry is present, and menus stay usable. |

### Power-user

| Action | Required result |
| --- | --- |
| Keyboard-only first image | `O` or `Ctrl/Cmd+O` opens a file; `Ctrl/Cmd+Shift+O` opens a folder; Left/Right, Home/End, Page Up/Page Down browse. |
| Multi-page TIFF and ICO | `[` and `]` plus Image Information and View step one page. Documents do not play. Crop and Spot Heal block a step. |
| Animation | GIF, WebP, and APNG play in bounds. While paused, `[` and `]` step frames. |
| Fit, pan, zoom | Space tap fits. Hold Space to pan. `Ctrl/Cmd+0` fits, `Ctrl/Cmd+1` is actual size, `+` / `-` zoom at the cursor. A source smaller than the viewport rests at 100 percent. |
| Panels | `T`, `G`, and `I` show Tools, Folder Previews, and Image Information. Persistent chrome never covers the photo. |
| Reload | `F5` reloads without blanking. An external change with unsaved edits keeps the last good frame and asks for F5. |
| Save As and Trash | `Ctrl/Cmd+Shift+S` exports. Delete moves the visible image to Trash. `U` restores the recoverable receipt when the platform can prove it. |

### Admin

| Action | Required result |
| --- | --- |
| `viewr doctor` | Reports binaries, worker protocol, windowing libraries, and graphics runtimes. A passing last line is not proof that a window opened. |
| Clean install of the current preview | The published installer command uses the immutable v0.5.0 asset until v0.6 is tagged. No background updater runs. |
| Help > Get latest release | The Update modal names the running version, refuses to check a network, and only the explicit button hands the release URL to the browser. |
| Unsigned preview | OS trust warnings may appear. Docs do not tell anyone to disable platform security. |

### Failure recovery

| Action | Required result |
| --- | --- |
| First-open decode failure | The empty canvas does not claim a previous image is visible. Retry stays available. |
| File deleted outside viewr | The last good frame stays. A polite status says the selected path no longer names that file. |
| Worker or preview loss | Named recovery status, not a silent busy state. Crop or preview work that cannot continue says to close and reopen before trying that path again. |
| Restore ownership unsettled | Undo Trash does not claim a settled receipt. A new Trash move waits. |
| Launch missing a windowing library or GPU runtime | The process prints an actionable message and exits non-zero. `doctor` names the same gap. |

### Visual polish and budgets

| Action | Required result |
| --- | --- |
| Empty, opening, and error cards | Opaque themed surfaces, AA text, stable geometry across unchanged frames, no vertical drift. |
| Light, Dark, Console, and inspection backgrounds | Chrome tokens stay AA. Image pixels are unchanged by Appearance. |
| Mixed DPI and multi-monitor | Text, focus rings, and panels stay usable at 100%, 150%, and 200%. Moving the window between monitors refreshes the display profile on unmanaged Windows-legacy and real X11. |
| Startup and navigation | Window-ready, first-pixel, navigation, idle-redraw, and memory numbers meet [PERFORMANCE.md](PERFORMANCE.md) on CI and do not regress on representative hardware. |

## Recording a result

Create no result file until one platform run is complete. Store a completed run at
`docs/release-evidence/product-quality/<version>/<platform>.md`, using `windows`,
`macos`, or `linux` for the platform name. Each record must include the tested
version, commit SHA, artifact filename, SHA-256, display scale, graphics adapter,
and Pass, Fail, or Approved exception for every row above.

Use only synthetic filenames and fixtures. Do not include a personal image,
private path, raw metadata, or unrelated screen content. If the tested artifact
bytes change, the record no longer closes the gate.

Do not tag v0.6.0 while any required platform row is unrecorded or any
high-severity product-quality issue remains.
