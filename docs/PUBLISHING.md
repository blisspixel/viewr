# Publishing viewr

This is the maintainer checklist for `https://github.com/blisspixel/viewr`. It
separates repository readiness, the first public pre-1.0 release, and the stronger
trust bar required before 1.0.

## Current repository state

Status last verified on 2026-08-18:

- [x] The repository is public with `main` as its only long-lived branch and
  Apache-2.0 detected by GitHub.
- [x] The description, issue forms, pull-request template, CODEOWNERS, security
  policy, documentation links, and focused repository topics are present.
- [x] Private vulnerability reporting, Dependabot alerts and security updates,
  secret scanning, and push protection are enabled.
- [x] GitHub Actions uses read-only default token permissions. Only the final
  tag-only publication job receives `contents: write`, `id-token: write`, and
  `attestations: write`.
- [x] Merged branches are deleted automatically. Dependabot limits each update lane
  to one grouped patch proposal.
- [x] `main` requires the seven stable CI checks, linear history, one code-owner
  approval with stale-review dismissal and last-push approval, and conversation
  resolution. Force pushes and deletion are blocked. Administrators retain an
  explicit emergency bypass; path-filtered fuzz remains a release-workflow gate.
- [x] Immutable releases are enabled before the first tag is published.
- [x] Publish and verify annotated tag `v0.1.0`. The release is immutable, contains
  the exact 12 expected assets, and has one GitHub release attestation covering
  every asset.
- [x] Publish and verify annotated tag `v0.1.1`, the patch release that makes a
  failed launch observable, under the same immutable 12-asset contract.
- [x] Publish and verify annotated tag `v0.1.2`, the patch release that resolves
  the windowing backend and reports the graphics runtime, under that same
  contract.
- [x] Publish and verify annotated tag `v0.1.3`, the patch release that restores
  OpenGL presentation, under that same contract.
- [x] Publish and verify annotated tag `v0.1.4`, the patch release that fits the
  first window to the monitor it opens on, under that same contract.
- [x] Publish and verify annotated tag `v0.1.5`, the patch release that stops a
  malformed JPEG XL file from terminating the viewer and opens a folder handed
  to viewr from outside the window, under that same contract.
- [x] Publish and verify annotated tag `v0.2.0`, the reliability architecture
  milestone, under that same contract. Published; see the GitHub release
  [v0.2.0](https://github.com/blisspixel/viewr/releases/tag/v0.2.0).
- [x] Publish and verify annotated tag `v0.3.0`, the display-correct SDR
  milestone, under that same contract. Published; see the GitHub release
  [v0.3.0](https://github.com/blisspixel/viewr/releases/tag/v0.3.0).
- [x] Publish and verify annotated tag `v0.4.0`, the file-coherence
  milestone, under that same contract. Published; see the GitHub release
  [v0.4.0](https://github.com/blisspixel/viewr/releases/tag/v0.4.0).

## Pre-1.0 release procedure

This is the procedure used for `v0.1.0` and repeated for each later tag. The
commands below are the worked `v0.4.0` example. The next allowed tag is
`v0.5.0`. An unsigned pre-1.0 release is acceptable only when its trust boundary is explicit.
It must never be presented as signed, notarized, store-reviewed, or ready for every
production environment.

1. Select one clean `main` commit and retain its green CI and fuzz links.
2. Update the workspace version, CHANGELOG, README status, roadmap gate, and all
   behavior-specific documentation.
3. Run the complete local checks in [Verification](VERIFY.md).
4. Confirm `THIRD_PARTY_LICENSES.txt` matches a fresh offline `cargo-about`
   render and the Flatpak source map matches the lockfile.
5. Confirm the tag is exactly `v<workspace-version>` and create an annotated tag:

   ```text
   git tag -a v0.4.0 -m "viewr 0.4.0"
   git push origin v0.4.0
   ```

   Use `git tag -s` when a configured signing identity is available. Do not weaken
   local Git verification merely to produce a signed tag.

6. Commit and review `docs/releases/v<version>.md`. It is the only release body;
   generated notes are not accepted.
7. Let the tag workflow rerun CI and fuzzing, build the exact four-target archive
   set, verify every archive and SHA-256 sidecar, add the fixed-version installer
   scripts and their sidecars, attest all 12 assets, and publish only that exact
   set. A manual workflow run is inspection-only and must never publish.

## Release verification

After publication:

```text
gh release view v0.4.0 --repo blisspixel/viewr
gh release verify v0.4.0 --repo blisspixel/viewr
gh attestation verify viewr-0.4.0-x86_64-pc-windows-msvc.zip \
  --repo blisspixel/viewr
```

`gh release verify` and `gh attestation` need GitHub CLI 2.49 or newer. Older
builds have neither subcommand, and the sidecar plus the archive's internal
per-file manifest remain the available integrity evidence.

Download at least one archive and its sidecar, then run:

```text
python scripts/release_artifact.py verify <archive>
viewr --version
viewr doctor
```

Exercise a clean install, same-version reinstall, update, application launch, file
open, explicit uninstall, and rollback from an injected activation failure. Verify
that the installed main executable and worker belong to the same release.

For v0.1.0, [main CI run 30642307317](https://github.com/blisspixel/viewr/actions/runs/30642307317)
passed all seven jobs and [fuzz run 30642307463](https://github.com/blisspixel/viewr/actions/runs/30642307463)
passed both targets on commit `86d3eef920ec5e523fbc6dbc286c4dcbd68e7f1b`.
[Release run 30643016336](https://github.com/blisspixel/viewr/actions/runs/30643016336)
then repeated the complete gates, built all four archives, verified checksums and
manifests, attested all 12 assets, and published the immutable
[v0.1.0 release](https://github.com/blisspixel/viewr/releases/tag/v0.1.0).

For v0.1.1, [main CI run 31896435500](https://github.com/blisspixel/viewr/actions/runs/31896435500)
passed all seven jobs and [fuzz run 31896435451](https://github.com/blisspixel/viewr/actions/runs/31896435451)
passed both targets on commit `cca11a28d101cb1bc28a903530189adde307d1cb`.
[Release run 31897338683](https://github.com/blisspixel/viewr/actions/runs/31897338683)
repeated the complete gates, built all four archives, verified checksums and
manifests, attested all 12 assets, and published the immutable
[v0.1.1 release](https://github.com/blisspixel/viewr/releases/tag/v0.1.1). The
published Linux archive was then re-verified from the release page: matching
SHA-256 sidecar, 30-file internal manifest, and one attestation binding it to
`release.yml@refs/tags/v0.1.1` at that commit.

For v0.1.2, [main CI run 31902602684](https://github.com/blisspixel/viewr/actions/runs/31902602684)
passed all seven jobs and [fuzz run 31902602696](https://github.com/blisspixel/viewr/actions/runs/31902602696)
passed both targets on commit `0b44b14544ea97a2fb1acae00b597372f1213757`.
[Release run 31903069276](https://github.com/blisspixel/viewr/actions/runs/31903069276)
repeated the complete gates and published the immutable
[v0.1.2 release](https://github.com/blisspixel/viewr/releases/tag/v0.1.2), whose
Linux archive re-verifies from the release page with a matching sidecar and one
attestation bound to `release.yml@refs/tags/v0.1.2` at that commit.

For v0.1.3, [main CI run 31959125360](https://github.com/blisspixel/viewr/actions/runs/31959125360)
passed all seven jobs and [fuzz run 31959125356](https://github.com/blisspixel/viewr/actions/runs/31959125356)
passed both targets on commit `2e3fb5522610cf8e212cc3fe69159d6e71923791`.
[Release run 31959585677](https://github.com/blisspixel/viewr/actions/runs/31959585677)
repeated the complete gates and published the immutable
[v0.1.3 release](https://github.com/blisspixel/viewr/releases/tag/v0.1.3), whose
Linux archive re-verifies from the release page with a matching sidecar and one
attestation bound to `release.yml@refs/tags/v0.1.3` at that commit.

For v0.1.4, [main CI run 32049375756](https://github.com/blisspixel/viewr/actions/runs/32049375756)
passed all seven jobs and [fuzz run 32049375806](https://github.com/blisspixel/viewr/actions/runs/32049375806)
passed both targets on commit `414651b76c7d40f931842cada05faa281bdcb6f8`.
[Release run 32050640235](https://github.com/blisspixel/viewr/actions/runs/32050640235)
repeated the complete gates and published the immutable
[v0.1.4 release](https://github.com/blisspixel/viewr/releases/tag/v0.1.4), whose
Linux archive re-verifies from the release page with a matching sidecar, a
33-file internal manifest, and one attestation bound to
`release.yml@refs/tags/v0.1.4` at that commit.

For v0.1.5, [main CI run 32077320223](https://github.com/blisspixel/viewr/actions/runs/32077320223)
passed all seven jobs and [fuzz run 32077320239](https://github.com/blisspixel/viewr/actions/runs/32077320239)
passed both targets on commit `edc80fd76f230baf7203a4c7002d4e917f7ccbf2`.
[Release run 32077953927](https://github.com/blisspixel/viewr/actions/runs/32077953927)
repeated the complete gates and published the immutable
[v0.1.5 release](https://github.com/blisspixel/viewr/releases/tag/v0.1.5). Its
Linux archive re-verifies from the release page with a matching SHA-256 sidecar,
a 34-file internal manifest, and one attestation bound to
`release.yml@refs/tags/v0.1.5` at that commit. The release rerun of the coverage
job first stalled for one hour and fifty-one minutes inside `apt-get install`
and was cancelled and rerun, completing in two minutes; every CI job now carries
an explicit timeout so a stalled runner fails rather than holding a tag open.

For v0.2.0, [main CI run 32153785138](https://github.com/blisspixel/viewr/actions/runs/32153785138)
passed all seven jobs and [fuzz run 32153785164](https://github.com/blisspixel/viewr/actions/runs/32153785164)
passed both targets on commit `183970282eae7e7698c46e9b4df65384055b2056`.
[Release run 32154781070](https://github.com/blisspixel/viewr/actions/runs/32154781070)
published the immutable [v0.2.0 release](https://github.com/blisspixel/viewr/releases/tag/v0.2.0)
on its second attempt after the first attempt lost three jobs at the apt-get
step. Its Linux archive re-verifies from the release page with a matching
SHA-256 sidecar, a 35-file internal manifest, and one attestation bound to
`release.yml@refs/tags/v0.2.0` at that commit. The official archive verifier
accepts that download.

For v0.3.0, [main CI run 32281431906](https://github.com/blisspixel/viewr/actions/runs/32281431906)
passed all seven jobs and [fuzz run 32281431889](https://github.com/blisspixel/viewr/actions/runs/32281431889)
passed both targets on commit `4cbcca1450f90cdb0061e890c4aa9c2cd9750205`.
[Release run 32282658062](https://github.com/blisspixel/viewr/actions/runs/32282658062)
published the immutable [v0.3.0 release](https://github.com/blisspixel/viewr/releases/tag/v0.3.0)
from that commit. Its Linux archive re-verifies from the release page with a
matching SHA-256 sidecar, a 36-file internal manifest, and one attestation
bound to `release.yml@refs/tags/v0.3.0` at that commit. The official archive
verifier accepts that download.

For v0.4.0, [main CI run 32306011198](https://github.com/blisspixel/viewr/actions/runs/32306011198)
passed all seven jobs and [fuzz run 32306011193](https://github.com/blisspixel/viewr/actions/runs/32306011193)
passed both targets on behavior commit `7657a74`. The tag workflow publishes
the immutable [v0.4.0 release](https://github.com/blisspixel/viewr/releases/tag/v0.4.0)
from the versioned source.

## Required before a broadly recommended 1.0

The [version path in the roadmap](ROADMAP.md#order-of-operations-to-10)
owns the dependency order. This checklist summarizes the final distribution and
acceptance conditions; it does not replace the intermediate reliability,
fidelity, coherence, and release-candidate gates.

- Complete Narrator, VoiceOver, and Orca evidence from
  [Accessibility](ACCESSIBILITY.md), bound to exact artifact hashes.
- Authenticode-sign the Windows executables and installer through a publicly trusted
  path described by [Microsoft's code-signing guidance](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options),
  then test every installed binary under Smart App Control.
- Developer ID-sign the macOS application, enable hardened runtime, follow
  [Apple's notarization workflow](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution),
  staple the ticket, and test Gatekeeper installation.
- Verify a normal Linux Flatpak or equivalent package on representative Wayland and
  X11 desktops without weakening the network-denied sandbox contract.
- Complete cold-launch, animation, large-image, mixed-DPI, multi-monitor,
  profiled-display, update, and uninstall acceptance on representative hardware.
- Close the format-contract gate in [Roadmap](ROADMAP.md). Wide-gamut and HDR
  may remain later work if their unsupported state is explicit.

## Current limits

- v0.4.0 is public, immutable, checksummed, and attested, and v0.3.0, v0.2.0, v0.1.5, v0.1.4,
  v0.1.3, v0.1.2, v0.1.1, and v0.1.0 remain published, the first preview with a
  known-issues note. Their executable archives
  are not Authenticode-signed or Apple-notarized, so each release remains an
  explicitly unsigned pre-1.0 preview.
- The foreground installer tools contact only the official GitHub repository after
  the user runs them. They do not create an updater service or add network access
  to viewr.
- Human Narrator, VoiceOver, and Orca evidence remains governed by
  [Accessibility](ACCESSIBILITY.md).
