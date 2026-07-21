//! OS-level limits for the `viewr-decode` helper process.
//!
//! - All platforms: terminate discarded workers; isolate process group (Unix).
//! - Windows: Job Object with kill-on-close.
//! - Linux: `no_new_privs`, non-dumpable, and a seccomp-bpf filter that **allows
//!   by default** but returns `EPERM` for network syscalls (socket/connect/…).

#![allow(unsafe_code)] // Win32 Job Object APIs, Unix process-group, Linux prctl/seccomp

use std::process::Child;

/// Apply platform limits after a successful spawn.
pub(crate) fn harden_child(child: &Child) {
    #[cfg(windows)]
    windows::assign_to_kill_on_close_job(child);

    #[cfg(not(windows))]
    {
        let _ = child;
    }
}

/// Configure the command before spawn (Unix process group, Linux privileges).
pub(crate) fn configure_command(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: pre_exec runs post-fork pre-exec; only async-signal-safe work.
        unsafe {
            cmd.pre_exec(|| {
                linux::apply_worker_sandbox();
                Ok(())
            });
        }
    }

    let _ = cmd;
}

/// Best-effort kill of a worker we are discarding.
pub(crate) fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(target_os = "linux")]
mod linux {
    use seccompiler::{SeccompAction, SeccompFilter, TargetArch};
    use std::collections::BTreeMap;
    use std::convert::TryInto;

    /// Install process-wide worker restrictions. Best-effort: never aborts spawn.
    pub(super) fn apply_worker_sandbox() {
        // Prevent privilege regain after exec.
        let _ = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        // Reduce ptrace/core leak surface.
        let _ = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };

        if let Ok(filter) = network_deny_filter() {
            // apply_filter installs on the calling thread (the child, pre-exec).
            let _ = seccompiler::apply_filter(&filter);
        }
    }

    /// Default-allow filter that fails closed on network-related syscalls.
    fn network_deny_filter() -> Result<seccompiler::BpfProgram, seccompiler::Error> {
        // Empty rule vec = match syscall regardless of args.
        let deny_syscalls: &[i64] = &[
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
            // socketpair can be used for IPC but also networking patterns; deny
            // keeps the worker honest (parent uses pipes + shmem only).
            libc::SYS_socketpair,
        ];

        let mut map: BTreeMap<i64, Vec<seccompiler::SeccompRule>> = BTreeMap::new();
        for &nr in deny_syscalls {
            map.insert(nr, Vec::new());
        }

        // mismatch_action = Allow (everything else)
        // match_action = EPERM on listed network syscalls
        let arch = TargetArch::try_from(std::env::consts::ARCH).map_err(|()| {
            seccompiler::Error::InvalidArgument("unsupported arch for seccomp".into())
        })?;
        let filter = SeccompFilter::new(
            map,
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM as u32),
            arch,
        )?;
        filter.try_into()
    }
}

#[cfg(windows)]
mod windows {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    struct JobHandle(HANDLE);

    // SAFETY: HANDLE is process-local and only touched from this crate.
    unsafe impl Send for JobHandle {}
    // SAFETY: OnceLock init is single-threaded; later use is Assign only.
    unsafe impl Sync for JobHandle {}

    impl Drop for JobHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                // SAFETY: we own this job handle.
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    static WORKER_JOB: OnceLock<Option<JobHandle>> = OnceLock::new();

    fn shared_job() -> Option<HANDLE> {
        WORKER_JOB
            .get_or_init(create_kill_on_close_job)
            .as_ref()
            .map(|j| j.0)
    }

    fn create_kill_on_close_job() -> Option<JobHandle> {
        // SAFETY: null name/security attributes create an anonymous job.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return None;
        }

        let mut basic = unsafe { std::mem::zeroed::<JOBOBJECT_BASIC_LIMIT_INFORMATION>() };
        basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let mut info = unsafe { std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() };
        info.BasicLimitInformation = basic;

        // SAFETY: `info` is a valid JOBOBJECT_EXTENDED_LIMIT_INFORMATION.
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::from_mut(&mut info).cast(),
                u32::try_from(std::mem::size_of_val(&info)).unwrap_or(u32::MAX),
            )
        };
        if ok == 0 {
            // SAFETY: handle still owned here after failed configure.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return None;
        }

        Some(JobHandle(handle))
    }

    pub(super) fn assign_to_kill_on_close_job(child: &Child) {
        let Some(job) = shared_job() else {
            return;
        };
        // SAFETY: Child's process handle is valid for the life of `child`.
        let process = child.as_raw_handle() as HANDLE;
        // SAFETY: job from CreateJobObjectW; process is the live child handle.
        let _ = unsafe { AssignProcessToJobObject(job, process) };
    }
}
