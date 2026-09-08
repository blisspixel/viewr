# Repository instructions

These instructions apply to the whole repository. This file is the canonical
agent guidance. `CLAUDE.md` only imports it.

## What viewr is

viewr is a fast, focused, local-only desktop image viewer for Windows, macOS,
and Linux. It opens a file or folder, decodes off the UI thread, presents
through a wgpu canvas with egui chrome, and offers ratings, crop, Spot Heal,
Save As, Trash, and a transient full-image collage. It is not a catalog,
cloud library, account product, or general editor.

The established stack is pinned Rust (`rust-toolchain.toml`, edition 2024),
winit, wgpu, egui, AccessKit, `image`, `jxl-oxide`, and an isolated
`viewr-decode` worker for optional C-backed formats. Do not reopen that
choice or add Tauri, Iced, a WebView, Qt, or a second GUI toolkit.

## Start here

Read `README.md`, `docs/ROADMAP.md`, `docs/STANDARDS.md`, `SECURITY.md`, and
`docs/README.md` before changing behavior. Then load only the surface that
matches the task:

| Task | Read |
| --- | --- |
| Controls, collage, Trash, Undo | `docs/DESIGN.md` |
| Jobs, playlist, decode, GPU, ownership | `docs/ARCHITECTURE.md` |
| Why this stack, rejected alternatives | `docs/STACK.md` |
| Format support and decoder limits | `docs/FORMATS.md` |
| JPEG XMP ratings and filters | `docs/RATINGS.md` |
| No-network and metadata rules | `docs/PRIVACY.md` |
| What each check proves | `docs/VERIFY.md` |
| Tags, version, public install links | `docs/PUBLISHING.md` |
| Optional models (not shipped) | `docs/LOCAL-INTELLIGENCE.md` |

Distinguish planned work (`docs/ROADMAP.md`), implemented behavior (source
and tests), shipped behavior (`CHANGELOG.md` and `docs/releases/`), and
proven behavior (evidence records). Source, tests, lockfiles, and release
notes outrank stale prose. A tagged preview is not a completed product, and
a roadmap row is not shipped.

Keep `README.md`, `CHANGELOG.md`, the roadmap, and the focused guide
accurate when behavior or a durable contract changes.

## Product law

These are product requirements, not style:

- No account, telemetry, advertising, cloud library, activity history, or
  background update checks.
- No application HTTP or TLS client. `cargo-deny` and the privacy scripts
  enforce this. Do not add `reqwest`, `hyper`, a TLS client, or similar.
- No catalog, sidecar, hidden rating database, or on-disk thumbnail library.
  Ratings write standard 0-to-5 XMP into supported JPEG files. The only
  persisted UI preferences are appearance, folder sort, and language: one
  validated word each, no image paths.
- No unrelated feature work or broad rewrite when a bounded fix is enough.
- No silent failure on important user, operator, privacy, or recovery paths.
- Do not ship a model runtime. Local intelligence is gated and unimplemented.
- Do not bump `rust-toolchain.toml`, the workspace version, or public
  installer commands as part of an ordinary fix. Public install commands
  must keep using immutable release assets, not moving branches.

## Canonical seams

`App` is the only mutable UI owner. Do not add a second store, service
locator, or parallel event loop. Put new domain policy in the existing
covered module, not in `app.rs`.

| Concern | Module |
| --- | --- |
| One-result background work | `job` |
| Folder catalog, filter, selection | `playlist` |
| Scan purpose and install disposition | `entry_state` |
| Exclusive-work blockers and wait copy | `current_work` |
| External file and folder changes | `file_coherence` |
| Trash, permanent delete, restore | `curate`, `curation_state` |
| Ratings | `ratings`, `rating_state` |
| Save As | `save_state` |
| Crop / heal / presentation / decode | `crop_state`, `edit_state`, `presentation`, `decode`, `color` |
| Neighbor decode | `prefetch` |
| Full-image collage | `mosaic` (user-facing name is collage) |
| Keyboard routing | `keyboard_route` |
| Generation / stale-work currency | `work_currency` |

Before adding a logger, cache, filesystem helper, persistence path, decoder,
color transform, or completion channel, find the existing one and extend it.

Folder membership refresh of an already-open folder uses in-place
`Playlist::reconcile`. Do not `install_playlist` or `replace_playlist` for
that path: those wipe ratings, filter, Undo scope, and neighbor decodes.
Only Open Folder and missing-selection recovery take the exclusive
folder-scan lock. Sibling and membership scans must not freeze Delete or
navigation. Optimistic Trash that empties a one-item catalog must not
cancel that sibling scan or reinstall a path already accepted for Trash.

Keep `vendor/jxl-color` and `vendor/jxl-render` as the reviewed security
patches. Do not revert them to crates.io to "upgrade."

Default `cargo` builds stay pure-Rust. Optional `viewr-decode` features
`avif` and `heic` need system C libraries. Do not pass `--all-features` on
the workspace.

Do not add new `unsafe` except inside the existing confined modules that
already document a Safety contract (decode-worker FFI, seccomp,
`display_probe`, Windows console attach).

## Verification

The compiler, Clippy with warnings denied, tests, `cargo-deny`, and the
privacy check are the external truth. After a change: implement, run the
relevant checks, inspect failures, fix the root cause, rerun, then prove
the behavior. Do not weaken Clippy, coverage, deny rules, privacy
tripwires, or tests to make a change pass.

Normal local loop after a Rust change:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

After Python tooling changes:

```text
python -B -m unittest discover -s scripts/tests
```

On a host with the pinned tools from `scripts/requirements-quality.txt`, also
run `ruff check scripts` and `ruff format --check scripts`. Meaningful
Rust logic coverage and Python tooling coverage stay at or above 85 percent.
Prefer tests that would fail if the behavior regressed. Playlist, cull,
file-coherence, and job-currency races need state assertions, not a function
that merely runs.

Green CI is the floor. Release-impacting work uses the full gate in
`docs/VERIFY.md`. Performance, accessibility, and product-quality claims
need the evidence those documents define.

## Engineering bar

- Keep changes small, readable, secure, accessible, and test-near.
- Treat image bytes, metadata, paths, worker responses, and external tool
  output as untrusted input.
- Do not add placeholders, TODOs, fake tests, fake metrics, dead code, or
  commented-out implementations.
- Do not add generated-by lines, assistant names, model names, emojis, or em
  dashes.
- Keep GitHub Actions pinned to full commit SHAs. Release notes are reviewed
  files under `docs/releases/`.
- Research current crate, toolchain, and platform docs before changing
  dependencies or compiler flags. Prefer the standard library or an existing
  crate over a new dependency. New crates must pass license, advisory, and
  privacy policy.

## Autonomy

Local inspection, reversible edits, tests, and isolated worktrees are in
scope. Do not commit, push, tag, publish, dispatch `Release artifacts`, or
change GitHub release state unless the user asked. Do not run the public
installers as part of development. Do not operate on the user's real photos;
use synthetic fixtures. Report vulnerabilities only through `SECURITY.md`.

Open an issue before a large behavior, dependency, format, or architecture
change. Small fixes do not need a ticket.

## Hygiene and working files

Application source lives under `crates/`, docs under `docs/`, packaging
under `packaging/`, automation under `scripts/`. Standard root project files
remain at the root.

Put local logs in ignored `logs/` and agent working files in ignored
`.agent/`. That directory may hold indexes, maps, receipts, and temporary
plans. Never store secrets there. Never treat an index as authoritative;
read the relevant source before editing. Promote durable facts into tracked
docs, tests, issues, or code.

Do not commit caches, generated coverage, build output, or scratch files.
Stage only intentional changes and keep `main` clean, linear, and passing.
