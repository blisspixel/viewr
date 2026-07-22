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

### Performance regression gate

The internal GUI probe is intentionally absent from the user-facing help surface.
Developers and CI run the complete release-binary contract through the wrapper:

```text
cargo build --release --workspace --locked
python -B scripts/performance_gate.py --binary target/release/viewr
```

On Windows, add `--no-xvfb` and use a console-enabled debug binary for local
output. Budgets, corpus shape, and interpretation are in
[`PERFORMANCE.md`](PERFORMANCE.md).

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

## Opt-in desktop integration

Packaging declares only the formats supported by the default pure-Rust build.
Optional AVIF, HEIC, HEIF, and RAW worker formats are not advertised until a
future package can prove those decoders are present. None of the paths below
changes the user's default image viewer during installation.

### Linux

The launcher entry and MIME associations live in `assets/linux/viewr.desktop`, and
the app icon is `assets/icon.svg`. The Flatpak profile installs both under the
application ID `com.github.blisspixel.viewr`. For a local source build, keep the
two executables together and install the same desktop assets into the user's XDG
locations:

```
install -Dm755 target/release/viewr ~/.local/bin/viewr
install -Dm755 target/release/viewr-decode ~/.local/bin/viewr-decode
install -Dm644 assets/linux/viewr.desktop ~/.local/share/applications/com.github.blisspixel.viewr.desktop
install -Dm644 assets/icon.svg ~/.local/share/icons/hicolor/scalable/apps/com.github.blisspixel.viewr.svg
update-desktop-database ~/.local/share/applications
```

The desktop entry uses a single-file `%f` launch because one viewr window opens
one selected image. Installing it only adds viewr to Open With menus. To opt in
as the default for a format, use the desktop environment's Open With dialog and
choose its remember or default option. The equivalent explicit command for JPEG
is:

```
xdg-mime default com.github.blisspixel.viewr.desktop image/jpeg
```

Run that command only for MIME types the user deliberately chooses. Removing
the two binaries and the two application-ID files above unregisters the local
source install; it does not change or delete photos.

### macOS

Launch Services associations require an application bundle. Build the release
binaries, create the locally ad-hoc-signed sandbox bundle, and copy the bundle to
the per-user Applications folder:

```
cargo build --release --workspace --locked
bash scripts/build-macos-sandboxed-app.sh target/release
mkdir -p "$HOME/Applications"
cp -R target/profile-check/macos/viewr.app "$HOME/Applications/viewr.app"
```

The bundle declares viewr as an alternate viewer for the core extension set.
Finder delivers selected files through Launch Services, which viewr handles
without relying on argv. To make viewr the default for a format, select a file in
Finder, open Get Info, choose viewr under Open with, and use Change All. Remove
the app bundle through Finder to uninstall this local build.

This is a local, ad-hoc-signed development bundle. It is not notarized and is not
presented as a public distribution artifact.

### Windows

Keep `viewr.exe` and `viewr-decode.exe` side by side in a stable folder chosen by
the user. Right-click an image, select Open with, then Choose another app and
Choose an app on your PC, and select `viewr.exe`. Select Always only when the
user intends to change that extension's default.

The AppContainer manifest declares the same core extension set and remains
capability-free. The repository can schema-validate an unsigned MSIX with:

```powershell
cargo build --workspace --locked
.\scripts\build-windows-appcontainer.ps1 -BinaryDirectory target\debug
```

The resulting package is an inspection artifact, not an installable public
release. It does not justify importing a signing certificate or weakening local
Windows policy. A portable source build can be uninstalled by removing its two
binaries after choosing another default viewer if necessary.

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

CI archives use the workspace's default pure-Rust feature set. They do
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
associations or modify the operating system. The source-built platform bundles
and desktop entry make viewr an available handler, but changing a default always
requires an explicit user choice.

## GPU-driver overlays

viewr requests the operating system's low-power graphics adapter, but the platform
and graphics driver make the final adapter choice. NVIDIA App, GeForce Experience,
and similar driver software may display their own overlay when a new GPU-backed
application starts. That overlay is not rendered by viewr, and viewr does not use
vendor APIs or install an overlay. Disable it in the graphics vendor's own overlay
settings if desired; viewr does not attempt unsupported suppression tricks.
