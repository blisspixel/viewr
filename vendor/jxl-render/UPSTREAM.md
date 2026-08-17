# jxl-render local hardening patch

This directory contains the published `jxl-render` 0.12.4 crate used by
`jxl-oxide` 0.12.6. The upstream source is licensed under MIT or Apache-2.0;
both license texts are preserved beside the source. The crates.io package
checksum for the unmodified source archive is
`d34386bfdb6a19b5a30cc9beb4d475d537422c31ae8c39bb69640fcce3fcaf19`.

The local deviations are deliberately narrow:

- `src/lib.rs` checks whether a progressive frame actually references an LF
  frame before indexing the four-entry LF-frame table. A malformed stream can
  legally carry an otherwise-unused out-of-range level, which upstream 0.12.4
  indexed before checking the reference flag and therefore panicked.
- `src/image.rs` publishes `ErrTaken` and notifies render waiters when frame
  composition fails after the shared state has entered `Rendering`. This keeps
  malformed input or allocation failure from stranding later callers in a
  condition-variable wait.
- `src/state.rs` centralizes terminal-state publication and waiter notification in
  one `RenderState` boundary, with an executable regression test proving that an
  error wakes a blocked caller. The crate manifest delegates workspace resolver
  selection to viewr's root manifest.
- `src/blend.rs` skips compositing a channel whose clipped frame region has zero
  area, while still appending the channel so the output keeps its shape. A
  malformed stream can clip a channel to nothing, and upstream 0.12.4 then
  borrowed a subgrid of the zero-width source grid, which asserts inside
  `jxl-grid`. Because the release profile aborts on panic, that turned one
  hostile JPEG XL file into a terminated viewer.

The hosted fuzz inputs that exposed these issues are retained as
`fuzz/corpus/decode_memory/regression-jxl-unused-lf-level` and
`fuzz/corpus/decode_memory/regression-jxl-empty-blend-region`, and both are
replayed by normal integration tests as well as future fuzz runs.

When updating `jxl-oxide`, replace this directory from the matching published
`jxl-render` crate, determine whether upstream now checks the reference flag
before indexing and publishes a terminal state on composition errors, and remove
each local patch that is present upstream. Otherwise reapply those corrections,
update the version and crates.io checksum in this file, diff every source file
against the new published archive, and run the complete offline test, coverage,
dependency, privacy, fuzz, and packaging gates.
