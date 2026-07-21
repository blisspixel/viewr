# Sandboxed Decode Worker Plan (Phase 6 / Phase 7)

## The Problem
To achieve "The VLC of Image Viewers" (Phase 6), we must support AVIF, HEIC, HEIF, and Camera RAW. However, there are no production-ready, pure-Rust decoders for these formats. They rely on massive C/C++ libraries (`libaom`, `libheif`, `libraw`). Linking them directly into our main binary violates our core security invariants because:
1. They bring in heavy build systems (`cmake`) and massive dependency trees.
2. Parsing complex, untrusted images in C/C++ is historically the largest source of memory safety bugs (buffer overflows, RCEs).

## The Solution: A Privately-Spawned Decode Process
We will build a dedicated, sandboxed decode worker: `viewr-decode`.
The main `viewr` process will remain pure-Rust, memory-safe, and dependency-light. When it encounters a format requiring C-decoders, it hands the file descriptor (or path) to the sandbox.

### Architecture
1. **Multi-Binary Workspace:** 
   - `crates/viewr` (Main UI, pure Rust, safe decoders: JPEG, PNG, JXL, SVG, etc.)
   - `crates/viewr-decode` (The worker process, links to C libraries: AVIF, HEIC, RAW)
2. **IPC (Inter-Process Communication):**
   - Main process spawns `viewr-decode` and communicates over standard pipes (`stdin`/`stdout`).
   - Main sends a message: `{"action": "decode", "path": "/path/to/image.avif"}`
   - Worker decodes it, and pipes back the raw `RGBA8` pixel buffer and dimensions.
3. **Hardened Sandbox (Phase 7):**
   - On Linux: We wrap the spawn with `seccomp-bpf` to completely block network access and limit filesystem access to read-only.
   - On macOS: We spawn it via `sandbox-exec` with a strict `deny default` profile.
   - On Windows: We launch the child process within a locked-down AppContainer or Job Object with the network explicitly denied.

### Step-by-Step Implementation for Next Sprint
1. **Create the `viewr-decode` crate** in the workspace.
2. **Setup IPC:** Implement a simple binary protocol over `stdout`/`stdin` to ferry the `DecodedImage` shape (dimensions + RGBA byte array).
3. **Migrate decoding:** Add AVIF support to the worker crate using `avif-decode`.
4. **Wire up `fs.rs` and `decode.rs`:** In the main process, `is_supported_image` checks if the extension is core or delegated. If delegated, `decode.rs` spins up the worker instead of using `image::open`.

## Why this is Exceptional
Most photo viewers either silently link vulnerable C code into their UI process, or they just don't support modern formats at all. By isolating C-decoders into an ephemeral, network-denied, unprivileged child process, we provide 100% format coverage with 0% risk to the user's host machine. This is how exceptional, privacy-first software is built.

## Implementation status (2026-07-21)

| Item | Status |
|------|--------|
| Multi-binary workspace (`viewr` + `viewr-decode`) | Done (feature-gated C backends) |
| stdin/stdout + shared-memory IPC | Done |
| Feature-gated C deps (CI pure-Rust) | Done |
| OS trash for curation | Done (`trash` crate) |
| Flatpak manifest (no network) | Sketch in `packaging/flatpak/` |
| macOS entitlements (no network) | Sketch in `packaging/macos/` |
| Windows AppContainer plan | Sketch in `packaging/windows/` |
| seccomp / Job Object privilege drop | Open (Phase 7 remaining) |
| Continuous fuzz CI | Open |
