//! OS-level limits for the `viewr-decode` helper process.
//!
//! - All platforms: terminate discarded workers; isolate a private session (Unix).
//! - Windows: per-worker Job Object with kill-on-close, one-process, and
//!   containment-wide memory limits.
//! - Linux: `no_new_privs`, non-dumpable, and seccomp-bpf filters that deny
//!   networking and child-process creation while allowing decoder threads.
//! - macOS: a private session plus an address-space limit; signed packages add
//!   the inherited App Sandbox boundary.
//! - Other supported Unix targets: a private session plus address-space and
//!   one-process resource limits.

#![allow(unsafe_code)] // Win32 Job Object APIs, Unix process-group, Linux prctl/seccomp

use std::process::Child;

const WORKER_MAX_ADDRESS_SPACE_BYTES: u64 = 1536 * 1024 * 1024;

/// Keeps platform lifetime controls alive for as long as the worker exists.
pub(crate) struct WorkerGuard {
    killer: WorkerKiller,
}

/// A cloneable capability that terminates exactly one worker containment unit.
#[derive(Clone)]
pub(crate) struct WorkerKiller {
    #[cfg(windows)]
    job: std::sync::Arc<windows::JobHandle>,
    #[cfg(unix)]
    process_group: i32,
}

impl WorkerGuard {
    /// Retain a scoped termination capability for deadline enforcement.
    pub(crate) fn killer(&self) -> WorkerKiller {
        self.killer.clone()
    }
}

impl WorkerKiller {
    /// Kill the worker and every descendant that remains in its containment unit.
    pub(crate) fn terminate(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            // SAFETY: the child is placed in a private process group whose id is
            // its PID before exec. A negative id targets only that group.
            if unsafe { libc::kill(-self.process_group, libc::SIGKILL) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }

        #[cfg(windows)]
        windows::terminate_job(&self.job)?;

        Ok(())
    }
}

/// Apply platform limits after a successful spawn.
// Windows creates and assigns a Job Object here; Unix has no post-spawn step.
// Keep one fallible API so the caller cannot accidentally ignore Windows setup.
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn harden_child(child: &Child) -> std::io::Result<WorkerGuard> {
    #[cfg(windows)]
    {
        windows::create_and_assign_kill_on_close_job(child).map(|job| WorkerGuard {
            killer: WorkerKiller {
                job: std::sync::Arc::new(job),
            },
        })
    }

    #[cfg(unix)]
    {
        let process_group = i32::try_from(child.id())
            .map_err(|_| std::io::Error::other("worker process id is not representable"))?;
        Ok(WorkerGuard {
            killer: WorkerKiller { process_group },
        })
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = child;
        Ok(WorkerGuard {
            killer: WorkerKiller {},
        })
    }
}

/// Configure the command before spawn (Unix private session, Linux privileges).
// The Windows build has no fallible pre-spawn operation, while Linux compiles a
// seccomp filter here. Keep one cross-platform Result API for the caller.
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn configure_command(cmd: &mut std::process::Command) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    let network_filter = linux::network_deny_filter()?;
    #[cfg(target_os = "linux")]
    let clone3_filter = linux::clone3_compat_filter()?;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: Linux compiles its filter in the parent. The child closure
        // only invokes setrlimit, prctl, and seccomp syscalls, then reports any
        // failure through Command::spawn.
        unsafe {
            cmd.pre_exec(move || {
                unix::create_private_session()?;
                unix::apply_address_space_limit()?;
                #[cfg(target_os = "linux")]
                linux::apply_worker_sandbox(&network_filter, &clone3_filter)?;
                Ok(())
            });
        }
    }

    let _ = cmd;
    Ok(())
}

#[cfg(unix)]
mod unix {
    pub(super) fn create_private_session() -> std::io::Result<()> {
        // A session leader cannot move to another process group. This makes
        // the retained group-kill capability stable for the worker lifetime.
        // SAFETY: setsid has no pointer arguments and runs in the single-threaded
        // post-fork child before exec.
        if unsafe { libc::setsid() } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn apply_address_space_limit() -> std::io::Result<()> {
        let limit = libc::rlim_t::try_from(super::WORKER_MAX_ADDRESS_SPACE_BYTES)
            .map_err(|_| std::io::Error::from_raw_os_error(libc::EOVERFLOW))?;
        let limits = libc::rlimit {
            rlim_cur: limit,
            rlim_max: limit,
        };
        // SAFETY: `limits` is a valid rlimit structure and the worker is the
        // only thread in the post-fork child.
        if unsafe { libc::setrlimit(libc::RLIMIT_AS, &raw const limits) } != 0 {
            return Err(std::io::Error::last_os_error());
        }

        // C decoders may create threads but never need subprocesses. On the
        // supported BSD targets, RLIMIT_NPROC=1 prevents a compromised worker
        // from creating a child that escapes its process group. macOS defines
        // RLIMIT_NPROC per user rather than per process, so lowering it to one
        // would reject worker startup whenever the user already has a process.
        #[cfg(any(
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        {
            let process_limit = libc::rlimit {
                rlim_cur: 1,
                rlim_max: 1,
            };
            if unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &raw const process_limit) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

/// Best-effort kill of a worker containment unit we are discarding.
pub(crate) fn terminate(child: &mut Child, guard: &WorkerGuard) {
    let _ = guard.killer.terminate();
    // Direct-handle fallback covers a platform containment failure without a
    // PID lookup and therefore cannot target an unrelated process.
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(target_os = "linux")]
mod linux {
    /// Install process-wide worker restrictions, aborting spawn on any failure.
    pub(super) fn apply_worker_sandbox(
        network_filter: &viewr_seccomp::CompiledFilter,
        clone3_filter: &viewr_seccomp::CompiledFilter,
    ) -> std::io::Result<()> {
        // Prevent privilege regain after exec.
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Returning ENOSYS for clone3 lets libc fall back to clone, whose flags
        // the policy filter restricts to same-process thread creation.
        if clone3_filter.apply().is_err() || network_filter.apply().is_err() {
            // Avoid formatting or heap allocation in the post-fork child.
            return Err(std::io::Error::from_raw_os_error(libc::EPERM));
        }
        Ok(())
    }

    /// Default-allow filter that fails closed on network-related syscalls.
    pub(super) fn network_deny_filter() -> std::io::Result<viewr_seccomp::CompiledFilter> {
        viewr_seccomp::network_deny_filter()
    }

    /// Make libc treat clone3 as unavailable so thread creation uses filtered clone.
    pub(super) fn clone3_compat_filter() -> std::io::Result<viewr_seccomp::CompiledFilter> {
        viewr_seccomp::clone3_compat_filter()
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_BASIC_LIMIT_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    pub(super) struct JobHandle(HANDLE);

    // SAFETY: Windows kernel handles are process-wide and may be closed from a
    // different thread. Ownership remains unique inside `JobHandle`.
    unsafe impl Send for JobHandle {}
    // SAFETY: Windows Job handles are process-wide kernel capabilities and the
    // operations used here support concurrent calls.
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

    fn create_kill_on_close_job() -> io::Result<JobHandle> {
        // SAFETY: null name/security attributes create an anonymous job.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        let mut basic = unsafe { std::mem::zeroed::<JOBOBJECT_BASIC_LIMIT_INFORMATION>() };
        basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_JOB_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        basic.ActiveProcessLimit = 1;
        let mut info = unsafe { std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() };
        info.BasicLimitInformation = basic;
        info.ProcessMemoryLimit = usize::try_from(super::WORKER_MAX_ADDRESS_SPACE_BYTES)
            .map_err(|_| io::Error::other("worker memory limit is not representable"))?;
        info.JobMemoryLimit = info.ProcessMemoryLimit;

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
            let error = io::Error::last_os_error();
            // SAFETY: handle still owned here after failed configure.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(error);
        }

        Ok(JobHandle(handle))
    }

    pub(super) fn create_and_assign_kill_on_close_job(child: &Child) -> io::Result<JobHandle> {
        let job = create_kill_on_close_job()?;
        // SAFETY: Child's process handle is valid for the life of `child`.
        let process = child.as_raw_handle() as HANDLE;
        // SAFETY: job from CreateJobObjectW; process is the live child handle.
        if unsafe { AssignProcessToJobObject(job.0, process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    pub(super) fn terminate_job(job: &JobHandle) -> io::Result<()> {
        // SAFETY: the retained handle names only this worker's Job Object.
        if unsafe { TerminateJobObject(job.0, 124) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{configure_command, harden_child};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    const CHILD_FLAG: &str = "VIEWR_TEST_TIMEOUT_CHILD";
    #[cfg(any(
        windows,
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    const PROCESS_CHILD_FLAG: &str = "VIEWR_TEST_PROCESS_CHILD";
    #[cfg(unix)]
    const GROUP_ESCAPE_CHILD_FLAG: &str = "VIEWR_TEST_GROUP_ESCAPE_CHILD";
    #[cfg(target_os = "linux")]
    const NETWORK_CHILD_FLAG: &str = "VIEWR_TEST_NETWORK_CHILD";

    #[test]
    fn timeout_child_waits_only_when_spawned_by_termination_test() {
        if std::env::var_os(CHILD_FLAG).is_some() {
            std::thread::sleep(Duration::from_mins(1));
        }
    }

    #[test]
    fn containment_termination_stops_private_worker_group() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("worker_limit::tests::timeout_child_waits_only_when_spawned_by_termination_test")
            .arg("--exact")
            .env(CHILD_FLAG, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let guard = harden_child(&child).unwrap();

        std::thread::sleep(Duration::from_millis(100));
        guard.killer().terminate().unwrap();
        assert!(!child.wait().unwrap().success());
    }

    #[test]
    #[cfg(any(
        windows,
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    fn process_creation_child_observes_containment_denial() {
        if std::env::var_os(PROCESS_CHILD_FLAG).is_none() {
            return;
        }

        let mut ready = [0_u8; 1];
        std::io::Read::read_exact(&mut std::io::stdin(), &mut ready).unwrap();
        match Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .spawn()
        {
            Err(_) => {}
            Ok(mut descendant) => {
                let _ = descendant.kill();
                let _ = descendant.wait();
                panic!("worker containment allowed descendant process creation");
            }
        }
    }

    #[test]
    #[cfg(any(
        windows,
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    fn containment_denies_descendant_process_creation() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("worker_limit::tests::process_creation_child_observes_containment_denial")
            .arg("--exact")
            .env(PROCESS_CHILD_FLAG, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let _guard = harden_child(&child).unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), &[1]).unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[cfg(unix)]
    #[test]
    fn group_escape_child_cannot_leave_private_session() {
        if std::env::var_os(GROUP_ESCAPE_CHILD_FLAG).is_none() {
            return;
        }

        let target_group = std::env::var("VIEWR_TEST_PARENT_GROUP")
            .unwrap()
            .parse::<i32>()
            .unwrap();
        // SAFETY: zero targets the current test process; the group id was
        // parsed from a parent-controlled environment variable.
        assert_eq!(unsafe { libc::setpgid(0, target_group) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EPERM)
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_session_prevents_process_group_escape() {
        // SAFETY: getpgrp has no arguments or memory-safety preconditions.
        let parent_group = unsafe { libc::getpgrp() };
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("worker_limit::tests::group_escape_child_cannot_leave_private_session")
            .arg("--exact")
            .env(GROUP_ESCAPE_CHILD_FLAG, "1")
            .env("VIEWR_TEST_PARENT_GROUP", parent_group.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let _guard = harden_child(&child).unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn network_syscall_child_observes_seccomp_denial() {
        if std::env::var_os(NETWORK_CHILD_FLAG).is_none() {
            return;
        }

        // clone3 must fall back to the argument-filtered clone path, where
        // CLONE_THREAD remains available to native decoder libraries.
        assert_eq!(std::thread::spawn(|| 42).join().unwrap(), 42);

        for (syscall, argument) in [
            (libc::SYS_socket, libc::c_long::from(libc::AF_INET)),
            (libc::SYS_io_uring_setup, 1),
        ] {
            // SAFETY: deliberately invokes the syscall with inert arguments;
            // the installed filter must reject it before argument inspection.
            let result = unsafe { libc::syscall(syscall, argument, 0, 0) };
            assert_eq!(result, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EPERM)
            );
        }
        // clone3 is reported as unavailable so libc can fall back to the
        // clone path that permits threads but rejects child processes.
        let result = unsafe { libc::syscall(libc::SYS_clone3, std::ptr::null::<u8>(), 0) };
        assert_eq!(result, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOSYS)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn seccomp_denies_classic_and_io_uring_network_paths_at_runtime() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("worker_limit::tests::network_syscall_child_observes_seccomp_denial")
            .arg("--exact")
            .env(NETWORK_CHILD_FLAG, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let _guard = harden_child(&child).unwrap();
        let status = child.wait().unwrap();
        assert!(status.success());
    }
}
