# Accessibility validation

**Status on 2026-08-27:** native AccessKit delivery is implemented on Windows,
macOS, and Linux. Semantic unit tests, contrast tests, keyboard tests, cross-target
builds, and an external Windows UI Automation smoke test pass. Windows CI retries
transient UI Automation focus rejection without weakening semantic assertions.
Manual Narrator, VoiceOver, and Orca acceptance remains required before Phase 8
can close.

## Product contract

viewr must remain fully useful without a pointer. Native assistive technology must
receive the same control identity, state, focus, and image context that a sighted
keyboard user receives. Visual focus, accessible focus, and the action actually
performed must agree.

The minimum contract is:

- The application, File, Edit, View, Tools, and Help menus have stable names and
  keyboard focus.
- Custom-painted tools, disclosure controls, and previews are actionable buttons
  with descriptive names and selected or expanded state where applicable.
- Tools, Folder Previews, and Image Information expose their visible state. Their
  `T`, `G`, and `I` shortcuts are visible in View > Panels and included in the
  accessible menu names. Tools and Folder Previews also expose collapse and expansion
  actions.
- Full-Image Mosaic exposes every ready complete photo as a position-only button,
  without putting a filename into the grid. Visual and semantic selected state
  agree. Arrows move selection, Enter opens, Page Up and Page Down change groups,
  and Escape returns. Loading and memory-constrained ready counts remain textual.
- Edit > Rating exposes Unrated and ratings 1 through 5 as a selected radio group
  with `0` through `5` shortcut text. View > Rating Filter exposes All images and
  minimum ratings 1 through 5 as a separate selected radio group. Current rating,
  active threshold, filtered position, no-match state, and write outcomes are
  textual. The first-write modal focuses Cancel once and blocks background input.
- The metadata-retention checkbox is named plainly and starts unchecked.
- Source Privacy exposes supported EXIF count, privacy-risk presence categories,
  and the limited-scan caveat as text. Sensitive raw metadata is not used as an
  accessible name or value.
- Current filename, folder position, source dimensions, and displayed zoom are
  available as text. During a genuine replacement load, the presented filename
  remains truthful to the visible pixels while a separate bounded filename names
  the selected target. Failure and Retry name that same target. Derived preview
  work is identified as preview preparation rather than a new file open. Loading,
  failure, and preview-preparation labels use polite AccessKit live-region
  semantics; long target text remains bounded and discoverable when visually
  elided. Completed or failed rating-write toasts are polite because they are the
  outcome source. `Saving rating...` and ordinary transient toasts remain semantic
  but non-live, so a coexisting visual toast is not a second announcement source.
- The empty state exposes drop, file-versus-folder session scope, and the
  local-only privacy line as visible text, followed by separately named Open File
  and Open Folder actions. Opening and failure headings name the selected file.
  Failure copy stays one short line with a named Retry action.
- Crop exposes exact source-pixel origin, output dimensions, ratio, eight resize
  handles, and keyboard controls at every positive selection size. If application
  fails while the same source is current, the exact selection returns as the
  persistent retry surface and the fixed visual message names Enter as retry.
- Spot Heal exposes brush radius, feather, ranked-source position, Refresh Source,
  Undo, Redo, and Done. `/` refreshes the source without requiring a pointer.
  Edit success is exposed only after decoded pixels, history, and presentation
  commit together. Busy destructive keyboard actions produce specific visible
  wait text instead of a silent no-op; transient toast announcement remains part
  of the manual target-OS matrix.
- Appearance exposes its current preference on the parent View entry, then System,
  Light, Dark, and Console as described radio choices.
  Each semantic name includes its visible outcome, System reports its effective
  Light or Dark mode while active, and the visible scope text distinguishes app
  appearance from image pixels and independent background overrides. Every
  resolved palette meets the same automated AA contrast floor. Normal missing
  state is quiet; abnormal startup fallback announces `Could not restore saved
  appearance. Using System.` once through the semantic status surface.
- About is a named modal window, blocks background input, describes the local-only
  privacy contract, and closes with an explicit button or Escape. It exposes the
  grouped shortcut catalog, including `[` / `]`, `F5`, `T` `G` `I`, Space-to-fit,
  `F` / `F11`, `Shift+G`, Escape, Save As, and Undo Trash. Close stays inside the
  minimum window.
- Update viewr is a separate named modal window that blocks background input and
  exposes the running version and one clearly named Get latest release button. It
  closes with an explicit button or Escape. The application performs no automatic
  check; only activating Get latest release asks the operating system to open the
  official stable release in an external browser.
- Animation exposes current frame, frame count, and pause/resume state.
  Previous frame and Next frame are named buttons with `[` and `]`.
- Multi-page TIFF and ICO expose current page or icon identity, count, and
  dimensions. Previous and Next are named buttons with `[` and `]`. Documents
  never auto-play.
- Reload and Retry remain reachable and expose progress or failure as semantic
  text without clearing the last good image.
- Open With is reachable from both File and the image right-click
  surface. Its help names the original source and external-app trust boundary.
  Successful delegation may reload a later external change when in-memory edits
  are idle; otherwise one persistent polite `F5` reminder stays with the last
  good frame. Cancellation and failure do not claim that an edit occurred.
- Restore exposes one polite operation status while native work runs. Conflicting
  open, navigation, edit, Trash, and permanent-delete controls are disabled; zoom,
  pan, panels, and appearance remain usable. Closing changes the status to say the
  restore is finishing before exit. No percentage or cancel control implies an
  unsupported guarantee. Unexpected worker loss replaces the active status with
  durable recovery guidance and directs `U` reconciliation. A new Trash move stays
  disabled until that ownership is settled, so a newer receipt cannot replace an
  uncertain action.
- Permanent delete uses a native warning dialog whose actions are named Delete
  permanently and Cancel. Its filename cannot inject controls, bidi overrides, or
  a second quote-delimited name into the confirmation text.
- Loading, empty, and error states are available as semantic text without relying
  on color. Target-OS announcement timing remains part of the manual matrix.
- No panel covers the image, including at high display scale or when both side
  panels share an edge.

## Automated native Windows smoke test

After building the workspace on Windows, run:

```powershell
pwsh -NoProfile -File scripts/accessibility-smoke.ps1 `
  -Binary target/debug/viewr.exe
```

UI interaction uses the operating system's UI Automation client against the real
out-of-process AccessKit tree. The script also uses local Win32 window and keyboard
messages, WPF only to encode a disposable JPEG, Shell Property System and
filesystem APIs for metadata and alternate-stream checks, and an optional GExiv2
probe. It creates three small disposable images beneath `target/` and verifies:

- the application root and menu focusability;
- the visible first-run drop and file-versus-folder session scope and both open
  actions;
- the Update viewr and About modals' native window identities, action paths,
  truthful browser-handoff and local-only application boundaries, and close actions;
- visible Appearance scope, descriptive System, Light, Dark, and Console radio
  names, all four selected-state transitions, the exact isolated preference file,
  and Console selection after a real process restart;
- filename, dimensions, and folder position;
- default-hidden panel state;
- native actions for showing and collapsing Tools, entering Spot Heal, and finding
  its named radius, feather, and refresh controls;
- distinct selected states and native actions for left/right panel placement;
- the metadata checkbox's default-off and toggled-on state;
- the enabled current-image Move to Trash action and absence of the removed
  mark, review, and batch-trash controls;
- the exact disabled `Undo Trash` label before a recoverable receipt exists;
- showing Folder Previews and discovering both thumbnail buttons; and
- accessible thumbnail activation and resulting image navigation;
- first-write rating disclosure and safe initial focus, native `0`, `4`, and `5`
  key handling, minimum-rating radio state, filtered-empty recovery, and rating
  persistence across a real process restart; and
- the resulting JPEG through Windows Shell Property System `System.SimpleRating`,
  with an additional GExiv2 read through a supplied `-GExiv2Python` executable or
  the default GIMP 3 Python when present. The result reports checked or skipped.

In-process semantic regressions separately cover full-image mosaic position-only
photo buttons and selected identity, settled Undo Trash ownership, its path-free
other-folder guidance, menu bounds, and generic copy while restore ownership is
active or uncertain. Mosaic keyboard and native dynamic-state behavior plus
announcement timing remain in the manual target-OS matrix.

It closes the exact process it launched and removes its three known fixtures, the
isolated `viewr/appearance` preference tree, and the empty unique directory. The
Windows CI job runs the same script against the debug binary. Every wait is bounded
both per operation and by one absolute five-minute suite deadline, which reports
the active probe and accessible tree if reached, so successful earlier waits cannot
extend the job indefinitely.

This test proves that the native provider and action path work. It does not prove
speech quality, reading order under every screen-reader mode, target-platform
keyboard conventions, or human usability. It therefore does not replace the
manual matrix below.

## Manual release matrix

Use disposable copies, never personal photos, for trash and overwrite checks.
Record the operating-system version, assistive-technology version, package type,
display scale, graphics adapter, and result for every platform.

| Platform | Required assistive technology | Status |
|---|---|---|
| Windows | Narrator | Not yet recorded |
| macOS | VoiceOver | Not yet recorded |
| Linux | Orca through AT-SPI | Not yet recorded |

Run the same workflow on every platform:

| Area | Action | Required result |
|---|---|---|
| Launch | Enable the screen reader, record the empty window dimensions, then open the first image in a folder of at least three | `viewr`, the current file, position, dimensions, and zoom are discoverable; focus is not lost during initial decode; the application client area keeps the recorded dimensions |
| Focus | Traverse forward and backward through the window | Order is predictable, every interactive item has one clear name, and visual focus follows assistive focus |
| Menus | Open File, Edit, View, Tools, Help, Panels, Panel Position, Image Background, and Appearance from the keyboard | Items, shortcuts, checked states, radio states, disabled states, and submenus are announced accurately |
| Panels | Show and hide Tools, Folder Previews, and Image Information with `T`, `G`, and `I` | State changes are announced, hidden controls leave the tree, and the image refits without being covered |
| Disclosure | Collapse and expand Tools and Folder Previews | The control name changes between Collapse and Expand, remains actionable, and preserves the panel's visible state |
| Position | Move Tools and Image Information left and right, including both on one side | Selected radio state is announced and controls remain reachable in a coherent order |
| Navigation | Use Left, Right, Home, End, Page Up, Page Down, and a preview button | Immediate reuse remains quiet; a genuine miss names the selected target while the visible filename remains tied to presented pixels; stale decode completion never announces the wrong image |
| Reload | Invoke File > Reload File and `F5` on a disposable file changed by another app | Reload is announced, the old frame remains until success, and a failed refresh exposes Retry without losing focus |
| Open With | Inspect File > Open With and the image right-click action; choose and cancel a disposable editor handoff, then make one external change | Both entry points have one clear name and boundary explanation; cancellation is quiet and safe; the selected app receives the original rather than unsaved viewr edits; a safe external change reloads without blanking; unsaved viewr edits keep the last good frame and ask for F5 |
| File gone | Delete the presented file from outside viewr | The last good image stays visible and a polite status says the selected path no longer names that file |
| Pending sibling gone | Select a sibling and remove it before its first presentation | The stale entry leaves the folder position, the last good frame remains until a surviving image opens, and recovery or Retry is announced once without exposing a path |
| View | Use Fit, Actual Size, Zoom In, Zoom Out, and Fullscreen | The action and resulting zoom are discoverable; fullscreen does not strand focus |
| Full-image mosaic | Enter from View or `Shift+G`; traverse every ready photo with arrows and the screen reader; change groups; open one; repeat under a rating filter; cite the bounded-admission and stable-announcement tests named by PQ-PW-08; leave with Escape | Each complete photo is a named position-only button with selected state and visible focus; filenames are absent; one stable loading state and the terminal ready or safely fitted count are announced without flooding; group order follows the active projection; open and leave restore coherent single-photo focus |
| Editing | Rotate, flip, and start crop | Each tool has one descriptive name and the visible result matches the invoked action |
| Crop | Select landscape, portrait, Original, and custom ratios; swap orientation; move with Arrow keys; resize with Shift plus Arrow keys and every pointer handle; apply with Enter; cancel with Escape; inspect a very small selection and an injected apply failure | Ratio and exact source origin/output size remain available at every positive size; a rotated 16:9 selection remains 16:9 in output; failure restores the exact selection and Enter retry; apply and cancel return focus predictably |
| Spot Heal | Enter with `J`; change radius and feather; paint a disposable defect; invoke Refresh Source with `/`; Undo and Redo; finish with Escape | Every control and busy state is named, source position changes, refresh remains one undo step, edit success follows visible presentation, and the pointer-only brush overlay is not the sole source of state |
| Appearance | Read the current preference on the parent View entry and the chooser scope and descriptions; select System, Light, Dark, and Console; then restart | The parent state, each full outcome, and each selected radio state are announced, System reports Light or Dark only while active, native and app chrome agree, the choice survives restart, and Console remains readable with monospaced interface type |
| Ratings | On disposable JPEG copies, use Edit > Rating and `0` through `5`; confirm and cancel the first-write disclosure; apply All, 3+, 4+, and 5+ filters; navigate into and recover from no matches; then restart | Rating and filter radio state, shortcut ownership, current rating, filtered position, outside-filter state, write outcome, and Show all images are announced without color or star-glyph dependence; Cancel initially owns modal focus; unsupported files remain untouched; the embedded rating survives restart |
| Update | Open Help > Get latest release; read its contents without activating the release action; close with its button and Escape | A modal named Update viewr exposes the running version, no-automatic-check behavior, browser handoff boundary, and one clearly named Get latest release button; background controls cannot activate and focus returns predictably |
| About | Open Help > About viewr; read its contents; close with its button and Escape | A modal named About viewr exposes version, platform, license, the grouped shortcut catalog including pages, reload, panels, and Space-to-fit, and privacy; background controls cannot activate while it is open; Close remains reachable on a short window; focus returns predictably |
| Animation | Open GIF, WebP, and APNG fixtures and toggle playback from Image Information | Frame position and play/pause state are announced without flooding speech on every timed frame |
| Pages | Open a multi-page TIFF and a multi-size ICO; use Image Information, View, `[`, and `]` | Page or icon identity, count, and dimensions are announced; documents do not play; crop and heal block a step |
| Metadata | Inspect Source Privacy with no EXIF and with each supported risk category, then toggle Keep camera metadata when saving | Tag count, category presence, and limited-scan caveat are announced without raw sensitive values; absent supported EXIF is not called clean; retention starts unchecked, announces checked state, and remains session-only |
| Save | Open Save As to a new disposable destination and complete or cancel it; then choose an existing disposable destination, inspect the app-owned overwrite modal, cancel once, and repeat to confirm replacement | The native dialog is usable; the modal has an accurate name and recheck disclosure, initially focuses Cancel, exposes only Cancel and Replace file as enabled actions, changes nothing when canceled, replaces only the confirmed destination when accepted, and returns focus predictably after either outcome |
| Trash | Use File > Move to Trash and `Delete` on disposable copies; confirm bare `B`, `M`, and normal-mode `X` do nothing; restore with `U`; inspect settled, active, and uncertain Undo ownership; try Delete during active Spot Heal; inspect a control-character filename in permanent-delete confirmation but cancel | Only the visible current image moves, the removed culling keys trigger no destructive or review action, active work has a specific result, confirmation is path-free and visually unambiguous, cancel is safe, unsettled Undo does not claim settled state, cross-folder Undo does not insert into the unrelated view, and restore uses only the exact receipt. Transient result announcement remains a manual target-OS check |
| Loading and errors | Open a large valid image, an unsupported file, and a malformed supported file | Loading, failure, and Retry name only the selected filename; each transition is announced once; menus and recovery actions remain reachable |
| Contrast and scale | Repeat key flows at 100%, 150%, and 200% scale in Light, Dark, and Console with white and dark image backgrounds | Focus and text stay visible; no clipping, overlap, or image-covering panel appears |

## Linux-specific checks

Record the distribution, desktop environment, display protocol, and whether the
app came from a bare build or Flatpak. Orca must receive the native AT-SPI tree
through local Unix IPC. Startup must fail rather than weaken privacy when a
configured `DBUS_SESSION_BUS_ADDRESS` or `AT_SPI_BUS_ADDRESS` uses a non-Unix
transport, or when the seccomp policy cannot be installed and verified.

The Linux seccomp runtime suite must pass on the same release candidate. It covers
native and x32 socket numbers on x86-64. This is separate from Orca behavior and
both results are required.

## Recording a result

Create no result file until one platform run is complete. Store a completed run at
`docs/release-evidence/accessibility/<version>/<platform>.md`, using `windows`,
`macos`, or `linux` for the platform name. Each record must include:

- the tested version, repository commit SHA, artifact filename, and SHA-256;
- the test date and accountable reviewer or team;
- the operating-system version, assistive-technology version, package type,
  display scale, graphics adapter, and Linux desktop and display protocol when
  applicable;
- Pass, Fail, or Approved exception for every workflow row above, with a concise
  evidence reference for each failure or exception;
- exact reproduction steps, expected and actual announcement, focus before and
  after, and whether each failure reproduces without the screen reader; and
- the scope, rationale, approver, and required retest condition for every Approved
  exception.

Use only synthetic filenames and fixtures in tracked evidence. Do not include a
personal image, private path, raw metadata, or unrelated screen content. Link the
platform's Status cell above to its completed record only after every row has a
disposition. If the tested artifact bytes change, the record no longer closes the
gate and that platform must be rerun against the new SHA-256. Do not mark the
ROADMAP accessibility item complete until all three platform records pass or each
remaining exception is deliberately scoped and approved.
