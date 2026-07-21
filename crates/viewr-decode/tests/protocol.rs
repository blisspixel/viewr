//! Process-level tests for the versioned decode-worker protocol.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

#[test]
fn worker_accepts_framed_native_path_and_returns_bounded_error() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_viewr-decode"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    viewr_protocol::write_path_request(&mut stdin, Path::new("folder/photo\n雪.avif")).unwrap();
    stdin.flush().unwrap();

    let response = viewr_protocol::read_worker_response(&mut stdout).unwrap();
    let viewr_protocol::WorkerResponse::Error(message) = response else {
        panic!("expected worker error response");
    };
    assert!(message.starts_with("AVIF support requires"));
    assert!(message.len() < 512);
    assert!(!message.contains("photo"));

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}
