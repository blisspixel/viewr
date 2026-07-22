# Optional Local Intelligence

Status: architecture accepted, model bake-off not yet complete. No model runtime
or model weights ship in viewr today.

This document defines the narrow boundary for optional model-backed features. It
exists to prevent a useful experiment from turning a focused image viewer into a
model manager, background service, photo index, or settings maze.

## Product boundary

viewr remains complete when no model is installed. Opening, viewing, navigating,
cropping, spot healing, converting, culling, and exporting must never depend on a
model runtime.

Only two model-backed capabilities are eligible:

1. **Describe Image**, an explicit one-shot request that returns a short textual
   description of the currently displayed image.
2. **Advanced Heal**, a possible later solver for a painted region that is too
   large or structurally complex for the built-in spot-heal algorithm.

Neither capability may run during startup, image loading, folder navigation,
prefetch, idle time, or export. They may not scan a folder, retain a result,
identify a person, group similar faces, construct embeddings, or build a library.

The shipped Spot Heal tool intentionally does not require a model. Small dust,
skin, sensor, and background blemishes are repaired by deterministic local patch
matching over a bounded region of interest. A model is justified only if measured
quality on larger repairs earns the additional runtime and packaging cost.

## Description interaction

The first eligible interface is a **Describe Image** button inside Image
Information. It is absent when no compatible model pack is installed. Activating
it describes only the current decoded pixels and keeps the result in memory until
navigation or window close.

The initial feature produces text. It does not start Narrator, VoiceOver, Orca,
or automatic speech. A screen reader may read the focused result because it is
ordinary accessible text. Built-in Read Aloud remains deferred: operating-system
voices are not uniformly offline, so viewr cannot currently prove that arbitrary
platform speech configuration meets the same zero-network contract.

Every generated result must be labeled as local and potentially inaccurate. The
system prompt must request observable objects, spatial relationships, visible
text, and uncertainty. It must prohibit guessing identity, ethnicity, health,
intent, or other sensitive traits.

## Privacy architecture

The model path must preserve the existing privacy invariant by construction:

- A separate `viewr-describe` process owns the model runtime. The main viewer
  does not link the runtime or load model weights.
- IPC uses inherited binary pipes, not localhost HTTP, WebSocket, or a server.
- The parent sends bounded, downscaled decoded pixels. It never sends the source
  path, filename, folder name, EXIF, or original encoded bytes.
- The worker can read only its explicitly installed model pack and runtime
  libraries. It receives no photo-library access and has no writable data or
  cache directory.
- A worker-specific sandbox grants no network, local IPC, process-inspection,
  child-process, registry-write, or filesystem-write capability. The viewer's
  broader application sandbox is not sufficient for this worker.
- Normal execution writes no prompt, pixels, response, history, cache, crash
  report, or log file. Runtime diagnostics are disabled in release builds. A
  bounded path-free developer diagnostic can exist only behind explicit local
  invocation, consistent with `docs/PRIVACY.md`.
- Navigation drops an in-flight response by generation identifier. Worker
  cancellation and process lifetime are bounded.
- Model packs are separate, optional artifacts with an exact model identifier,
  license, size, SHA-256 digest, runtime version, and supported architecture.
  viewr never downloads them automatically.

### Mandatory worker capability boundary

The worker is a one-shot child, not a daemon. Its only application IPC is one
versioned, length-prefixed protocol over directly inherited anonymous pipes.
Before it parses a request, it must close every unexpected handle and file
descriptor, clear the environment, change to a non-writable empty working
directory, and retain only the protocol pipes plus the exact read-only handles
needed for the reviewed runtime and model pack. It must never open the current
photo, its folder, the user's home directory, app data, temporary storage, or a
model cache.

The default-deny worker policy must block all socket families, including local
and Unix-domain sockets, plus `socket`, `socketpair`, `bind`, `listen`,
`connect`, `accept`, RPC, D-Bus, COM activation, named-pipe creation, io_uring,
process creation, and process inspection. Model inference may create bounded
same-process compute threads, but may not create another process. Release tests
must prove these denials rather than infer them from configuration.

Each supported operating system needs an equivalent process-specific boundary:

- Windows uses a restricted AppContainer token with no network capabilities,
  an exact read-only package ACL, and a Job Object limited to one process with
  kill-on-close and memory limits.
- macOS uses a separately signed sandboxed helper with no network entitlement,
  no writable container grant, exact read-only model access, and parent-death
  termination.
- Linux combines a private mount and PID namespace or equivalent package
  boundary with read-only model mounts, no writable mounts, a one-process
  cgroup limit, and a worker-specific default-deny seccomp policy. The viewer's
  current AccessKit-oriented Unix-socket policy must not be reused.

The platform design must be validated against the current official
[Windows app file-access model](https://learn.microsoft.com/windows/apps/develop/files/file-access-permissions),
[Apple App Sandbox file-access model](https://developer.apple.com/documentation/security/accessing-files-from-the-macos-app-sandbox),
and [Flatpak sandbox permissions](https://docs.flatpak.org/en/latest/sandbox-permissions.html)
at implementation time. These references were reviewed on 2026-07-22.

### Resource and lifecycle contract

The protocol and supervisor enforce these hard limits before model-specific
evaluation begins:

- At most one worker and one inference request exist at a time.
- Input is RGBA8, at most 2048 by 2048 pixels and 16 MiB, with no metadata.
- The fixed reviewed prompt is at most 4 KiB. User-supplied prompt text is not
  accepted.
- Output is at most 256 generated tokens and 16 KiB of valid UTF-8.
- A request has a 120-second wall-clock deadline. Cancellation gets a 2-second
  grace period, after which the complete job is forcibly terminated and reaped.
- Parent death closes the job and terminates the worker on every platform.
- Every pack declares measured peak RAM and VRAM ceilings. Admission fails
  before launch unless the device can reserve those ceilings. No supported pack
  may exceed a 32 GiB process-memory ceiling, and the OS limit must enforce the
  declared lower ceiling for that pack.
- The worker unloads the model and exits after one response, cancellation, or
  error. It never stays resident, prewarms, starts with viewr, or runs after the
  viewer closes.

Process-level tests on Windows, Linux, and macOS must cover the single-worker
limit, input and output caps, deadline, cancel-and-reap behavior, parent death,
RAM admission and enforcement, prohibited socket and process operations, zero
filesystem and registry writes, and absence of a surviving child.

### Native runtime and model-pack supply chain

Cargo policy checks do not cover a native runtime or model weights. A separate
release gate is required before any optional pack can ship:

- Pin the exact `llama.cpp` source commit and verified source-archive SHA-256.
- Build offline from that verified source with a reviewed target allowlist and
  CMake-option allowlist based on the upstream
  [`llama.cpp` CMake configuration](https://github.com/ggml-org/llama.cpp/blob/master/CMakeLists.txt).
  Disable servers, examples, tests, tools, RPC, Curl, OpenSSL, download helpers,
  and every unused backend or feature.
- Produce a native SBOM and license inventory, scan native advisories, and scan
  the final binary's imports and exported symbols. Network libraries, server
  symbols, and unreviewed dynamic dependencies fail the build.
- Record the original model repository revision, original artifact digest,
  conversion tool revision and command, quantization parameters, output GGUF
  digest, license, and evaluation result. Conversion must be reproducible from
  those inputs.
- Anchor the runtime and pack digests in a signed viewr release manifest that is
  verified before launch. A digest supplied only beside the artifact is not a
  trust root. Until viewr has release signing and this dedicated native gate, no
  model pack is eligible for release.

The strongest current runtime candidate is a pinned minimal build of
[`llama.cpp`](https://github.com/ggml-org/llama.cpp) and its multimodal
`libmtmd` path. It supports CPU, Metal, CUDA, HIP, Vulkan, and other local
backends from one upstream runtime. viewr must embed or launch the reviewed
worker directly, never `llama-server`.

Ollama is excluded from the strict path. Its official configuration exposes a
localhost HTTP service, history and server logging controls, and a request-body
debug option. Configuration switches are weaker than removing network and
persistence capabilities from the process. See the
[`Ollama` environment configuration](https://github.com/ollama/ollama/blob/main/envconfig/config.go).

## July 2026 model bake-off

The following shortlist is current as of 2026-07-22. Memory figures are planning
estimates for Q4-class weights, a short captioning context, vision-projector
state, and runtime overhead. They are not release claims and must be replaced by
measurements on each target platform.

| Tier | Candidates | Planning memory | Purpose |
|---|---|---:|---|
| Low resource | SmolVLM2 500M | About 2 GB | Fallback only if caption quality clears the minimum bar |
| Mainstream | Qwen3-VL 4B Q4 | 4 to 6 GB | Primary typical-device candidate |
| Mainstream challenger | Gemma 4 E2B Q4 | 5 to 7 GB | Edge-oriented Apache 2.0 challenger |
| Quality | Qwen3-VL 8B or Gemma 4 E4B | 7 to 10 GB | Midrange GPU or unified-memory systems |
| High quality | Gemma 4 12B | 10 to 14 GB | 16 GB and larger systems |
| Maximum | Qwen3-VL 30B-A3B or Gemma 4 26B-A4B | 20 to 24 GB | Short-context 24 GB GPU evaluation |

Primary model sources:

- [Qwen3-VL 4B Instruct](https://huggingface.co/Qwen/Qwen3-VL-4B-Instruct),
  [8B Instruct](https://huggingface.co/Qwen/Qwen3-VL-8B-Instruct), and
  [30B-A3B Instruct](https://huggingface.co/Qwen/Qwen3-VL-30B-A3B-Instruct):
  Apache 2.0, image-text generation, OCR, spatial understanding, and current
  quantized-runtime guidance.
- [Gemma 4 model card](https://ai.google.dev/gemma/docs/core/model_card_4) and
  [official llama.cpp integration](https://ai.google.dev/gemma/docs/integrations/llamacpp):
  Apache 2.0, with E2B/E4B aimed at edge devices and larger variants aimed at
  consumer GPUs and workstations.
- [SmolVLM2 release](https://huggingface.co/blog/smolvlm2) and
  [500M model card](https://huggingface.co/HuggingFaceTB/SmolVLM2-500M-Video-Instruct):
  Apache 2.0 and an official 1.8 GB GPU-memory report for video inference, with
  explicit accuracy limitations.

Mixture-of-experts active parameter counts reduce compute per token, not the
total model storage or weight-residency requirement. A 30B-A3B model must not be
presented as a 3B download or a 3B-memory model.

## Bake-off gate

No model becomes a default recommendation until a reproducible offline bake-off
passes all of these checks:

- 100 or more licensed local images covering people, pets, landscapes, UI
  screenshots, documents, low light, unusual crops, visible text, and ambiguous
  scenes.
- Blind human scoring for factual correctness, useful detail, uncertainty,
  screen-reader utility, and inappropriate sensitive inference.
- Explicit object-presence, OCR, spatial-relation, and hallucination cases with
  forbidden-claim assertions.
- Cold start, first description, warm description, cancellation, peak resident
  memory, peak GPU memory, and model unload measurements.
- CPU-only Windows/Linux, Apple Silicon macOS, a mainstream 8 GB GPU, and a 24 GB
  GPU represented before tier recommendations are finalized.
- Process-level tests proving Internet socket denial, no localhost listener, no
  filesystem write, no prompt or output on stderr, no source path in IPC, and no
  retained result after navigation.

If no small model produces a description that is reliably more useful than
misleading, viewr ships no low-resource tier. An unavailable feature is better
than a confident false description.

## Acceptance criteria for shipping Describe Image

- The default release and normal startup performance remain unchanged when no
  model pack is installed.
- The menu action is explicit, optional, and absent without a compatible pack.
- No inference begins before direct activation.
- The worker receives only bounded decoded pixels and a fixed reviewed prompt.
- Network and write-denial probes pass on Windows, Linux, and macOS packages.
- Normal inference produces zero app-owned log output and zero files.
- Cancellation and navigation cannot display a stale result.
- The selected model tier passes the bake-off and license review.
- The complete workspace remains above 85 percent meaningful logic coverage and
  all existing privacy, dependency, lint, test, and packaging gates remain green.
