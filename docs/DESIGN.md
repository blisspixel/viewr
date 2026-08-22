# Design

The visual and interaction system for viewr. It is deliberately small, because
the product is: the photo is the hero and everything else earns its place or
disappears. This spec is the converged result of two rounds of design critique
(visual and motion), recorded so implementation has one source of truth.

## Principles

1. The photo is the hero. It fills the frame edge to edge, letterboxed against a
   solid background, with no border radius and no drop shadow. A photo has no
   rounded corners and casts no shadow; anything else reads as "a card on a web
   page," which is the opposite of what we are.
2. Calm, precise, premium. Nothing moves at rest. Optional chrome is quiet,
   docked, explicitly collapsible, and fully hideable from View. Controls do not
   disappear on a timer or compete with the photo. The interface should feel like
   a native instrument, not a web app.
3. Simple surface, deep engineering. The restraint on screen sits on top of the
   rigor in `STANDARDS.md`. Minimal is the visible result of the work, not the
   absence of it.

## Layout

- The image is scaled to fit the window (aspect preserved) and centered. The
  letterbox area is the solid theme background, nothing else. Fit shrinks a
  large image and leaves a small one alone: it never enlarges past 100 percent,
  because scaling a 64px by 64px source to fill the window is arithmetically
  honest and visually wrong, and a soft interpolated wall no longer reads as a
  small image. A source smaller than the viewport therefore rests at actual
  size, and enlarging it is something the player asks for with zoom.
- Loading an image never changes the application-window dimensions. The initial
  1000px by 720px logical window, or the dimensions the user chooses by resizing,
  remains stable while image fit and zoom resolve inside its viewport.
- The first window is bounded by the monitor it opens on and placed near the top
  of it. A desktop reserves space no process can query here, so viewr asks for at
  most 90 percent of the monitor's logical width and 70 percent of its height,
  never less than the 640px by 480px minimum, and leaves a 5 percent margin above
  the window. The remaining bottom fifth of the monitor stays free for a taskbar,
  dock, or panel, because desktop furniture is not symmetric and a centered or
  cascaded window would spend that margin in the wrong place. A small display
  therefore opens a smaller window that is fully reachable, and Wayland, which
  does not accept client placement, still gets the bounded size.
- Persistent chrome never overlaps the image. The image fit rectangle is computed
  from the window minus every visible docked panel. Opening, closing, or resizing
  chrome refits and recenters the photo inside the remaining viewport.
- Top bar: a fixed 40px neutral surface with five conventional menus: File,
  Edit, View, Tools, and Help. Each menu title carries 8px of padding on each
  side and no spacing between titles, so titles sit about 16px apart and
  neighboring highlights meet instead of leaving a dead seam for a pointer
  crossing an open menu bar. The right side shows a stable folder counter and, when space
  permits, the filename, dimensions, and physical zoom percentage, where 100
  percent means one source pixel per physical display pixel. Filename, dimensions,
  and zoom use dedicated 8px reading gaps rather than inheriting the compact menu
  spacing. Long names truncate with the full value available as a tooltip.
- Tools: hidden by default for a clean image-first surface. View > Panels or `T`
  shows a 64px docked panel containing only high-frequency image operations:
  rotate, flip, crop, and Spot Heal. Its vector chevron collapses it to a 44px rail.
  View > Panel Position docks it on either the left or right. Save and destructive
  actions remain in File so the tool surface stays calm.
- Folder Previews: hidden by default. When a folder contains multiple images,
  View > Panels or `G` shows a 112px docked thumbnail strip with the current item
  identified. Its chevron collapses it to a 44px bottom rail.
  Thumbnails decode only while the panel is visible and expanded. The strip stays
  bounded to four neighbors on either side.
- Image Information: an optional 304px panel contains file facts and the explicit
  export-privacy checkbox. Its Source Privacy section reports bounded EXIF tag and
  risk-category presence without displaying raw sensitive values, and states that
  the limited scan cannot prove other metadata or hidden pixel data is absent.
  View > Panels or `I` toggles it, and
  View > Panel Position independently docks it on the left or right.
- View > Panels renders `T`, `G`, and `I` as right-aligned accelerators on selected
  menu buttons. The same shortcuts are included in accessible menu output; no
  hover is required to discover them.
- Empty and loading states use an opaque themed card with tested AA text contrast.
  They remain readable on black, gray, white, and theme-driven image backgrounds.
  The empty state explains drop, ambient sibling browsing, and explicit session
  folder selection without adding a confirmation step. Opening names the selected
  file. A failed open keeps one short error line and Retry. Its measured card
  geometry is stable across unchanged frames, so no resting content drifts
  vertically.
- Crop mode: GPU dims outside the live UV rect to 45 percent brightness. egui draws
  a precise border, rule-of-thirds guides, eight visible pointer handles, exact
  output dimensions, a compact aspect popover, and Apply/Cancel. The popover
  groups Free, Original, 1:1, landscape and portrait photo/video ratios, plus
  numeric custom width and height. A swap control reverses the active ratio.
  Esc cancels; Enter applies.
- Zoom is focal-point anchored (pixel under cursor stays put). Trackpad pixel
  deltas and wheel detents both supported.
- Space held + drag = temporary pan (classic hand tool); Space tap without drag
  resets fit. Fit clears zoom and pan only; rotation, flip, and an in-progress
  crop stay. Left-drag without Space does not pan. The resting cursor over the
  photo is the arrow; Grab appears only while Space is held.
- Fullscreen (`F` or `F11`) is immersive: the top bar and docked panels hide, and
  the photo uses the whole window. Stored panel flags are unchanged, so exiting
  restores them. Spot Heal still docks its inspector while it is active. Escape
  closes a context menu, then cancels crop, then leaves Spot Heal, then exits
  fullscreen. Chrome does not reappear on a timer or mouse move.

## Color

- View summarizes the selected preference as Appearance: System, Light, Dark, or
  Console, then offers those four choices. System follows live
  operating-system changes. Explicit Light and Dark also update native window
  decoration. Console uses a near-black canvas, green phosphor-inspired chrome,
  and monospaced interface type. One validated appearance word is remembered in
  the platform configuration directory through same-directory atomic
  replacement.
- The Appearance chooser exposes one concrete outcome line per radio choice.
  System reports the effective Light or Dark mode while it is active; Console
  identifies itself as the green-screen look. A scope line states that appearance
  changes app chrome and the default canvas, not decoded image pixels, and that
  Image Background overrides the canvas independently. The same complete text is
  the radio's accessible name.
- No preference is the quiet System default. Invalid, oversized, unreadable, or
  unavailable saved state also uses System but produces one semantic, path-free
  startup toast: `Could not restore saved appearance. Using System.` Selecting an
  appearance replaces rejected state when writable; startup never repairs it
  automatically.
- Every appearance owns a complete token set for panel, raised and pressed
  surfaces, borders, primary and secondary text, active state, and text on the
  active state. Standard widgets and custom-painted controls use the same tokens.
  Contrast tests enforce WCAG AA for all three resolved palettes.
- The default image background follows the resolved appearance. Dark uses deep
  ink `#0B0E14`; light uses `#F4F5F7` rather than pure white so bright photos
  retain an edge; Console uses `#010502`. View also offers explicit black,
  neutral-gray, and white inspection backgrounds independently of chrome.
- Dark mode retains accent amber `#F7A845`. Light uses a darker amber that remains
  legible on a bright panel. Console uses phosphor green. In every theme, the
  accent marks active or affirmative state only, never decoration.
- Decoded image pixels use an sRGB GPU texture and mip chain. A bounded embedded
  RGB ICC profile is converted to sRGB before upload; Image Information reports
  conversion or fallback status. The window's display identity is tracked, and
  Image Information reports the sRGB swapchain as operating-system managed, as
  a display profile applied on an unmanaged path, or as a fallback. Unmanaged
  X11 without an admitted profile uses the fallback rather than claiming
  compositor management. A first-open decode failure says Retry is available;
  it does not claim a previous image remains visible when the canvas is empty.
  This is correct for the current SDR working path, not a claim of preserved
  wide-gamut values, CMYK profile handling, or HDR presentation. Those remain
  later roadmap items.

## Typography and icons

- System font stack (`-apple-system`, `Segoe UI`, `system-ui`). Filename ~13.5px
  at weight 550; counter ~12.5px muted, tabular figures.
- Icons: single consistent stroke weight (~1.75px at 24px), rounded joins, drawn
  on a 24px grid. Icon-only buttons with tooltips, except the zoom value.

## Motion and interaction

The current interface uses immediate state changes. It does not claim transition,
inertia, or reduced-motion behavior that has not been implemented and tested.

### Navigation (the most-touched interaction)
- Default is an instant texture swap, no crossfade. Held arrows during rapid review
  must never fight an animation.
- Reversing to a pristine frame that is still presented cancels the abandoned
  replacement and settles without another decode or texture upload. Once a new
  frame from a move within two positions is presented, the just-left pristine
  source decode can remain in the existing bounded neighbor cache for a normal
  immediate cache hit. Larger jumps, crop and Spot Heal results, animation
  playback frames, explicit-Reload state, and over-budget or evicted images use
  the normal loading path.
- A prefetched cache hit replaces the texture immediately. On a cache miss,
  reload, or failed replacement, the last good image and its filename remain
  visible while the selected folder position advances. Loading and failure status
  names the selected target by a bounded, path-free filename; the visible
  filename, dimensions, and zoom remain attached to the pixels on screen. Crop
  preview preparation uses separate copy, and immediate reuse or a full-resolution
  cache hit emits no loading state. Source-load preview or upload failures stay in
  the existing durable Retry flow. Target status is width-capped, contracts
  further at the minimum window width to preserve separate menu and playlist
  controls, and exposes its bounded full text when elided. A
  coexisting visual toast is not a second live region. There is no
  black/background flash, shimmer, slide, crossfade, or edge bounce.
- File > Reload File (`F5`) bypasses the decoded-neighbor cache and reads the
  current path again. It resets in-memory view edits only when the action is safe,
  retains the last good frame during decode, and exposes Retry on failure.
- File and the image right-click surface expose Open With.... The action uses a
  native user-mediated chooser for the exact accepted source and never
  constructs a shell command or stores an editor preference: Windows
  `SHOpenWithDialog`, macOS application picker plus NSWorkspace, and Linux
  desktop-portal OpenURI with `ask`. Compact help explains that the original
  metadata is included, unsaved viewr edits are not, and the chosen app can
  modify the source. When that app changes the file, viewr reloads it if an
  in-progress or applied crop, heal, rotate, and flip are idle; otherwise a
  path-free status asks for `F5`. A missing path keeps that last good frame
  with a durable status. A rename follows the same object in the folder list.
  Cancellation and launch failure are distinct. A session watcher also observes
  the current file and folder without writing history.

### Zoom and pan
- Wheel zoom is focal-point anchored: the pixel under the cursor stays under the
  cursor. This is non-negotiable and is half of what makes the viewer feel correct.
- Zoom steps are geometric (x1.15 per wheel detent). Trackpad pixel deltas map to
  bounded continuous steps. `0` fits, `1` selects exact physical pixels, and `+`
  or `-` zooms around the image-safe viewport center.
- Pan follows the pointer directly with no inertia or rubber banding. Holding Space
  temporarily selects pan; tapping Space without dragging resets fit.

### Ratings and folder filter

Ratings are implemented for the deliberately narrow writable profile defined by
`docs/RATINGS.md`. Ordinary, identity-bound JPEG files with supported metadata
are writable on Windows, macOS, and Linux. Unix sources with extra hard links,
and unsupported containers, remain visibly read-only.

- The source image is the only durable rating record. viewr writes standard
  embedded 0-to-5 metadata after one explicit disclosure per session and does not
  create a database, sidecar, alternate stream, timestamp, or activity record.
- In normal viewing mode, `0` clears a rating and `1` through `5` assign it. Fit
  Image to View and Actual Size move to the primary modifier plus `0` and `1`.
  Numeric input, menus, popups, modals, Crop, Spot Heal, and key repeat always win
  over rating shortcuts.
- Edit owns rating assignment. View owns the session-only minimum-rating filter.
  The current textual rating and any active filter remain visible outside menus.
  When no image matches the filter, Escape or folder-navigation keys restore All
  images instead of doing nothing.
- One canonical natural-order folder catalog owns Trash and Undo positions. A
  filter derives visible indices for navigation, Folder Previews, and prefetch.
- Unsupported, malformed, conflicting, read-only, changed, or unsafe sources stay
  untouched and expose fixed recovery copy. No persistence fallback exists.

### Delete and undo
- `Delete` moves only the currently displayed file to the operating-system Trash.
  File > Move to Trash exposes the same action. There is no bare-letter shortcut,
  mark state, review mode, or batch-trash action. Destructive intent therefore
  stays attached to a conventional key and a visible current target.
- Accepted pixels retain their exact open source handle through foreground,
  worker, animated, prefetched, cropped, and edited presentation. Trash runs only
  when the pathname still identifies the retained source that supplied accepted
  pixels.
  A missing, replaced, linked, or unverifiable entry fails closed without changing
  playlist or Undo state. Success advances the playlist deterministically and
  shows a three-second non-blocking toast. `U` owns the latest safely recoverable
  Trash action. Windows and Linux identify each move by its
  sole new Trash item identifier only when that item's native identity matches the
  live accepted-source handle; macOS uses the exact resulting URL with the same
  handle. Restore repeats the identity check and never falls back to an older item
  with the same original pathname. Restore then makes a later platform move using
  the checked identifier on Windows and Linux or checked Trash URL on macOS, so
  this final boundary is narrow but is not a handle-bound transaction against a
  hostile concurrent swap.
- File > Undo Trash does not expose filenames, paths, or history. Its label stays
  generic while restore work is active or its result is uncertain.
- A successful move without an exact in-app receipt remains recoverable through
  the system Trash and preserves any previous valid `U` action. Permanent delete
  never replaces or clears that prior Trash action, and its success copy explicitly
  separates the non-recoverable deletion from the older action still assigned to
  `U`.
- Restore transfers exact native inputs to one typed worker. The top bar shows a
  polite operation state while playlist, edit, and destructive actions wait;
  zoom, pan, panels, and appearance remain responsive. A normal close request
  changes the status to finishing, then waits for result reconciliation and worker
  join. The design offers no false percentage or cancel control. Spawn failure
  keeps state unchanged; result-channel loss keeps the receipt and directs system
  Trash review through durable, operation-bound polite status. Unresolved restore
  recovery disables a new Trash move until `U` produces a typed reconciliation,
  preventing newer receipt ownership from replacing an uncertain action.
- Foreground reload, preview preparation, crop, Save As, an active Spot Heal
  stroke, or a heal worker owns current state. Trash, permanent delete, and restore
  wait instead of racing that work. Keyboard shortcuts give a specific visual wait
  reason. Restore retains only transient and resolvable receipts for `U`. Missing
  exact items end the in-app retry; ambiguous, unsupported, and invalid receipts
  direct the user to system Trash review without claiming that `U` can help.
- During Crop, `X` remains the crop-ratio orientation shortcut.
- Permanent delete verifies the retained accepted source before showing its
  bounded control-safe and quote-safe filename, labels its affirmative action
  Delete permanently, and labels the alternative Cancel. After confirmation it
  verifies the same source again immediately before deletion; a confirmation-time
  replacement remains untouched and receives fixed recovery guidance.
  Platform Trash and restore failures cross a fixed path-free category boundary
  before entering interface copy or operator diagnostics.

### Crop
- Enter crop mode with `C` or select Crop from the Tools panel. Crop immediately
  dims outside a centered default rectangle. The aspect popover provides Free,
  Original, 1:1, 3:2, 2:3, 4:3, 3:4, 5:4, 4:5, 5:3, 3:5, 16:9, 9:16, and a
  numeric custom ratio. Landscape and portrait choices are grouped, and the swap
  control reverses any fixed choice without reopening the popover.
- Crop begins with a usable centered selection, so it never requires a pointer.
  Arrow keys move the selection; Shift plus an arrow resizes it; holding Ctrl uses
  a fine adjustment step; Enter applies; Esc cancels. Locked aspect ratios remain
  locked during keyboard resizing.
- Pointer drag on the interior moves the selection, the eight handles resize it,
  and a drag outside redraws it. Exact source origin and output-pixel dimensions
  are visible and published to the accessibility tree. Fixed ratios describe the
  visible exported orientation, so selecting 16:9 after a 90-degree rotation still
  produces a 16:9 output. Confirming applies the crop at full decoded resolution
  off the UI thread. The attempt carries the exact selection, view, paused
  animation state, source generation, and decoded-image identity until renderer
  presentation succeeds. Compute, preview, or renderer failure leaves original
  pixels unchanged and restores the same selection for Enter-key retry. A source
  change cooperatively cancels obsolete row copying and never restores stale
  state. After a failed image load or Reload, crop entry and Apply stay blocked
  until Retry succeeds, so retained last-good pixels cannot become a new edit
  source. Esc cancels crop before it can affect fullscreen state.

### Source animation

- GIF, WebP, and APNG timing is content, not decorative interface motion. Frames
  are bounded in count and bytes, honor container delay and loop behavior, and can
  be paused or resumed from Image Information. `[` and `]` step one frame without
  wrapping and pause timed playback first.
- Navigation and a successfully presented crop deterministically stop or discard
  playback state tied to the old source. A failed crop restores the paused
  playback and pending auxiliary ownership it captured. Rotation, flips, and
  pixel edits are applied consistently to each displayed frame. A late animation
  decode cannot replace a newer image.

### Source pages

- Multi-page TIFF and multi-size ICO are documents, not animations. They reuse
  the bounded sequence model, never auto-play, and may differ in size. Image
  Information and View expose Previous/Next with `[` and `]`. TIFF identity is
  Page N of M. ICO identity is Icon N of M plus pixel size, starting on the
  already-presented largest still. An in-progress crop or Spot Heal refuses a
  page change instead of destroying the edit. A dimension change refits.

### Spot Heal

- Enter with `J`, Edit > Spot Heal, or the Tools icon. The temporary inspector
  docks beside Tools on the selected left or right edge and reserves viewport
  space. It never floats over the photo.
- The inspector exposes brush radius, feather, Refresh Source, Undo, Redo, and
  Done. A translucent brush mask and dual-contrast cursor ring are the only
  elements drawn over the image. `/` advances to the next ranked source.
- Drag over one small blemish and release to repair it off the UI thread. The
  solver ranks up to eight spatially distinct clean sources using robust boundary
  color, local tone, and edge-gradient agreement. The selected patch receives a
  bounded per-channel tone adjustment before feathered compositing. If no clean
  translated source fits, a distance-ordered directional fill continues local
  gradients instead of repeatedly averaging a flat blur. The source file remains
  untouched; Save As is the only edit-persistence path.
- Repair, undo, and redo apply decoded pixels and present the same bounded patch
  before committing history or success copy. If patch presentation is
  unavailable, full-texture presentation is attempted. If both fail, exact
  inverse pixels are restored and history remains unchanged. An internal inverse
  failure automatically reloads the selected source. Successful edits regenerate
  the dependent mip chain. If the GPU cannot display the complete decoded image
  in one texture, Spot Heal is
  unavailable instead of risking an edit at the wrong source coordinate.
- `Ctrl+Z` or `Command+Z` undoes an in-memory pixel patch, the shifted equivalent
  redoes it, and Esc leaves the tool. A submitted repair finishes and applies
  after the inspector closes; navigation clears edit history and any stale worker
  result.
- Spot Heal is deliberately scoped to small repairs. It does not expose prompts,
  model settings, generative fill choices, or an automatic enhancement mode.

These controls follow the practical Heal contract documented by
[Adobe Lightroom](https://helpx.adobe.com/lightroom/desktop/using/heal-tool.html):
size, feather, and a way to refresh automatically chosen source content. The
ranking design takes the bounded, deterministic part of the nearest-neighbor
patch approach described by
[PatchMatch](https://gfx.cs.princeton.edu/pubs/Barnes_2009_PAR/index.php), while
the fallback follows the structure-propagation direction of exemplar and fast
marching inpainting rather than adding a model runtime. Full global PatchMatch,
generative fill, and an unbounded Poisson solve are outside this focused tool.

### Help, updates, and product identity

- Help > About viewr opens a centered modal that blocks background input and
  closes with its Close button, backdrop click, or Escape. On a short window the
  body scrolls so Close stays reachable.
- It exposes version, platform, license, the grouped shortcut catalog, and the
  local-only privacy contract. About, the empty state, and README essential
  controls quote `shortcuts` instead of a truncated one-line summary. Its modal
  container has an explicit accessible window name.
- Help > Get latest release opens a separate centered modal with the running
  version and one prominent Get latest release action. It does not check a network,
  claim that the running build is latest, download, or install. The action opens
  the official stable release in an external browser only after the user activates
  it. Terminal installation commands remain in the install guide and CLI.

### Micro-interactions
- Buttons use deterministic hover, active, selected, and focus colors. Custom
  controls paint a visible 2px amber focus ring.
- Panel state is immediate and deterministic. `T`, `G`, and `I` control full
  visibility; chevrons control compact collapse; View controls left/right position.
  Each action performs one state change, reserves the exact new viewport, and
  refits the image. No panel slides over the photo, no control hides on pointer
  movement, and no idle timer changes layout. Never animate backdrop blur or
  box-shadow.

### Anti-patterns (do not)
Crossfade on every navigation; spinners on prefetched images; a black or
background flash between images; easing on directly-dragged values; staggered or
bouncy chrome entrances; a confirmation modal for delete; any idle motion on the
photo (parallax, Ken Burns, bounce); floating tool or thumbnail panels that cover
the image; disappearing controls that are difficult to summon; unprompted AI suggestions, "smart" tooltips, or any proactive UI popups; ASCII arrows used
as disclosure icons.

### Reduced motion
Current interactions are immediate and contain no positional, spring, parallax, or
idle animation. If motion is added later, reduced-motion behavior must land with it
and be validated before the motion can ship.

## Accessibility

- Every interactive element has a visible `:focus-visible` amber ring and a
  keyboard path. Keyboard-first is the primary mode.
- Hit targets are at least 36px; icon resting color holds AA contrast on the
  background; there is no permanent low-contrast tutorial text (the mockup's
  helper line does not ship).
- Custom-painted icon buttons, disclosure buttons, and thumbnails publish
  explicit button labels and selected state to egui's accessibility tree.
- Crop publishes its exact source-pixel origin and dimensions. Windows and macOS
  connect the egui tree directly to native assistive technology through AccessKit.
  Linux connects through AccessKit/AT-SPI only after startup validates local Unix
  D-Bus addresses and installs a fail-closed kernel policy denying Internet socket
  creation and io_uring. An external Windows UI Automation test exercises the
  native provider and action path. Manual screen-reader acceptance remains
  required on all three targets; `ACCESSIBILITY.md` defines the release matrix.
- Tooltips carry the keyboard shortcut so power users learn the keys. Automated
  tests enforce at least a 4.5:1 contrast ratio for normal text, muted text,
  accent controls, and primary-button text on their actual surfaces.

## The invariants that define "exceptional"

1. Focal-point-anchored wheel zoom: the pixel under the cursor stays locked under
   the cursor.
2. Instant navigation that holds the old texture until the new one is ready, so
   there is never a black frame between images.
3. Color fidelity from the source profile to the display that owns the window,
   with explicit fallback instead of silent guessing. Input RGB profile conversion
   is implemented. Unmanaged Windows-legacy and real X11 apply the admitted
   display ICC to presented pixels and refresh it when the window changes
   monitor. Wide-gamut preservation and HDR remain later work.

Every other item here is refinement. These are what make the viewer feel correct
in a way users trust without needing to name the implementation.
