# Sandboxed Decode Worker Plan (Phase 6 / Phase 7)

## The Problem
To achieve "The VLC of Image Viewers" (Phase 6), we must support AVIF, HEIC, HEIF, and Camera RAW. However, there are no production-ready, pure-Rust decoders for these formats. They rely on massive C/C++ libraries (`libaom`, `libheif`, `libraw`). Linking them directly into our main binary violates our core security invariants because:
1. They bring in heavy build systems (`cmake`) and massive dependency trees.
2. Parsing complex, untrusted images in C/C++ is historically the largest source of memory safety bugs (buffer overflows, RCEs).

## The Solution: A Privately-Spawned Decode Process
We will build a dedicated, sandboxed decode worker: `viewr-decode`.
The main `viewr` process remains pure-Rust, memory-safe, and dependency-light. When it encounters a format requiring a C decoder, it sends a versioned native-path frame to the isolated worker. The worker still has the filesystem access of its enclosing package, so OS profile enforcement remains part of Phase 7.

### Architecture
1. **Multi-Binary Workspace:** 
   - `crates/viewr` (Main UI, pure Rust, safe decoders: JPEG, PNG, JXL, SVG, etc.)
   - `crates/viewr-decode` (The worker process, links to C libraries: AVIF, HEIC, RAW)
2. **IPC (Inter-Process Communication):**
   - Main process spawns `viewr-decode` and communicates over standard pipes (`stdin`/`stdout`).
   - Main sends a versioned, length-prefixed native path frame.
   - Worker decodes it, sends an exact-length bounded `RGBA8` stream, and waits for a versioned acknowledgement.
3. **Hardened Sandbox (Phase 7):**
   - On Linux: fail-closed `no_new_privs`, a one-process seccomp policy, and denial of classic plus io_uring network paths; Flatpak supplies the filesystem and whole-app boundary.
   - On macOS: a private session and one-process resource limit protect worker lifetime; an App Sandbox entitlement profile omits network client/server grants. Runtime packaging verification remains open.
   - On Windows: a fail-closed, one-process Job Object supplies lifetime and aggregate memory limits. AppContainer network denial remains a packaging task.

### Step-by-Step Implementation for Next Sprint
1. **Create the `viewr-decode` crate** in the workspace.
2. **Setup IPC:** Implement versioned native-path and acknowledgement frames over `stdin`, typed length-prefixed responses over `stdout`, and an exact-length pixel stream after a validated shape frame.
3. **Migrate decoding:** Add AVIF support to the worker crate using `avif-decode`.
4. **Wire up `fs.rs` and `decode.rs`:** In the main process, `is_supported_image` checks if the extension is core or delegated. If delegated, `decode.rs` spins up the worker instead of using `image::open`.

## Why this is Exceptional
Moving optional C decoders out of the UI process materially reduces blast radius, but process isolation is not zero risk and seccomp alone is not a complete sandbox. The defensible design layers bounded IPC, explicit resource limits, request timeouts, a network-denying process policy where implemented, and an enclosing OS package profile. Claims stay limited to controls that can be reproduced locally.

## Implementation status (2026-07-21)

| Item | Status |
|------|--------|
| Multi-binary workspace (`viewr` + `viewr-decode`) | Done (feature-gated C backends) |
| Versioned path/response/ack frames + bounded pixel-stream IPC | Done |
| Feature-gated C deps (CI pure-Rust) | Done |
| OS trash for curation | Done (`trash` crate) |
| Flatpak manifest (no network) | Sketch in `packaging/flatpak/` |
| macOS entitlements (no network) | Sketch in `packaging/macos/` |
| Windows AppContainer plan | Sketch in `packaging/windows/` |
| Windows Job Object (kill-on-close + one process + 1.5 GiB job memory) | Done, fail-closed and runtime-tested (`worker_limit`) |
| Unix private session and one-process worker policy | Done; Linux seccomp is runtime-tested (`worker_limit`) |
| Linux no_new_privs + post-exec dumpable=0 | Done, fail-closed (`worker_limit` + worker startup) |
| seccomp-bpf network deny (default-allow + EPERM list) | Done, fail-closed (`seccompiler` in `worker_limit`) |
| Shared decode shape limit + hard 30-second send/receive deadline | Done and pipe-saturation tested (`viewr-protocol` + `sandbox`) |
| Optional default-deny allowlist for C decoders | Open |
| Filmstrip real thumbnails (async) | Done (`thumbs` + egui textures) |
| Continuous fuzz CI | Open (adversarial corpus tests exist) |
