//! Out-of-process decode worker client (C-backed formats).
//!
//! This module talks to the `viewr-decode` helper binary over stdin/stdout and
//! shared memory. It is process/IPC glue rather than pure image logic: unit
//! coverage requires a built worker binary and OS shared-memory, so CI treats
//! this file like other display/IPC glue (see `docs/STANDARDS.md`).
//!
//! Workers are lifetime-hardened via [`crate::worker_limit`]: Windows Job Object
//! (kill-on-close) and a private Unix process group.

#![allow(unsafe_code)] // shared-memory mapping requires a raw slice until a safe API exists

use std::io::BufReader;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use crate::decode::DecodedImage;
use crate::error::Error;
use crate::worker_limit;

/// Soft cap: 256 megapixels × 4 bytes ≈ 1 GiB RGBA — above this is hostile input.
const MAX_WORKER_RGBA_BYTES: usize = 256 * 1024 * 1024 * 4;

/// Decode via the isolated `viewr-decode` worker (AVIF / HEIC / RAW paths).
pub(crate) fn load_via_worker(path: &Path) -> Result<DecodedImage, Error> {
    use shared_memory::ShmemConf;
    use std::io::{BufRead, Write};

    let mut worker = get_worker()?;

    let path_str = path
        .to_str()
        .ok_or_else(|| Error::Decode("invalid path string".into()))?;

    if let Err(e) = writeln!(worker.stdin, "{path_str}") {
        return Err(Error::Decode(format!("failed to send path: {e}")));
    }
    if let Err(e) = worker.stdin.flush() {
        return Err(Error::Decode(format!("failed to flush: {e}")));
    }

    let mut response = String::new();
    if let Err(e) = worker.stdout.read_line(&mut response) {
        return Err(Error::Decode(format!(
            "failed to read worker response: {e}"
        )));
    }

    let response = response.trim();
    if let Some(rest) = response.strip_prefix("SHM ") {
        let parts: Vec<&str> = rest.split(' ').collect();
        if parts.len() != 3 {
            return Err(Error::Decode("invalid SHM response format".into()));
        }

        let shm_id = parts[0];
        let width: u32 = parts[1]
            .parse()
            .map_err(|_| Error::Decode("invalid width".into()))?;
        let height: u32 = parts[2]
            .parse()
            .map_err(|_| Error::Decode("invalid height".into()))?;
        // Reject zero / absurd sizes before allocating (overflow-safe).
        if width == 0 || height == 0 {
            return Err(Error::Decode("invalid image dimensions".into()));
        }
        let expected_size = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|px| px.checked_mul(4))
            .and_then(|b| usize::try_from(b).ok())
            .ok_or_else(|| Error::Decode("image dimensions too large".into()))?;
        if expected_size > MAX_WORKER_RGBA_BYTES {
            return Err(Error::Decode("image dimensions exceed safety limit".into()));
        }

        let shmem = ShmemConf::new()
            .os_id(shm_id)
            .open()
            .map_err(|e| Error::Decode(format!("failed to open shmem: {e}")))?;

        if shmem.len() < expected_size {
            return Err(Error::Decode("shmem too small".into()));
        }

        // # Safety
        // `ShmemConf::open` mapped a region of at least `expected_size` bytes owned
        // by the worker until we ACK. We copy out immediately, then release.
        // `expected_size` is bounded above and checked against the mapping length.
        let mut rgba = vec![0_u8; expected_size];
        let slice = unsafe { std::slice::from_raw_parts(shmem.as_ptr(), expected_size) };
        rgba.copy_from_slice(slice);

        let _ = writeln!(worker.stdin, "ACK");
        let _ = worker.stdin.flush();

        return_worker(worker);

        Ok(DecodedImage {
            rgba,
            width,
            height,
        })
    } else if let Some(err) = response.strip_prefix("ERR ") {
        return_worker(worker);
        Err(Error::Decode(format!("worker error: {err}")))
    } else {
        Err(Error::Decode(format!(
            "unknown worker response: {response}"
        )))
    }
}

struct DaemonWorker {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Drop for DaemonWorker {
    fn drop(&mut self) {
        worker_limit::terminate(&mut self.child);
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
        worker_limit::configure_command(&mut cmd);

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

        worker_limit::harden_child(&child);

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Decode("worker stdin missing".into()))?;
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| Error::Decode("worker stdout missing".into()))?,
        );
        Ok(Self {
            child,
            stdin,
            stdout,
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
    if let Ok(mut workers) = pool.lock()
        && let Some(worker) = workers.pop()
    {
        return Ok(worker);
    }
    DaemonWorker::new()
}

fn return_worker(worker: DaemonWorker) {
    let pool = WORKER_POOL.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut workers) = pool.lock()
        && workers.len() < 4
    {
        workers.push(worker);
        // If the pool is full, `worker` drops here and `Drop` terminates the child.
    }
}
