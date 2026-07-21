# Linux seccomp for `viewr-decode`

## Applied at runtime (worker `pre_exec`)

Implemented in `crates/viewr/src/worker_limit.rs` via `seccompiler`:

1. `PR_SET_NO_NEW_PRIVS` — cannot regain privileges after exec  
2. `PR_SET_DUMPABLE=0` — reduces ptrace/core leak surface  
3. **Default-allow seccomp-bpf** that returns `EPERM` for network syscalls:
   - `socket`, `socketpair`, `connect`, `accept`, `accept4`, `bind`, `listen`
   - `sendto`, `recvfrom`, `sendmsg`, `recvmsg`, `sendmmsg`, `recvmmsg`
   - `getsockopt`, `setsockopt`, `shutdown`

Decode, pipes, and shared memory continue to work because the filter is
**mismatch = Allow**. Only the listed network syscalls match and fail closed.

## Packaging layer (still recommended)

- Flatpak: no `--share=network` (see `packaging/flatpak/…`)
- Optional tighter **default-deny** allowlist once C-decoder feature sets are
  fully enumerated under load (AVIF/HEIC with system libs)

## Verification

```text
# On Linux, after building viewr + worker:
strace -f -e network target/release/viewr-decode </dev/null
# Network syscalls from a hostile decoder should return EPERM
```