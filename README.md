# viewr

**Fast photos. No account. No tracking. No subscription.**

[![CI](https://github.com/blisspixel/viewr/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/blisspixel/viewr/actions/workflows/ci.yml)

Tired of photo software that wants to become a cloud library, editing suite,
storefront, and subscription? viewr rejects the spyware-and-bloatware pattern of
turning a simple utility into a data-collection platform. It stays a viewer. Open
a file or folder, move through photos quickly, inspect metadata, rate the keepers,
make a small edit, and get back to what you were doing.

viewr is a focused desktop image viewer for Windows, macOS, and Linux. **No
tracking is literal.** The application has no account, cloud service, telemetry,
analytics, advertising, background indexer, activity history, crash-report
uploader, or automatic update check.

## Why viewr

- **Fast by design.** Decoding begins while the window initializes, neighboring
  images are prefetched within fixed memory limits, and performance budgets run in
  CI.
- **Private by construction.** The application dependency graph contains no HTTP
  or TLS client. Photos, paths, metadata, ratings, and diagnostics stay local.
- **Focused, not bare.** Folder navigation, ratings, privacy inspection, crop,
  Spot Heal, conversion, and safe Trash cover the useful viewer workflow without
  a catalog or subscription.
- **Cross-platform.** The same Rust codebase is built and tested on Windows,
  macOS, and Linux, with native dialogs, menus, shortcuts, and accessibility
  semantics.

## Interface

![viewr displaying an image in Console appearance with phosphor-green File, Edit, View, Tools, and Help menus](docs/screenshots/viewr-console-example.png)

Console appearance with docked menus and a clean image-first viewport. The
capture is only the application window, with no private path or unrelated desktop
content.

## Install

v0.3.0 is the current public preview. Its portable archives are checksummed and
attested, but they are not Authenticode-signed on Windows or notarized on macOS.
Normal operating-system trust warnings may appear. Do not disable platform
security controls to force a launch.

### Windows 10 or 11, x64

```powershell
irm https://github.com/blisspixel/viewr/releases/download/v0.3.0/install.ps1 | iex
```

viewr installs for the current user under `%LOCALAPPDATA%\Programs\viewr`, adds
that directory to the user PATH, and creates a Start menu shortcut. No elevation
or updater service is required.

### macOS or Linux

```sh
curl -fsSL https://github.com/blisspixel/viewr/releases/download/v0.3.0/install.sh | sh
```

viewr installs under `~/.local`. The preview supports Intel and Apple Silicon
macOS plus x86-64 glibc Linux.

The published command downloads the v0.3.0 installer, which installs the v0.3.0
archive after verifying its SHA-256 sidecar and internal manifest, without
giving the application network access.
Run the same command again for an explicit update. For review-first installation,
manual archive verification, uninstall steps, platform prerequisites, or a source
build, see [Installing viewr](docs/INSTALL.md).

## What it does

- Opens a broad pure-Rust core format set, including JPEG, PNG, GIF, WebP, TIFF,
  SVG, JPEG XL, OpenEXR, and common bitmap formats.
- Navigates naturally sorted folders without blanking the last good frame during
  a cache miss or failed replacement.
- Provides GPU pan, zoom, fit, animation, rotation, crop, bounded Spot Heal,
  Save As, and format conversion.
- Assigns embedded 0-to-5 XMP ratings and filters a folder by minimum rating
  without creating a catalog, sidecar, or activity history.
- Shows a presence-only Source Privacy summary for sensitive EXIF categories.
- Strips supported metadata from saved copies by default, with a session-only
  option to retain supported EXIF fields.
- Moves only the visible image to system Trash with exact-receipt Undo when the
  platform can prove it. Permanent delete is separate and confirmed.
- Offers native dialogs, keyboard-first controls, AccessKit semantics, four chrome
  appearances, and independent image-inspection backgrounds.

The exact format table and current limits are in [Formats](docs/FORMATS.md).
Implemented behavior and remaining work are in [Roadmap](docs/ROADMAP.md).

## Privacy and security

viewr sends no photos, filenames, paths, metadata, diagnostics, ratings, or usage
data anywhere. Normal runs initialize no logger and create no logs. Explicit
developer-only console diagnostics are opt-in, path-private, local, and never
written to a log file. Optional C-backed image formats execute in a bounded helper
that receives encoded bytes rather than a filesystem path, and platform packages
add network-denied sandbox profiles.

Opening an image still means parsing untrusted data. The codebase uses bounded
decoding, dimension and allocation limits, source-identity checks around
destructive operations, fail-closed metadata writes, dependency policy, fuzzing,
and native platform tests. See [Privacy](docs/PRIVACY.md),
[Architecture](docs/ARCHITECTURE.md), and [Security Policy](SECURITY.md) for the
precise boundaries.

Installer scripts are separate foreground tools. They contact only the official
GitHub repository after the user runs them. viewr itself never checks for updates
automatically.

## Essential controls

| Action | Control |
| --- | --- |
| Open file or folder | `Ctrl/Cmd+O`, `Ctrl/Cmd+Shift+O` |
| Previous or next image | Left/Right, `A`/`D` |
| Fit or actual size | Primary modifier + `0`/`1` |
| Zoom | `+`, `-`, wheel or trackpad |
| Pan | Drag, or hold Space and drag |
| Tools, folder previews, image information | `T`, `G`, `I` |
| Rate or clear rating | `1` through `5`, `0` |
| Crop or Spot Heal | `C`, `J` |
| Reload after an external edit | `F5` |
| Move current image to Trash | Delete |
| Undo the latest recoverable Trash action | `U` |

Menus expose the same actions and shortcuts. There is no flag, review queue,
batch-trash mode, or bare-letter delete shortcut. Detailed interaction behavior is
in [Design](docs/DESIGN.md) and [Accessibility](docs/ACCESSIBILITY.md).

Rating **writes** are currently limited to ordinary supported JPEG files on
Windows. macOS and Linux builds read embedded ratings and filter a folder by
them, but do not write a rating back to a file yet. The exact scope, states, and
safety contract are in [Ratings](docs/RATINGS.md).

## Appearance

- **System** follows the operating system's light or dark setting.
- **Light** uses bright neutral chrome.
- **Dark** uses low-glare charcoal chrome.
- **Console** uses near-black chrome, phosphor-green text, and monospace type.

Image Background independently offers Theme Default, Black, Neutral Gray, and
White. Appearance changes interface chrome and canvas only, never image pixels.

## Project status

v0.3.0 is the current public preview and install target, not a percentage-complete
score or a claim that the product is finished. It is the closed display-correct
SDR milestone after the published v0.2.0 reliability architecture release.
The first preview v0.1.0, the v0.1.1 through v0.1.5 patches, and v0.2.0 remain
published. `main` continues the logical order in the
[roadmap](docs/ROADMAP.md#order-of-operations-to-10): **v0.4.0** file coherence
through v0.9 publisher authentication, then v1.0.
The [release notes](docs/releases/v0.3.0.md) state the exact preview limits.

## Development

The repository pins its Rust toolchain and lockfiles. The normal local gate is:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

CI also enforces at least 85 percent meaningful logic coverage, Python
release-tool tests, dependency license and advisory policy, privacy tripwires,
fuzz targets, platform sandbox profiles, native accessibility, deterministic
release archives, and GUI performance budgets. Read
[Contributing](CONTRIBUTING.md) and [Verification](docs/VERIFY.md) before
submitting a change.

The [documentation index](docs/README.md) links every user, architecture,
quality, privacy, and release document. [CHANGELOG.md](CHANGELOG.md) records
user-visible changes.

## License

viewr is licensed under the [Apache License 2.0](LICENSE), including its express
patent grant and standard warranty disclaimer. [NOTICE](NOTICE) identifies the
project distribution. Third-party components retain their own licenses, collected
in [THIRD_PARTY_LICENSES.txt](THIRD_PARTY_LICENSES.txt).
