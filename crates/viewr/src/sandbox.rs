//! Out-of-process decode worker client (C-backed formats).
//!
//! This module talks to the `viewr-decode` helper binary over stdin/stdout and
//! a bounded pixel stream. It is process/IPC glue rather than pure image logic,
//! so CI treats this file like other end-to-end surfaces (see `docs/STANDARDS.md`).
//!
//! Workers are lifetime-hardened via [`crate::worker_limit`]: Windows Job Object
//! (kill-on-close) and a private Unix session.

use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use crate::decode::DecodedImage;
use crate::error::Error;
use crate::worker_limit;

const WORKER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const WORKER_TERMINATION_GRACE: Duration = Duration::from_secs(2);
const MAX_IDLE_WORKERS: usize = 2;

/// Decode via the isolated `viewr-decode` worker (AVIF / HEIC / RAW paths).
pub(crate) fn load_via_worker(path: &Path) -> Result<DecodedImage, Error> {
    let worker = get_worker()?;
    run_worker_request_with_timeout(
        worker,
        path.to_owned(),
        WORKER_REQUEST_TIMEOUT,
        WORKER_TERMINATION_GRACE,
    )
}

struct WorkerTransaction {
    result: Result<DecodedImage, Error>,
    reusable_worker: Option<DaemonWorker>,
}

fn run_worker_request_with_timeout(
    worker: DaemonWorker,
    path: PathBuf,
    timeout: Duration,
    termination_grace: Duration,
) -> Result<DecodedImage, Error> {
    run_worker_operation_with_timeout(worker, timeout, termination_grace, move |worker| {
        exchange_with_worker(worker, &path)
    })
}

fn run_worker_operation_with_timeout(
    worker: DaemonWorker,
    timeout: Duration,
    termination_grace: Duration,
    operation: impl FnOnce(&mut DaemonWorker) -> Result<DecodedImage, Error> + Send + 'static,
) -> Result<DecodedImage, Error> {
    let killer = worker.guard.killer();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let request_thread = std::thread::Builder::new()
        .name("viewr-worker-request".into())
        .spawn(move || {
            let mut worker = worker;
            let result = operation(&mut worker);
            let reusable_worker = result.is_ok().then_some(worker);
            let _ = sender.send(WorkerTransaction {
                result,
                reusable_worker,
            });
        })
        .map_err(|error| Error::Decode(format!("failed to start worker request: {error}")))?;

    match receiver.recv_timeout(timeout) {
        Ok(mut transaction) => {
            request_thread
                .join()
                .map_err(|_| Error::Decode("worker request thread failed".into()))?;
            if let Some(worker) = transaction.reusable_worker.take() {
                return_worker(worker);
            }
            transaction.result
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            let termination = killer.terminate();
            let stopped = receiver.recv_timeout(termination_grace).is_ok();
            if stopped {
                request_thread.join().map_err(|_| {
                    Error::Decode("worker request thread failed during cleanup".into())
                })?;
            }
            if let Err(error) = termination {
                return Err(Error::Decode(format!(
                    "worker request timed out and containment termination failed: {error}"
                )));
            }
            if !stopped {
                return Err(Error::Decode(
                    "worker request timed out; cleanup did not finish within the safety grace period"
                        .into(),
                ));
            }
            Err(Error::Decode("worker request timed out".into()))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            request_thread
                .join()
                .map_err(|_| Error::Decode("worker request thread failed".into()))?;
            Err(Error::Decode(
                "worker request ended without a response".into(),
            ))
        }
    }
}

fn exchange_with_worker(worker: &mut DaemonWorker, path: &Path) -> Result<DecodedImage, Error> {
    use std::io::Write;

    if let Err(e) = viewr_protocol::write_path_request(&mut worker.stdin, path) {
        return Err(Error::Decode(format!("failed to send path: {e}")));
    }
    if let Err(e) = worker.stdin.flush() {
        return Err(Error::Decode(format!("failed to flush: {e}")));
    }

    receive_worker_output(worker)
}

fn receive_worker_output(worker: &mut DaemonWorker) -> Result<DecodedImage, Error> {
    use std::io::{Read, Write};

    let response = viewr_protocol::read_worker_response(&mut worker.stdout)
        .map_err(|e| Error::Decode(format!("failed to read worker response: {e}")))?;

    match response {
        viewr_protocol::WorkerResponse::PixelStream { width, height } => {
            let expected_size = viewr_protocol::checked_rgba_len(width, height)
                .map_err(|error| Error::Decode(error.to_string()))?;
            let mut rgba = vec![0_u8; expected_size];
            worker
                .stdout
                .read_exact(&mut rgba)
                .map_err(|e| Error::Decode(format!("failed to read worker pixels: {e}")))?;

            viewr_protocol::write_ack(&mut worker.stdin)
                .and_then(|()| worker.stdin.flush())
                .map_err(|e| Error::Decode(format!("failed to acknowledge worker output: {e}")))?;

            Ok(DecodedImage {
                rgba,
                width,
                height,
            })
        }
        viewr_protocol::WorkerResponse::Error(message) => {
            Err(Error::Decode(format!("worker error: {message}")))
        }
    }
}

struct DaemonWorker {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    guard: worker_limit::WorkerGuard,
}

impl Drop for DaemonWorker {
    fn drop(&mut self) {
        worker_limit::terminate(&mut self.child, &self.guard);
    }
}

impl DaemonWorker {
    fn new() -> Result<Self, Error> {
        let decode_exe = resolve_worker_binary();
        // If the path is not absolute/PATH-only, verify the co-located binary exists.
        if decode_exe.is_absolute() && !decode_exe.is_file() {
            return Err(Error::Decode(
                "viewr-decode worker executable not found beside viewr (build with `cargo build -p viewr-decode`)"
                    .into(),
            ));
        }

        let mut cmd = Command::new(decode_exe);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        worker_limit::configure_command(&mut cmd)
            .map_err(|e| Error::Decode(format!("failed to configure worker sandbox: {e}")))?;

        // Avoid flashing a console window for the helper on Windows desktops.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Decode(format!("failed to spawn worker: {e}")))?;

        let guard = match worker_limit::harden_child(&child) {
            Ok(guard) => guard,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Decode(format!(
                    "failed to apply worker lifetime limits: {error}"
                )));
            }
        };

        let Some(stdin) = child.stdin.take() else {
            worker_limit::terminate(&mut child, &guard);
            return Err(Error::Decode("worker stdin missing".into()));
        };
        let Some(stdout) = child.stdout.take() else {
            worker_limit::terminate(&mut child, &guard);
            return Err(Error::Decode("worker stdout missing".into()));
        };
        let stdout = BufReader::new(stdout);
        Ok(Self {
            child,
            stdin,
            stdout,
            guard,
        })
    }
}

/// Locate `viewr-decode` next to the running binary, then fall back to `PATH`.
fn resolve_worker_binary() -> std::path::PathBuf {
    let mut candidates = Vec::new();

    if let Ok(explicit) = std::env::var("VIEWR_DECODE_BIN") {
        candidates.push(std::path::PathBuf::from(explicit));
    }

    if let Ok(current) = std::env::current_exe() {
        let mut beside = current.clone();
        beside.set_file_name(worker_file_name());
        candidates.push(beside);

        // `cargo run` places both binaries in the same target profile dir.
        if let Some(dir) = current.parent() {
            candidates.push(dir.join(worker_file_name()));
        }
    }

    for path in candidates {
        if path.is_file() {
            return path;
        }
    }

    // Last resort: rely on PATH resolution at spawn time.
    std::path::PathBuf::from(worker_file_name())
}

fn worker_file_name() -> &'static str {
    if cfg!(windows) {
        "viewr-decode.exe"
    } else {
        "viewr-decode"
    }
}

static WORKER_POOL: std::sync::OnceLock<Mutex<Vec<DaemonWorker>>> = std::sync::OnceLock::new();

fn get_worker() -> Result<DaemonWorker, Error> {
    let pool = WORKER_POOL.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut workers) = pool.lock() {
        while let Some(mut worker) = workers.pop() {
            if worker.child.try_wait().is_ok_and(|status| status.is_none()) {
                return Ok(worker);
            }
        }
    }
    DaemonWorker::new()
}

fn return_worker(worker: DaemonWorker) {
    let pool = WORKER_POOL.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut workers) = pool.lock()
        && workers.len() < MAX_IDLE_WORKERS
    {
        workers.push(worker);
        // If the pool is full, `worker` drops here and `Drop` terminates the child.
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DaemonWorker, exchange_with_worker, receive_worker_output,
        run_worker_operation_with_timeout, run_worker_request_with_timeout,
    };
    use crate::worker_limit;
    use std::io::{BufRead, BufReader, Write};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const HUNG_CHILD_FLAG: &str = "VIEWR_TEST_HUNG_WORKER";
    const PARTIAL_PIXEL_CHILD_FLAG: &str = "VIEWR_TEST_PARTIAL_PIXEL_WORKER";
    const SUCCESS_CHILD_FLAG: &str = "VIEWR_TEST_SUCCESS_WORKER";
    const PIXELS_FLUSHED_MARKER: &str = "VIEWR_TEST_PIXELS_FLUSHED";
    const READY_MARKER: &str = "VIEWR_TEST_WORKER_READY";

    fn signal_ready() {
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{READY_MARKER}").unwrap();
        stdout.flush().unwrap();
    }

    #[test]
    fn hung_worker_child() {
        if std::env::var_os(HUNG_CHILD_FLAG).is_some() {
            signal_ready();
            std::thread::sleep(Duration::from_mins(1));
        }
    }

    #[test]
    fn partial_pixel_worker_child() {
        if std::env::var_os(PARTIAL_PIXEL_CHILD_FLAG).is_none() {
            return;
        }

        signal_ready();
        viewr_protocol::read_path_request(&mut std::io::stdin().lock())
            .unwrap()
            .expect("missing path request");
        let mut stdout = std::io::stdout().lock();
        viewr_protocol::write_worker_response(
            &mut stdout,
            &viewr_protocol::WorkerResponse::PixelStream {
                width: 2,
                height: 1,
            },
        )
        .unwrap();
        stdout.write_all(&[1, 2]).unwrap();
        stdout.flush().unwrap();
        let mut stderr = std::io::stderr().lock();
        writeln!(stderr, "{PIXELS_FLUSHED_MARKER}").unwrap();
        stderr.flush().unwrap();
        std::thread::sleep(Duration::from_mins(1));
    }

    #[test]
    fn successful_worker_child() {
        if std::env::var_os(SUCCESS_CHILD_FLAG).is_none() {
            return;
        }

        signal_ready();
        let Some(path) = viewr_protocol::read_path_request(&mut std::io::stdin().lock()).unwrap()
        else {
            panic!("missing path request");
        };
        assert_eq!(path, PathBuf::from("photo\n雪.avif"));
        let pixels = [1_u8, 2, 3, 4, 5, 6, 7, 8];
        let mut stdout = std::io::stdout().lock();
        viewr_protocol::write_worker_response(
            &mut stdout,
            &viewr_protocol::WorkerResponse::PixelStream {
                width: 2,
                height: 1,
            },
        )
        .unwrap();
        stdout.write_all(&pixels).unwrap();
        stdout.flush().unwrap();
        viewr_protocol::read_ack(&mut std::io::stdin().lock()).unwrap();
    }

    fn spawn_test_worker(
        test_name: &str,
        flag: &str,
    ) -> (DaemonWorker, BufReader<std::process::ChildStderr>) {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg(test_name)
            .arg("--exact")
            .arg("--no-capture")
            .env(flag, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        worker_limit::configure_command(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let guard = worker_limit::harden_child(&child).unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let stderr = BufReader::new(child.stderr.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(stdout.read_line(&mut line).unwrap(), 0);
            if line.trim() == READY_MARKER {
                break;
            }
        }
        (
            DaemonWorker {
                child,
                stdin,
                stdout,
                guard,
            },
            stderr,
        )
    }

    #[test]
    fn request_deadline_covers_a_worker_that_never_responds() {
        let started = Instant::now();
        // This frame is larger than an OS pipe buffer, so the assertion covers
        // a worker that stops reading while the parent is still writing.
        let pipe_saturating_path = PathBuf::from("x".repeat(500_000));
        let error = run_worker_request_with_timeout(
            spawn_test_worker("sandbox::tests::hung_worker_child", HUNG_CHILD_FLAG).0,
            pipe_saturating_path,
            Duration::from_millis(100),
            Duration::from_secs(1),
        )
        .err()
        .unwrap();

        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn request_deadline_covers_an_incomplete_pixel_stream() {
        let (mut worker, mut progress) = spawn_test_worker(
            "sandbox::tests::partial_pixel_worker_child",
            PARTIAL_PIXEL_CHILD_FLAG,
        );
        viewr_protocol::write_path_request(&mut worker.stdin, &PathBuf::from("partial.avif"))
            .unwrap();
        worker.stdin.flush().unwrap();

        let (progress_sender, progress_receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut marker = String::new();
            let result = progress.read_line(&mut marker).map(|_| marker);
            let _ = progress_sender.send(result);
        });
        let marker = progress_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("partial worker did not report pixel progress")
            .unwrap();
        assert_eq!(marker.trim(), PIXELS_FLUSHED_MARKER);

        let started = Instant::now();
        let error = run_worker_operation_with_timeout(
            worker,
            Duration::from_millis(100),
            Duration::from_secs(1),
            receive_worker_output,
        )
        .err()
        .unwrap();

        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn pixel_stream_response_and_ack_complete_end_to_end() {
        let (mut worker, _stderr) = spawn_test_worker(
            "sandbox::tests::successful_worker_child",
            SUCCESS_CHILD_FLAG,
        );
        let image = exchange_with_worker(&mut worker, &PathBuf::from("photo\n雪.avif")).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgba, [1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
