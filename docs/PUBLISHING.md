# Publishing viewr

This checklist prepares `https://github.com/blisspixel/viewr` without claiming that
the repository, a release, signing, or hosted evidence already exists.

## First publication

1. Create the public GitHub repository with `main` as the default branch.
2. Add and verify the remote:

   ```text
   git remote add origin https://github.com/blisspixel/viewr.git
   git remote -v
   git push -u origin main
   ```

3. Enable private vulnerability reporting under Settings > Security > Code security.
4. Enable Dependabot alerts and security updates.
5. Protect `main`. Require pull requests, dismiss stale approvals after new commits,
   require conversation resolution, block force pushes and deletion, and require the
   repository's CI, coverage, supply-chain, fuzz, and platform-profile checks.
6. Keep the default GitHub Actions token read-only. The release workflow grants
   `contents: write`, `id-token: write`, and `attestations: write` only to its final
   tag-only publication job.
7. Enable immutable releases. A release is assembled as a draft, receives all
   verified assets, and is published only after provenance attestations succeed.
8. Verify the repository description, website, topics, Apache-2.0 license detection,
   issue forms, security policy, and documentation links on GitHub.

## Release preparation

1. Confirm `main` is clean and synchronized with `origin/main`.
2. Update the workspace version, CHANGELOG, current README claims, and relevant docs.
3. Run the complete local checks in [VERIFY.md](VERIFY.md).
4. Confirm `THIRD_PARTY_LICENSES.html` matches a fresh offline `cargo-about` render.
5. Confirm the tag will be exactly `v<workspace-version>`.
6. Create and push an annotated tag:

   ```text
   git tag -a v0.1.0 -m "viewr 0.1.0"
   git push origin v0.1.0
   ```

   Use `git tag -s` instead when a configured signing identity is available. Do not
   weaken local Git verification merely to produce a signed tag.

The tag workflow reruns CI and fuzzing, builds four target archives, verifies every
archive and SHA-256 sidecar as one exact set, creates GitHub build-provenance
attestations, uploads the set to a draft release, and publishes that release. A
manual workflow run builds inspection artifacts but never publishes a release.

## Release verification

After publication:

```text
gh release view v0.1.0 --repo blisspixel/viewr
gh release verify v0.1.0 --repo blisspixel/viewr
gh attestation verify viewr-0.1.0-x86_64-pc-windows-msvc.zip \
  --repo blisspixel/viewr
```

Download at least one archive and its sidecar, run
`python scripts/release_artifact.py verify <archive>`, and verify the installed
binary with `viewr --version` and `viewr doctor`.

## Current limits

- Release archives are portable and checksummed. They are not code-signed Windows
  installers, notarized macOS applications, or store packages.
- The one-command installers perform explicit foreground downloads from the official
  GitHub release only. They do not create an updater service or enable background
  network access in viewr.
- Human Narrator, VoiceOver, and Orca evidence remains governed by
  [ACCESSIBILITY.md](ACCESSIBILITY.md).
