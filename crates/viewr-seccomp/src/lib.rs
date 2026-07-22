//! Shared Linux seccomp policies for viewr and its isolated decode worker.

#![cfg(target_os = "linux")]
#![allow(unsafe_code)] // Policy activation and probes invoke integer-only Linux syscalls.

use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};
use std::collections::BTreeMap;
use std::convert::TryInto;
use std::io;

/// A compiled filter that can be installed without allocation or formatting.
pub struct CompiledFilter(BpfProgram);

impl CompiledFilter {
    /// Install this filter on the calling thread, failing closed with `EPERM`.
    ///
    /// # Errors
    ///
    /// Returns `EPERM` when the kernel rejects the compiled filter.
    pub fn apply(&self) -> io::Result<()> {
        seccompiler::apply_filter(&self.0).map_err(|_| io::Error::from_raw_os_error(libc::EPERM))
    }
}

fn target_architecture() -> io::Result<TargetArch> {
    TargetArch::try_from(std::env::consts::ARCH)
        .map_err(|error| io::Error::other(error.to_string()))
}

fn compile_filter(filter: SeccompFilter) -> io::Result<CompiledFilter> {
    filter
        .try_into()
        .map(CompiledFilter)
        .map_err(|error: seccompiler::BackendError| io::Error::other(error.to_string()))
}

fn unconditional_syscalls(syscalls: &[i64]) -> BTreeMap<i64, Vec<SeccompRule>> {
    syscalls
        .iter()
        .copied()
        .map(|syscall| (syscall, Vec::new()))
        .collect()
}

#[cfg(target_arch = "x86_64")]
const X32_SYSCALL_BIT: i64 = 0x4000_0000;

// x32 syscalls share AUDIT_ARCH_X86_64 but set bit 30 in the syscall number.
// Every default-allow denial therefore needs an identical x32 rule.
#[cfg(target_arch = "x86_64")]
fn insert_x32_aliases(rules: &mut BTreeMap<i64, Vec<SeccompRule>>) {
    let aliases = rules
        .iter()
        .map(|(syscall, syscall_rules)| (*syscall | X32_SYSCALL_BIT, syscall_rules.clone()))
        .collect::<Vec<_>>();
    rules.extend(aliases);
}

#[cfg(not(target_arch = "x86_64"))]
fn insert_x32_aliases(_: &mut BTreeMap<i64, Vec<SeccompRule>>) {}

fn non_unix_socket_rule() -> io::Result<SeccompRule> {
    let condition = SeccompCondition::new(
        0,
        SeccompCmpArgLen::Qword,
        SeccompCmpOp::Ne,
        u64::try_from(libc::AF_UNIX)
            .map_err(|_| io::Error::other("AF_UNIX is not representable"))?,
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    SeccompRule::new(vec![condition]).map_err(|error| io::Error::other(error.to_string()))
}

/// Compile the application policy that permits local Unix IPC but denies Internet sockets.
///
/// `connect` remains available because seccomp cannot dereference its socket-address
/// pointer. The only socket descriptors the process can create after this filter
/// are Unix-domain descriptors, so local display, portal, and AT-SPI connections
/// continue to work while Internet and other socket families fail with `EPERM`.
/// `io_uring` is denied because its socket operation could bypass `SYS_socket`.
///
/// # Errors
///
/// Returns an error when a rule cannot be represented for the target architecture
/// or the resulting filter cannot be compiled.
pub fn application_internet_deny_filter() -> io::Result<CompiledFilter> {
    let mut rules = unconditional_syscalls(&[
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
    ]);
    rules.insert(libc::SYS_socket, vec![non_unix_socket_rule()?]);
    rules.insert(libc::SYS_socketpair, vec![non_unix_socket_rule()?]);
    insert_x32_aliases(&mut rules);

    compile_filter(
        SeccompFilter::new(
            rules,
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM as u32),
            target_architecture()?,
        )
        .map_err(|error| io::Error::other(error.to_string()))?,
    )
}

/// Install and verify the application Internet-socket policy on the calling thread.
///
/// Call this before creating application threads. Linux threads inherit seccomp
/// filters, so all later GUI, decode, and accessibility work receives the same
/// boundary. Failure is returned to the caller rather than silently weakening it.
///
/// # Errors
///
/// Returns an error when privilege locking or filter installation fails, or when
/// the post-installation Internet-socket probe does not fail with `EPERM`.
pub fn apply_application_internet_policy() -> io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    application_internet_deny_filter()?.apply()?;

    let socket = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if socket >= 0 {
        unsafe {
            libc::close(socket);
        }
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "application Internet-socket policy did not activate",
        ));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(libc::EPERM) {
        return Err(error);
    }
    Ok(())
}

/// Compile the baseline default-allow policy that denies network and processes.
///
/// # Errors
///
/// Returns an error when a rule cannot be represented for the target architecture
/// or the resulting filter cannot be compiled.
pub fn network_deny_filter() -> io::Result<CompiledFilter> {
    let mut rules = unconditional_syscalls(&[
        libc::SYS_socket,
        libc::SYS_connect,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_getsockopt,
        libc::SYS_setsockopt,
        libc::SYS_sendto,
        libc::SYS_recvfrom,
        libc::SYS_sendmsg,
        libc::SYS_recvmsg,
        libc::SYS_sendmmsg,
        libc::SYS_recvmmsg,
        libc::SYS_shutdown,
        libc::SYS_fork,
        libc::SYS_vfork,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_setpgid,
        libc::SYS_setsid,
        libc::SYS_socketpair,
    ]);
    let process_clone = SeccompCondition::new(
        0,
        SeccompCmpArgLen::Qword,
        SeccompCmpOp::MaskedEq(libc::CLONE_THREAD as u64),
        0,
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    rules.insert(
        libc::SYS_clone,
        vec![
            SeccompRule::new(vec![process_clone])
                .map_err(|error| io::Error::other(error.to_string()))?,
        ],
    );
    insert_x32_aliases(&mut rules);

    compile_filter(
        SeccompFilter::new(
            rules,
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM as u32),
            target_architecture()?,
        )
        .map_err(|error| io::Error::other(error.to_string()))?,
    )
}

/// Compile a compatibility policy that makes libc replace `clone3` with `clone`.
///
/// # Errors
///
/// Returns an error when the target architecture is unsupported or the filter
/// cannot be compiled.
pub fn clone3_compat_filter() -> io::Result<CompiledFilter> {
    let mut rules = BTreeMap::from([(libc::SYS_clone3, Vec::new())]);
    insert_x32_aliases(&mut rules);
    compile_filter(
        SeccompFilter::new(
            rules,
            SeccompAction::Allow,
            SeccompAction::Errno(libc::ENOSYS as u32),
            target_architecture()?,
        )
        .map_err(|error| io::Error::other(error.to_string()))?,
    )
}

fn read_only_openat_rule() -> io::Result<SeccompRule> {
    let temporary_file_bit = libc::O_TMPFILE & !libc::O_DIRECTORY;
    let forbidden_flags = libc::O_ACCMODE
        | libc::O_CREAT
        | libc::O_TRUNC
        | libc::O_APPEND
        | libc::O_PATH
        | temporary_file_bit;
    let condition = SeccompCondition::new(
        2,
        SeccompCmpArgLen::Qword,
        SeccompCmpOp::MaskedEq(
            u64::try_from(forbidden_flags)
                .map_err(|_| io::Error::other("open flags are not representable"))?,
        ),
        0,
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    SeccompRule::new(vec![condition]).map_err(|error| io::Error::other(error.to_string()))
}

fn c_decoder_allow_filter() -> io::Result<CompiledFilter> {
    let mut rules = unconditional_syscalls(&[
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_close,
        libc::SYS_fstat,
        libc::SYS_newfstatat,
        libc::SYS_statx,
        libc::SYS_lseek,
        libc::SYS_pread64,
        libc::SYS_getdents64,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_brk,
        libc::SYS_madvise,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_sigaltstack,
        libc::SYS_futex,
        libc::SYS_set_robust_list,
        libc::SYS_rseq,
        libc::SYS_sched_getaffinity,
        libc::SYS_sched_yield,
        libc::SYS_clock_gettime,
        libc::SYS_clock_nanosleep,
        libc::SYS_nanosleep,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_exit,
        libc::SYS_exit_group,
        // The stacked compatibility filter changes this allowed call to ENOSYS.
        libc::SYS_clone3,
    ]);
    rules.insert(libc::SYS_openat, vec![read_only_openat_rule()?]);
    let thread_clone = SeccompCondition::new(
        0,
        SeccompCmpArgLen::Qword,
        SeccompCmpOp::MaskedEq(libc::CLONE_THREAD as u64),
        libc::CLONE_THREAD as u64,
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    rules.insert(
        libc::SYS_clone,
        vec![
            SeccompRule::new(vec![thread_clone])
                .map_err(|error| io::Error::other(error.to_string()))?,
        ],
    );

    compile_filter(
        SeccompFilter::new(
            rules,
            SeccompAction::Errno(libc::EPERM as u32),
            SeccompAction::Allow,
            target_architecture()?,
        )
        .map_err(|error| io::Error::other(error.to_string()))?,
    )
}

/// Install the feature-gated production C-decoder policy and verify it is active.
///
/// # Errors
///
/// Returns an error when policy compilation or installation fails, or when the
/// post-installation default-deny probe does not fail with `EPERM`.
pub fn apply_production_c_decoder_policy() -> io::Result<()> {
    let clone3_filter = clone3_compat_filter()?;
    let allow_filter = c_decoder_allow_filter()?;
    clone3_filter.apply()?;
    allow_filter.apply()?;

    // `getuid` is harmless and deliberately absent. A successful call proves
    // that the default-deny filter did not take effect.
    let result = unsafe { libc::syscall(libc::SYS_getuid) };
    let error = io::Error::last_os_error();
    if result != -1 || error.raw_os_error() != Some(libc::EPERM) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "default-deny seccomp policy did not activate",
        ));
    }
    Ok(())
}
