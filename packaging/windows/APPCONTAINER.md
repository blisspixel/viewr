# Windows AppContainer plan (Phase 7)

## Goal

Ship viewr (and preferably `viewr-decode`) under an AppContainer with **no
internet capability**, matching Microsoft’s least-privilege AppContainer model
(file isolation, network isolation, process isolation).

## Capabilities

- Enable AppContainer for the package (MSIX / Store or packaged desktop).
- Do **not** grant `internetClient`, `internetClientServer`, or `privateNetworkClientServer`.
- Grant only what is required for GPU presentation and user-selected files
  (package identity + optional broadFileSystemAccess is a last resort; prefer
  picker-based access).

## Child worker

`viewr-decode` should inherit a restricted job or run as a low-integrity child:

1. Parent creates a Job Object with kill-on-close and active process limit.
2. Spawn worker with restricted token when packaging allows.
3. Keep the versioned stdin/stdout protocol as the only IPC.

Implementation lives in `crates/viewr/src/sandbox.rs` over time; this document
is the packaging contract until the launcher code lands.

## Verification

- `cargo deny check` still bans network crates in the dependency tree.
- Runtime: confirm with Process Explorer / `CheckNetIsolation` that the
  packaged process has no network capability.
- Functional: open JPEG/PNG, trash/restore, folder navigation.
