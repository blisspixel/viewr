# jxl-color local hardening patch

This directory contains the published `jxl-color` 0.11.0 crate used by
`jxl-oxide` 0.12.6. The upstream source is licensed under MIT or Apache-2.0;
both license texts are preserved beside the source. The crates.io package
checksum for the unmodified source archive is
`f316b1358c1711755b3ee8e8cb5c4a1dad12e796233088a7a513440782de80b2`.

The local deviations from that archive are deliberately narrow:

- `src/icc/decode.rs` applies viewr's 10 MiB embedded ICC ceiling to encoded,
  declared, and appended output sizes before allocation or amplification.
- `src/icc/synthesize.rs` adds a fallible synthesizer for encodings that the
  matrix/TRC builder can represent. The compatibility wrapper returns an empty
  profile for hostile or unsupported XYB, unknown-space, unknown-transfer, and
  zero-gamma inputs instead of unwinding.
- `src/icc.rs` exposes the shared ICC limit and fallible synthesizer.
- `src/convert.rs` uses fallible synthesis internally, rejects hostile unknown,
  zero-gamma, and grayscale HLG enum transforms before the no-op shortcut, and
  returns errors for invalid operation channel counts instead of unwinding. The
  equivalent grayscale HLG case is intentionally rejected because its metadata
  is not a supported color encoding even when no sample conversion is needed.
  The file also documents the grayscale invariant previously represented by a
  stale placeholder comment.
- `src/icc/parse.rs` makes one elided return lifetime explicit and removes test
  diagnostics so the pinned compiler and anti-debug checks remain clean.
- `src/convert/tone_map.rs` corrects a test channel zip so the red, green, and
  blue results are all asserted.

Supported color-encoding synthesis and decoding remain byte-for-byte compatible
with upstream. Unsupported enum encodings now take a visible fallback or error
path instead of terminating a decode executor thread.

When updating `jxl-oxide`, replace this directory from the matching published
`jxl-color` crate, determine whether upstream now exposes an equivalent bounded
initialization API, and remove the patch if it does. Otherwise reapply the one
constant, allocation comparisons, append guards, fallible synthesis path, and
the warning and test-quality fixes listed above. Update the version and crates.io
checksum in this file, diff every source file against the new published archive,
and run the complete offline test, coverage, dependency, privacy, and packaging
gates.
