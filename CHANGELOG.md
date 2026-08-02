# Changelog

All notable changes to this project are documented here. The format is human-written
and organized by user-visible concern.

## Unreleased

### Security

- Filmstrip labels, status filenames, Image Information basenames, and thumbnail
  texture names now use the same privacy-safe filename helper. Full directory
  paths no longer enter display chrome or in-process texture debug names.

### Changed

- The third-party license inventory is plain text (`THIRD_PARTY_LICENSES.txt`)
  instead of HTML. CI still regenerates it offline from the locked release graph
  and compares the inventory by meaning. No application runtime change.
- Save As lifecycle policy now lives in a 100 percent line-covered private seam.
  Start blockers, folder-scan save gates, terminal close disposition, and app
  close wait coordination are exhaustive pure transitions. The application
  remains the sole destination, worker, image, dialog, and close-application
  owner. User-visible behavior is unchanged.
- Rating lifecycle policy now lives in a 100 percent line-covered private seam.
  Presented state, indeterminate-recovery handling, filter discovery scheduling,
  terminal writer reconciliation, and deferred-close decisions are exhaustive
  pure transitions. The application remains the sole accepted-source, path,
  worker, disclosure, playlist, visible-recovery, UI-dispatch, and close owner.
  User-visible behavior is unchanged.
- Curation lifecycle policy now lives in a 100 percent line-covered private seam.
  Trash, permanent-delete, and restore recovery priority, fixed guidance,
  operation status, count grammar, and deferred-close decisions are exhaustive
  pure transitions, while the application remains the sole worker, path, receipt,
  playlist, and visible-recovery owner. User-visible behavior is unchanged.
- CI now runs the exact locked, all-target formatting, Clippy, test, and build
  contract documented for contributors, with documentation tests kept as a
  separate explicit gate.
- Windows native accessibility automation now closes an unsuccessfully expanded
  View submenu before retrying it, so each attempt starts from a known menu state
  while retaining the exact accessible-child assertions. If UI Automation Invoke
  leaves the submenu unpublished, the retry focuses that exact accessible item
  and uses the conventional Right Arrow expansion before checking the child
  again. Focused shortcut checks prefer `SendInput` and use a native queued-key
  fallback only when the runner denies foreground ownership; every key remains
  verified through the resulting accessible application state. UI Automation
  focus on the live application root or a submenu node is a best-effort hint when
  Windows rejects `SetFocus`; stale elements and unexpected failures still abort
  the attempt.
- Dependency policy now denies every unreviewed duplicate crate version against
  an exact, rationale-backed transitive baseline. Both workspace lockfiles must
  pass cargo-deny without unexplained warnings.
- Current-image details, animation discovery, and rating observation now return
  through a tested single-result owner with a one-item channel. Replaced work
  cannot publish, exact path and generation identity gate every result, and a
  closed completion endpoint wakes the event loop, clears busy state, and exposes
  explicit restart guidance instead of failing silently or promising that an
  unsupervised executor can recover in place.
- Save As now uses the same bounded, consuming completion contract instead of an
  unbounded result channel. Closing viewr waits for the captured output
  transaction to finish successfully, including the defensive case where Trash
  reconciliation is also active. Failure keeps the window open so its result is
  visible, while a lost completion clears busy state, persistently blocks another
  export, and requires restart. Rating replacement and Save As cannot overlap.
  Save completion changes only status for its captured snapshot and cannot mutate
  a newer foreground image.
- Full-resolution crop now uses the bounded, consuming completion owner instead
  of an unbounded result channel. Source replacement signals cooperative
  cancellation and drops ownership so late pixels cannot publish. Exact
  generation, selected path, presented path, and decoded-allocation checks still
  gate recovery and presentation. A normal computation failure restores the
  selection for direct retry, while unexpected completion loss clears busy state,
  keeps Cancel Crop available, persistently blocks another crop, and requires a
  restart without claiming in-process thread recovery.
- Over-limit display preview preparation now uses the bounded, consuming
  completion owner instead of an unbounded result channel. The event loop retains
  exact path, generation, presentation kind, source, and crop recovery context;
  replacing the owner rejects late publication. Typed preparation failures remain
  retryable. The presentation queue closes and drops pending work if its worker
  exits, so replacing an owner cannot hide executor loss or strand the next job.
  Unexpected executor loss clears busy state, restores a current crop selection
  when possible, disables misleading retry only for the affected load, leaves
  later ordinary decode failures retryable, and does not globally disable
  otherwise-ready viewing or export actions. Work that needs the lost preview
  executor remains blocked until restart.
- Filmstrip thumbnails now use a tested event-loop-owned schedule capped at nine
  active jobs instead of an unbounded shared result channel. Exact generation and
  visible-window checks reject stale uploads, while thumbnail workers return only
  validated sRGB pixels or path-free failure categories. Executor saturation
  remains retryable without a redraw loop. A failed or disconnected visible path
  keeps its placeholder without being decoded repeatedly, then becomes eligible
  after leaving the visible window or after an explicit generation reset.
- Neighbor prefetch now gives each accepted speculative decode one bounded,
  event-loop-owned result endpoint instead of sharing an unbounded result channel.
  At most four owners exist across current and cancelled generations. Exact owner
  context rejects stale pixels, a successful foreground open cancels and wins over
  same-path work, and accepted completion loss is observable. Queue rejection
  owns nothing and remains retryable without a wake loop. Worker diagnostics use
  stable path-free categories while terminal suppression and the five-entry,
  256 MiB decoded cache policy remain unchanged.
- Folder scanning now has one consuming completion owner, cooperative replacement
  cancellation during enumeration and natural sorting, an explicit 100,000-image
  ceiling, and a 64 MiB cumulative encoded-path budget. Oversized folders fail
  visibly without installing a truncated playlist; opening one selected image
  still succeeds as a one-item playlist. A lost worker endpoint clears busy state
  and follows a fixed recovery path. Enumeration and nonblocking child opens are
  bound to one retained directory handle, and automatic entries carry exact
  identity-plus-version provenance through foreground decode, prefetch,
  thumbnails, ratings, Trash restore, and explicit F5 refresh. Decode and rating
  results are discarded if their accepted object changes before publication.
  Markable accepted sources also retain their selected pathname in memory and
  reopen it without following its final component during version checks. This
  catches pathname replacement even when a Windows retained handle reports
  unchanged cached timestamps. Windows accepted sources also keep a SHA-256
  content witness only in memory, so an in-place rewrite is rejected even when
  its length and writable timestamp fields are restored. Source acceptance
  rejects encoded files above the shared 512 MiB limit before hashing, and
  superseded decode work stops between fixed 64 KiB witness chunks. Folder
  rating discovery uses a separate read-only native-version seam, reads at most
  the 16 MiB JPEG header twice, compares the exact bytes consumed as well as the
  parsed result, stops between segments, and cannot authorize writes. Two
  different malformed or non-JPEG headers can no longer appear stable merely
  because parsing returned the same error category.
- The restored-provenance regression now changes an explicit modification-time
  field instead of assuming every filesystem reports a version change for an
  immediate rename round trip.
- Full Windows source-content comparisons now run only in owned background work.
  Windows Open With uses one generation-cancellable verification job before the
  native chooser. Trash and permanent delete use the existing single curation
  worker for final strong validation and the platform operation, while restored
  rating and provenance evidence is prepared on that same worker. The event loop
  remains the only playlist and Undo owner. Conflicting mutations wait, ordinary
  navigation cancels Open With verification and every strong content comparison
  in the replace-latest animation, details, and rating task. Indeterminate Trash
  or permanent delete explicitly requires closing and reopening viewr, and a
  failed or partial curation result cancels deferred close so the result remains
  visible.
- Save As now binds metadata retention to serialized reads from the accepted
  source handle that supplied the displayed pixels and rejects length,
  modification-time, or operating-system change-time differences around
  extraction. It retains the canonical parent identity plus destination absence
  or an exact native identity and version after the native dialog. The event-loop
  destination capability exposes no reader, performs no content hash, and does
  not inherit the encoded-source size ceiling; the worker retains every
  source-byte and output-transaction check.
  Every captured existing file now receives an app-owned overwrite prompt whose
  consent is revalidated against that exact object. Its copy accurately describes
  the final identity recheck without overstating the documented pathname race.
  The prompt exclusively owns
  pointer, keyboard, and assistive-technology actions. An operating-system open,
  dropped file, completed explicit folder open, or rating-filter result that
  replaces or clears the source cancels it before changing source. Save As cannot
  begin while an explicit folder open is still being scanned. Save As then rejects
  a changed parent, created destination, replacement, source alias, or in-place
  rewrite before commit. Handle identity
  comparison also covers a
  directly selected symlink target independently of mutation eligibility. Pixel
  encoding and EXIF insertion use the retained temporary handle, whose pathname
  identity is checked before commit. An absent destination uses a no-clobber
  install so a file created after confirmation is preserved. Default metadata
  stripping remains intact.
- SVG rendering now rejects embedded raster resources and external image hrefs
  explicitly, preventing selected markup from reading another local image or
  bypassing its input budget. Gzip-compressed SVG and document type declarations
  are rejected before parser expansion. Markup depth, element count, cumulative
  attribute count and bytes, and live opacity-layer memory are bounded before
  raster allocation. Marker and `use`
  expansion are disabled, while cumulative path geometry has byte and token
  ceilings. Stylesheets, inline styles, text references, gradients, strokes, and features
  with unbounded renderer scratch space are rejected explicitly. Parsed nodes,
  path segments, tiled edge work, and cumulative painted area are also bounded.
  Solid-filled vector shapes, bounded paths, and bounded opacity groups remain
  supported.
- The reviewed JPEG XL patches now require exact zero-reserved 12-byte CICP tags,
  round-trip PQ and HLG CICP transfer evidence,
  require an executable AVX2-plus-FMA dispatch predicate before AVX2 color
  conversion, preserve the first parallel transform error, and publish a terminal
  render state after composition failure. Previously
  vacuous PQ vector tests now exercise the optimized upstream paths against
  explicit accuracy bounds, and both patched vendor crates are workspace-tested.
- Python CI now lints and format-checks the complete `scripts/` tree so new owned
  tooling cannot silently fall outside the gate.
- Manual release workflow runs are inspection-only at the executable job gate;
  only a tag push can receive the write-enabled publication path.
- Dock layout and menu presentation now come from one covered, immutable chrome
  model rather than paint-time policy spread across the event loop and UI. One
  captured dock snapshot drives both image-safe viewport insets and visible
  panels. Shared tool controls now publish consistent readiness, selected state,
  labels, shortcuts, and accessibility semantics across menus, context actions,
  and docks, while active Spot Heal remains closable during conflicting work or
  after opening an image that exceeds the GPU edit limit.
- The egui and AccessKit UI adapter is now included in the enforced 85 percent
  logic-coverage floor. The remaining whole-file exclusions are exact-path scoped,
  so this filter cannot also remove similarly named integration tests or vendored
  source. Mixed native and pure helpers in those files remain explicit v0.2
  extraction debt rather than being described as untestable platform glue.
- GPU image sizing, mip planning, linear-light alpha-correct preview preparation,
  and upload selection now live in a covered CPU-only policy module. The wgpu
  adapter retains device and texture ownership and consumes the same validated
  decisions, so preview behavior and memory limits are unchanged while malformed,
  stale, wrong-color, and canceled inputs receive direct regression coverage.
- GPU surface-format choice, Spot Heal patch upload geometry, placement-buffer
  packing, and clear-color mapping now live in a second private covered policy
  module. Exact tests cover first-supported sRGB selection, every placement field,
  full-resolution and bounds requirements, integer overflow, byte-length mismatch,
  and all clear-color channels without creating a device or texture.
- Selected-versus-presented navigation policy now lives in a private covered
  module instead of the event-loop adapter. Exact tests cover pristine-pixel reuse,
  the near-navigation retention boundary, edited-image rejection, opening-state
  classification, durable loaded-image errors, and exact preview generation/path
  identity while `App` retains all path, cache, session, and job ownership.
- The Windows external-edit reminder is now owned by the last-good frame. A
  failed `F5` refresh or navigation keeps both that frame and its path-private
  reminder instead of implying that the externally handed source was refreshed.
  Fresh accepted pixels or explicit frame invalidation clears both ownership and
  the reminder.

### Documentation

- Clarified the README's "No tracking" promise as a literal product boundary:
  viewr rejects spyware and bloatware patterns, has no collection or upload
  machinery, initializes no logger during normal runs, and writes no log file.
  Explicit developer console diagnostics remain local, path-private, and opt-in.
- Corrected the installation guide to identify v0.1.0 as the public, immutable,
  checksummed, attested, and explicitly unsigned preview, and added a repository
  test that rejects stale prerelease claims across canonical release surfaces.
- Recorded the immutable v0.1.0 release, exact hosted CI and fuzz evidence,
  verified 12-asset publication, and the remaining platform-trust gates in the
  roadmap and maintainer publishing guide.
- Replaced the vague jump from v0.1.0 to 1.0 with dependency-ordered v0.2 through
  v0.9 release gates for reliability architecture, display correctness, file
  coherence, format contracts, integrated product proof, accessibility evidence,
  release readiness, and final publisher-authenticated candidate validation.
  Signing and notarization now occur only after the product scope is stable;
  version numbers represent evidence gates rather than percent complete.

## 0.1.0 - 2026-07-31

### Changed

- Reframed the project overview around its fast, focused, cross-platform, and
  local-only product promise; added a concise feature tour, essential controls,
  appearance choices, preview limitations, and a direct documentation path.
- Added reviewed release notes and durable repository instructions so product,
  quality, privacy, documentation, and release boundaries stay explicit as the
  project evolves.
- Public documentation now distinguishes the source build available today, the
  first unsigned pre-1.0 release, and the stronger 1.0 trust bar. The roadmap opens
  with a concise release dashboard, puts trustworthy distribution before new
  product work, links the recorded green hosted evidence, and keeps RAW, HDR,
  advanced healing, clipboard, and touch work from obscuring actual release gates.
- The public repository now enables private vulnerability reporting, Dependabot
  alerts and security updates, secret scanning, push protection, focused discovery
  topics, automatic merged-branch deletion, and immutable releases. Protected
  `main` now requires the seven always-running CI checks, linear history, code-owner
  review, last-push approval, and conversation resolution while blocking force
  pushes and deletion; administrators retain explicit emergency bypass.
- Release packaging now accepts only the explicitly listed, bounded PNG interface
  capture as binary documentation and validates its chunk structure before adding
  it to an archive. The Windows installer smoke also parses the staged version
  without relying on ambient PowerShell match state and now runs at the declared
  Windows PowerShell 5.1 compatibility floor in CI.
- The README now includes a privacy-safe capture of the real desktop interface
  using a clear example image, with dimensions, rating, zoom, and folder-position
  status visible.
- Third-party license drift checks now compare the locked package versions, license
  assignments, and license text semantically. Harmless upstream repository-link,
  section-order, and line-ending differences no longer break cross-platform CI.
  Two dependencies that package the same MIT file now use checksum-backed
  clarifications so every host selects those exact license bytes.
- The newest libheif compatibility check now verifies its observed output profile
  when one is available and the decoder contract's explicit sRGB fallback when the
  converted output is untagged.
- CI uses the current checksum-pinned tool installer release. Dependabot now limits
  each update lane to one grouped patch proposal and suppresses routine breaking
  version proposals, keeping GPU, accessibility, color, and platform transitions
  in deliberate compatibility work while security updates remain enabled.
- Reworked the public project surface around a concise README and a linked
  documentation index. Added focused contribution guidance, structured issue and
  pull-request templates, CODEOWNERS, Dependabot configuration, a public-repository
  checklist, and explicit current-stage claims for the planned canonical GitHub
  home.
- Added user-local one-command installers for Windows and macOS/Linux. They resolve
  only official GitHub Releases, verify the SHA-256 sidecar and bounded archive
  structure before installation, refuse to replace unowned paths, and create no
  updater service. Running the same command performs an explicit update.
- Release tags now assemble a verified four-target archive set plus fixed-version
  installer scripts and checksum sidecars, attest every published asset, use
  reviewed human release notes, and publish only after CI and fuzzing pass. Manual
  workflow runs remain non-publishing inspection builds.
- Restored the verbatim Apache License 2.0 text, added the project NOTICE, and made a
  generated third-party license inventory part of the release archive and CI drift
  checks.
- Help > Get latest release now offers one clear click to the latest official stable
  release while retaining the no-automatic-check contract. Terminal installer
  commands remain in `viewr update` and the install guide instead of occupying the
  graphical Help surface.
- The Windows performance probe now starts its strict 500 ms settled-idle window
  only after egui has no delayed hover or activation repaint outstanding. The
  two-redraw budget is unchanged; a UI that keeps scheduling frames still reaches
  the existing hard timeout. A reproduced 11-redraw activation outlier now yields
  a quiet measured interval, and the optimized seven-process gate records at most
  one redraw per completed idle window.
- The native Windows accessibility smoke now waits for the exact initial toggle
  state of Tools and Image Information before activating either panel. A transient
  accessibility-tree refresh can no longer produce an unhelpful null-parameter
  failure, while a missing or wrongly selected control still reaches the existing
  bounded diagnostic timeout.
- Windows JPEGs now support disclosed 0-to-5 ratings through standard embedded
  `xmp:Rating`; a valid existing `0x4746` SimpleRating value is kept in agreement.
  Bare `1` through `5` assign and `0` clears in normal viewing mode. Edit exposes
  the same radio choices, View filters the current folder to a minimum rating, and
  navigation, previews, counters, Trash, Undo, and restart all preserve canonical
  folder ownership. The bounded parser and failure-atomic writer reject ambiguous
  or stale sources, preserve unrelated bytes, create no catalog, sidecar, alternate
  stream, metadata timestamp, or activity history, and fail closed with persistent
  recovery guidance if replacement cannot be proven settled. Other formats and
  platforms remain visibly read-only.
- Image Information now reads metadata through the retained source handle used for
  the displayed pixels and reports a bounded Source Privacy summary. It counts
  supported EXIF tags and identifies location, ownership, unique identifiers,
  comments, software history, embedded thumbnails, and maker-specific data by
  presence only. Raw sensitive values remain hidden, and the panel explicitly says
  that no supported EXIF is not proof that other metadata or hidden pixel data is
  absent.
- View > Panels now displays `T`, `G`, and `I` as right-aligned shortcuts and
  includes the shortcut text in accessible menu names while preserving selected and
  disabled state.
- Windows now offers Open With... in File and the image right-click surface. It
  uses the native single-file chooser with the current accepted source, never a
  shell command, and does not remember an editor. The handoff explains that the
  external app receives the original file and metadata, excludes unsaved viewr
  edits, and may modify the source. A persistent path-free status directs `F5`
  reload after a successful handoff; cancellation and launch failure remain
  distinct outcomes. macOS and Linux chooser parity remains roadmap work.
- Opening the first image no longer resizes the application window around that
  image. The window keeps its current dimensions while the image fits inside the
  available viewport; window dragging and explicit View zoom or fit actions remain
  under the user's control.
- Removed the session-only mark, review, and batch-trash workflow. Bare `B` and
  `M` are unassigned, normal-mode `X` is unassigned, and `X` remains only the Crop
  ratio-orientation shortcut while Crop is active. `Delete` and File > Move to
  Trash now form the sole recoverable destructive path for the visible image;
  `U` restores its latest exact receipt. Obsolete top-bar, File, Tools, thumbnail,
  state, diagnostics, automation, tests, and documentation were removed together.
- The unchanged first-run card now stays fixed instead of drifting vertically,
  and filename, dimensions, and zoom use distinct reading gaps in the top status.
  Help now includes an accessible explicit Update viewr modal with the running
  version, trusted-channel guidance, and one explicit Get latest release action.
  The CLI retains detailed install and source-build guidance without assuming
  `git pull`; neither surface checks a network or claims the running build is
  latest, and only the explicit graphical action opens a browser.
- The explicit performance probe now preserves path-free per-run idle evidence:
  delivered redraws, non-redraw window events, event-driven and scheduled egui
  repaint requests, final focus, and pointer-inside state. The harness prints it
  on request and automatically when completed reports violate a gate, while normal
  launches remain silent. No repaint scheduling or two-redraw budget changed.
- Default-silent logging now has direct behavior-level proof: absent logging
  variables and unsupported external-only directives construct no logger, while
  `RUST_LOG` keeps precedence over `VIEWR_LOG`. Privacy wrappers retain narrow
  orchestration and ephemeral regression tripwires without claiming to prove every
  possible future Rust file write.
- README now states the current pre-1.0 distribution boundary beside the product
  introduction and links directly to the supported local install and verification
  paths. Archive guidance now distinguishes structure and byte checks against
  co-produced records from publisher authentication and independent source
  provenance. No public release or platform signature is claimed; exact hosted CI
  and fuzz evidence is linked separately.
- File > Undo Trash now exposes settled availability without retaining or showing
  a history count. Its path-free help explains that the latest recoverable action
  may belong to another folder. If a restore worker stops without a result,
  a new Trash move waits for `U` to reconcile the retained receipt; a newer action
  can no longer replace uncertain recovery ownership or falsely clear guidance.
- Appearance save failures now keep the chosen theme for the current session,
  show fixed recovery guidance, and expose only the failed persistence phase to
  opt-in diagnostics. Raw storage errors and configuration paths cannot enter
  the interface or diagnostic record.
- Appearance startup now keeps a normal missing preference quiet while explaining
  abnormal fallback with `Could not restore saved appearance. Using System.`
  Invalid, oversized, unreadable, and unavailable state remain bounded, use fixed
  path-free diagnostic categories, and are never rewritten without an explicit
  appearance choice.
- View > Appearance now describes System, Light, Dark, and Console before
  selection, including Console's near-black, phosphor-green, monospaced
  green-screen treatment. The chooser reports System's effective mode while
  active and clarifies that themes change app chrome and the default canvas, not
  image pixels or an explicit Image Background override. The complete descriptions
  are native accessible radio names, Windows automation selects all four, and the
  parent View entry now summarizes the current preference.
- The first-run surface now explains that Open File browses its containing folder
  when access allows, while Open Folder selects that folder explicitly for the
  session. Both actions add concise pointer help, the explanation is present in
  the native accessibility tree, and README and privacy guidance now disclose
  nearby sibling prefetch. The vague “Maximum privacy” slogan is gone.
- Source pixels now cross one validated, cancellable normalization boundary into
  an explicit RGBA8 sRGB working encoding. Still, JPEG XL, animated, and isolated
  worker decodes check supersession between ICC rows. Crop and pixel transforms
  preserve the encoding; preview generation, thumbnails, export, and renderer
  upload reject incompatible encodings before applying sRGB math or touching an
  output destination. The renderer owns a matching output transform and refuses
  non-sRGB presentation surfaces instead of silently changing the transfer
  contract. Failed or superseded transforms cannot expose partially converted
  pixels.
- Extracted playlist data, GUI performance-probe state, crop/output geometry, and
  selected/loading/presented session state from the application orchestrator.
  Session and performance transitions now have direct unit coverage.
- Page Up and Page Down now navigate one image at a time. Undo Trash is disabled
  when no recoverable receipt exists, and its keyboard shortcut reports that state
  instead of failing silently.
- Trash Undo now binds each receipt to the exact playlist that created it.
  Restoring after a folder change no longer inserts prior-folder files into the
  current view, and the result explains when the source folder needs a refresh.
- Trash Undo now owns one latest safely recoverable action. Windows and Linux
  retain a new system Trash item identifier only when its native file identity
  matches the live accepted-source handle, macOS keeps the exact resulting URL
  with that handle, and restore never substitutes an older item with the same
  original pathname.
  Receiptless successful moves direct recovery to system
  Trash without erasing a prior valid `U` action. Permanent delete also preserves
  that action and its success message makes clear that `U` applies only to the
  earlier Trash operation. Restore failures retain `U` only for transient or
  resolvable conditions; manual-review and terminal outcomes no longer advertise
  a false retry.
- Opt-in curation diagnostics now distinguish baseline Trash listing failure,
  final listing failure, no new candidate, ambiguous candidates, and retained
  source-identity mismatch using fixed categories. Undo reports its total native
  restore duration. These stderr-only records contain counts and elapsed milliseconds,
  never paths, filenames, receipt identifiers, native identities, or raw platform
  errors. User-facing recovery copy and behavior are unchanged.
- Trash restore now runs through one typed native worker instead of blocking
  window repaint. The top bar exposes a polite operation state, conflicting open,
  navigation, edit, and destructive actions wait, and normal close finishes
  reconciliation before exit. Playlist scope and Undo receipts still commit once
  on the event loop. Spawn failure leaves state unchanged; unexpected worker loss
  keeps the receipt and directs system Trash review without claiming success. A
  terminal wake runs even when the worker unwinds. The active state deliberately
  offers no false percentage, estimate, or cancellation control.
- Corrected privacy and build-verification documentation so native dialog history,
  operating-system paging, and cross-environment linker limits are explicit.

### Security

- Updated runtime-reachable `event-listener` to 5.4.2, which fixes
  `RUSTSEC-2026-0221` unsafe cross-thread access through `StackSlot`. Root and fuzz
  audits now fail on every unreviewed warning instead of accepting newly reported
  unsoundness in successful CI output.
- JPEG XL ICC parsing now rejects overflowing tag counts and ranges with a normal
  decode error instead of allowing crafted profile arithmetic to panic the viewer.
- The optional decode helper must be an explicit valid file or the exact colocated
  release binary. Missing or invalid helpers now fail closed and can no longer
  execute a same-named program from `PATH`.
- Public one-command installation now bootstraps from immutable `v0.1.0` release
  assets. The release workflow publishes checksum sidecars for those scripts and
  creates provenance attestations for the complete exact asset set.

- Deterministic release archives now include `SECURITY.md` and the complete
  canonical Markdown documentation set. Verification also rejects unresolved
  local README links written in the repository's simple inline Markdown form with
  repository-relative destinations, so the current package cannot omit its
  advertised privacy, recovery, architecture, accessibility, or disclosure
  guidance.
- Added a security policy with supported-version scope, privacy-safe synthetic
  reproduction guidance, and explicit decode, file-mutation, privacy, sandbox,
  packaging, dependency, and build-provenance boundaries. It records that no
  verified private channel is operational yet and prohibits publishing technical
  details while that release prerequisite remains open.
- Corrected the reviewed `quick-xml` advisory exception to cover both real
  dependency paths. Wayland XML remains fixed build-time input; little_exif uses
  a plain reader and viewr calls its XMP-rewriting write path only on a freshly
  encoded private temporary with no text or XMP chunks. Untrusted source metadata
  crosses the existing bounded TIFF parser into typed tags and never reaches the
  vulnerable XML paths. No compatible patched transitive release exists yet.
- Single Trash and permanent delete now bind destructive intent to the retained
  file handle that supplied accepted pixels. Delete rejects a changed, missing,
  linked, or unverifiable entry before Trash. Shift+Delete verifies the source
  before opening confirmation and repeats the check after acceptance immediately
  before removal, so a confirmation-window replacement remains untouched. Fixed
  path-free categories distinguish identity rejection from platform failure. The
  final pathname operation remains documented as a narrow non-atomic boundary.
- Exact Trash receipts remain in memory only and are neither logged nor
  persisted. Windows and Linux snapshot existing Trash identifiers before the
  move, accept exactly one new same-origin identifier afterward only when its
  native file identity matches the retained source, and repeat that identity
  check before restore. The live handle prevents identifier reuse. In-app restore
  never falls back to matching only the original pathname. The later platform
  identifier resolution remains documented as a narrow non-atomic boundary.
- Opt-in logging now enables only viewr-owned targets. Bare levels and
  `viewr=<level>` remain supported, while dependency directives are ignored so
  path-bearing external warnings cannot cross the documented privacy boundary.
  The boundary accepts only `viewr` or a `viewr::` descendant and rejects prefix
  lookalikes.
- Full-resolution crop work now checks cooperative cancellation before
  allocation and between copied rows. Navigation cannot accumulate obsolete
  crop copies, same-path Reload cannot accept an old-generation result, and
  failed image loads cannot expose last-good pixels to keyboard crop commands.
  Non-finite crop coordinates are rejected instead of becoming unintended
  minimum geometry.
- Permanent-delete confirmation now uses the same bounded, path-free,
  control-safe, bidi-safe, and quote-safe filename contract as loading status.
  Its affirmative button is labeled Delete permanently. Trash and restore errors
  map external platform payloads to fixed actionable categories, so debug
  descriptions, directories, and hostile filenames cannot reach failure copy.
- Windows executables now embed the Common Controls v6 activation manifest
  required by the native custom-button warning dialog, preventing a loader
  failure while keeping the application at normal user privilege.
- Automatic sibling scans now admit regular files only instead of following a
  supported-image symlink to a target outside the selected directory. A directly
  selected symlink remains openable as the user's explicit one-file selection.
- Replaced reachable indexing and conversion assumptions at protocol and
  user-input boundaries with checked access and explicit errors. The existing
  hostile-input rule remains scoped to paths reachable from files and user input;
  test assertions and documented internal invariants may still use unwrap or
  expect.

### Fixed

- Private temporary workspaces now use atomic ownership and remove only the exact
  directory created by the current process. Startup no longer sweeps broadly
  named directories that could belong to another process.
- macOS decode workers no longer apply unsupported address-space or per-user
  process limits, which could reject startup on a normal logged-in system. The
  worker retains its private session, bounded decode protocol, hard deadline,
  and inherited App Sandbox boundary. Cross-platform CI now also
  normalizes upstream license-file line endings, tracks the complete Flatpak
  Cargo source set, and waits for observable rating state changes in the native
  Windows accessibility smoke path.
- Malformed JPEG XL input with an unused out-of-range LF-frame level now returns
  through the bounded decode path instead of triggering an index panic. The
  exact hosted fuzz discovery is retained in the permanent corpus and replayed
  by the normal integration suite.
- Offline Flatpak builds now resolve registry archives from their generated
  `cargo/vendor` directory without colliding with reviewed local dependency
  patches. Latest-libheif compatibility checks also compare the worker with the
  decoder's observed output profile instead of inferring behavior from a
  library version number.
- The top status now contracts further at the 640-pixel minimum width so File,
  Edit, View, Tools, Help, and playlist position remain separate. Pointer actions
  for Trash and Undo share the same Crop, Spot Heal, folder-scan, save, load, and
  restore blockers as their keyboard paths instead of appearing available before
  returning guidance.
- Crop now keeps its exact selection, view transform, paused animation state,
  source generation, and decoded-image identity through computation, preview,
  and renderer presentation. Any current-source failure leaves original pixels
  unchanged, restores the selection, and names the Enter-key retry. Preview
  channel disconnection clears busy ownership instead of hanging indefinitely,
  and very small selections retain exact AccessKit bounds without claiming an
  unusable inner drag target.
- Spot Heal, Undo, and Redo now commit decoded pixels, bounded history, renderer
  state, and success copy as one transaction. A failed patch or full-texture
  presentation restores exact CPU pixels and history; an internal rollback
  failure starts a source reload instead of leaving export state different from
  the canvas. Destructive shortcuts and Trash restore wait for foreground reload,
  preview, crop, Save As, active Spot Heal strokes, and active heal workers while
  explaining the owning work. Restore copy identifies receipts retained for retry
  without exposing paths.
- Genuine cache-miss and first-load status now names the selected target by a
  bounded, control-safe filename while the visible filename, dimensions, and zoom
  continue to describe the last presented pixels. Failure and Retry use the same
  target identity, cropped-image preview preparation is no longer mislabeled as a
  file open, and immediate reverse reuse or full-resolution cache hits remain free
  of loading status. Source-load preview queue, preparation, and GPU upload
  failures now remain durable and retryable. Persistent loading, failure, and
  preview-preparation statuses use polite AccessKit live-region semantics, while
  transient toasts retain their prior semantic, non-live behavior. Target copy is
  capped at the minimum window width without hiding the bounded full text when
  elided.
- Immediate reverse navigation now cancels the abandoned replacement and settles
  on a pristine frame that is already presented without another decode or texture
  upload. After a move within two positions completes, the just-left pristine
  decode is shared into the existing five-entry, 256 MiB LRU without copying
  pixels when those limits permit. Cache selection removes the shared alias before
  edits can resume; larger jumps, crop and Spot Heal results, animation playback
  frames, explicit Reload state, oversized or evicted entries, and old playlists
  remain ineligible.
- Appearance persistence now assembles and syncs the validated one-word value in
  the configuration directory before atomic replacement, so an interrupted
  assembly cannot truncate the last valid choice.
- Neighbor prefetch now tags work with a playlist generation and suppresses a
  failed or over-budget result until that playlist changes or a successful
  foreground presentation proves the path usable. Stale completions cannot seed or
  suppress replacement-playlist or explicit-Reload state, and a late speculative
  failure cannot override a successful foreground presentation. Obsolete reads
  now cancel cooperatively, and the first valid decode for a newly selected
  neighbor wins instead of discarding ready pixels. Each effective terminal outcome
  emits one bounded, filename-only operator diagnostic instead of an autonomous
  retry loop.
- GPU patch updates now reject reduced preview textures instead of applying
  full-resolution Spot Heal coordinates to them. The shared heal, undo, and redo
  fallback rebuilds the displayed image through the existing asynchronous preview
  path, so CPU pixels and the visible preview cannot silently diverge.
- A retained frame can no longer accept Spot Heal input while another image is
  loading. Heal jobs bind both the selected path and image generation, so a late
  result cannot mutate a replacement image even when its dimensions match.
- Optional AVIF and HEIC workers now attach bounded ICC or typed CICP color
  evidence to protocol V2 pixel streams. AVIF decode preserves libavif metadata;
  HEIC reads source ICC size before fallible allocation, preserves source NCLX
  when newer libheif versions would otherwise choose a different output target,
  enables libheif 1.23 bitstream-profile passthrough when no container NCLX
  exists, and keeps the source ICC when passthrough performs no extra gamut
  conversion. Decoded output CICP replaces ICC only when a requested transform
  demonstrably changes the color encoding.
  System-libheif and embedded 1.23 release tests cover the compatibility floor,
  dual-profile precedence, HEVC VUI-only color, matching ICC plus VUI metadata,
  and a libde265 version that can propagate that VUI. The main process converts
  worker ICC input after the cancellable IPC transaction has joined, recognizes
  tagged sRGB, and exposes unsupported or unknown color as an explicit fallback
  instead of silently assuming tagged sRGB.
- Crop export now validates source buffers, uses fallible allocation, and
  quantizes locked selections to exact reduced integer ratios even on odd-sized
  images. The same quantizer supplies exact accessible output bounds. Save As
  validates aliases, formats, pixels, and retention support before touching its
  destination, then persists a completed sibling temporary output. EXIF detection
  follows content rather than extensions; retention supports JPEG, PNG, and WebP.
- Current-image animation, image-information, and GPU-preview work now use
  independent replace-latest queues. Rapid navigation cannot strand the final
  selection behind a full speculative queue. Still-image readers stop at I/O
  boundaries after supersession, animation stops between frames, and worker
  input reads and blocked IPC requests terminate cooperatively. Renamed animated
  containers are detected by content.
- Over-limit GPU previews are now prepared off the window thread with a
  cancellable linear-light, alpha-correct area filter and fallible bounded
  allocation. The release performance gate exercises that path and a high-resolution
  corpus where entry-only cache eviction would exceed the 256 MiB byte budget.
- Image Information now reports detected file content before a misleading filename
  extension. Bounded TIFF metadata is sought anywhere in a large source and
  compacted without pixel strips. PNG text, EXIF, and ICC plus WebP EXIF and ICC
  payloads are bounded before decoder materialization, including post-IDAT APNG
  metadata. JPEG XL initialization now enforces the same 10 MiB ICC ceiling and
  rejects command-stream output amplification. Decoder dimensions and declared
  output bytes, including animation canvases, are validated before pixel-buffer
  allocation.
- The native Windows accessibility gate now verifies the exact Console preference,
  relaunches the process to prove restoration, and enforces one absolute suite
  deadline in addition to per-operation deadlines.
- Tools and Folder Previews can now be fully hidden instead of leaving a disclosure
  rail in the image viewport. View owns explicit visibility controls for all three
  panels, while disclosure chevrons remain available for compact collapse.
- Egui redraw requests no longer schedule another redraw from inside the current
  redraw event. The settled viewer now returns to the sleeping event loop instead
  of continuously presenting frames, and repeated identical chrome styling no
  longer requests needless visual updates. The performance probe now waits for
  legitimate delayed hover repaints to finish before measuring settled idle, so
  machine cursor placement cannot create a false regression.
- Bounded prefetch and thumbnail work now refills from completion events instead
  of depending on incidental paint events. Explicit timer wakes make the
  performance probe's idle interval and one-minute deadline deterministic, while
  normal directory scans avoid redundant per-entry metadata lookups.
- The empty-state privacy line now reads "Maximum privacy. It just works." instead
  of exposing implementation-detail copy inside the viewer.
- Initial image decode and sibling-folder discovery now start before GPU
  initialization and wake the event loop when their background work completes.
  The first window stays responsive with an explicit loading state, and small
  images no longer collapse the application into a cramped layout.
- Persistent tools, folder previews, and Image Information now reserve viewport
  space. Expanding them refits and recenters the photo instead of covering it.
  Empty-state guidance remains readable on a pure-white image background.
- Image drawing is GPU-scissored to the panel-safe viewport at every zoom and pan
  level, and crop geometry is converted from physical pixels to logical UI points
  on high-DPI displays. Load failures now remain visible in the empty state.
- Keyboard shortcuts remain active when a non-text control has accessibility
  focus, while open menus and control activation keep ownership of their keys.
  macOS now uses Command for Open File and Open Folder instead of displaying or
  requiring a Windows-style Control shortcut.
- Delayed egui repaints now drive the sleeping event loop, so stationary-hover
  tooltips and other timed UI appear without unrelated input. Static toasts
  schedule only their expiry repaint instead of forcing a continuous redraw.
- Folder-preview textures are retained only for the bounded visible window and
  completed off-screen thumbnail work is discarded, keeping image-cache memory
  independent of folder length.
- macOS bundles now receive Finder and Open With requests by adding the missing
  Launch Services selector to winit's existing application delegate, preserving
  winit lifecycle ownership and waking its event loop through `EventLoopProxy`.
  macOS trash Undo also preserves the exact resulting item URL and refuses to
  replace an existing restore target.
- Trash receipts now store absolute original paths so relative command-line
  opens restore correctly on Windows and Linux. Undo retains retryable receipts
  and reports when manual system Trash review is required.
- Restored the quality baseline: `cargo fmt`, pedantic `clippy -D warnings`, and the full test suite are green again.
- Raised measured logic coverage from 79.61% to 89.16% by testing CLI behavior and decode-boundary invariants, including diagnostics, benchmark paths, resource limits, explicit in-memory format dispatch, trash receipts, viewport geometry, and the corpus contract.
- Prevented one process from deleting another process's live temporary test workspace by holding and respecting standard-library file locks during stale-debris cleanup.
- Serialized tests that invoke global stale-debris cleanup so parallel test execution cannot erase another test's scrub-safe fixture.
- Implemented SVG decode with pure-Rust `resvg` (corpus and unit tests pass). Default features avoid system fonts and text shaping so the trusted core stays free of unmaintained shaping crates.
- Coverage gate again measures meaningful logic only; CI excludes display/IPC glue (`app`, `gpu`, `ui`, `sandbox`, `worker_limit`, `error`, `main`) per `docs/STANDARDS.md`. Current measured logic coverage remains above 88% lines under that floor, including above 86% for the new healing core.
- Initial image decode now runs off the winit event thread, invalidates stale displayed pixels, and applies only if its path is still current. A two-slot, foreground-priority decode gate bounds aggregate work, and superseded foreground jobs cancel before file access.
- Decode resource limits reject zero, oversized, or inconsistent pixel shapes before parent allocation and pixel-stream copy. SVG and worker inputs are capped, while the C-worker address-space ceiling also bounds allocations performed inside third-party decoders.
- Worker-bound host files are verified as regular files and read with a bounded, fallible allocator before a worker is reserved. The IPC deadline thread now contains only cancellable child-pipe work, and encoded bytes are released immediately after transfer.
- Sandboxed file opens now degrade safely to a one-image playlist when sibling enumeration is denied. **Open Folder** provides explicit session-scoped directory consent for next/previous navigation without broad filesystem capabilities.
- The Windows accessibility smoke gate now allows its complete multi-action UIA
  flow 60 seconds while retaining per-step polling, process-exit detection, and
  the existing hard 120-second parameter ceiling. This removes a reproducible
  cold-machine CI timeout without weakening any accessibility assertion.

### Added

- Added bounded GIF, WebP, and APNG playback with deterministic timing,
  pause/resume state, finite and infinite loop handling, and current-image-only
  memory limits.
- Added Free, Original, 1:1, ten standard landscape and portrait crop ratios,
  orientation swap, exact numeric custom ratios, eight pointer handles, and full
  keyboard movement, resizing, apply, and cancel behavior.
- Added Image Information, a functional accessible About modal, and complete
  System, Light, Dark, and phosphor-green Console appearances. The selected
  appearance is stored as one validated local word and survives restart.
- Added bounded embedded RGB ICC conversion into the current sRGB pipeline, an
  explicit fallback status, and a complete GPU-generated mip chain for stable
  minification. Per-display output transforms and higher-precision wide-gamut or
  HDR presentation remain roadmap work.
- Added focused Spot Heal for small blemishes. `J` opens a temporary docked
  inspector that reserves image space; sparse image-space strokes run through a
  bounded pure-Rust patch-matching worker with feathered compositing and
  byte-bounded in-memory undo/redo. Repairs never write the source file and add
  no model or native dependency. Done and Esc leave the tool without dropping an
  already-submitted repair, and integration coverage verifies repair, undo, redo,
  pixel-only export, and reopen as one flow. Repair, undo, and redo upload only
  the changed texture region; editing is unavailable when a GPU texture limit
  prevents the complete source image from being displayed.
- Documented the strict gate for any future optional local description model:
  explicit one-image activation, separate model packs, path-free pixel IPC,
  process-level network and write denial, zero app-owned logs, no automatic
  speech, and a cross-platform offline model bake-off. No model runtime ships yet.
- Added a dependency-free Windows UI Automation smoke gate over the real app and
  native AccessKit provider. It verifies focusable menus, image context, default
  panel state, panel and disclosure actions, distinct left/right docking state,
  metadata checked state, the Spot Heal action path, previews, and accessible
  thumbnail navigation. The
  canonical manual Narrator, VoiceOver, and Orca matrix remains required and is
  documented separately.
- Added independent left/right docking for Tools and Image Information through
  View > Panel Position. Insets accumulate correctly when panels share an edge,
  every layout refits the image-safe viewport, and hidden panels reserve zero space.
- Added native Linux AccessKit/AT-SPI delivery behind an early fail-closed privacy
  boundary. Startup accepts only Unix-domain D-Bus environment addresses, permits
  local Unix IPC, denies Internet socket families and io_uring before application
  threads start, mirrors every blocked syscall onto its x86-64 x32 ABI alias, and
  fails launch if the policy cannot be verified. The worker baseline filter closes
  the same x32 alias path. Cargo-deny confines the generic D-Bus and process-helper
  crates to the reviewed AccessKit dependency path. Cross-target builds and policy
  contracts are automated; manual Orca acceptance remains release evidence rather
  than an implementation claim.
- Added a dependency-free black-box GUI performance gate for first-window-frame
  and first-image latency, sampled navigation, settled idle redraws, peak resident
  memory, 50,000-file folder scaling, and exact decoded/thumbnail cache bounds.
  The explicit path-free probe is absent from normal launches and CI enforces
  conservative release-mode limits under a virtual display. Large test corpora
  shard hard links across bounded source counts and clean their temporary
  workspace on successful and handled-error exits.
- Added exact-set, opt-in core-image associations for the Linux desktop entry,
  macOS application bundle, and Windows AppContainer manifest. The Flatpak build
  now installs its desktop entry and scalable icon. Contract tests keep all
  declarations aligned with the default pure-Rust decoder set and reject silent
  default-viewer takeover behavior.
- Added a shared, feature-gated Linux default-deny seccomp policy for production AVIF/HEIC workers. It permits only measured decoder, read-only plugin, thread, memory, signal-runtime, time, and pipe syscalls; denies direct and inherited-pipe cross-process signaling; proves activation with an unlisted syscall; and is exercised by release-mode AVIF and HEIC protocol decodes on Ubuntu 24.04 CI.
- Added deterministic dual-binary release archives for Linux x86-64, Windows x86-64, and Intel/Apple Silicon macOS. Each archive revalidates the exact archived executable structures, contains a canonical per-file manifest, has a SHA-256 sidecar, and is built only after the reusable complete CI and fuzz gates pass. The workflow is read-only and does not publish or sign.
- Added exact-set verified Flatpak, macOS App Sandbox, and Windows AppContainer profiles. Platform CI performs a checksum-pinned offline Flatpak build and worker probe, verifies and probes an ad-hoc signed macOS bundle, and validates an unsigned dual-binary MSIX with the Windows SDK. Destructive packaging outputs are fixed beneath `target/profile-check` and reject symlink or reparse-point staging paths.
- Added buildable coverage-guided fuzz targets for every declared pure-Rust decoder and the worker protocol, self-contained deterministic seed regeneration covering every decoder, pinned short-on-change plus 600-second scheduled CI runs, and supply-chain checks for the separate fuzz lockfile.
- Pinned every executable GitHub Action to a reviewed immutable commit and added a weekly audit-only CI run so newly published advisories do not wait for repository activity.
- A dependency-free `viewr-protocol` crate owns versioned bounded encoded-input requests, typed response frames, acknowledgements, and the shared 512 MiB RGBA8 output invariant.
- Packaged `viewr doctor` now requires an exact typed encoded-input protocol handshake from `viewr-decode`, so a present but obsolete helper cannot satisfy the package smoke test with an arbitrary error frame.
- Worker requests have a hard 30-second send/receive deadline, bounded cleanup grace, a 1.5 GiB containment memory ceiling, and tested one-process containment, pipe-saturating termination, and pixel-stream acknowledgement.
- CLI: `viewr help`, `doctor`, `benchmark [dir]`, `update` (local instructions only),
  `version`, `open <path>`; image path still opens the GUI. Windows console attach
  for subcommands under the GUI subsystem.
- Interaction polish: responsive File/Edit/View menus with aligned shortcuts,
  stable filename and folder status, a high-contrast empty/loading card, trash
  toast, an honest crop border and ratio strip, Esc cancel crop, double-click
  fit/actual-size toggle, and grab cursors.
- Cursor-anchored wheel/trackpad zoom; compact disclosure rails for docked Tools
  and Folder Previews; asynchronous real thumbnails; consistent vector icons;
  and keyboard/menu commands for fit, actual size, zoom in, and zoom out.
- The zoom readout now reports physical image scale, so Actual Size is 100 percent
  instead of exposing the internal multiplier relative to Fit.
- Accessibility semantics for custom-painted tool, disclosure, and thumbnail
  controls and exact crop bounds; visible keyboard focus; automated WCAG AA
  contrast checks; and native AccessKit delivery on Windows, macOS, and Linux.
- Filmstrip shows async real thumbnails (`thumbs` module); Space-hold temporary pan, tap Space resets view.
- Linux worker: `no_new_privs`, non-dumpable, and seccomp-bpf that EPERMs classic and io_uring network paths (`seccompiler`).
- Shift+Delete permanent delete with confirmation.
- `viewr-decode` is a workspace member with feature-gated C backends (`avif`, `heic`, `raw`); default build needs no system libraries.
- Folder navigation recognizes worker formats (AVIF/HEIC/RAW extensions); decode routes through the worker when present.
- `docs/FORMATS.md` capability table (core vs worker, RAW deferred).
- System trash via the `trash` crate (`curate` module) with undo from the OS recycle bin / trash.
- Adversarial truncated/garbage image fixtures assert decode returns errors without panicking.
- packaging sketches for Flatpak (no network), macOS sandbox entitlements, Windows AppContainer.
- Decode workers join a one-process Windows Job Object (kill-on-close with parent) or a private Unix session with a one-process policy; discarded workers are terminated on drop.

### Changed

- The main process now opens user-selected C-decoder inputs and sends bounded encoded bytes to `viewr-decode`; the worker receives no path and therefore does not depend on inherited dynamic file grants.
- Replaced worker-owned shared memory with exact-length pixel streaming over the existing pipe, removing mutable-mapping size races and the `shared_memory` dependency.
- Linux seccomp and privilege setup now fail worker spawn on error; network denial covers classic socket and io_uring paths, and non-dumpable state is applied after exec. Windows Job Object creation, assignment, termination, and memory limits also fail closed.
- Removed the unused `muda` dependency and its GTK3 dependency chain, reducing the lockfile by 41 packages and eliminating eight obsolete advisory exceptions.
- CI runs on pushes to `main`, pull requests, its weekly schedule, reusable workflow
  calls, and explicit maintainer dispatch.
- Store publication through the Mac App Store, Microsoft Store, or Flathub remains
  outside the 1.0 gate. Trusted direct distribution does not: Windows signing and
  macOS Developer ID signing and notarization are required before viewr is presented
  as a broadly recommended 1.0. Local builds and explicitly unsigned pre-releases
  remain available without those external credentials.
- Extracted `sandbox.rs` for the `viewr-decode` worker client so process/IPC glue is separate from pure decode logic.
- Phase 6 residuals closed in ROADMAP; next milestone is Phase 7 OS sandbox packaging.
- `deny.toml`: allow OFL/Ubuntu font licenses for egui default fonts; retain a documented exception for the unmaintained but vulnerability-free `paste` crate while required by EXR and metadata dependencies.
