# Installing viewr

viewr is pre-1.0. v0.1.2 is the current public GitHub Release, a patch over the
first public release v0.1.0. Its portable
archives are checksummed, manifest-verified, and attested, but the Windows
artifacts are not Authenticode-signed and the macOS artifacts are not Developer
ID-signed or notarized. Normal operating-system trust warnings may appear.
Installation is per user and never requires elevation.

## One-command install and update

### Windows 10 or 11, x64

```powershell
irm https://github.com/blisspixel/viewr/releases/download/v0.1.2/install.ps1 | iex
```

The installer:

- resolves the latest stable release through the GitHub Releases API;
- accepts only the expected `blisspixel/viewr` asset URL;
- verifies the archive SHA-256 sidecar, safe ZIP paths, release identity, manifest,
  size, and per-file hashes;
- replaces only an installer-owned directory, with rollback if activation fails;
- installs under `%LOCALAPPDATA%\Programs\viewr`;
- adds that directory to the user PATH and creates a Start menu shortcut.

Close viewr before updating. Windows will not replace a running executable. Run the
same command again for an explicit update. viewr itself never checks in the
background.

To install a specific version from a reviewed local copy of the script:

```powershell
irm https://github.com/blisspixel/viewr/releases/download/v0.1.2/install.ps1 `
  -OutFile $env:TEMP\viewr-install.ps1
& $env:TEMP\viewr-install.ps1 -Version 0.1.2
```

`-NoPath` skips the user PATH change, and `-NoShortcut` skips the Start menu
shortcut. `-InstallDir` is accepted only inside the current user's
`%LOCALAPPDATA%\Programs` directory.

### macOS and Linux

```sh
curl -fsSL https://github.com/blisspixel/viewr/releases/download/v0.1.2/install.sh | sh
```

The shell installer:

- resolves the latest stable release through the official GitHub release redirect;
- downloads only the archive and sidecar for the detected supported target;
- requires HTTPS with TLS 1.2 or newer and verifies SHA-256 before extraction;
- rejects duplicate, traversal, absolute, and unexpected archive paths;
- installs versioned binaries under `${XDG_DATA_HOME:-$HOME/.local/share}/viewr`;
- atomically updates an installer-owned symlink in `~/.local/bin`;
- installs the desktop entry and icon on Linux without changing default handlers.

Supported release targets are macOS Intel, macOS Apple Silicon, and Linux x86-64
glibc. Linux ARM64 and musl users must build from source for now.

To pin a release or override the user-local locations:

```sh
VIEWR_VERSION=0.1.2 \
VIEWR_INSTALL_ROOT="$HOME/.local/share/viewr" \
VIEWR_BIN_DIR="$HOME/.local/bin" \
sh install.sh
```

If `~/.local/bin` is not already on PATH, add it through the shell's normal profile
configuration. The installer reports this without editing profile files.

## Review before running

Pipe-to-shell commands are convenient but execute installer code. The commands
above are fixed to the immutable `v0.1.2` release rather than a moving branch.
To review it first:

```sh
curl -fsSLO https://github.com/blisspixel/viewr/releases/download/v0.1.2/install.sh
less install.sh
sh install.sh
```

On Windows, use the two-step version-pinning example above and inspect the downloaded
file before invoking it. The installer performs foreground network requests only
after the user runs it. The installed application has no HTTP or TLS client and
does not inherit installer network capability.

If the requested release or its verification material cannot be resolved, the
installer stops without changing the machine.

## Manual release installation

Download the archive and matching `.sha256` file from the
[official releases page](https://github.com/blisspixel/viewr/releases). Release
names are:

- `viewr-<version>-x86_64-pc-windows-msvc.zip`
- `viewr-<version>-x86_64-unknown-linux-gnu.zip`
- `viewr-<version>-x86_64-apple-darwin.zip`
- `viewr-<version>-aarch64-apple-darwin.zip`

Verify the sidecar before extraction. If GitHub CLI 2.49 or newer is installed,
also verify build provenance. Older `gh` builds do not have the `attestation`
command, in which case the sidecar and the internal per-file manifest remain the
available integrity evidence:

```text
gh attestation verify <archive> --repo blisspixel/viewr
```

Extract the archive and keep `bin/viewr` and `bin/viewr-decode` side by side. The
archive also contains the project license, notice, third-party license inventory,
security policy, canonical documentation, and a per-file release manifest.

GitHub checksums and attestations improve integrity and provenance. The v0.1.2
portable archives are not Authenticode-signed or Apple-notarized, so
operating-system trust dialogs may still apply. Do not disable platform security
controls to force a launch.

## Build from source

Install the Rust toolchain selected by `rust-toolchain.toml`, clone the repository,
and run:

```text
git clone https://github.com/blisspixel/viewr.git
cd viewr
cargo build --release --workspace --locked
```

The binaries are written to `target/release`. Keep the main executable and worker
from the same build together. On Linux, the build also needs the normal Wayland/X11
development packages listed in `.github/workflows/ci.yml`.

Optional AVIF and HEIC support requires native libheif dependencies and a separate
worker build:

```text
cargo build --release -p viewr-decode --features avif,heic
```

Default public archives use the pure-Rust feature set. They do not claim optional
C-backed formats or RAW support.

## Platform integration

### Windows

The installer creates a Start menu shortcut but does not change file associations.
Use an image's Open with dialog to select `viewr.exe`, and choose Always only for
extensions you deliberately want viewr to own.

The capability-free AppContainer package remains a locally validated profile, not
a signed public installer. Maintainers can validate it with:

```powershell
cargo build --workspace --locked
.\scripts\build-windows-appcontainer.ps1 -BinaryDirectory target\debug
```

### Linux

Portable Linux archives link the C runtime only. The windowing stack loads its
keyboard and display libraries at run time, so `ldd bin/viewr` does not list
them and a missing package appears only when a window is opened. A desktop
session needs:

| Session | Libraries | Debian or Ubuntu | Fedora or RHEL | Arch |
| --- | --- | --- | --- | --- |
| X11 | `libxkbcommon.so.0`, `libxkbcommon-x11.so.0`, `libX11.so.6` | `libxkbcommon0 libxkbcommon-x11-0 libx11-6` | `libxkbcommon libxkbcommon-x11 libX11` | `libxkbcommon libxkbcommon-x11 libx11` |
| Wayland | `libxkbcommon.so.0`, `libwayland-client.so.0` | `libxkbcommon0 libwayland-client0` | `libxkbcommon libwayland-client` | `libxkbcommon wayland` |

Presenting images needs one of the two graphics runtimes viewr renders through,
also loaded at run time. Mesa's DRI drivers alone are not enough, because the
OpenGL backend reaches them through EGL:

| Runtime | Library | Debian or Ubuntu | Fedora or RHEL | Arch |
| --- | --- | --- | --- | --- |
| OpenGL, including Mesa software rendering | `libEGL.so.1` | `libegl1 libegl-mesa0` | `mesa-libEGL` | `mesa` |
| Vulkan | `libvulkan.so.1` plus an installed driver | `mesa-vulkan-drivers` | `mesa-vulkan-drivers` | `vulkan-swrast` |

The Vulkan loader is packaged separately from every driver, so a host can have
`libvulkan.so.1` and still enumerate nothing. `viewr doctor` reports that state
as a loader without a driver rather than as a working runtime.

Most desktop installations already have all of this. Minimal containers, remote
X hosts, and virtual machines often do not.

`viewr doctor` checks the windowing libraries for the current session and both
graphics runtimes, and names the package to install when one is missing.
Launching without them prints the same guidance and exits non-zero rather than
aborting inside the dynamic loader or failing without a reason.

`WAYLAND_DISPLAY` left over from an earlier session names a compositor that is
not running. viewr uses the X server named by `DISPLAY` in that case rather than
failing, and says so in the doctor report.

The installer registers `com.github.blisspixel.viewr.desktop` as an available image
viewer. It never changes a default. To opt in for JPEG explicitly:

```sh
xdg-mime default com.github.blisspixel.viewr.desktop image/jpeg
```

Run similar commands only for MIME types you deliberately choose. Linux startup
requires standard seccomp support and local Unix D-Bus for accessibility. It exits
rather than running with a weakened network boundary.

### macOS

The portable command launches the GUI but is not a notarized application bundle.
Developers can build the local ad-hoc-signed sandbox bundle with:

```sh
cargo build --release --workspace --locked
bash scripts/build-macos-sandboxed-app.sh target/release
```

This local bundle is for validation only. Do not present it as a notarized public
distribution.

## Uninstall

Close viewr and change any file-type defaults first.

On Windows, remove the installer-owned
`%LOCALAPPDATA%\Programs\viewr` directory, remove that exact directory from the user
PATH, and remove the `viewr` Start menu shortcut. Do not delete a directory that
lacks `.viewr-install.json` unless it is a reviewed legacy installation containing
only the two viewr executables.

On macOS or Linux, first confirm that `~/.local/bin/viewr` points inside the
installer-owned viewr releases directory, then remove that symlink and the
`${XDG_DATA_HOME:-$HOME/.local/share}/viewr` directory. Linux users may also remove
the `com.github.blisspixel.viewr.desktop` and `com.github.blisspixel.viewr.svg`
files from their user data directories.

Uninstalling viewr never changes or deletes photos.

## Verify an installation

```text
viewr --version
viewr doctor
```

`doctor` verifies binary placement, the worker protocol, platform identity,
privacy boundaries, an in-memory decode self-test, and the windowing
prerequisites of the current desktop session. It performs no network request and
creates no diagnostic log.

What `doctor` cannot prove is a working GPU surface, because it never opens a
window. It says so in its report. If viewr then fails to present, the launch
prints the reason on stderr and exits non-zero; developer logging is not
required to see it.

`bin/viewr-decode` is the isolated decode worker. viewr starts it and speaks a
binary protocol over its standard input and output. Running it by hand prints a
short explanation and exits.

Developers and maintainers should use the complete matrix in [VERIFY.md](VERIFY.md).
Release publication is documented separately in [PUBLISHING.md](PUBLISHING.md).
