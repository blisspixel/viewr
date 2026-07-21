# Linux seccomp for `viewr-decode`

## Applied at runtime

Implemented in `crates/viewr/src/worker_limit.rs` via `seccompiler`:

1. `PR_SET_NO_NEW_PRIVS` in `pre_exec`, with spawn aborted on failure
2. `PR_SET_DUMPABLE=0` in the worker after exec, with startup aborted on failure
3. A 1.5 GiB address-space ceiling before exec
4. A compatibility filter returns `ENOSYS` for `clone3`, allowing libc to fall
   back to the argument-filterable `clone` syscall
5. **Default-allow seccomp-bpf** that returns `EPERM` for network and
   process-creation syscalls:
   - `socket`, `socketpair`, `connect`, `accept`, `accept4`, `bind`, `listen`
   - `sendto`, `recvfrom`, `sendmsg`, `recvmsg`, `sendmmsg`, `recvmmsg`
   - `getsockopt`, `setsockopt`, `shutdown`
   - `io_uring_setup`, `io_uring_enter`, `io_uring_register` (prevents bypass via io_uring network operations)
   - `fork`, `vfork`, and `clone` unless `CLONE_THREAD` is present (decoder
     threads remain available, child processes do not)
   - `setsid`, `setpgid` (preserves the private session and process group)

Decode and pipe IPC continue to work because the filter is
**mismatch = Allow**. Only the listed network syscalls match. Filter compilation
or installation failure aborts worker spawn rather than silently weakening it.

## Packaging layer (still recommended)

- Flatpak: no `--share=network` (see `packaging/flatpak/…`)
- Optional tighter **default-deny** allowlist once C-decoder feature sets are
  fully enumerated under load (AVIF/HEIC with system libs)

## Verification

The filter is installed by the parent process immediately before it executes
the worker. Invoking `viewr-decode` directly does not exercise that boundary.
On Linux, `cargo test -p viewr worker_limit::tests` spawns a contained child and
asserts that `socket` and `io_uring_setup` fail with `EPERM`, `clone3` reports
`ENOSYS`, a same-process thread can still run, and a descendant process cannot
be created. A package-level test must additionally launch a decode through
`viewr` inside Flatpak.
