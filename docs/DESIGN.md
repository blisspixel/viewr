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
2. Calm, precise, premium. Nothing moves at rest. Chrome is quiet and auto-hides.
   The interface should feel like a native instrument, not a web app.
3. Simple surface, deep engineering. The restraint on screen sits on top of the
   rigor in `STANDARDS.md`. Minimal is the visible result of the work, not the
   absence of it.

## Layout

- The image is scaled to fit the window (aspect preserved) and centered. The
  letterbox area is the solid theme background, nothing else.
- No chrome overlaps the image except the auto-hiding top bar and the floating
  control pill, both of which fade out when idle.
- Top bar: the rune logo and filename on the left; the counter (`7 / 214`) on the
  far right in tabular figures so it does not jitter as you page. Height ~46px,
  with a soft top scrim no heavier than needed (or a small backdrop-blur chip
  behind the text so bright photos do not wash it out).
- Toolbar: left-aligned floating glass toolbar (36×36 icon buttons) that appears when
  the pointer moves, when it is near the left edge, or while crop mode is active; it
  auto-hides after ~2.8s idle. Classic top menus remain as discovery/accessibility.
- Status chip (bottom-left, low contrast): `name · W × H · i / n · zoom%` when chrome
  is visible. Empty state centers drop/open guidance plus the privacy line.
- Crop mode: GPU dims outside the live UV rect (~45%); egui draws amber handles, a
  top ratio strip (Free/1:1/4:3/16:9), and Apply/Cancel. Esc cancels; Enter applies.
- Zoom is focal-point anchored (pixel under cursor stays put). Trackpad pixel
  deltas and wheel detents both supported.
- Bottom filmstrip: appears only near the bottom edge / when chrome is awake;
  shows a window of neighbor basenames (flagged highlighted); click jumps.

## Color

- Dark (default): background deep ink `#0B0E14`, text `#E8EDF3`, muted `#B8C0CC`
  at rest for icons (reserve full-strength text for hover), hairlines at
  `rgba(255,255,255,.08)`.
- Light: background `#F4F5F7` (not pure white, so bright photos do not blend into
  the frame), text `#1A1D23`, muted `#5A6472`. The rune logo uses `currentColor`
  and inverts for free.
- Accent amber `#F7A845` (dark) / `#C77A12` for amber-as-text on light (to hold
  4.5:1). The accent rule is strict: amber marks the active or affirmative state
  only. It appears on the focus ring, the current tool when armed (for example
  Crop), the live zoom value when zoom is not 100 percent, and the Undo action.
  It never appears as decoration and never on the logo.

## Typography and icons

- System font stack (`-apple-system`, `Segoe UI`, `system-ui`). Filename ~13.5px
  at weight 550; counter ~12.5px muted, tabular figures.
- Icons: single consistent stroke weight (~1.75px at 24px), rounded joins, drawn
  on a 24px grid. Icon-only buttons with tooltips, except the zoom value.

## Motion and interaction

Baseline easing: standard `cubic-bezier(0.2, 0, 0, 1)`, exit `cubic-bezier(0.4, 0,
1, 1)`, spring `cubic-bezier(0.34, 1.4, 0.64, 1)`.

### Navigation (the most-touched interaction)
- Default is an instant texture swap, no crossfade. Held arrows during culling
  must never fight an animation.
- On a discrete, deliberate press, the incoming image does a 12px micro-slide in
  the travel direction over ~110ms, opacity 0.85 to 1. The outgoing image is
  simply replaced. When key-repeat is active or two commits land within ~130ms,
  suppress the slide and swap instantly.
- Cache miss: never blank to background, never flash a spinner. Hold the current
  image, and after a 150ms delay show a thin 2px amber top-edge progress shimmer;
  when the decode lands, crossfade over ~90ms (a real crossfade only because there
  was a genuine wait). This is the one place a crossfade is correct.
- At the first or last image, a further press rubber-bands 12px and springs back
  over ~220ms.

### Zoom and pan
- Wheel zoom is focal-point anchored: the pixel under the cursor stays under the
  cursor. This is non-negotiable and is half of what makes the viewer feel correct.
- Zoom step is geometric (x1.15 per detent), eased to target over ~90ms so a fast
  scroll reads as smooth continuous zoom. Trackpad pinch is continuous.
- Pan has inertia on flick with edge rubber-banding; no inertia for keyboard or
  wheel pan. Directly dragged values (active pan, crop handles) are never eased.
- Space toggles Fit and 100 percent over ~200ms. Free zoom within +/-4 percent of
  100 percent magnet-snaps to exactly 100 percent so pixel-peeping is exact.

### Delete and undo
- Delete drops the current image (translateY +16px, scale 0.98, fade) over ~160ms,
  distinct from horizontal navigation so the two never blur together. The next
  image slides up into place over ~160ms, overlapping the drop by ~40ms.
- Toast enters from below with a spring over ~180ms and dwells 5000ms; the dwell
  resets if the pointer enters the toast. Consecutive deletes stack into one toast
  with a count ("3 moved to Trash") rather than spawning many.
- Undo (click or Ctrl+Z) plays the delete in reverse: the image slides back down
  into place. The reversal reading is what makes undo feel trustworthy.

### Crop
- Enter (C) or select from the left-aligned toolbar dims outside a default rect (scrim to 0.55 over ~160ms), fades in eight
  handles plus a rule-of-thirds grid (grid delayed ~60ms). The toolbar provides
  aspect ratio options: Free, 1:1, 4:3, 16:9, and an Apply crop button.
- Handles have a >=24px hit target even at ~10px visual size; the excluded scrim
  updates live at 1:1 during drag. Shift constrains aspect with magnetic snap to
  common ratios. Confirming (Apply) applies crops directly, zooming the kept region up to fill over ~240ms so the result
  becomes the viewed image. Esc cancels (crop first, then fullscreen; one Esc never
  does two things).

### Micro-interactions
- Button hover: background fade over ~120ms; press: scale 0.94 down ~90ms, spring
  back ~160ms. Focus: a 2px amber ring at ~60 percent alpha, 2px offset, appearing
  instantly with `:focus-visible` semantics (focus rings never animate in).
- Chrome reveal (mouse move >2px, or Tab, or key with UI): opacity plus 6px drift
  over ~140ms, top bar and pill as one layer (no stagger). Hide (mouse idle
  2500ms, or 400ms after keyboard nav): opacity plus inverse drift over ~380ms.
  The OS cursor hides with the chrome and returns on first move. Cursor never hides
  during zoom, pan, or crop. Never animate backdrop blur or box-shadow.

### Anti-patterns (do not)
Crossfade on every navigation; spinners on prefetched images; a black or
background flash between images; easing on directly-dragged values; staggered or
bouncy chrome entrances; a confirmation modal for delete; any idle motion on the
photo (parallax, Ken Burns, bounce); over-hiding chrome so it is hard to summon.

### Reduced motion
Respect the OS `prefers-reduced-motion`. Replace positional and scale motion with
a plain opacity fade (~100ms) or an instant state change; remove navigation slide,
delete drop, crop zoom, press bounce, and rubber-band. Keep the feedback: a deleted
image still disappears and the toast still appears. Keep focus rings and hover
color changes. Accessibility means calmer motion, not missing state.

## Accessibility

- Every interactive element has a visible `:focus-visible` amber ring and a
  keyboard path. Keyboard-first is the primary mode.
- Hit targets are at least 36px; icon resting color holds AA contrast on the
  background; there is no permanent low-contrast tutorial text (the mockup's
  helper line does not ship).
- Tooltips carry the keyboard shortcut so power users learn the keys.

## The two invariants that define "exceptional"

1. Focal-point-anchored wheel zoom: the pixel under the cursor stays locked under
   the cursor.
2. Instant navigation that holds the old texture until the new one is ready, so
   there is never a black frame between images.

Every other item here is refinement. These two are what make the viewer feel
correct in a way users trust without being able to name.
