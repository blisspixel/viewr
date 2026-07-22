# Installing viewr

## Current availability

viewr does not yet publish signed installers, GitHub Releases, Homebrew formulas,
Flathub listings, Windows Store packages, or Arch packages. Commands that imply
those channels exist would be unsafe and misleading, so this document only covers
paths that can be reproduced from the current repository.

The supported paths today are a source build, a locally verified sandbox-profile
artifact, or a checksummed dual-binary release archive. The CI release workflow
retains the same archives as workflow artifacts; it deliberately does not create
a public release.

## Build from source (any OS)

With a Rust toolchain installed (see `rust-toolchain.toml`):

```
cargo build --release --workspace --locked
# Binaries land in target/release/viewr and target/release/viewr-decode
# Keep them side by side so C-backed formats can spawn the worker.
```

### CLI (local tools, no network)

```
viewr help
viewr doctor              # layout, worker, decode self-test
viewr benchmark [dir]     # decode timings (temp corpus if dir omitted)
viewr update              # print local update instructions only
viewr version
viewr path\to\image.jpg   # open GUI
```

`viewr update` never downloads anything; viewr does not phone home.

Optional C-backed formats (needs system libraries):

```
cargo build --release -p viewr-decode --features avif,heic
```

On Linux, building needs the usual windowing dev packages
(`libwayland-dev libxkbcommon-dev libx11-dev`).

### Inspect the network-denied package profiles

The repository keeps platform package boundaries independently verifiable before
public installers exist. Run the cross-platform exact-set test with:

```
cargo test -p viewr --test sandbox_profiles
```

Platform-native verification commands and their limits are documented in
[`SANDBOX_PLAN.md`](SANDBOX_PLAN.md). They produce only local unsigned or ad-hoc
signed artifacts under `target/profile-check/`; they do not publish or install
anything.

## Desktop integration on Linux

The launcher entry and MIME associations live in `assets/linux/viewr.desktop`, and
the app icon is `assets/icon.svg`. Packaging installs them into the standard XDG
locations so viewr appears in menus and as an "Open with" choice. For a local
source build you can install them by hand:

```
install -Dm644 assets/linux/viewr.desktop ~/.local/share/applications/viewr.desktop
install -Dm644 assets/icon.svg ~/.local/share/icons/hicolor/scalable/apps/viewr.svg
update-desktop-database ~/.local/share/applications
```

## Build and verify a release archive

The release archive contains the main executable and `viewr-decode` side by side,
plus the license, README, and a canonical file manifest. The packaging tool checks
that both executable formats match the requested target, normalizes text files,
uses a commit-derived `SOURCE_DATE_EPOCH`, stores deterministic ZIP metadata, and
writes a standard SHA-256 sidecar.

Build the locked workspace with the pinned toolchain, then package the native
target. On Windows x86-64:

```powershell
cargo build --release --workspace --locked
python scripts/release_artifact.py build `
  --target x86_64-pc-windows-msvc `
  --binary-dir target/release
python scripts/release_artifact.py verify `
  target/release-artifacts/viewr-0.0.0-x86_64-pc-windows-msvc.zip
```

On Linux x86-64, Intel macOS, or Apple Silicon macOS, replace the target with one
of the following and point `--binary-dir` at the matching Cargo output:

- `x86_64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

For an explicit target build, Cargo writes binaries beneath
`target/<target>/release`. The workspace version in `Cargo.toml` determines the
archive name; do not copy the example version blindly after it changes.

Phase 7 CI archives use the workspace's default pure-Rust feature set. They do
not claim AVIF, HEIC, or RAW support from optional C backends. AVIF/HEIC builds
still require their native toolchain and libheif dependencies; on Linux, those
features activate the tested default-deny policy documented in
`packaging/linux/SECCOMP.md`.

The tag-triggered `.github/workflows/release.yml` repeats this contract for all
four targets. A tag must equal `v<workspace-version>` or packaging fails closed.
Manual workflow runs are allowed for pre-release verification. The workflow has
read-only repository permission and retains archives for inspection; it does not
publish, sign, notarize, install, or create a GitHub Release. Artifact jobs wait
for the repository's complete reusable CI and short fuzz workflows, including
coverage, supply-chain, privacy, and native package-profile checks.

The archive is reproducible from the same checked-out source, target, compiler,
lockfile, and binary inputs. The manifest and sidecar make every produced byte
verifiable. This is not a claim that different operating-system images or linker
versions produce identical executables.

## What current build and archive paths do not do

No account. No background service. No auto-update daemon. No telemetry opt-in
screen. Current release archives are portable files and do not register file
associations or modify the operating system.
