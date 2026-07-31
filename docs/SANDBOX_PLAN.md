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
   - Worker decodes it, sends bounded typed color evidence followed by an exact-length bounded `RGBA8` stream, and waits for a versioned acknowledgement.
   - Package smoke tests require an exact typed handshake response from the worker; an arbitrary decoder error does not count as protocol compatibility.
3. **Hardened Sandbox (Phase 7):**
   - On Linux: fail-closed `no_new_privs`, a one-process seccomp policy, and denial of classic plus io_uring network paths. AVIF/HEIC builds add a default-deny syscall allowlist with read-only plugin discovery and thread-only clone; Flatpak supplies the filesystem and whole-app boundary.
   - On macOS: a private session, bounded decode protocol, and parent deadline protect worker lifetime; the verified App Sandbox helper inherits the application boundary, omits network client/server grants, and is probed through main-to-worker IPC.
   - On Windows: a fail-closed, one-process Job Object supplies lifetime and aggregate memory limits; the packaged-classic AppContainer manifest grants no capabilities.
   - The main UI offers separate file and folder pickers. A file-only grant remains a one-image playlist when sibling enumeration is denied; **Open Folder** obtains explicit session-scoped consent for navigation without a broad library capability.

### Implemented boundary

The workspace worker, versioned IPC, routing, process lifetime controls, and
optional AVIF/HEIC backends are implemented. Profile artifacts cover the whole
application on all three desktop platforms. Linux C-decoder builds install a
fail-closed default-deny policy before reading IPC. Shared runtime tests prove
its denial semantics, while release-mode Ubuntu tests decode generated AVIF and
HEIC inputs under the policy, compare worker pixels with direct decoder output,
and verify bounded ICC/CICP precedence against both the distro compatibility
floor and embedded libheif 1.23. The latest lane requires libde265 1.0.7 or newer
and exercises HEVC-VUI-only color as well as container metadata. Parent tests
separately prove that received ICC data is normalized only after the cancellable
IPC transaction has joined.

The parent accepts only a canonical explicit helper override or the exact regular
file installed beside the running viewer. A missing or invalid helper fails
closed; helper execution never falls back to a same-named program on `PATH`.

## Why this is Exceptional
Moving optional C decoders out of the UI process materially reduces blast radius, but process isolation is not zero risk and seccomp alone is not a complete sandbox. The defensible design layers bounded IPC, explicit resource limits, request timeouts, a network-denying process policy where implemented, and an enclosing OS package profile. Claims stay limited to controls that can be reproduced locally.

## Implementation status (2026-07-22)

| Item | Status |
|------|--------|
| Multi-binary workspace (`viewr` + `viewr-decode`) | Done (feature-gated C backends) |
| Versioned encoded-input/response/ack frames + bounded pixel-stream IPC | Done; V2 carries ICC/CICP/unknown color status and the worker receives no path |
| Feature-gated C deps (CI pure-Rust) | Done |
| OS trash for curation | Done; `trash` crate on Windows/Linux and exact-result `NSFileManager` receipts on macOS |
| Flatpak manifest (no network) | Exact-set tested 25.08 profile; installs the desktop entry and icon; Linux CI performs an offline Cargo build and worker probe |
| macOS entitlements (no network) | Main/helper profiles; exact core-format alternate-viewer declaration; native Launch Services delivery; CI builds, ad-hoc signs, and worker-probes a local bundle |
| Windows AppContainer profile | Empty-capability Appx manifest with exact core-format association; Windows SDK validates a local unsigned MSIX |
| Windows Job Object (kill-on-close + one process + 1.5 GiB job memory) | Done, fail-closed and runtime-tested (`worker_limit`) |
| Unix worker containment | Private session, bounded decode protocol, deadline, and inherited package sandbox on macOS; Linux additionally denies process creation with runtime-tested seccomp; supported BSD targets apply address-space and `RLIMIT_NPROC` limits |
| Linux no_new_privs + post-exec dumpable=0 | Done, fail-closed (`worker_limit` + worker startup) |
| seccomp-bpf network deny (default-allow + EPERM list) | Done, fail-closed (`viewr-seccomp`, installed by `worker_limit`) |
| Shared decode shape limit + hard 30-second send/receive deadline | Done and pipe-saturation tested (`viewr-protocol` + `sandbox`); bounded host reads occur before worker reservation, and both reads and blocked IPC stop on foreground-generation supersession |
| Explicit directory-consent navigation | Done; file-only denial degrades to one image and Open Folder enables sibling scanning |
| Feature-gated default-deny allowlist for C decoders | Done; shared policy semantics and release-mode AVIF/HEIC decodes run on Ubuntu 24.04 CI |
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

For the production C-decoder policy, install the native dependencies listed in
the CI job, then run:

```bash
cargo test -p viewr-seccomp --locked
cargo test --release -p viewr-decode \
  --features avif,heic --test c_decoder_sandbox --locked -- --test-threads=1
```

These are local profile proofs, not store publication, notarization, or a claim
that an unsigned MSIX was installed. A user who runs bare Cargo binaries does not
receive the package-level boundary.
