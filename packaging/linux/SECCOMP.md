# Linux seccomp for viewr and `viewr-decode`

## Application startup policy

`crates/viewr/src/privacy.rs` applies the shared
`viewr_seccomp::apply_application_internet_policy` before logging, worker setup,
GUI initialization, or application threads. The policy:

1. requires every configured `DBUS_SESSION_BUS_ADDRESS` and `AT_SPI_BUS_ADDRESS`
   entry to use a `unix:` transport;
2. sets `PR_SET_NO_NEW_PRIVS` and aborts startup on failure;
3. allows `socket` and `socketpair` only when their family is `AF_UNIX`, preserving
   local Wayland/X11, portal, D-Bus, and AT-SPI IPC;
4. returns `EPERM` for every other socket family and for `io_uring_setup`,
   `io_uring_enter`, and `io_uring_register`; and
5. mirrors every blocked rule onto its bit-30 x32 syscall alias on x86-64, where
   x32 shares the native audit architecture; and
6. probes an `AF_INET` socket and aborts unless the kernel returns `EPERM`.

`connect` remains available because classic seccomp cannot dereference the target
address pointer. Only Unix-domain descriptors can be created after the policy is
installed, and it runs before application-owned threads so every descendant thread
inherits it. The network-denied Flatpak profile remains an independent package
boundary.

## Decode-worker baseline policy

Implemented in the shared `viewr-seccomp` crate and applied by
`crates/viewr/src/worker_limit.rs`:

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
   On x86-64, every listed denial is also installed for the corresponding x32
   syscall number so the shared audit architecture cannot bypass the filter.

Decode and pipe IPC continue to work because the filter is
**mismatch = Allow**. Only the listed network syscalls match. Filter compilation
or installation failure aborts worker spawn rather than silently weakening it.

## Feature-gated C-decoder allowlist

Linux workers built with `avif` or `heic` install a second policy inside
`viewr-decode`, after the dynamic loader has started the process and before the
first protocol frame is read. This policy is **default deny** with `EPERM` and
allows only these syscall groups:

- Pipe lifecycle: `read`, `write`, `close`, `exit`, `exit_group`
- Bounded memory management: `brk`, `mmap`, `mprotect`, `munmap`, `mremap`,
  `madvise`
- Read-only decoder/plugin discovery: `fstat`, `newfstatat`, `statx`, `lseek`,
  `pread64`, `getdents64`, and argument-filtered `openat`
- Thread and signal runtime: `clone` only when `CLONE_THREAD` is set, `futex`,
  `rseq`, `set_robust_list`, `rt_sigaction`, `rt_sigprocmask`, `sigaltstack`,
  `getpid`, `gettid`, `sched_getaffinity`, and `sched_yield`
- Time waits: `clock_gettime`, `clock_nanosleep`, `nanosleep`

`openat` is rejected when its flags request write access, creation, truncation,
append, path-only handles, or unnamed temporary files. A stacked compatibility
filter returns `ENOSYS` for `clone3`, forcing libc through the argument-filtered
`clone` rule. The worker then invokes the harmless, deliberately unlisted
`getuid` syscall and refuses startup unless it observes `EPERM`, proving the
default-deny filter became active.

Everything else, including process creation, execution, cross-process signals,
networking, io_uring, ptrace, BPF, keyrings, mounts, and new filesystem mutation
paths, remains denied. Filter compilation or installation failure aborts worker
startup.

## Packaging layer

- Flatpak: no `--share=network` (see `packaging/flatpak/…`)
- seccomp reduces the kernel surface but is not a complete sandbox. The
  argument-filtered read-only `openat` exists so system libheif can discover its
  decoder plugin; the Flatpak filesystem boundary remains necessary.

## Verification

The baseline network/process filter is installed by the parent process
immediately before it executes the worker. Invoking `viewr-decode` directly does
not exercise that baseline boundary. On Linux,
`cargo test -p viewr worker_limit::tests` checks it. Feature-enabled workers
install the production default-deny policy themselves, including when invoked
directly. `cargo test -p viewr-seccomp` proves that the application policy
preserves Unix IPC and threads while denying IPv4, IPv6, io_uring, and the x32
socket alias on x86-64. A dedicated baseline-worker child proves both native and
x32 socket denials independently of the default-deny C filter. The C-worker test
also proves that the policy allows a real thread and read-only open while denying
write-open, asynchronous-signal configuration, socket, fork, and direct
cross-process signal calls, and that `clone3` reports `ENOSYS`.

The `c-decoder-policy` CI job runs on Ubuntu 24.04 with the pinned Rust toolchain,
libheif 1.17, and native AV1/HEVC codecs. It generates small AVIF and HEIC images
in memory, launches the release-mode worker through the real framed protocol,
and requires both decodes to complete under the installed default-deny policy.
A package-level test additionally launches a worker probe through `viewr` inside
Flatpak.
