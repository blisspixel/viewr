# Sandboxed Decode Worker Plan (Phase 6 / Phase 7)

## The Problem
To achieve "The VLC of Image Viewers" (Phase 6), we must support AVIF, HEIC, HEIF, and Camera RAW. However, there are no production-ready, pure-Rust decoders for these formats. They rely on massive C/C++ libraries (`libaom`, `libheif`, `libraw`). Linking them directly into our main binary violates our core security invariants because:
1. They bring in heavy build systems (`cmake`) and massive dependency trees.
2. Parsing complex, untrusted images in C/C++ is historically the largest source of memory safety bugs (buffer overflows, RCEs).

## The Solution: A Privately-Spawned Decode Process
viewr uses a dedicated, sandboxed decode worker: `viewr-decode`.
The main `viewr` process remains pure-Rust, memory-safe, and dependency-light. When it encounters a format requiring a C decoder, it opens the user-selected file and sends the bounded encoded bytes to the isolated worker. The worker receives no path and needs no dynamic filesystem grant.

### Architecture
1. **Multi-Binary Workspace:** 
   - `crates/viewr` (Main UI, pure Rust, safe decoders: JPEG, PNG, JXL, SVG, etc.)
   - `crates/viewr-decode` (The worker process, links to C libraries: AVIF, HEIC, RAW)
2. **IPC (Inter-Process Communication):**
   - Main process spawns `viewr-decode` and communicates over standard pipes (`stdin`/`stdout`).
   - Main sends a versioned frame containing a validated format identifier and at most 512 MiB of encoded input.
   - Worker decodes it, sends an exact-length bounded `RGBA8` stream, and waits for a versioned acknowledgement.
   - Package smoke tests require an exact typed handshake response from the worker; an arbitrary decoder error does not count as protocol compatibility.
3. **Hardened Sandbox (Phase 7):**
   - On Linux: fail-closed `no_new_privs`, a one-process seccomp policy, and denial of classic plus io_uring network paths; Flatpak supplies the filesystem and whole-app boundary.
   - On macOS: a private session and one-process resource limit protect worker lifetime; the verified App Sandbox bundle omits network client/server grants and probes main-to-worker IPC.
   - On Windows: a fail-closed, one-process Job Object supplies lifetime and aggregate memory limits; the packaged-classic AppContainer manifest grants no capabilities.
   - The main UI offers separate file and folder pickers. A file-only grant remains a one-image playlist when sibling enumeration is denied; **Open Folder** obtains explicit session-scoped consent for navigation without a broad library capability.

### Implemented boundary

The workspace worker, versioned IPC, routing, process lifetime controls, and
optional AVIF/HEIC backends are implemented. Profile artifacts now cover the
whole application on all three desktop platforms. The remaining worker-specific
hardening item is a default-deny Linux syscall allowlist suitable for enabled C
decoders; the current Linux filter deliberately denies network and process
creation while allowing the decoder's broader syscall surface.

## Why this is Exceptional
Moving optional C decoders out of the UI process materially reduces blast radius, but process isolation is not zero risk and seccomp alone is not a complete sandbox. The defensible design layers bounded IPC, explicit resource limits, request timeouts, a network-denying process policy where implemented, and an enclosing OS package profile. Claims stay limited to controls that can be reproduced locally.

## Implementation status (2026-07-21)

| Item | Status |
|------|--------|
| Multi-binary workspace (`viewr` + `viewr-decode`) | Done (feature-gated C backends) |
| Versioned encoded-input/response/ack frames + bounded pixel-stream IPC | Done; worker receives no path |
| Feature-gated C deps (CI pure-Rust) | Done |
| OS trash for curation | Done (`trash` crate) |
| Flatpak manifest (no network) | Exact-set tested 25.08 profile; Linux CI performs an offline Cargo build and worker probe |
| macOS entitlements (no network) | Main/helper profiles; CI builds, ad-hoc signs, and worker-probes a local bundle |
| Windows AppContainer profile | Empty-capability Appx manifest; Windows SDK validates a local unsigned MSIX |
| Windows Job Object (kill-on-close + one process + 1.5 GiB job memory) | Done, fail-closed and runtime-tested (`worker_limit`) |
| Unix private session and one-process worker policy | Done; Linux seccomp is runtime-tested (`worker_limit`) |
| Linux no_new_privs + post-exec dumpable=0 | Done, fail-closed (`worker_limit` + worker startup) |
| seccomp-bpf network deny (default-allow + EPERM list) | Done, fail-closed (`seccompiler` in `worker_limit`) |
| Shared decode shape limit + hard 30-second send/receive deadline | Done and pipe-saturation tested (`viewr-protocol` + `sandbox`); bounded host reads occur before worker reservation and the IPC deadline thread |
| Explicit directory-consent navigation | Done; file-only denial degrades to one image and Open Folder enables sibling scanning |
| Optional default-deny allowlist for C decoders | Open |
| Filmstrip real thumbnails (async) | Done (`thumbs` + egui textures) |
| Continuous fuzz CI | Done (`fuzz/` targets, deterministic seeds, change and weekly runs) |

## Local profile verification

Run the platform-neutral semantic checks everywhere:

```text
cargo test -p viewr --test sandbox_profiles
```

On Windows, package actual workspace binaries and have the installed Windows SDK
validate the manifest and payload:

```powershell
cargo build --workspace --locked
scripts/build-windows-appcontainer.ps1
```

On macOS, build, ad-hoc sign, verify, and smoke-test the App Sandbox bundle:

```bash
cargo build --workspace --locked
bash scripts/build-macos-sandboxed-app.sh
```

On Linux, install the 25.08 Freedesktop SDK, generate the checksum-pinned Cargo
source list, then perform the same clean build and in-sandbox worker probe as CI:

```bash
python3 scripts/generate-flatpak-cargo-sources.py
flatpak-builder --force-clean --disable-rofiles-fuse \
  target/profile-check/flatpak \
  packaging/flatpak/com.github.blisspixel.viewr.yml
flatpak-builder --run \
  target/profile-check/flatpak \
  packaging/flatpak/com.github.blisspixel.viewr.yml \
  viewr doctor
```

These are local profile proofs, not store publication, notarization, or a claim
that an unsigned MSIX was installed. A user who runs bare Cargo binaries does not
receive the package-level boundary.
