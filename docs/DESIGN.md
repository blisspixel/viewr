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
- Top bar: a fixed 40px neutral surface with three conventional menus: File,
  Edit, and View. The right side shows a stable folder counter and, when space
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
- Empty and loading states use an opaque dark card with tested AA text contrast.
  They remain readable on black, gray, white, and system-driven image backgrounds.
- Crop mode: GPU dims outside the live UV rect to 45 percent brightness. egui draws
  a precise border, exact pixel dimensions, a top ratio strip
  (Free/1:1/4:3/16:9), and Apply/Cancel. It does not draw resize handles because
  pointer-handle dragging is not implemented. Esc cancels; Enter applies.
- Zoom is focal-point anchored (pixel under cursor stays put). Trackpad pixel
  deltas and wheel detents both supported.
- Space held + drag = temporary pan (classic hand tool); Space tap without drag
  resets fit.

## Color

- The image background follows the operating-system theme by default. Dark uses
  deep ink `#0B0E14`; light uses `#F4F5F7` rather than pure white so bright photos
  retain an edge. View also offers explicit black, neutral-gray, and white
  backgrounds for inspection.
- Persistent chrome remains neutral dark on every image background: panel
  `#0F131A`, raised surface `#1A202A`, text `#E8EDF3`, and muted text `#B8C0CC`.
  This prevents the readability of controls or guidance from depending on the
  selected image background.
- Accent amber `#F7A845`. The accent rule is strict: amber marks the active or
  affirmative state
  only. It appears on the focus ring, the current tool when armed (for example
  Crop), the live zoom value when zoom is not 100 percent, and the Undo action.
  It never appears as decoration and never on the logo.

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
- A prefetched cache hit replaces the texture immediately. A cache miss shows the
  same high-contrast loading surface used at startup until the requested image is
  ready. There is no unimplemented shimmer, slide, crossfade, or edge bounce.

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
  dims outside a centered default rectangle. The crop ratio strip provides Free,
  1:1, 4:3, 16:9, Apply, and Cancel.
- Crop begins with a usable centered selection, so it never requires a pointer.
  Arrow keys move the selection; Shift plus an arrow resizes it; holding Ctrl uses
  a fine adjustment step; Enter applies; Esc cancels. Locked aspect ratios remain
  locked during keyboard resizing.
- Pointer drag redraws the selection; keyboard movement and resizing edit the
  existing selection. Exact source-pixel bounds are visible and published to the
  accessibility tree. Confirming applies the crop directly. Esc cancels crop
  before it can affect fullscreen state.

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
the image; disappearing controls that are difficult to summon; ASCII arrows used
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
  creation and io_uring. Manual screen-reader acceptance remains required on all
  three targets.
- Tooltips carry the keyboard shortcut so power users learn the keys. Automated
  tests enforce at least a 4.5:1 contrast ratio for normal text, muted text,
  accent controls, and primary-button text on their actual surfaces.

## The two invariants that define "exceptional"

1. Focal-point-anchored wheel zoom: the pixel under the cursor stays locked under
   the cursor.
2. Instant navigation that holds the old texture until the new one is ready, so
   there is never a black frame between images.

Every other item here is refinement. These two are what make the viewer feel
correct in a way users trust without being able to name.
