# Publishing viewr

This is the maintainer checklist for `https://github.com/blisspixel/viewr`. It
separates repository readiness, the first public pre-1.0 release, and the stronger
trust bar required before 1.0.

## Current repository state

This checklist describes the current repository controls. Live branch protection,
workflow results, and release assets remain the source of truth:

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
- [x] Publish and verify annotated tag `v0.5.0`, the format-contract
  milestone, under that same contract. Published; see the GitHub release
  [v0.5.0](https://github.com/blisspixel/viewr/releases/tag/v0.5.0).
- [x] Publish and verify annotated tag `v0.6.0`, the integrated product-quality
  beta, under that same contract. Published; see the GitHub release
  [v0.6.0](https://github.com/blisspixel/viewr/releases/tag/v0.6.0). It was
  published without its required representative-hardware evidence: no such
  record exists for it, and its release notes state that limit.

## Version state policy

Four version states must not be conflated:

1. **Public version:** the newest immutable published tag and its assets. It is
   currently v0.6.0. README and INSTALL call this the install target until a
   later release actually exists.
2. **Workspace version:** the semantic version compiled into `viewr`, used in
   archive names, and recorded in `Cargo.toml` and `Cargo.lock`. It is currently
   `0.6.0` while the carried v0.6 hardware gate remains open.
3. **Candidate identity:** the full commit SHA plus one non-publishing
   `Release artifacts` workflow run. A candidate is not a public release, even
   when its workspace version matches the public version. Never identify it by
   archive name or version string alone.
4. **Intended tag:** the next milestone version after every prerequisite gate is
   closed. The next intended tag is v0.7.0, but it is not yet the workspace or
   public version.

Advance the workspace version once, before collecting evidence for the intended
tag. That reviewed release-preparation change updates `Cargo.toml`, `Cargo.lock`,
compiled version-specific commands, the changelog candidate content,
`docs/releases/v<version>.md`, and every status document that describes the
workspace. Do not use a changing `-dev` suffix or bump the version on ordinary
development commits. For v0.7.0, the update from `0.6.0` happens only after the
carried v0.6 product-quality matrix closes and before v0.7 accessibility evidence
begins.

Application source, dependencies, workflows, packaging, or user-facing behavior
instructions changed after a candidate run invalidate that candidate and all
evidence affected by it. Evidence records, actual release dates, public
release-status text, immutable-download links, and tests that pin those public
strings are status-only changes and may follow without resetting the candidate.
No other change is a status-only exception.

The final tag-ready change moves the candidate changelog content under the target
version and the date on which the tag operation is being performed, advances
README and INSTALL from the prior immutable release to the intended fixed-version
URLs, and marks the roadmap tag-ready without claiming publication. Require green
CI and fuzz on that exact commit, then tag that commit. If tagging does not happen
on the recorded date, correct the changelog and revalidate before tagging. This is
an execution record, not a forecast.

After publication, the intended tag becomes the public version. Verify the public
assets, then use a status-only follow-up to record the release link and advance
the roadmap to the next open gate. Planning documents use dependency order and
exit evidence, never calendar forecasts or duration estimates.

## Pre-1.0 release procedure

This is the procedure used for `v0.1.0` and repeated for each later tag. The
next allowed tag is `v0.7.0`, and it is blocked until the v0.6 hardware matrix
closes. An unsigned pre-1.0 release is acceptable only
when its trust boundary is explicit.
It must never be presented as signed, notarized, store-reviewed, or ready for every
production environment.

1. Close every prerequisite gate in the roadmap before changing the workspace
   version. For the next release, validate the carried v0.6 product-quality set
   before beginning v0.7 work:

   ```text
   python -B scripts/product_quality_evidence.py gate docs/release-evidence/product-quality/v0.6.0
   ```

2. On a feature branch, make the single version transition described in
   [Version state policy](#version-state-policy). Commit and review
   `docs/releases/v<version>.md` before building a candidate because the tag
   workflow uses that file as its only release body. Generated notes are not
   accepted. Keep README and INSTALL on the prior public release at this stage.
3. Run the complete local checks in [Verification](VERIFY.md). Confirm
   `THIRD_PARTY_LICENSES.txt` matches a fresh offline `cargo-about` render and
   the Flatpak source map matches the lockfile.
4. Integrate the release-preparation change through normal review. Select its
   exact clean `main` commit only after CI and fuzz pass there, and retain both
   run links.
5. Dispatch one non-publishing `Release artifacts` run for that commit. Complete
   every evidence set required by the target milestone against that one candidate.
   For v0.7.0 this is the Narrator, VoiceOver, and Orca matrix in
   [Accessibility](ACCESSIBILITY.md). For v0.8.0 and later, also complete every
   product-quality, install, update, rollback, packaging, and platform-trust gate
   assigned to that milestone by the roadmap.
6. If a gate fails, correct the defect, repeat the complete automated validation,
   produce a replacement candidate, and repeat every affected evidence row. If
   the candidate passes, integrate the evidence and other permitted status-only
   changes through normal review.
7. Prepare the final tag commit. Move the changelog candidate content under the
   target version and current release-operation date, advance README and INSTALL
   to the intended immutable URLs, mark the roadmap tag-ready, confirm the
   candidate remains valid, and require green CI and fuzz on the exact commit.
   The tag workflow rebuilds and verifies archives containing the final public
   documents.
8. Confirm the tag is exactly `v<workspace-version>`, then create and push an
   annotated tag:

   ```text
   git tag -a v0.7.0 -m "viewr 0.7.0"
   git push origin v0.7.0
   ```

   Use `git tag -s` when a configured signing identity is available. Do not weaken
   local Git verification merely to produce a signed tag.

9. Let the tag workflow rerun CI and fuzzing, build the exact four-target archive
   set, verify every archive and SHA-256 sidecar, add the fixed-version installer
   scripts and their sidecars, attest all 12 assets, and publish only that exact
   set. A manual workflow run is inspection-only and must never publish.
10. Verify the public release and attestations, then land the status-only roadmap
    and documentation update that records the immutable release and advances the
    immediate focus to the next gate.

## Release verification

After publication:

```text
gh release view v0.6.0 --repo blisspixel/viewr
gh release verify v0.6.0 --repo blisspixel/viewr
gh attestation verify viewr-0.6.0-x86_64-pc-windows-msvc.zip \
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

For v0.4.0, [main CI run 32310138360](https://github.com/blisspixel/viewr/actions/runs/32310138360)
passed all seven jobs and [fuzz run 32310138375](https://github.com/blisspixel/viewr/actions/runs/32310138375)
passed both targets on commit `645edcdcdaa441444cb4e3016b89d7bf19d428b7`.
[Release run 32310142370](https://github.com/blisspixel/viewr/actions/runs/32310142370)
published the immutable [v0.4.0 release](https://github.com/blisspixel/viewr/releases/tag/v0.4.0)
from that commit. Its Linux archive re-verifies from the release page with a
matching SHA-256 sidecar, a 38-file internal manifest, and one attestation
bound to `release.yml@refs/tags/v0.4.0` at that commit. The official archive
verifier accepts that download.

For v0.5.0, [main CI run 32333137825](https://github.com/blisspixel/viewr/actions/runs/32333137825)
passed all seven jobs and [fuzz run 32333137800](https://github.com/blisspixel/viewr/actions/runs/32333137800)
passed both targets on commit `1a1eec191a763c78e863664c38a725ceb57143e4`.
[Release run 32333672485](https://github.com/blisspixel/viewr/actions/runs/32333672485)
published the immutable [v0.5.0 release](https://github.com/blisspixel/viewr/releases/tag/v0.5.0)
from that commit. Its Linux archive re-verifies from the release page with a
matching SHA-256 sidecar, a 38-file internal manifest, and one attestation
bound to `release.yml@refs/tags/v0.5.0` at that commit. The official archive
verifier accepts that download.

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
- Close the integrated product-quality gate in [Roadmap](ROADMAP.md). Wide-gamut and HDR
  may remain later work if their unsupported state is explicit.

## Current limits

- v0.6.0 is public, immutable, checksummed, and attested, and v0.5.0, v0.4.0, v0.3.0, v0.2.0, v0.1.5, v0.1.4,
  v0.1.3, v0.1.2, v0.1.1, and v0.1.0 remain published, the first preview with a
  known-issues note. v0.6.0 additionally carries no representative-hardware
  acceptance evidence, which its release notes state. Their executable archives
  are not Authenticode-signed or Apple-notarized, so each release remains an
  explicitly unsigned pre-1.0 preview.
- The foreground installer tools contact only the official GitHub repository after
  the user runs them. They do not create an updater service or add network access
  to viewr.
- Human Narrator, VoiceOver, and Orca evidence remains governed by
  [Accessibility](ACCESSIBILITY.md).
