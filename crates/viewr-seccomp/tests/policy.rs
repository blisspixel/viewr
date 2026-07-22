//! Runtime verification for the feature-gated default-deny policy.

#![cfg(target_os = "linux")]
#![allow(unsafe_code)] // Tests invoke inert syscalls to verify filter outcomes.

use std::process::{Command, Stdio};

const POLICY_CHILD: &str = "VIEWR_TEST_C_DECODER_POLICY_CHILD";

fn assert_errno(result: libc::c_long, expected: i32) {
    assert_eq!(result, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(expected)
    );
}

#[test]
fn policy_child_observes_default_deny_and_thread_only_clone() {
    if std::env::var_os(POLICY_CHILD).is_none() {
        return;
    }

    let parent_pid = unsafe { libc::getppid() };
    let mut inherited_pipe = [-1; 2];
    assert_eq!(
        unsafe { libc::pipe2(inherited_pipe.as_mut_ptr(), libc::O_CLOEXEC) },
        0
    );
    viewr_seccomp::apply_production_c_decoder_policy().unwrap();
    assert_eq!(std::thread::spawn(|| 42).join().unwrap(), 42);

    let read_fd = unsafe {
        libc::syscall(
            libc::SYS_openat,
            libc::AT_FDCWD,
            c"/dev/null".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
            0,
        )
    };
    assert!(read_fd >= 0);
    assert_eq!(unsafe { libc::syscall(libc::SYS_close, read_fd) }, 0);

    assert_errno(
        unsafe {
            libc::syscall(
                libc::SYS_openat,
                libc::AT_FDCWD,
                c"/dev/null".as_ptr(),
                libc::O_WRONLY | libc::O_CLOEXEC,
                0,
            )
        },
        libc::EPERM,
    );
    assert_errno(
        unsafe {
            libc::syscall(
                libc::SYS_fcntl,
                inherited_pipe[0],
                libc::F_SETOWN,
                parent_pid,
            )
        },
        libc::EPERM,
    );
    assert_errno(
        unsafe { libc::syscall(libc::SYS_socket, libc::AF_INET, libc::SOCK_STREAM, 0) },
        libc::EPERM,
    );
    for fd in inherited_pipe {
        assert_eq!(unsafe { libc::syscall(libc::SYS_close, fd) }, 0);
    }
    assert_errno(unsafe { libc::syscall(libc::SYS_fork) }, libc::EPERM);
    assert_errno(
        unsafe { libc::syscall(libc::SYS_tgkill, parent_pid, parent_pid, 0) },
        libc::EPERM,
    );
    assert_errno(
        unsafe { libc::syscall(libc::SYS_clone3, std::ptr::null::<u8>(), 0) },
        libc::ENOSYS,
    );
}

#[test]
fn production_policy_is_default_deny_at_runtime() {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("policy_child_observes_default_deny_and_thread_only_clone")
        .arg("--exact")
        .env(POLICY_CHILD, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(child.wait().unwrap().success());
}
