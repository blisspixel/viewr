# Windows AppContainer profile

## Goal

Ship viewr (and preferably `viewr-decode`) under an AppContainer with **no
internet capability**, matching Microsoft’s least-privilege AppContainer model
(file isolation, network isolation, process isolation).

## Enforced package contract

- `AppxManifest.xml` sets `uap10:TrustLevel="appContainer"` and
  `uap10:RuntimeBehavior="packagedClassicApp"`.
- `<Capabilities />` is deliberately empty. In particular, the package has no
  `internetClient`, `internetClientServer`, `privateNetworkClientServer`,
  `broadFileSystemAccess`, or `runFullTrust` capability.
- User-selected files remain the intended access path. Broad library or host
  filesystem capabilities are outside this profile.
- The File menu exposes a separate **Open Folder** picker for explicit,
  session-scoped sibling access. If a file picker grants only one file, viewr
  keeps that image usable and does not assume access to its parent directory.

## Child worker

`viewr-decode` is shipped beside the main executable and inherits the parent's
AppContainer token. Its existing Job Object adds a separate lifetime boundary:

1. Parent creates a Job Object with kill-on-close and active process limit.
2. The AppContainer parent spawns the worker without adding capabilities.
3. The parent opens selected files and sends bounded encoded bytes over the
   versioned stdin/stdout protocol; the worker receives no filesystem path.

`scripts/build-windows-appcontainer.ps1` packages real workspace binaries and
uses the Windows SDK's `MakeAppx.exe` schema validator. It creates an unsigned
local inspection artifact at
`target/profile-check/windows/viewr-appcontainer.msix` only; signing,
installation, and publication are not part of Phase 7.

## Verification

- `cargo deny check` still bans network crates in the dependency tree.
- `cargo test -p viewr --test sandbox_profiles` checks the exact empty
  capability set and AppContainer trust level.
- `scripts/build-windows-appcontainer.ps1` must produce a schema-valid MSIX
  from `viewr.exe` and `viewr-decode.exe`.
- Installation requires a trusted local signing certificate and is intentionally
  left as an explicit operator step. Do not claim runtime verification from the
  unsigned package alone.
