# Roadmap

The plan from an empty repository to a viewer people install to escape the bloat.
Phases are ordered by dependency, so each one builds on what came before. There are
no dates and no time estimates here on purpose. The order is the plan, and a phase
or version is finished when its Definition of done is true, not when a calendar
says so.

Two rules hold across every phase:

1. A feature ships only if it earns its place. When in doubt, leave it out. The
   restraint is the product.
2. The quality bar in STANDARDS.md applies from the first commit. Coverage target
   is 85 percent or higher on logic, the privacy invariant is enforced in CI, and
   the decode path is fuzzed. Quality is not a later phase, it is the baseline.

## Current position

| Item | State |
| --- | --- |
| Published install target | Immutable [v0.1.0](https://github.com/blisspixel/viewr/releases/tag/v0.1.0) |
| Active development line | `main`, working toward **v0.2.0** |
| Next tag allowed | **v0.2.0** only after its exit criteria below are true |
| Later tags | Blocked until every earlier minor gate is closed |

Phases 0 through 5 and Phase 7 are complete for their local repository scope.
Phase 6 has broad core-format coverage, isolated optional AVIF/HEIC decoding, and
honest capability reporting, but camera RAW and multi-page viewing remain open.
Phase 8 has local install paths, accessibility automation, native AccessKit,
performance budgets, hosted multi-OS CI, and the signed-attested but
publisher-unsigned v0.1.0 archives. Human assistive-technology evidence, platform
signing and notarization, representative-hardware acceptance, and display
fidelity remain open.

The shipped product already includes bounded GIF/WebP/APNG playback, eight-way
EXIF orientation, RGB ICC-to-sRGB normalization, trilinear GPU mips, GPU-limited
previews with full-resolution export, last-good-frame navigation, async crop and
Save As, image information, Reload (`F5`), Spot Heal, About, and System, Light,
Dark, and Console appearances. Appearance is the only persistent UI preference
and contains no image or activity data.

## Order of operations to 1.0

Work is a single dependency chain. Do not tag a later version while an earlier
gate remains incomplete. Patch releases such as v0.1.1 may fix shipped bugs or
security issues without pulling later milestone scope forward.

```text
v0.1.0  Public foundation          [released]
   |
   v
v0.2.0  Reliability architecture   [next]
   |
   v
v0.3.0  Display-correct SDR preview
   |
   v
v0.4.0  File-coherence preview
   |
   v
v0.5.0  Format-contract preview
   |
   v
v0.6.0  Integrated product-quality beta
   |
   v
v0.7.0  Accessibility evidence preview
   |
   v
v0.8.0  Release-readiness beta
   |
   v
v0.9.0  Publisher-authenticated RC
   |
   v
v1.0.0  Broadly recommended release
   |
   +--> optional post-1.0 candidates (below)
```

| Version | Role | Why this order | Required outcome before tagging |
| --- | --- | --- | --- |
| **v0.1.0** | Public foundation, released | Ship a usable, honest baseline | Released: fast file and folder viewing, bounded decoding, ratings and filters, Source Privacy, Trash and Undo, Open With on Windows, focused editing, privacy invariants, coverage floor, immutable checksummed archives, attestations, and explicit unsigned-preview limits. |
| **v0.2.0** | Reliability architecture beta | Later features must not deepen unowned async races | Background details, animation, crop, save, thumbnails, and prefetch have one bounded job owner. Stale work cannot mutate a newer selection or edit. Failure paths are observable and recoverable. Native glue is thin. Race contracts are tested. Logic coverage stays at least 85 percent. |
| **v0.3.0** | Display-correct SDR preview | Wrong color fails the primary job even if the app is fast | Tagged SDR output matches reference conversions. Display profile refreshes when the window moves between monitors. Worker-decoded images preserve color status. Deterministic sRGB fallback stays visible. Wide-gamut and HDR stay off until a higher-precision path is proven. |
| **v0.4.0** | File-coherence preview | Viewers sit beside editors and downloaders | External edits, replacement, rename, deletion, and noisy watcher events produce deterministic visible states without blanking the last good frame or discarding unsaved edits. Open With reaches supported user-mediated chooser APIs on all three platforms. |
| **v0.5.0** | Format-contract preview | Format breadth without honest page and color contracts misleads users | Multi-page and multi-frame containers expose bounded, identifiable navigation. The format table distinguishes decode, animation, page, metadata, and color behavior. Camera RAW either meets the isolated-worker bar or is explicitly deferred from 1.0. |
| **v0.6.0** | Integrated product-quality beta | Isolated green checks do not prove the product feels right | Primary first-time, power-user, admin, failure-recovery, and visual-polish paths pass on representative Windows, macOS, and Linux hardware. Startup, navigation, memory, mixed-DPI, multi-monitor, empty, loading, and error states meet budgets with no unresolved high-severity product-quality issue. |
| **v0.7.0** | Accessibility evidence preview | Implementation can be green while real AT fails | Narrator, VoiceOver, and Orca matrices complete against exact artifact hashes. Keyboard-only operation, focus, names, roles, selected and busy state, high contrast, text scaling, loading, errors, crop, ratings, panels, and recovery have no unresolved critical or high-severity accessibility defect. |
| **v0.8.0** | Release-readiness beta | Install and update must be boring before trust theater | Representative-hardware, clean-install, reinstall, explicit update, uninstall, rollback, file-association, provenance, and complete product-acceptance matrices pass. Unsigned candidates remain acceptable only with an explicit trust boundary. |
| **v0.9.0** | Publisher-authenticated release candidate | Signing is last because scope must be frozen first | Windows Authenticode-signed delivery; macOS Developer ID-signed, hardened, notarized, and stapled; normal Linux package verified on Wayland and X11. Full security, dependency, fuzz, coverage, performance, privacy, accessibility, packaging, upgrade, rollback, documentation, and hardware matrices pass on the exact candidate. |
| **v1.0.0** | Broadly recommended release | Not a new feature tranche after v0.9 | Proven v0.9 scope ships. Docs match artifacts. Normal workflows need no developer tools. No known critical or high-severity defect remains. |

### Immediate focus

**Immediate focus: v0.2 reliability architecture, followed by v0.3 display
correctness and v0.4 file coherence.** Pure-policy seams and the owned-logic
coverage floor are evidenced (90.18 percent lines under the CI llvm-cov
contract). Before tagging v0.2.0, complete residual thin-native-glue judgment
for the remaining whole-file exclusions and the full VERIFY.md gate. Preserve
bounded job, thumbnail, prefetch, chrome, and GPU contracts. Do not start v0.3
monitor/profile work that deepens unowned event-loop races.

Current position: v0.1.0 is released and verified. v0.2.0 is the next planned
minor release. After v0.2.0: v0.3 display correctness, then v0.4 file coherence,
then v0.5 formats, then v0.6 product quality, then v0.7 through v0.9 evidence and
trust.

### Release rules

1. Patch releases such as v0.1.1 fix shipped behavior or security issues only.
2. A later milestone cannot compensate for an earlier failed gate.
3. Scope may be removed or explicitly deferred when evidence says it does not
   belong in 1.0. Acceptance criteria are not weakened to preserve a version label.
4. There are no calendar promises or duration estimates.
5. v1.0 is the release earned when the v0.9 candidate evidence holds.

## Release gate

This table is the operational front door. Detailed phase history remains below,
but completed history does not override an open gate here.

| Gate | Status | Evidence or next action |
| --- | --- | --- |
| Public repository and hosted quality | Complete | `main` is public. [CI run 30642307317](https://github.com/blisspixel/viewr/actions/runs/30642307317) passed all seven jobs and [fuzz run 30642307463](https://github.com/blisspixel/viewr/actions/runs/30642307463) passed both targets on release commit `86d3eef920ec5e523fbc6dbc286c4dcbd68e7f1b`. |
| Security intake and release integrity | Complete | Private vulnerability reporting, Dependabot alerts and security updates, secret scanning, push protection, and immutable releases are enabled. |
| First public pre-1.0 release | Complete | [v0.1.0](https://github.com/blisspixel/viewr/releases/tag/v0.1.0) is immutable. [Release run 30643016336](https://github.com/blisspixel/viewr/actions/runs/30643016336) published the exact 12-asset set with attestations. Public installer commands use fixed-version release URLs. |
| Protected `main` policy | Complete | Seven always-running CI checks, linear history, review, and conversation resolution are required; force pushes and deletion are blocked. |
| Reliability architecture | Open for v0.2 | Pure seams and 90.18 percent owned-logic coverage measured; finish residual thin-native glue judgment and full VERIFY before tagging v0.2.0. |
| Display correctness | Partial for v0.3 | Embedded RGB profiles normalize into the bounded sRGB path; per-display transforms, reference fixtures, wide-gamut, and HDR remain. |
| File coherence | Open for v0.4 | Session watcher, non-Windows Open With, deterministic external-edit states. |
| Format contract | Open for v0.5 | Multi-page navigation; RAW decision. |
| Integrated product quality | Open for v0.6 | Representative hardware polish matrices. |
| Human accessibility evidence | Open for v0.7 | Narrator, VoiceOver, and Orca records under `docs/release-evidence/accessibility/`. |
| Release readiness | Open for v0.8 | Clean install, update, uninstall, rollback, and acceptance matrices. |
| Native platform trust | Deferred to v0.9 | Authenticode, Developer ID + notarization, normal Linux package proof. |

The first public pre-1.0 release may remain clearly labeled as unsigned. A broadly
recommended 1.0 must close the human accessibility, native platform trust,
representative-hardware, and tagged-SDR display-correctness gates. RAW, HDR,
advanced Spot Heal controls, clipboard features, and touch gestures do not block
that release unless evidence shows a core workflow depends on them.

## What keeps viewr from exceptional

This is a bounded product plan, not a request to copy every feature from a larger
viewer. The research signal is consistent:

- Minimal qView treats fast preloading and animation as baseline, while recent
  releases added Reload File and fixed embedded-profile, CMYK, and per-display ICC
  failures. Its public downloads cover a Windows installer, macOS disk image,
  AppImage, Flatpak, and native repositories. See the official
  [feature page](https://interversehq.com/qview/),
  [changelog](https://interversehq.com/qview/changelog/), and
  [downloads](https://interversehq.com/qview/download/).
- ImageGlass treats live file-change refresh, multi-frame navigation, animation,
  color management, thumbnails, and touch input as viewer capabilities rather
  than editor bloat. See its official
  [feature matrix](https://imageglass.org/docs/features).
- nomacs demonstrates the remaining format-depth bar with optional RAW and
  multi-page TIFF support. See its official
  [repository and build options](https://github.com/nomacs/nomacs).

viewr already has a stronger privacy and hostile-input story than those references.
What is missing is not another toolbar. It is end-to-end fidelity, complete edge
behavior, installability, and maintainable proof of correctness.

### Release-close gates, v0.7 through v0.9: accessibility and trusted distribution

Why later: accessible implementation remains a baseline throughout development,
but artifact-bound human evidence and publisher authentication should be gathered
against a stable product candidate. The unsigned v0.1.0 preview remains honest
while reliability, fidelity, coherence, formats, and integrated product quality
are completed first.

- [x] Run the complete hosted Linux, macOS, and Windows workflow for one pinned
  commit and retain the [green CI run](https://github.com/blisspixel/viewr/actions/runs/30642307317)
  and [green fuzz run](https://github.com/blisspixel/viewr/actions/runs/30642307463).
- [x] Enable private vulnerability reporting, Dependabot alerts and security
  updates, secret scanning, push protection, and immutable releases in the public
  repository.
- [x] Protect `main` with the seven stable CI checks, linear history, review and
  conversation resolution, blocked force pushes and deletion, and administrator
  emergency bypass. Path-filtered fuzz remains mandatory in the release workflow.
- [ ] Complete Narrator, VoiceOver, and Orca acceptance using
  `docs/ACCESSIBILITY.md`, including crop, reload, animation, errors, and busy
  states.
- [x] Publish [v0.1.0](https://github.com/blisspixel/viewr/releases/tag/v0.1.0)
  as checksummed dual-binary archives from the green commit with reviewed notes,
  GitHub build provenance, and clear optional file-association guidance.
- [x] Make the release-state and quality-gate contract executable: canonical
  documentation now agrees on the public unsigned v0.1.0 state, CI runs the exact
  locked all-target commands, and cargo-deny rejects unreviewed duplicate versions
  against an explicit transitive baseline without unexplained warnings.
- [ ] For v0.9, produce and verify a signed Windows delivery, a Developer ID-signed and
  notarized macOS application and disk image, and a normal Linux Flatpak or
  equivalent package. Store publication remains optional.

Definition of done for these release-close gates: a user can download,
authenticate, install, exercise, update,
and remove viewr without compiling it, changing defaults silently, or trusting an
unrecorded manual build, and the core workflows have completed artifact-bound
human accessibility evidence on all three platforms.

### Priority 1, v0.2: make correctness easier to preserve

Why now: later milestones add monitor transitions, file watchers, page state,
and optional worker formats. `app.rs` and `ui.rs` already own too many independent
state transitions, so those capabilities should not deepen concentrated async
state before job ownership and test seams are explicit.

- [x] Extract pure crop/output geometry and its keyboard/pointer transitions into
  a covered module.
- [x] Extract selected path, presented path, generation, receiver, and load-error
  transitions into a covered `Session` owner. Native scheduling and retry remain
  in `App` until bounded job coordination is extracted.
- [x] Extract `Playlist` folder list, index, and scan-purpose data into its own
  module without introducing a second mutable store.
- [x] Extract explicit `PerformanceProbe` state and transitions into a covered
  module.
- [x] Extract bounded job coordination for image details, animation, crop, save,
  folder scans, thumbnails, and prefetch, leaving `App` responsible for platform
  events. The first covered slice now owns the one-result image-details, animation-discovery,
  and rating-observation lifecycle, including closed-completion wakeup and exact
  path/generation rejection. The second slice gives Save As a one-item consuming
  completion, fail-closed terminal disconnect handling, and success-only close
  reconciliation; its captured output transaction cannot overlap rating mutation
  or mutate foreground image state. A captured existing destination receives an
  app-owned overwrite confirmation and identity revalidation before the job can
  start, so native-dialog timing cannot adopt an unconfirmed file. The third
  slice gives crop the same bounded,
  consuming completion owner, retains cooperative cancellation and exact
  generation/path/pixel-allocation recovery checks, rejects late publication,
  restores retryable typed failures, and persistently blocks another crop after
  indeterminate endpoint loss. The fourth slice gives replace-latest over-limit
  display previews one bounded consuming completion, retains exact path,
  generation, presentation kind, source, and crop recovery context on the event
  loop, rejects late publication, and leaves typed failures retryable. Endpoint
  loss closes the presentation queue, drops pending work even after owner
  replacement, restores a current crop selection when possible, disables load
  retry only for the affected selection, and persistently blocks work that needs
  the lost executor instead of queuing forever. Later ordinary decode failures
  remain retryable. The fifth slice moves filmstrip thumbnails behind a
  nine-owner maximum with event-loop-owned path and generation context. Thumbnail
  pixels are structurally validated and path-free, stale or off-window results
  cannot upload, executor saturation remains retryable without a wake loop, and a
  typed or disconnected failure is attempted once until the path leaves the
  visible window or its generation resets. Executor supervision remains open;
  release builds do not claim general recovery from an in-process thread panic.
  The sixth slice replaces prefetch's shared unbounded completion channel with
  at most four event-loop-owned one-result jobs across current and cancelled
  generations. Owner context supplies the only publishable path and generation,
  cancellation remains cooperative, a successful foreground open wins the
  same-path race, stale pixels cannot publish, and fixed path-free failures retain
  the existing terminal-retry policy. A shared acceptance-armed wake contract
  makes fast completion and accepted endpoint loss observable while rejected work
  owns nothing and remains retryable.
  The seventh slice gives each folder scan one event-loop-owned result endpoint
  and context-owned cancellation. Replacing or dropping the owner stops obsolete
  enumeration cooperatively, including cancellation observation during natural
  sorting. Endpoint loss is visible, and more than 100,000 supported regular files
  or 64 MiB of cumulative encoded path storage produces an explicit safe failure
  instead of unbounded allocation or a silently truncated playlist. Enumeration
  and nonblocking child opens are bound to one retained directory identity;
  identity-plus-version provenance now reaches foreground decode, prefetch,
  thumbnails, ratings, restore, and explicit F5 replacement refresh. Accepted
  decode, animation, details, and rating results reject an in-place rewrite before
  publication. The eighth slice keeps full Windows content-witness comparisons
  off the event loop: Open With owns one generation-cancellable job; Trash,
  permanent delete, and restored-file inspection use the single typed curation
  worker; replace-latest animation, details, and rating inspection propagates
  generation cancellation through every full witness comparison; Save As
  destination consent retains only native identity and version. Failed or partial
  curation cancels deferred close so recovery remains visible.
- [x] Move dock/menu view models out of paint code so enablement and accessibility
  state can be exhaustively tested without a window. One immutable projection now
  derives dock layout, control readiness, selected state, labels, shortcuts, and
  accessibility copy from a single raw frame snapshot. Covered blocker matrices
  include recovery ownership, concurrent work, unavailable Spot Heal, and the
  requirement that an active tool always remains closable.
- [x] Narrow the coverage exclusion as each seam becomes pure. The first
  enforcement step now includes the egui/AccessKit `ui.rs` adapter after chrome
  policy moved to its pure projection, while the measured floor remains above 85
  percent. The exclusion is exact-path scoped so it cannot hide similarly named
  integration tests or vendored source. The second step moves GPU image sizing,
  mip planning, linear-light preview preparation, and upload selection into the
  covered CPU-only `gpu_image` seam at 95.76 percent line coverage; the complete
  measured floor was 89.20 percent. The third step moves first-supported sRGB
  surface selection, patch upload geometry, placement packing, and clear-color
  mapping into the 100 percent covered private `gpu_policy` seam. The complete
  measured floor was 89.30 percent. The fourth step moves pristine-pixel reuse,
  selected-versus-presented navigation planning, opening-state classification,
  durable load errors, and exact preview identity into the 100 percent covered
  private `presentation` seam. The complete measured floor was 89.36 percent.
  The fifth step moves curation operation identity, recovery priority and copy,
  source-removal preflight, count grammar, status, and deferred-close decisions
  into the 100 percent covered private `curation_state` seam. `App` remains the
  only worker, path, receipt, playlist, and recovery-application owner. The sixth
  step moves presented-rating, recovery, discovery, terminal-write, and deferred-
  close decisions into the 100 percent covered private `rating_state` seam.
  `App` remains the only accepted-source, path, worker, disclosure, playlist,
  recovery-application, UI-dispatch, and close owner. The seventh step moves
  Save As start blockers, folder-scan save gates, terminal close disposition,
  and app close wait coordination into the 100 percent covered private
  `save_state` seam. `App` remains the only destination, worker, image, dialog,
  and close-application owner. The eighth step moves concurrent-work preflight
  (`current_work`), crop recovery identity (`crop_state`), keyboard routing
  (`keyboard_route`), folder-scan dispositions (`entry_state`), generation-path
  currency (`work_currency`), edit presentation failure copy (`edit_state`),
  expanded curation outcome and permanent-delete confirmation copy
  (`curation_state`), rating write and auxiliary disconnect guidance
  (`rating_state`), prefetch destination routing (`prefetch`), and filter
  source-change gates (`playlist`) behind pure unit-tested seams. `App` remains
  the only event-loop, worker, dialog, playlist-mutation, toast, and
  UI-dispatch owner. Owned-logic line coverage remeasured with the CI
  `cargo-llvm-cov` contract (workspace, locked, exact-path exclusions for
  `app`/`gpu`/`sandbox`/`worker_limit`/`error`/`main`, jxl vendor packages
  outside the owned-logic denominator, `--fail-under-lines 85`) at **90.18
  percent** lines (summary report), above the 85 percent floor. Workspace
  `cargo fmt --check`, Clippy `-D warnings`, and
  `cargo test --workspace --all-targets --locked` remain green. Preserve the
  bounded job, thumbnail, prefetch, chrome, preview, presentation, curation,
  rating, save, and GPU upload contracts.

Definition of done: important state transitions have one owner and one pure test
surface, native glue is thin, and a late worker result cannot mutate a newer image,
edit, or panel state.

### Priority 2, v0.3: color that is correct on the actual display

Why next: a viewer that renders the wrong color is failing its primary job, even
when it is fast. The current RGB ICC-to-sRGB normalization prevents the most common
embedded-profile error, but an RGBA8 sRGB working path cannot preserve wide-gamut
or HDR source values, and the output is not transformed for the monitor that owns
the window. Apple exposes display profiles and transforms through
[ColorSync](https://developer.apple.com/documentation/colorsync), while Windows
exposes device profile associations and transforms through the
[Windows Color System](https://learn.microsoft.com/en-us/windows/win32/api/_wcs/).
wgpu 30 adds explicit surface color spaces and display HDR information; viewr is
currently on wgpu 29. See the official
[`SurfaceConfiguration`](https://docs.rs/wgpu/latest/wgpu/type.SurfaceConfiguration.html)
and [`Surface`](https://docs.rs/wgpu/latest/wgpu/struct.Surface.html) APIs.

- [x] Read bounded embedded RGB ICC data and convert it into the current sRGB
  path, including animated frames, with an explicit fallback status.
- [x] Generate the full GPU mip chain in the sRGB texture pipeline so minification
  is stable and linear-light filtered.
- [x] Carry trustworthy color metadata through the optional worker protocol rather
  than silently treating AVIF/HEIC output as untagged sRGB. Protocol V2 bounds
  ICC to 10 MiB, types CICP fields, and makes unknown output explicit; optional
  release tests compare AVIF/HEIC worker pixels with decoder references, exercise
  both the Ubuntu system-libheif floor and an embedded libheif 1.23 dual-profile
  path, and pair with parent IPC-normalization tests. HEIC ICC extraction is
  size-first and fallible; newer libheif output is explicitly held to the source
  NCLX contract, version-10 bitstream-profile passthrough is enabled, and decoded
  output evidence supersedes ICC only after a demonstrated encoding change. ICC
  remains authoritative under no-transform passthrough, including matching
  bitstream-only NCLX. The latest-codec lane enforces the libde265
  VUI-propagation floor before exercising both container NCLX and HEVC-VUI-only
  fixtures.
- [x] Separate source pixels, working color space, and output transform so future
  wide-gamut values are not clipped by the current RGBA8 sRGB intermediate.
  Decoder-owned pixels now cross one consuming normalization boundary into a
  typed RGBA8 sRGB working encoding. Edits preserve that encoding; preview
  generation, thumbnails, export, and the renderer explicitly reject an
  incompatible working encoding instead of clipping or relabeling it. The
  renderer owns the matching output transform and requires an sRGB surface. Unit
  and integration tests cover core, animation, worker, edit, preview, thumbnail,
  export, upload, and surface-selection paths.
- [ ] Upgrade the wgpu/egui-wgpu integration only after a focused compatibility
  spike proves surface color-space and HDR behavior on all three backends.
- [ ] Resolve and refresh the profile for the display that currently contains the
  window, including a move between differently profiled monitors.
- [ ] Add CMYK/profile fallback fixtures plus sRGB, Display P3, and Adobe RGB
  reference-vector tests. Keep a deterministic sRGB fallback when platform profile
  information is unavailable.
- [ ] Enable wide-gamut and HDR presentation only after a higher-precision working
  path, tone mapping, capability checks, and real-display acceptance tests exist.

Definition of done: tagged SDR images match reference conversions, moving the
window between profiled displays updates output without a restart, worker-decoded
images never lose color status silently, and HDR or wide-gamut modes cannot engage
without an end-to-end higher-precision path.

### Priority 3, v0.4 and v0.5: file and format coherence

Why after color: image viewers commonly sit beside editors, exporters, scanners,
and download tools. A stale view or a container that exposes only its first page
makes the application feel unreliable even when the decoder technically succeeded.

- [x] Keep the last good image visible during a cache miss or failed replacement.
- [x] Add File > Reload File (`F5`) with cache bypass and no blank frame.
- [x] Add a Windows-native Open With handoff in File and the image context surface.
  A generation-cancellable background job first verifies the exact accepted
  current source, then passes it through `SHOpenWithDialog`. Navigation discards
  obsolete work. The handoff persists no editor or history, exposes the external-
  app privacy boundary, and keeps a path-private `F5` reminder after successful
  delegation.
- [ ] Add equivalent user-mediated chooser behavior on macOS and Linux only
  through supported workspace or desktop-portal APIs, with package-sandbox and
  native accessibility evidence. Do not substitute a shell command or silently
  launch the default application.
- [ ] Add a session-scoped file watcher for the current image and folder. Coalesce
  noisy events, preserve the old frame until a successful refresh, update the
  playlist deterministically, and write no history or database.
- [ ] Add first-class frame/page navigation for multi-page TIFF and ICO, reusing
  the bounded animation/page model without auto-playing documents.
- [ ] Ship camera RAW only through the path-free bounded worker, with orientation,
  color metadata, representative camera fixtures, fuzz seeds, and the same memory
  and deadline contracts as AVIF/HEIC.
- [ ] Decide clipboard open/copy and touch gestures from measured user workflows,
  not from feature-count pressure. They remain behind the work above.

Definition of done: external edits appear predictably, every selected page/frame
is identifiable and bounded, and the format table distinguishes container support
from page, animation, metadata, and color behavior.

### Priority 4, v0.6 through v0.9: integrated product and release proof

Why last: individual capabilities can pass in isolation while the complete product
still feels rough or fails under real platform conditions. These releases add no
broad feature category. They prove and refine the accumulated viewer.

- [ ] Exercise first-time, fast-path, admin, failure-recovery, keyboard-only, and
  visual-polish workflows on published Windows, macOS, and Linux artifacts.
- [ ] Close evidence-backed layout, spacing, copy, loading, empty, error, recovery,
  and diagnostic issues without adding decorative controls or unrelated features.
- [ ] Repeat startup, animation, large-image, 50,000-file, mixed-DPI,
  multi-monitor, and profiled-display acceptance on representative hardware.
- [ ] Prove clean install, same-version reinstall, update from each supported
  pre-1.0 line, uninstall, file-association opt-in, and injected rollback on the
  signed release candidates.
- [ ] Freeze scope for v0.9 and rerun the complete security, privacy, dependency,
  fuzz, coverage, performance, accessibility, packaging, documentation, and
  release-provenance gates against the exact candidate artifacts.
- [ ] Enter v1.0 with no known critical or high-severity product, security,
  accessibility, reliability, or distribution defect and no essential workflow
  that depends on developer tools.

Definition of done: the release candidate proves the complete product on real
platforms, every remaining limitation is explicit and non-essential, and v1.0 can
ship without adding another feature tranche.

### Completed track: durable ratings without a photo-library database

Why it shipped: the product owner selected the Lightroom-style workflow
of rating the current image from 0 through 5 and narrowing a folder to a minimum
rating. This is useful curation, but only if it stays local, interoperable, and
durable without becoming an activity index or risking source corruption. The full
approved behavior and safety contract is in `docs/RATINGS.md`.

- [x] Choose standard embedded metadata as the only persistence model:
  `xmp:Rating` with the 0-to-5 IFD0 `0x4746` SimpleRating mirror where supported.
  Reject sidecars, manifests, databases, alternate streams, extended attributes,
  timestamps, and viewing history.
- [x] Define Unrated, ratings 1 through 5, Rejected, Conflict, Unsupported, and
  Unreadable without silently rounding or repairing external metadata.
- [x] Define first-write disclosure, bare `0` through `5` assignment guards,
  modifier-based Fit and Actual Size, visible current state, minimum filters,
  no-match recovery, and accessible names and selected state.
- [x] Replace the advisory-affected XMP path with a bounded parser and writer that
  can consume hostile metadata without an ignored runtime vulnerability.
- [x] Prove failure-atomic ordinary-JPEG replacement, exact source-version checks,
  permission and security-metadata preservation, rollback, unrelated-metadata
  preservation, and cross-tool interoperability.
- [x] Refactor Playlist to retain one canonical catalog and a tested filtered
  index projection. Prove deterministic navigation, prefetch, Trash, and Undo
  against canonical positions before adding UI state.
- [x] Add the write worker, session-only folder scan and cache, Edit and View
  surfaces, numeric shortcuts, persistent active-filter status, filtered-empty
  state, and native accessibility coverage.
- [x] Keep the existing 85 percent logic-coverage floor and 50,000-file memory,
  startup, navigation, and idle budgets green with ratings present.

Definition of done: ratings survive restart and ordinary rename, other compliant
software sees the same value, filters govern every navigation surface, unsupported
files remain untouched, and no database, sidecar, history, or silent source write
exists.

## Phase 0 through Phase 8

The numbered phases below are historical build order. They remain for audit and
contributor orientation. **Version gates above override phase narrative** when
deciding what may be tagged next.

### Phase 0: Foundations

Establish the ground truth so quality is enforced from the very first commit.

- Cargo workspace and the module skeleton described in ARCHITECTURE.md.
- Apache 2.0 LICENSE in place.
- Pinned toolchain (rust-toolchain.toml), committed Cargo.lock, declared MSRV.
- CI on Linux, macOS, and Windows running: fmt check, clippy at pedantic with
  warnings as errors, nextest, and coverage via cargo-llvm-cov.
- The privacy invariant as CI and runtime gates: cargo-deny bans remote-service
  client stacks and constrains Linux D-Bus to AccessKit, while Linux startup denies
  Internet socket creation before application threads. This lands before features.
- cargo-audit and cargo-deny wired for supply-chain and license checks.

Definition of done: an empty window builds and runs on all three platforms in CI,
every quality gate is green, and adding an HTTP crate would fail the build.

### Phase 1: It opens an image

The smallest thing that is genuinely useful and genuinely fast.

- Open from a command-line argument, a native Open dialog (`rfd`), and the operating
  system "open with" association.
- Decode the common baseline formats (JPEG, PNG, GIF, WebP, BMP) via image-rs.
- Display through our own winit and wgpu pipeline, fit to window by default, large images scaled
  correctly on first paint.
- [x] Keep application-window dimensions independent of image dimensions. Opening
  the first image fits it inside the existing viewport without moving or resizing
  the window.
- First-pixel latency tracked as a metric from day one.

Definition of done: double-clicking a JPEG or PNG opens it near-instantly and
scaled correctly on Linux, macOS, and Windows, with tests covering the open path.

### Phase 2: Folder navigation that feels instant

The core experience, which is flipping through a folder with no perceptible lag.

- Scan the containing folder off-thread, in natural-sort order so img2 comes before
  img10.
- Left and right arrows, Home and End, navigate the folder.
- Neighbor prefetch into a bounded decoded-image RAM cache, so the next image is
  usually decoded before it is requested and needs only a GPU upload. Immediate
  reversal settles on a pristine frame that is still presented; after a move
  within two positions completes, the just-left pristine decode is eligible for
  the same bounded cache without a pixel copy. Genuine misses name the selected
  target by a path-free filename while presented metadata remains tied to the
  visible pixels; immediate reuse and full-resolution cache hits stay quiet.
- Animated GIF, WebP, and APNG playback with bounded frames, correct frame timing,
  pause/resume, and container loop behavior.

Definition of done: holding the arrow key through a folder of 4K images is smooth
with no stutter, memory stays flat on a folder of 50,000 images, and property tests
cover ordering and cache eviction.

### Phase 3: Look at it properly

Make viewing excellent, not merely functional.

- [x] GPU pan by dragging or holding Space, focal-point scroll zoom, and explicit
  keyboard commands for fit (`0`), actual pixels (`1`), zoom in (`+`), and zoom
  out (`-`). A Space tap resets fit; double-click toggles fit and actual pixels.
- [x] Rotate 90 degrees either direction, and flip.
- [x] Fullscreen and a frameless immersive mode that is just the picture.
- [x] System-driven default image background via winit, updating live when the
  operating-system setting changes, with explicit black, neutral-gray, and white
  alternatives.
- [x] Complete System, Light, Dark, and Console appearances covering native
  decoration, GPU canvas, standard widgets, custom controls, overlays, and
  typography. All resolved palettes have automated AA contrast checks and the
  one-word selection persists locally.
- [x] Descriptive, accessible Appearance rows explain all four outcomes, identify
  Console as the green-screen look, report System's effective mode while active,
  and distinguish app appearance from image pixels and independent background
  overrides. The parent View entry summarizes the current preference. Native UI
  Automation selects every option and verifies restart.
- [x] Appearance persistence assembles and syncs the validated word beside its
  destination before atomic replacement, preserving the previous choice if
  assembly fails.
- [x] Appearance loading distinguishes quiet missing state from invalid,
  oversized, unreadable, and unavailable state. Abnormal fallback uses System,
  announces one path-free recovery status, emits only a fixed category to opt-in
  diagnostics, and leaves repair to an explicit appearance choice.
- [x] Compact docked `egui` controls with keyboard shortcuts and explicit
  disclosure rails. Persistent chrome reserves viewport space and never covers
  the image.
- [x] Stable resting chrome: unchanged first-run content retains fixed geometry,
  and filename, dimensions, and physical zoom have distinct reading gaps in the
  top status.

Definition of done: viewing feels polished and obvious, the default image
background follows the operating system live, persistent chrome stays compact and
collapsible, and no control or preview covers the photo.

### Phase 4: Curation, delete and cull

The feature that makes viewr a daily tool, done carefully.

- [x] Current-image Trash: `Delete` and File > Move to Trash move only the visible
  image through the supported platform Trash API, preserve playlist position, and
  advance to the image that replaces it rather than jumping to the top.
- [x] Conventional destructive input: the former bare `B` mark/review/batch-trash
  workflow was removed after product review. `B` and `M` are unassigned, and `X`
  swaps crop-ratio orientation only while Crop is active. There is no hidden mark
  state or batch action behind the simplified surface.
- [x] Exact Undo (`U`): Windows and Linux accept a new Trash identifier only when
  its native identity matches the retained accepted-source handle; macOS keeps the
  exact resulting URL with that handle. Restore repeats the identity check and has
  no pathname fallback. Receiptless moves preserve a prior valid action, and only
  transient or resolvable failures remain retryable. Cross-folder Undo restores on
  disk without inserting the source-folder path into an unrelated current view.
- [x] Nonblocking curation ownership: Trash, permanent delete after confirmation,
  and native restore run through one typed worker and one-result wake channel.
  The worker owns final strong source validation, platform operations, and
  restored rating/provenance inspection. The event loop retains playlist scope,
  indices, prior Undo ownership, and the only commit. Conflicting mutations wait
  while view controls repaint. Fixed operation status avoids false percentage,
  estimate, or cancellation claims. Normal close exits only after successful
  reconciliation and join; failure stays visible; spawn failure changes no state;
  worker loss retains durable guidance; indeterminate Trash or permanent delete
  requires a process restart after filesystem inspection, while indeterminate
  restore retains `U`; new Trash waits for uncertain ownership to settle.
- [x] Source-bound single destructive actions: the displayed image retains the
  handle that supplied accepted pixels. Delete verifies matching no-follow
  identity, version, and available content evidence on its worker immediately
  before Trash. Missing, replaced, linked, and unverifiable entries fail closed
  with fixed path-free categories and do not mutate playlist or Undo state.
- [x] `Shift+Delete` permanent delete with an explicit bounded confirmation.
  Native source identity and version are checked before confirmation. After
  acceptance, the worker repeats the full check immediately before removal.
  Cancel performs no filesystem action, and permanent delete preserves any prior
  valid Trash action.
- [x] Destructive-action readiness: foreground load and preview work, Crop, Save
  As, active Spot Heal strokes, and heal workers block Trash, permanent delete,
  and restore with a specific visible reason. External platform errors cross a
  fixed path-free boundary before interface copy or diagnostics.
- [ ] Security and reliability debt: investigate platform-specific staged or
  handle-relative Trash and restore operations that can close the remaining races
  between the final worker comparison and later pathname or Trash-identifier
  consumption by the operating system. Do not describe the current portable
  preflight as atomic.

Definition of done: a user can move through a folder deleting junk quickly,
source-bound preflights reject stale, replaced, linked, or unverifiable intent,
normal Trash never opens a modal, and tests cover delete, undo, input routing, and
index preservation. The later platform handoff remains the explicit security and
reliability debt above.

### Phase 5: Basic tools, save, convert, crop

The simple tools people actually reach for, and nothing beyond them.

- [x] Crop with a GPU preview, eight pointer handles, keyboard movement/resizing,
  output-oriented Free, Original, 1:1, 3:2, 2:3, 4:3, 3:4, 5:4, 4:5, 5:3,
  3:5, 16:9, and 9:16 presets, reversible orientation, numeric custom ratios,
  exact dimensions, and direct full-resolution application.
- [x] Crop transaction hardening: exact selection, view, paused animation,
  generation, and decoded-image identity survive through renderer commit;
  current-source failure restores immediate retry; navigation cancels obsolete
  row copying; disconnected preview work cannot retain a permanent busy state;
  and every positive selection retains exact accessible bounds.
- [x] Focused Spot Heal for small blemishes: sparse image-space brush input,
  bounded deterministic edge-aware ranking of up to eight distinct sources off
  the UI thread, robust boundary tone adaptation, adjustable feathering,
  directional fallback inpainting, Refresh Source (`/`), in-memory undo/redo,
  bounded GPU texture-region updates, and a temporary docked inspector that never
  covers the photo. It adds no model or native dependency, refuses ambiguous
  GPU-clamped source mappings, commits pixels and history only after successful
  presentation, rolls both back on presentation failure, and never changes the
  source file.
- [x] Save As and convert between supported output formats off the UI thread,
  applying the visible rotation and flips exactly.
- [x] Metadata strip on export, presented prominently, with location and
  identifying fields stripped by default. Explicit session-only retention
  normalizes orientation, dimensions, and stale thumbnail offsets while retaining
  descriptive, camera, and GPS tags.
- [x] Bind Image Information inspection to the accepted source handle and add a
  bounded Source Privacy summary for EXIF tag count plus location, authorship,
  identifiers, comments, software history, embedded thumbnails, and maker data.
  Keep sensitive values off-screen and state that absent supported EXIF does not
  prove other metadata or hidden pixel data is absent.

Definition of done: a user can crop or spot-heal an image, export it to another
format, and be confident their location data did not ride along, with tests over
the edit, undo/redo, export, and metadata-strip paths.

### Spot Heal quality residuals

The current refinement follows the size, feather, and resample controls in the
official [Adobe Lightroom Heal documentation](https://helpx.adobe.com/lightroom/desktop/using/heal-tool.html)
and uses a bounded deterministic candidate set rather than a global synthesis
pass. The research basis for later work is the primary
[PatchMatch paper](https://gfx.cs.princeton.edu/pubs/Barnes_2009_PAR/index.php),
[exemplar-based structure propagation](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/criminisi_tip2004.pdf),
and [Poisson image editing](https://legacy.sites.fas.harvard.edu/~cs278/papers/poisson.pdf).

- [x] Add defect fixtures for edge agreement, tone-shift seam reduction, ranked
  source determinism and wrapping, zero feather, and directional ramp
  continuation.
- [x] Expose adjustable feather and deterministic alternate-source refresh while
  preserving one undo step for the repair.
- [ ] Add an explicit manual source anchor only after pointer and keyboard
  interaction can expose its source-to-target relationship accessibly.
- [ ] Add a high-contrast Visualize Spots inspection mode only with real dust and
  low-contrast blemish fixtures that prove it improves discovery without changing
  pixels.
- [ ] Build a licensed small-repair corpus with hidden clean references and gate
  seam error, edge continuity, defect removal, latency, and peak memory. Do this
  before considering multi-patch synthesis or a gradient-domain blend.

Why these remain after display fidelity and release proof: automatic healing is
already useful and bounded, while manual sourcing and inspection add interaction
surface. They should land only when objective fixtures prove a quality gain and
the controls work equally with pointer, keyboard, and assistive technology.

### Phase 6: Support every format, the VLC of image viewers

The goal here is simple to state: if it is an image, viewr opens it, and the user
never has to think about which app handles which file. Formats are added in order
of how many people they serve and how safely they can be decoded.

- Pure-Rust formats covered by image-rs and friends: JPEG, PNG, GIF, WebP, BMP,
  TIFF, ICO, PNM, TGA, QOI, DDS, HDR, OpenEXR, farbfeld.
- Modern formats: AVIF, and JPEG XL via jxl-oxide.
- Vector: SVG via a pure-Rust renderer (resvg).
- High-value formats that need care: HEIC and HEIF, and camera RAW (Canon, Nikon,
  Sony, Fujifilm, and the rest), all decoded inside the sandboxed worker rather than
  linked into the main process.
- A clear, honest capability list in the docs stating exactly which formats are
  supported and which are decoded in isolation, so there are no surprises.

Every format added ships with golden-file decode tests and is added to the fuzz
corpus. Breadth never lowers the safety or coverage bar.

Definition of done: the supported-format list covers what ordinary people and
photographers actually have on disk, each format has decode tests, and opening any
of them just works.

### Phase 6 residuals (tracked)

- [x] SVG via pure-Rust `resvg` (shapes/paths; text shaping feature intentionally off to keep the trusted core lean).
- [x] Add `viewr-decode` as a workspace member with feature-gated C deps (`avif` / `heic` / `raw`; default empty for CI).
- [x] List AVIF/HEIC/RAW extensions in `fs` for browsing; decode routes through the worker.
- [x] RAW currently returns a stable, documented unsupported error instead of a
  false success claim.
- [ ] Implement and ship representative camera RAW families through the isolated
  worker.
- [ ] Add multi-page TIFF and ICO navigation instead of exposing only one decoded
  image.
- [x] Carry worker color metadata into the main process and test both sides of
  the boundary (`viewr-protocol` V2, release C-worker pixel/profile comparisons,
  and parent IPC-normalization/cancellation tests).
- [x] Honest format capability table: `docs/FORMATS.md`.

### Phase 7: Hardening and the privacy proof

Turn "we designed it to be private and safe" into something a third party can
verify **locally** (build, run, inspect). This phase is **not** about app-store
submission.

- [x] Sandbox *profiles* on all three platforms with the network denied (local
  profiles and runtime limits, not store listing).
- [x] Continuous fuzzing of every decoder, with any crash a release blocker.
- [x] Neighbor full-decode prefetch into a bounded in-memory LRU (no disk cache).
- [x] Default-silent logger configuration is isolated behind pure filter selection
  and construction seams.
- [x] Pristine reverse-navigation reuse and accepted-source integrity hardening.
- [x] Reproducibly buildable local/CI release artifacts.
- [x] Deletes use system Trash through the `trash` crate on Windows and Linux or
  `NSFileManager` on macOS, not a local `_trash` folder.

Definition of done: the app runs correctly with network denied by packaging
profile and/or process policy where implemented, fuzzing finds no crashes at the
decode boundary, and a release binary can be built and verified from this repo
without requiring third-party store accounts.

### Phase 8: 1.0, the viewer people recommend

Polish and **local-first** distribution so switching costs nothing for people who
install from source or a simple GitHub-style release artifact.

- [x] Local/CI install paths, file associations, documentation, release plumbing.
- [x] Performance budget locked in CI.
- [x] Public, checksummed, manifest-verified, and attested v0.1.0 artifacts.
- [ ] Manual screen-reader validation on Windows, macOS, and Linux with artifact-bound records.
- [ ] Display-fidelity acceptance from Priority 2 remains open.
- [ ] Publisher-authenticated native install surfaces once external signing and
  notarization credentials are available.

Definition of done: a careful user can build or download a release artifact, set
viewr as their image viewer if they choose, and never think about bloat again.
**Store shelves are not required for 1.0.**

## Distribution scope

Trusted direct installation is v0.9 release-candidate work. Store presence is not.

- [ ] Authenticode-sign direct Windows deliverables through a publicly trusted
  signing path.
- [ ] Developer ID-sign and notarize the direct macOS application or disk image,
  with hardened runtime and a stapled ticket.
- Microsoft Store MSIX / Partner Center publication
- Mac App Store publication
- Flathub (or other store) *publication* (local Flatpak *build* sketches may still
  exist for sandbox testing)

Signing credentials and notarization may require external accounts. They do not
block v0.2 through v0.8 development or an explicitly unsigned pre-1.0 archive,
but their absence remains visible and blocks v0.9 and a broadly recommended 1.0.

## Beyond 1.0, candidates held to the same bar

Listed so the answer to "will you add X" is "it is tracked and weighed," not
silence, and so that scope creep stays visible and deliberate.

- Optional, local-only, one-click-clearable recent folders.
- Simple non-destructive adjustments (lossless rotate, straighten, basic exposure),
  only if they stay simple and never turn viewr into an editor.
- A simple slideshow.
- Localization.
- A user-initiated **Check for Updates** command after a canonical release
  repository and signed release policy exist. It must never run at launch or in
  the background, and it must show the destination before opening a browser or
  downloading anything.
- Optional **Describe Image** after the offline bake-off and process-level privacy
  proof in `docs/LOCAL-INTELLIGENCE.md` pass on Windows, Linux, and macOS. It must
  be absent without a separately installed model pack, run **only** on explicit
  manual activation, receive decoded pixels rather than a source path, retain no
  result after navigation, and produce no app-owned logs or files. **Under no
  circumstances will it write descriptions to the file's EXIF data or a
  background database.** Built-in speech and model-assisted large-area removal
  remain separate later decisions.

## Explicit non-goals, the anti-bloat charter

viewr will not add, now or later: accounts, cloud sync, sharing services, ads,
discover or feed surfaces, face or AI grouping, background services, automatic or
background update checks, telemetry or analytics of any kind, or a plugin marketplace. These
are the features that turned every big-company photo app into the thing we are
replacing. Leaving them out is a permanent part of the product, not a stage of it.

An explicit one-image local model action does not relax this charter. Optional
models may not become a library scanner, automatic classifier, required runtime,
background process, download client, or reason to retain user data. **Adding
generated metadata to a user's files without explicit intent is spyware and is
an absolute non-starter.**
