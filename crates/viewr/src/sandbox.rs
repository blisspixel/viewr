//! Out-of-process decode worker client (C-backed formats).
//!
//! This module talks to the `viewr-decode` helper binary over stdin/stdout and
//! a bounded pixel stream. It is process/IPC glue rather than pure image logic,
//! so CI treats this file like other end-to-end surfaces (see `docs/STANDARDS.md`).
//!
//! Workers are lifetime-hardened via [`crate::worker_limit`]: Windows Job Object
//! (kill-on-close) and a private Unix session.

use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::decode::{DecodeGeneration, DecodedImage};
use crate::error::Error;
use crate::worker_limit;

const WORKER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const WORKER_TERMINATION_GRACE: Duration = Duration::from_secs(2);
const WORKER_CANCELLATION_POLL: Duration = Duration::from_millis(25);
const MAX_IDLE_WORKERS: usize = 2;

/// Decode via the isolated `viewr-decode` worker (AVIF / HEIC / RAW paths).
pub(crate) fn load_via_worker(
    path: &Path,
    file: crate::fs::ImageSourceReader<'_>,
    generation: DecodeGeneration<'_>,
) -> Result<DecodedImage, Error> {
    // Resolve and read the user-selected file before reserving a worker. Host
    // filesystem I/O is bounded by the decode executor rather than the IPC
    // timeout thread, whose cancellation can only terminate the child process.
    let format = worker_format(path)?;
    let encoded = read_bounded_input_if_current(file, generation)?;
    generation
        .ensure_current()
        .map_err(|error| Error::Decode(error.to_string()))?;
    let worker = get_worker()?;
    generation
        .ensure_current()
        .map_err(|error| Error::Decode(error.to_string()))?;
    let output = run_worker_request_with_timeout(
        worker,
        format,
        encoded,
        WORKER_REQUEST_TIMEOUT,
        WORKER_TERMINATION_GRACE,
        generation,
    )?;
    finish_worker_output(output, generation)
}

/// Verify that the packaged worker can start and complete one bounded IPC request.
pub(crate) fn probe_worker() -> Result<(), Error> {
    let worker = get_worker()?;
    run_worker_operation_with_timeout(
        worker,
        WORKER_REQUEST_TIMEOUT,
        WORKER_TERMINATION_GRACE,
        probe_worker_exchange,
    )
}

struct WorkerTransaction<T> {
    result: Result<T, Error>,
    reusable_worker: Option<DaemonWorker>,
}

fn run_worker_request_with_timeout(
    worker: DaemonWorker,
    format: String,
    encoded: Vec<u8>,
    timeout: Duration,
    termination_grace: Duration,
    generation: DecodeGeneration<'_>,
) -> Result<WorkerDecodedOutput, Error> {
    run_worker_operation_with_cancellation(
        worker,
        timeout,
        termination_grace,
        generation,
        move |worker| exchange_with_worker(worker, &format, encoded),
    )
}

fn worker_format(path: &Path) -> Result<String, Error> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|format| !format.is_empty())
        .ok_or_else(|| Error::Decode("worker input has no valid format extension".into()))
}

#[cfg(test)]
fn read_bounded_input(path: &Path) -> Result<Vec<u8>, Error> {
    let source = crate::fs::ImageSource::open(path)
        .map_err(|error| Error::Decode(format!("failed to open worker input: {error}")))?;
    let file = source
        .clone_for_decode()
        .map_err(|error| Error::Decode(format!("failed to open worker input: {error}")))?;
    read_bounded_input_if_current(file, DecodeGeneration::unconditional())
}

fn read_bounded_input_if_current(
    file: crate::fs::ImageSourceReader<'_>,
    generation: DecodeGeneration<'_>,
) -> Result<Vec<u8>, Error> {
    let metadata = file
        .metadata()
        .map_err(|error| Error::Decode(format!("failed to inspect open worker input: {error}")))?;
    if !metadata.is_file() {
        return Err(Error::Decode("worker input must be a regular file".into()));
    }
    let declared_length = metadata.len();
    if declared_length > viewr_protocol::MAX_ENCODED_INPUT_BYTES {
        return Err(Error::Decode(
            "encoded input exceeds worker safety limit".into(),
        ));
    }
    let initial_capacity = usize::try_from(declared_length).unwrap_or(0);
    read_bounded_if_current(
        file,
        initial_capacity,
        viewr_protocol::MAX_ENCODED_INPUT_BYTES,
        generation,
    )
}

#[cfg(test)]
fn read_bounded(
    reader: impl Read,
    initial_capacity: usize,
    max_bytes: u64,
) -> Result<Vec<u8>, Error> {
    read_bounded_if_current(
        reader,
        initial_capacity,
        max_bytes,
        DecodeGeneration::unconditional(),
    )
}

fn read_bounded_if_current(
    mut reader: impl Read,
    initial_capacity: usize,
    max_bytes: u64,
    generation: DecodeGeneration<'_>,
) -> Result<Vec<u8>, Error> {
    let max_bytes = usize::try_from(max_bytes)
        .map_err(|_| Error::Decode("worker input limit is not representable".into()))?;
    generation
        .ensure_current()
        .map_err(|error| Error::Decode(error.to_string()))?;
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(initial_capacity.min(max_bytes))
        .map_err(|_| Error::Decode("not enough memory to read worker input".into()))?;

    let mut chunk = [0_u8; 16 * 1024];
    loop {
        generation
            .ensure_current()
            .map_err(|error| Error::Decode(error.to_string()))?;
        let remaining = max_bytes.saturating_sub(encoded.len());
        let read_limit = chunk.len().min(remaining.saturating_add(1));
        let count = if let Some(slice) = chunk.get_mut(..read_limit) {
            reader
                .read(slice)
                .map_err(|error| Error::Decode(format!("failed to read worker input: {error}")))?
        } else {
            0
        };
        generation
            .ensure_current()
            .map_err(|error| Error::Decode(error.to_string()))?;
        if count == 0 {
            break;
        }
        if count > remaining {
            return Err(Error::Decode(
                "encoded input exceeds worker safety limit".into(),
            ));
        }
        encoded
            .try_reserve_exact(count)
            .map_err(|_| Error::Decode("not enough memory to read worker input".into()))?;
        if let Some(slice) = chunk.get(..count) {
            encoded.extend_from_slice(slice);
        }
    }
    Ok(encoded)
}

fn run_worker_operation_with_timeout<T: Send + 'static>(
    worker: DaemonWorker,
    timeout: Duration,
    termination_grace: Duration,
    operation: impl FnOnce(&mut DaemonWorker) -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    run_worker_operation_with_cancellation(
        worker,
        timeout,
        termination_grace,
        DecodeGeneration::unconditional(),
        operation,
    )
}

fn run_worker_operation_with_cancellation<T: Send + 'static>(
    worker: DaemonWorker,
    timeout: Duration,
    termination_grace: Duration,
    generation: DecodeGeneration<'_>,
    operation: impl FnOnce(&mut DaemonWorker) -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
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

    let started = Instant::now();
    loop {
        if !generation.is_current() {
            return stop_worker_operation(
                &killer,
                &receiver,
                request_thread,
                termination_grace,
                WorkerStopReason::Superseded,
            );
        }

        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return stop_worker_operation(
                &killer,
                &receiver,
                request_thread,
                termination_grace,
                WorkerStopReason::TimedOut,
            );
        };
        let wait = remaining.min(WORKER_CANCELLATION_POLL);
        match receiver.recv_timeout(wait) {
            Ok(mut transaction) => {
                request_thread
                    .join()
                    .map_err(|_| Error::Decode("worker request thread failed".into()))?;
                if let Some(worker) = transaction.reusable_worker.take() {
                    return_worker(worker);
                }
                return transaction.result;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if wait < remaining => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return stop_worker_operation(
                    &killer,
                    &receiver,
                    request_thread,
                    termination_grace,
                    WorkerStopReason::TimedOut,
                );
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                request_thread
                    .join()
                    .map_err(|_| Error::Decode("worker request thread failed".into()))?;
                return Err(Error::Decode(
                    "worker request ended without a response".into(),
                ));
            }
        }
    }
}

#[derive(Clone, Copy)]
enum WorkerStopReason {
    TimedOut,
    Superseded,
}

fn stop_worker_operation<T>(
    killer: &worker_limit::WorkerKiller,
    receiver: &std::sync::mpsc::Receiver<WorkerTransaction<T>>,
    request_thread: std::thread::JoinHandle<()>,
    termination_grace: Duration,
    reason: WorkerStopReason,
) -> Result<T, Error> {
    let label = match reason {
        WorkerStopReason::TimedOut => "timed out",
        WorkerStopReason::Superseded => "was superseded",
    };
    let termination = killer.terminate();
    let stopped = receiver.recv_timeout(termination_grace).is_ok();
    if stopped {
        request_thread
            .join()
            .map_err(|_| Error::Decode("worker request thread failed during cleanup".into()))?;
    }
    if let Err(error) = termination {
        return Err(Error::Decode(format!(
            "worker request {label} and containment termination failed: {error}"
        )));
    }
    if !stopped {
        return Err(Error::Decode(format!(
            "worker request {label}; cleanup did not finish within the safety grace period"
        )));
    }
    Err(Error::Decode(format!("worker request {label}")))
}

fn exchange_with_worker(
    worker: &mut DaemonWorker,
    format: &str,
    encoded: Vec<u8>,
) -> Result<WorkerDecodedOutput, Error> {
    send_worker_input(worker, format, &encoded)?;
    drop(encoded);
    receive_worker_output(worker)
}

fn send_worker_input(worker: &mut DaemonWorker, format: &str, encoded: &[u8]) -> Result<(), Error> {
    use std::io::Write;

    viewr_protocol::write_decode_request(&mut worker.stdin, format, encoded)
        .map_err(|error| Error::Decode(format!("failed to send worker input: {error}")))?;
    worker
        .stdin
        .flush()
        .map_err(|error| Error::Decode(format!("failed to flush worker input: {error}")))
}

fn probe_worker_exchange(worker: &mut DaemonWorker) -> Result<(), Error> {
    send_worker_input(worker, viewr_protocol::PROBE_FORMAT, &[])?;
    match viewr_protocol::read_worker_response(&mut worker.stdout)
        .map_err(|error| Error::Decode(format!("failed to read worker probe: {error}")))?
    {
        viewr_protocol::WorkerResponse::Probe => Ok(()),
        viewr_protocol::WorkerResponse::Error(_) => Err(Error::Decode(
            "worker rejected the encoded-input protocol probe".into(),
        )),
        viewr_protocol::WorkerResponse::PixelStream { .. } => Err(Error::Decode(
            "worker returned pixels for the encoded-input protocol probe".into(),
        )),
    }
}

fn receive_worker_output(worker: &mut DaemonWorker) -> Result<WorkerDecodedOutput, Error> {
    use std::io::{Read, Write};

    let response = viewr_protocol::read_worker_response(&mut worker.stdout)
        .map_err(|e| Error::Decode(format!("failed to read worker response: {e}")))?;

    match response {
        viewr_protocol::WorkerResponse::PixelStream {
            width,
            height,
            color_profile,
        } => {
            let expected_size = viewr_protocol::checked_rgba_len(width, height)
                .map_err(|error| Error::Decode(error.to_string()))?;
            let mut rgba = Vec::new();
            rgba.try_reserve_exact(expected_size)
                .map_err(|_| Error::Decode("not enough memory for worker pixels".into()))?;
            rgba.resize(expected_size, 0);
            worker
                .stdout
                .read_exact(&mut rgba)
                .map_err(|e| Error::Decode(format!("failed to read worker pixels: {e}")))?;

            viewr_protocol::write_ack(&mut worker.stdin)
                .and_then(|()| worker.stdin.flush())
                .map_err(|e| Error::Decode(format!("failed to acknowledge worker output: {e}")))?;

            Ok(WorkerDecodedOutput {
                source: crate::decode::SourceImage::new(rgba, width, height)?,
                color_profile,
            })
        }
        viewr_protocol::WorkerResponse::Error(message) => {
            Err(Error::Decode(format!("worker error: {message}")))
        }
        viewr_protocol::WorkerResponse::Probe => Err(Error::Decode(
            "worker sent an unexpected protocol-probe response".into(),
        )),
    }
}

struct WorkerDecodedOutput {
    source: crate::decode::SourceImage,
    color_profile: viewr_protocol::WorkerColorProfile,
}

fn finish_worker_output(
    output: WorkerDecodedOutput,
    generation: DecodeGeneration<'_>,
) -> Result<DecodedImage, Error> {
    normalize_worker_color_profile(output.source, output.color_profile, generation)
}

fn normalize_worker_color_profile(
    source: crate::decode::SourceImage,
    color_profile: viewr_protocol::WorkerColorProfile,
    generation: DecodeGeneration<'_>,
) -> Result<DecodedImage, Error> {
    let ensure_current = || {
        generation
            .ensure_current()
            .map_err(|error| Error::Decode(error.to_string()))
    };
    ensure_current()?;
    let normalizer = match color_profile {
        viewr_protocol::WorkerColorProfile::Unknown => {
            crate::decode::ColorNormalizer::unknown_worker_profile()
        }
        viewr_protocol::WorkerColorProfile::Icc(profile) => {
            let normalizer = crate::decode::ColorNormalizer::from_icc_profile(&profile);
            ensure_current()?;
            normalizer
        }
        viewr_protocol::WorkerColorProfile::Cicp(cicp) if cicp.is_srgb() => {
            crate::decode::ColorNormalizer::tagged_srgb()
        }
        viewr_protocol::WorkerColorProfile::Cicp(_) => {
            crate::decode::ColorNormalizer::unsupported_profile()
        }
    };
    normalizer
        .normalize_if_current(source, generation)
        .map_err(|error| Error::Decode(error.to_string()))
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
        let decode_exe = resolve_worker_binary()?;

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

/// Locate and canonicalize the exact `viewr-decode` helper.
fn resolve_worker_binary() -> Result<std::path::PathBuf, Error> {
    let explicit = std::env::var_os("VIEWR_DECODE_BIN").map(std::path::PathBuf::from);
    let current_exe = std::env::current_exe().ok();
    select_worker_binary(explicit.as_deref(), current_exe.as_deref())
}

fn select_worker_binary(
    explicit: Option<&Path>,
    current_exe: Option<&Path>,
) -> Result<std::path::PathBuf, Error> {
    if let Some(explicit) = explicit {
        return canonical_worker_file(explicit).ok_or_else(|| {
            Error::Decode("configured viewr-decode worker executable is unavailable".into())
        });
    }

    let colocated = current_exe
        .and_then(Path::parent)
        .map(|directory| directory.join(worker_file_name()));
    colocated
        .as_deref()
        .and_then(canonical_worker_file)
        .ok_or_else(|| {
            Error::Decode(
                "viewr-decode worker executable not found beside viewr (build with `cargo build -p viewr-decode`)"
                    .into(),
            )
        })
}

fn canonical_worker_file(path: &Path) -> Option<std::path::PathBuf> {
    let canonical = path.canonicalize().ok()?;
    canonical.is_file().then_some(canonical)
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
        DaemonWorker, exchange_with_worker, finish_worker_output, normalize_worker_color_profile,
        read_bounded, read_bounded_if_current, read_bounded_input, read_bounded_input_if_current,
        receive_worker_output, run_worker_operation_with_cancellation,
        run_worker_operation_with_timeout, select_worker_binary, worker_file_name,
    };
    use crate::decode::DecodeGeneration;
    use crate::ephemeral::TempWorkspace;
    use crate::worker_limit;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    const HUNG_CHILD_FLAG: &str = "VIEWR_TEST_HUNG_WORKER";
    const PARTIAL_PIXEL_CHILD_FLAG: &str = "VIEWR_TEST_PARTIAL_PIXEL_WORKER";
    const SUCCESS_CHILD_FLAG: &str = "VIEWR_TEST_SUCCESS_WORKER";
    const PIXELS_FLUSHED_MARKER: &str = "VIEWR_TEST_PIXELS_FLUSHED";
    const READY_MARKER: &str = "VIEWR_TEST_WORKER_READY";

    #[test]
    fn worker_selection_canonicalizes_explicit_file() {
        let workspace = TempWorkspace::new("worker_explicit").unwrap();
        let worker = workspace.path().join(worker_file_name());
        std::fs::write(&worker, b"test worker").unwrap();

        let selected = select_worker_binary(Some(&worker), None).unwrap();
        assert!(selected.is_absolute());
        assert_eq!(selected, worker.canonicalize().unwrap());
    }

    #[test]
    fn invalid_explicit_worker_does_not_fall_back() {
        let workspace = TempWorkspace::new("worker_invalid_explicit").unwrap();
        let current = workspace.path().join("viewr-test");
        std::fs::write(workspace.path().join(worker_file_name()), b"colocated").unwrap();

        let missing = workspace.path().join("missing");
        let error = select_worker_binary(Some(&missing), Some(&current)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "could not open image: configured viewr-decode worker executable is unavailable"
        );
    }

    #[test]
    fn worker_selection_canonicalizes_colocated_file() {
        let workspace = TempWorkspace::new("worker_colocated").unwrap();
        let current = workspace.path().join("viewr-test");
        let worker = workspace.path().join(worker_file_name());
        std::fs::write(&worker, b"test worker").unwrap();

        let selected = select_worker_binary(None, Some(&current)).unwrap();
        assert!(selected.is_absolute());
        assert_eq!(selected, worker.canonicalize().unwrap());
    }

    #[test]
    fn missing_worker_fails_without_path_fallback() {
        let workspace = TempWorkspace::new("worker_missing").unwrap();
        let current = workspace.path().join("viewr-test");

        let error = select_worker_binary(None, Some(&current)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "could not open image: viewr-decode worker executable not found beside viewr (build with `cargo build -p viewr-decode`)"
        );
    }

    fn display_p3_profile() -> Vec<u8> {
        moxcms::ColorProfile::new_display_p3()
            .encode()
            .expect("encode Display P3 test profile")
    }

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
        viewr_protocol::read_decode_request(&mut std::io::stdin().lock())
            .unwrap()
            .expect("missing decode request");
        let mut stdout = std::io::stdout().lock();
        viewr_protocol::write_worker_response(
            &mut stdout,
            &viewr_protocol::WorkerResponse::PixelStream {
                width: 2,
                height: 1,
                color_profile: viewr_protocol::WorkerColorProfile::Unknown,
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
        let Some(request) =
            viewr_protocol::read_decode_request(&mut std::io::stdin().lock()).unwrap()
        else {
            panic!("missing decode request");
        };
        assert_eq!(request.format, "avif");
        assert_eq!(request.encoded, b"selected image bytes");
        let pixels = [210_u8, 120, 35, 17, 40, 180, 210, 231];
        let mut stdout = std::io::stdout().lock();
        viewr_protocol::write_worker_response(
            &mut stdout,
            &viewr_protocol::WorkerResponse::PixelStream {
                width: 2,
                height: 1,
                color_profile: viewr_protocol::WorkerColorProfile::Icc(display_p3_profile()),
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
        // This frame is larger than an OS pipe buffer, so the assertion covers
        // a worker that stops reading while the parent is still writing.
        let encoded = vec![0_u8; 500_000];
        let worker = spawn_test_worker("sandbox::tests::hung_worker_child", HUNG_CHILD_FLAG).0;
        let started = Instant::now();
        let error = run_worker_operation_with_timeout(
            worker,
            Duration::from_millis(100),
            Duration::from_secs(1),
            move |worker| exchange_with_worker(worker, "avif", encoded),
        )
        .err()
        .unwrap();

        assert!(error.to_string().contains("timed out"));
        // Instrumented builds can make process termination substantially slower
        // than the configured one-second cleanup grace. Keep a generous outer
        // bound while still proving that a blocked pipe cannot hang the request.
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn superseded_generation_terminates_a_blocked_worker_request() {
        let worker = spawn_test_worker("sandbox::tests::hung_worker_child", HUNG_CHILD_FLAG).0;
        let generation = AtomicU64::new(7);
        let started = Instant::now();
        let error = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                generation.store(8, Ordering::Release);
            });
            run_worker_operation_with_cancellation(
                worker,
                Duration::from_secs(30),
                Duration::from_secs(1),
                DecodeGeneration::tracked(&generation, 7),
                move |worker| exchange_with_worker(worker, "avif", vec![0_u8; 500_000]),
            )
            .err()
            .expect("superseded worker request unexpectedly completed")
        });

        assert!(error.to_string().contains("superseded"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn request_deadline_covers_an_incomplete_pixel_stream() {
        let (mut worker, mut progress) = spawn_test_worker(
            "sandbox::tests::partial_pixel_worker_child",
            PARTIAL_PIXEL_CHILD_FLAG,
        );
        viewr_protocol::write_decode_request(&mut worker.stdin, "avif", b"partial").unwrap();
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
        let output =
            exchange_with_worker(&mut worker, "avif", b"selected image bytes".to_vec()).unwrap();
        let original = [210_u8, 120, 35, 17, 40, 180, 210, 231];
        assert_eq!(
            output.color_profile,
            viewr_protocol::WorkerColorProfile::Icc(display_p3_profile())
        );

        // Normalization deliberately happens only after the timed IPC thread
        // has joined, while the caller still owns its shared decode permit.
        let image = finish_worker_output(output, DecodeGeneration::unconditional()).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(
            image.color_profile,
            crate::decode::ColorProfileStatus::ConvertedToSrgb
        );
        assert_eq!(
            image.working_color,
            crate::color::WorkingColorEncoding::SRGB_RGBA8
        );
        assert_ne!(image.rgba, original);
        assert_eq!([image.rgba[3], image.rgba[7]], [17, 231]);
    }

    #[test]
    fn worker_color_evidence_is_applied_or_falls_back_explicitly() {
        let source = || crate::decode::SourceImage::new(vec![20, 40, 60, 255], 1, 1).unwrap();

        let unknown = normalize_worker_color_profile(
            source(),
            viewr_protocol::WorkerColorProfile::Unknown,
            DecodeGeneration::unconditional(),
        )
        .unwrap();
        assert_eq!(
            unknown.color_profile,
            crate::decode::ColorProfileStatus::UnknownWorkerProfileFallback
        );

        let srgb = normalize_worker_color_profile(
            source(),
            viewr_protocol::WorkerColorProfile::Cicp(viewr_protocol::CicpColor {
                color_primaries: 1,
                transfer_characteristics: 13,
                matrix_coefficients: 0,
                full_range: true,
            }),
            DecodeGeneration::unconditional(),
        )
        .unwrap();
        assert_eq!(
            srgb.color_profile,
            crate::decode::ColorProfileStatus::TaggedSrgb
        );

        let hdr = normalize_worker_color_profile(
            source(),
            viewr_protocol::WorkerColorProfile::Cicp(viewr_protocol::CicpColor {
                color_primaries: 9,
                transfer_characteristics: 16,
                matrix_coefficients: 9,
                full_range: false,
            }),
            DecodeGeneration::unconditional(),
        )
        .unwrap();
        assert_eq!(
            hdr.color_profile,
            crate::decode::ColorProfileStatus::EmbeddedProfileFallback
        );

        let original = vec![20, 40, 60, 255];
        let icc = normalize_worker_color_profile(
            source(),
            viewr_protocol::WorkerColorProfile::Icc(display_p3_profile()),
            DecodeGeneration::unconditional(),
        )
        .unwrap();
        assert_eq!(
            icc.color_profile,
            crate::decode::ColorProfileStatus::ConvertedToSrgb
        );
        assert_ne!(icc.rgba, original);
        assert_eq!(
            icc.working_color,
            crate::color::WorkingColorEncoding::SRGB_RGBA8
        );

        let generation = AtomicU64::new(2);
        let error = normalize_worker_color_profile(
            source(),
            viewr_protocol::WorkerColorProfile::Icc(display_p3_profile()),
            DecodeGeneration::tracked(&generation, 1),
        )
        .err()
        .expect("a superseded color conversion must fail");
        assert!(error.to_string().contains("superseded"));
    }

    #[test]
    fn bounded_parent_read_rejects_input_growth() {
        assert_eq!(read_bounded(&b"abcd"[..], 4, 4).unwrap(), b"abcd");
        let error = read_bounded(&b"abcde"[..], 0, 4).unwrap_err();
        assert!(error.to_string().contains("exceeds worker safety limit"));
    }

    #[test]
    fn bounded_parent_read_stops_after_generation_supersession() {
        struct SupersedingReader<'a> {
            inner: &'a [u8],
            generation: &'a AtomicU64,
        }

        impl Read for SupersedingReader<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let count = self.inner.read(buffer)?;
                self.generation.store(2, Ordering::Release);
                Ok(count)
            }
        }

        let generation = AtomicU64::new(1);
        let error = read_bounded_if_current(
            SupersedingReader {
                inner: b"encoded image bytes",
                generation: &generation,
            },
            0,
            1024,
            DecodeGeneration::tracked(&generation, 1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("superseded"));
    }

    #[test]
    fn parent_rejects_non_regular_worker_input() {
        let workspace = TempWorkspace::new("worker_non_regular_input").unwrap();
        let error = read_bounded_input(workspace.path()).unwrap_err();
        assert!(error.to_string().contains("must be a regular file"));
    }

    #[test]
    fn worker_input_reads_the_accepted_handle_after_path_replacement() {
        let workspace = TempWorkspace::new("sandbox_source_identity").unwrap();
        let path = workspace.path().join("source.avif");
        let retained = workspace.path().join("retained.avif");
        std::fs::write(&path, b"accepted worker bytes").unwrap();
        let source = crate::fs::ImageSource::open(&path).unwrap();
        std::fs::rename(&path, &retained).unwrap();
        std::fs::write(&path, b"replacement bytes").unwrap();

        let encoded = read_bounded_input_if_current(
            source.clone_for_decode().unwrap(),
            DecodeGeneration::unconditional(),
        )
        .unwrap();
        assert_eq!(encoded, b"accepted worker bytes");
        assert_eq!(
            source.matches_path(&path),
            crate::fs::ImageSourceMatch::Changed
        );
    }
}
