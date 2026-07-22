//! Process-level tests for the versioned decode-worker protocol.

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn worker_accepts_protocol_probe_then_framed_encoded_input() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_viewr-decode"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    viewr_protocol::write_decode_request(&mut stdin, viewr_protocol::PROBE_FORMAT, &[]).unwrap();
    stdin.flush().unwrap();
    assert_eq!(
        viewr_protocol::read_worker_response(&mut stdout).unwrap(),
        viewr_protocol::WorkerResponse::Probe
    );

    viewr_protocol::write_decode_request(&mut stdin, "avif", b"malformed image").unwrap();
    stdin.flush().unwrap();

    let response = viewr_protocol::read_worker_response(&mut stdout).unwrap();
    let viewr_protocol::WorkerResponse::Error(message) = response else {
        panic!("expected worker error response");
    };
    #[cfg(not(feature = "avif"))]
    assert!(message.starts_with("AVIF support requires"));
    #[cfg(feature = "avif")]
    assert!(!message.is_empty());
    assert!(message.len() < 512);
    assert!(!message.contains("malformed image"));

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}
