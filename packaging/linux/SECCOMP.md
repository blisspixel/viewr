# Linux seccomp plan for `viewr-decode`

## Already applied (runtime)

In the worker `pre_exec` hook (`worker_limit`):

- `PR_SET_NO_NEW_PRIVS` — cannot regain privileges after exec
- `PR_SET_DUMPABLE=0` — reduces ptrace/core leak surface

## Next step (packaging)

Install a `seccomp-bpf` filter that **denies network syscalls** while allowing
decode + IPC:

Deny (examples): `socket`, `connect`, `bind`, `listen`, `accept`, `sendto`,
`recvfrom`, `sendmsg`, `recvmsg`.

Allow: `read`, `write`, `openat`, `close`, `mmap`, `munmap`, `brk`, `futex`,
`clock_gettime`, `exit_group`, shared-memory related syscalls as required by
the platform.

Implementation options:

1. **libseccomp** C library + `libseccomp-sys` (most maintainable allowlists)
2. Hand-written BPF with `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, …)`

Verification:

```text
# After packaging with a filter:
strace -f -e network ./viewr-decode < /dev/null
# should show denied/blocked network attempts
```

Flatpak already ships without `--share=network` for the whole app
(`packaging/flatpak/…`).