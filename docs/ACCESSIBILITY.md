# Accessibility validation

**Status on 2026-07-25:** native AccessKit delivery is implemented on Windows,
macOS, and Linux. Semantic unit tests, contrast tests, keyboard tests, cross-target
builds, and an external Windows UI Automation smoke test pass. Manual Narrator,
VoiceOver, and Orca acceptance remains required before Phase 8 can close.

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
- Tools, Folder Previews, and Image Information expose their visible state. Tools
  and Folder Previews also expose collapse and expansion actions.
- The metadata-retention checkbox is named plainly and starts unchecked.
- Current filename, folder position, source dimensions, and displayed zoom are
  available as text.
- Crop exposes exact source-pixel origin, output dimensions, ratio, eight resize
  handles, and keyboard controls.
- Spot Heal exposes brush radius, feather, ranked-source position, Refresh Source,
  Undo, Redo, and Done. `/` refreshes the source without requiring a pointer.
- Appearance exposes System, Light, Dark, and Console as named radio choices.
  Every resolved palette meets the same automated AA contrast floor.
- About is a named modal window, blocks background input, describes the local-only
  privacy contract, and closes with an explicit button or Escape.
- Animation exposes current frame, frame count, and pause/resume state.
- Reload and Retry remain reachable and announce progress or failure without
  clearing the last good image.
- Loading, empty, and error states are announced without relying on color.
- No panel covers the image, including at high display scale or when both side
  panels share an edge.

## Automated native Windows smoke test

After building the workspace on Windows, run:

```powershell
pwsh -NoProfile -File scripts/accessibility-smoke.ps1 `
  -Binary target/debug/viewr.exe
```

The script uses only the operating system's UI Automation client. It creates two
small disposable images beneath `target/`, launches the real app, discovers the
out-of-process AccessKit tree, and verifies:

- the application root and menu focusability;
- the About modal's native window identity, action path, and close action;
- System and Console appearance radio state, the exact isolated preference file,
  and Console selection after a real process restart;
- filename, dimensions, and folder position;
- default-hidden panel state;
- native actions for showing and collapsing Tools, entering Spot Heal, and finding
  its named radius, feather, and refresh controls;
- distinct selected states and native actions for left/right panel placement;
- the metadata checkbox's default-off and toggled-on state;
- showing Folder Previews and discovering both thumbnail buttons; and
- accessible thumbnail activation and resulting image navigation.

It closes the exact process it launched and removes only its two known fixtures
and empty unique directory. The Windows CI job runs the same script against the
debug binary. Every wait is bounded both per operation and by one absolute
three-minute suite deadline, so a sequence of individually successful waits cannot
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
| Launch | Enable the screen reader, then open the first image in a folder of at least three | `viewr`, the current file, position, dimensions, and zoom are discoverable; focus is not lost during initial decode |
| Focus | Traverse forward and backward through the window | Order is predictable, every interactive item has one clear name, and visual focus follows assistive focus |
| Menus | Open File, Edit, View, Tools, Help, Panels, Panel Position, Image Background, and Appearance from the keyboard | Items, shortcuts, checked states, radio states, disabled states, and submenus are announced accurately |
| Panels | Show and hide Tools, Folder Previews, and Image Information with `T`, `G`, and `I` | State changes are announced, hidden controls leave the tree, and the image refits without being covered |
| Disclosure | Collapse and expand Tools and Folder Previews | The control name changes between Collapse and Expand, remains actionable, and preserves the panel's visible state |
| Position | Move Tools and Image Information left and right, including both on one side | Selected radio state is announced and controls remain reachable in a coherent order |
| Navigation | Use Left, Right, Home, End, Page Up, Page Down, and a preview button | Filename and folder position update once per action; stale decode completion never announces the wrong image |
| Reload | Invoke File > Reload File and `F5` on a disposable file changed by another app | Reload is announced, the old frame remains until success, and a failed refresh exposes Retry without losing focus |
| View | Use Fit, Actual Size, Zoom In, Zoom Out, and Fullscreen | The action and resulting zoom are discoverable; fullscreen does not strand focus |
| Editing | Rotate, flip, and start crop | Each tool has one descriptive name and the visible result matches the invoked action |
| Crop | Select landscape, portrait, Original, and custom ratios; swap orientation; move with Arrow keys; resize with Shift plus Arrow keys and every pointer handle; apply with Enter; cancel with Escape | Ratio and exact source origin/output size update; a rotated 16:9 selection remains 16:9 in output; apply and cancel return focus predictably |
| Spot Heal | Enter with `J`; change radius and feather; paint a disposable defect; invoke Refresh Source with `/`; Undo and Redo; finish with Escape | Every control and busy state is named, source position changes, refresh remains one undo step, and the pointer-only brush overlay is not the sole source of state |
| Appearance | Select System, Light, Dark, and Console, then restart | The selected radio state is announced, native and app chrome agree, the choice survives restart, and Console remains readable with monospaced interface type |
| About | Open Help > About viewr; read its contents; close with its button and Escape | A modal named About viewr exposes version, platform, license, shortcuts, and privacy; background controls cannot activate while it is open; focus returns predictably |
| Animation | Open GIF, WebP, and APNG fixtures and toggle playback from Image Information | Frame position and play/pause state are announced without flooding speech on every timed frame |
| Metadata | Toggle Keep camera metadata when saving | It starts unchecked, announces checked state, and remains session-only |
| Save | Open Save As and complete or cancel a disposable export | The native dialog is usable, cancellation is safe, and focus returns to viewr |
| Trash | Trash and undo a disposable copy; open permanent-delete confirmation but cancel | Confirmation and result are announced; cancel is safe; Undo restores the exact copy |
| Loading and errors | Open a large valid image, an unsupported file, and a malformed supported file | Loading and failure are announced once with useful text; menus and recovery actions remain reachable |
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

For each failure, record exact reproduction steps, expected announcement, actual
announcement, focus before and after, screenshot or short recording when useful,
and whether the defect reproduces without the screen reader. Do not mark the
ROADMAP accessibility item complete until every row passes on all three platforms
or a deliberately scoped exception is documented and approved.
