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
  letterbox area is the solid theme background, nothing else.
- Persistent chrome never overlaps the image. The image fit rectangle is computed
  from the window minus every visible docked panel. Opening, closing, or resizing
  chrome refits and recenters the photo inside the remaining viewport.
- Top bar: a fixed 40px neutral surface with five conventional menus: File,
  Edit, View, Tools, and Help. The right side shows a stable folder counter and, when space
  permits, the filename, dimensions, and physical zoom percentage, where 100
  percent means one source pixel per physical display pixel. Long names truncate
  with the full value available as a tooltip.
- Tools: hidden by default for a clean image-first surface. View > Panels or `T`
  shows a 64px docked panel containing only high-frequency image operations:
  rotate, flip, crop, and flag. Its vector chevron collapses it to a 44px rail.
  View > Panel Position docks it on either the left or right. Save and destructive
  actions remain in File so the tool surface stays calm.
- Folder Previews: hidden by default. When a folder contains multiple images,
  View > Panels or `G` shows a 112px docked thumbnail strip with current and
  flagged states. Its chevron collapses it to a 44px bottom rail. Thumbnails decode
  only while the panel is visible and expanded.
- Image Information: an optional 304px panel contains file facts, review state,
  and the explicit export-privacy checkbox. View > Panels or `I` toggles it, and
  View > Panel Position independently docks it on the left or right.
- Empty and loading states use an opaque themed card with tested AA text contrast.
  They remain readable on black, gray, white, and theme-driven image backgrounds.
- Crop mode: GPU dims outside the live UV rect to 45 percent brightness. egui draws
  a precise border, rule-of-thirds guides, eight visible pointer handles, exact
  output dimensions, a compact aspect popover, and Apply/Cancel. The popover
  groups Free, Original, 1:1, landscape and portrait photo/video ratios, plus
  numeric custom width and height. A swap control reverses the active ratio.
  Esc cancels; Enter applies.
- Zoom is focal-point anchored (pixel under cursor stays put). Trackpad pixel
  deltas and wheel detents both supported.
- Space held + drag = temporary pan (classic hand tool); Space tap without drag
  resets fit.

## Color

- View > Appearance offers System, Light, Dark, and Console. System follows live
  operating-system changes. Explicit Light and Dark also update native window
  decoration. Console uses a near-black canvas, green phosphor-inspired chrome,
  and monospaced interface type. One validated appearance word is remembered in
  the platform configuration directory.
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
  conversion or fallback status. This is correct for the current SDR working path,
  not a claim of per-monitor output conversion, preserved wide-gamut values, CMYK
  profile handling, or HDR presentation. Those are release roadmap items.

## Typography and icons

- System font stack (`-apple-system`, `Segoe UI`, `system-ui`). Filename ~13.5px
  at weight 550; counter ~12.5px muted, tabular figures.
- Icons: single consistent stroke weight (~1.75px at 24px), rounded joins, drawn
  on a 24px grid. Icon-only buttons with tooltips, except the zoom value.

## Motion and interaction

The current interface uses immediate state changes. It does not claim transition,
inertia, or reduced-motion behavior that has not been implemented and tested.

### Navigation (the most-touched interaction)
- Default is an instant texture swap, no crossfade. Held arrows during culling
  must never fight an animation.
- A prefetched cache hit replaces the texture immediately. On a cache miss,
  reload, or failed replacement, the last good image remains visible while a clear
  loading or error status names the selected path. There is no black/background
  flash, shimmer, slide, crossfade, or edge bounce.
- File > Reload File (`F5`) bypasses the decoded-neighbor cache and reads the
  current path again. It resets in-memory view edits only when the action is safe,
  retains the last good frame during decode, and exposes Retry on failure.

### Zoom and pan
- Wheel zoom is focal-point anchored: the pixel under the cursor stays under the
  cursor. This is non-negotiable and is half of what makes the viewer feel correct.
- Zoom steps are geometric (x1.15 per wheel detent). Trackpad pixel deltas map to
  bounded continuous steps. `0` fits, `1` selects exact physical pixels, and `+`
  or `-` zooms around the image-safe viewport center.
- Pan follows the pointer directly with no inertia or rubber banding. Holding Space
  temporarily selects pan; tapping Space without dragging resets fit.

### Delete and undo
- Delete moves the current file to the operating-system trash, advances the
  playlist deterministically, and shows a three-second non-blocking toast. `U`
  restores the latest successful single or batch trash action.

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
  off the UI thread. Esc cancels crop before it can affect fullscreen state.

### Source animation

- GIF, WebP, and APNG timing is content, not decorative interface motion. Frames
  are bounded in count and bytes, honor container delay and loop behavior, and can
  be paused or resumed from Image Information.
- Navigation and crop deterministically stop or discard playback state tied to
  the old source. Rotation, flips, and pixel edits are applied consistently to
  each displayed frame. A late animation decode cannot replace a newer image.

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
- Repair, undo, and redo update only the bounded changed base-texture region and
  regenerate its dependent mip chain. If the GPU cannot display the complete
  decoded image in one texture, Spot Heal is
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

### About and product identity

- Help > About viewr opens a centered modal that blocks background input and
  closes with its Close button, backdrop click, or Escape.
- It exposes version, platform, license, core shortcuts, and the local-only
  privacy contract. Its modal container has an explicit accessible window name.

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
   is implemented; per-display output, wide-gamut preservation, and HDR remain the
   largest unfinished fidelity milestone.

Every other item here is refinement. These are what make the viewer feel correct
in a way users trust without needing to name the implementation.
