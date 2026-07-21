# Installing viewr

The goal is that getting viewr is one step on any operating system, and that it
behaves like a native app once installed: it opens when you double-click an image,
it shows up in your app menu, and it never asks anything of you.

## For users

Once releases are published, installing is a single command.

### macOS

```
brew install viewr
```

Or download the `.dmg` from the releases page and drag viewr to Applications. The
build is signed and notarized, so macOS opens it without warnings.

### Windows

Download and run the `.msi` installer from the releases page, or:

```powershell
irm https://github.com/viewr/viewr/releases/latest/download/viewr-installer.ps1 | iex
```

The installer registers viewr as an option in "Open with" and adds a Start menu
shortcut. It does not set itself as your default viewer without asking.

### Linux

```
flatpak install viewr
```

Or, on Arch:

```
paru -S viewr        # or your AUR helper of choice
```

Or the portable installer:

```
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/viewr/viewr/releases/latest/download/viewr-installer.sh | sh
```

The Flatpak ships with no network permission at all (see `docs/PRIVACY.md`), so it
is structurally unable to reach the internet.

### From source (any OS)

With a Rust toolchain installed (see `rust-toolchain.toml`):

```
cargo build --release --workspace
# Binaries land in target/release/viewr and target/release/viewr-decode
# Keep them side by side so C-backed formats can spawn the worker.
```

Optional C-backed formats (needs system libraries):

```
cargo build --release -p viewr-decode --features avif,heic
```

On Linux, building needs the usual windowing dev packages
(`libwayland-dev libxkbcommon-dev libx11-dev`).

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

## For maintainers: cutting a release

Installers and binaries for all three operating systems are produced by
[`dist`](https://github.com/axodotdev/cargo-dist) from a single tag. The seed
configuration is in the root `Cargo.toml` under `[workspace.metadata.dist]`
(targets, installer types, no auto-updater). To set it up once:

```
cargo install cargo-dist
dist init            # pins the tool version and generates the release workflow
git add . && git commit -m "ci: set up dist"
```

Then every release is:

```
git tag v0.1.0
git push --tags      # the release workflow builds the .dmg, .msi, shell/ps1
                     # installers, and checksummed archives, and attaches them
```

Signing and notarization (macOS) and code signing (Windows) use secrets configured
in the CI environment; those are the only manual maintainer setup beyond `dist
init`. viewr's installers deliberately include no auto-updater: the app never
contacts a server on its own, and updates come through the same channels users
already trust (their package manager or a download they choose to run).

## What installing does not do

No account. No background service. No auto-update daemon. No telemetry opt-in
screen. Installing viewr adds an app and its file associations, and nothing else
runs when the app is closed.
