//! OS-level limits for the `viewr-decode` helper process.
//!
//! Goal: if the main process dies or a worker is discarded, helpers do not
//! linger as orphans, and on Linux the child starts with `no_new_privs`.
//! Full `seccomp-bpf` syscall allow-lists and App Container packaging remain
//! packaging work on top of this foundation.

#![allow(unsafe_code)] // Win32 Job Object APIs, Unix process-group, Linux prctl

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
        // New process group so the helper is not in the UI session group.
        cmd.process_group(0);
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: pre_exec runs in the child after fork, before exec. We only
        // call prctl(PR_SET_NO_NEW_PRIVS) which is async-signal-safe and does
        // not allocate. Failure is ignored so decode still works on kernels
        // without the feature; hardening is best-effort.
        unsafe {
            cmd.pre_exec(|| {
                let rc = libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
                if rc != 0 {
                    // Non-fatal: worker still runs without the bit.
                    let _ = rc;
                }
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
    // SAFETY: OnceLock initialization is single-threaded; later use is Assign only.
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
