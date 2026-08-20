//! Isolated decode worker for formats that need C-backed libraries.
//!
//! Protocol (versioned framing over stdin and stdout):
//! 1. Parent opens the selected file and writes a bounded encoded-image frame.
//! 2. Worker replies with a typed pixel-stream or bounded error frame.
//! 3. A successful frame is followed by exactly `width * height * 4` RGBA8 bytes.
//! 4. After receiving the pixel stream, the parent sends an ACK frame.
//!
//! Feature flags gate optional system libraries so the default workspace build
//! never requires libheif / libavif / libraw on CI hosts.

#![allow(missing_docs)] // binary entry; module docs live above

use std::io::{BufReader, IsTerminal, Write};
use viewr_protocol::{WorkerColorProfile, WorkerResponse};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One-line explanation for a person who ran this binary by hand.
fn hand_run_notice() -> String {
    format!(
        "viewr-decode {VERSION} is viewr's isolated decode worker, not a command you run.\n\
viewr starts it and speaks a binary protocol over stdin and stdout. It takes no\n\
options and no file path. Use `viewr doctor` to check that the pair is installed\n\
side by side, and see docs/FORMATS.md for the formats it decodes.\n"
    )
}

/// Decide how an invocation should end before the protocol loop starts.
///
/// A worker that waits forever on a terminal looks like a hang, so an explicit
/// argument or an interactive stdin gets one line of explanation instead.
fn hand_run_exit(arguments: &[String], stdin_is_terminal: bool) -> Option<i32> {
    match arguments.first().map(String::as_str) {
        Some("--help" | "-h" | "help" | "--version" | "-V" | "version") => Some(0),
        Some(_) => Some(2),
        None if stdin_is_terminal => Some(0),
        None => None,
    }
}

#[cfg(feature = "avif")]
mod avif;
#[cfg(feature = "heic")]
mod heic;

struct DecodedOutput {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    color_profile: WorkerColorProfile,
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = hand_run_exit(&arguments, std::io::stdin().is_terminal()) {
        if code == 0 {
            print!("{}", hand_run_notice());
        } else {
            eprint!("{}", hand_run_notice());
        }
        std::process::exit(code);
    }

    #[cfg(target_os = "linux")]
    {
        if let Err(error) = harden_worker_process() {
            eprintln!("viewr-decode refused to start without process hardening: {error}");
            std::process::exit(70);
        }
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let mut input = BufReader::new(stdin.lock());

    loop {
        let request = match viewr_protocol::read_decode_request(&mut input) {
            Ok(Some(request)) => request,
            Ok(None) => break,
            Err(error) => {
                let _ = send_response(
                    &mut stdout,
                    &worker_error(format!("invalid request: {error}")),
                );
                break;
            }
        };

        if request.format == viewr_protocol::PROBE_FORMAT && request.encoded.is_empty() {
            if !send_response(&mut stdout, &WorkerResponse::Probe) {
                break;
            }
            continue;
        }

        match decode_input(&request.format, &request.encoded).and_then(validate_decoded) {
            Ok(decoded) => {
                let DecodedOutput {
                    width,
                    height,
                    rgba,
                    color_profile,
                } = decoded;
                if !send_response(
                    &mut stdout,
                    &WorkerResponse::PixelStream {
                        width,
                        height,
                        color_profile,
                    },
                ) {
                    break;
                }
                if stdout
                    .write_all(&rgba)
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    break;
                }
                drop(rgba);

                if viewr_protocol::read_ack(&mut input).is_err() {
                    break;
                }
            }
            Err(e) => {
                if !send_response(&mut stdout, &worker_error(e)) {
                    break;
                }
            }
        }
    }
}

fn send_response(writer: &mut impl Write, response: &WorkerResponse) -> bool {
    viewr_protocol::write_worker_response(writer, response)
        .and_then(|()| writer.flush())
        .is_ok()
}

fn worker_error(message: impl AsRef<str>) -> WorkerResponse {
    let mut bounded = String::new();
    for character in message.as_ref().chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if bounded.len() + character.len_utf8() > viewr_protocol::MAX_WORKER_ERROR_BYTES {
            break;
        }
        bounded.push(character);
    }
    if bounded.is_empty() {
        bounded.push_str("worker decode failed");
    }
    WorkerResponse::Error(bounded)
}

fn validate_decoded(decoded: DecodedOutput) -> Result<DecodedOutput, String> {
    let expected_size = viewr_protocol::checked_rgba_len(decoded.width, decoded.height)
        .map_err(|error| error.to_string())?;
    if expected_size != decoded.rgba.len() {
        return Err("decoder returned an invalid RGBA buffer".into());
    }
    Ok(decoded)
}

#[cfg(any(feature = "avif", feature = "heic", test))]
fn copy_strided_rgba(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<Vec<u8>, String> {
    let expected_size =
        viewr_protocol::checked_rgba_len(width, height).map_err(|error| error.to_string())?;
    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "decoder returned an invalid RGBA stride".to_string())?;
    let rows = usize::try_from(height)
        .map_err(|_| "decoder returned an invalid RGBA height".to_string())?;
    if stride < row_bytes {
        return Err("decoder returned an invalid RGBA stride".into());
    }
    let required_size = stride
        .checked_mul(rows)
        .ok_or_else(|| "decoder returned an invalid RGBA buffer".to_string())?;
    if data.len() < required_size {
        return Err("decoder returned a truncated RGBA buffer".into());
    }

    let mut rgba = Vec::new();
    rgba.try_reserve_exact(expected_size)
        .map_err(|_| "not enough memory for decoded pixels".to_string())?;
    for row in data.chunks_exact(stride).take(rows) {
        rgba.extend_from_slice(&row[..row_bytes]);
    }
    let decoded = DecodedOutput {
        width,
        height,
        rgba,
        color_profile: WorkerColorProfile::Unknown,
    };
    validate_decoded(decoded).map(|decoded| decoded.rgba)
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // prctl hardening requires one direct, integer-only syscall
fn harden_worker_process() -> Result<(), String> {
    for (option, argument, label) in [
        (libc::PR_SET_NO_NEW_PRIVS, 1, "no_new_privs"),
        (libc::PR_SET_DUMPABLE, 0, "non-dumpable state"),
    ] {
        // SAFETY: prctl is called in the single-threaded worker at startup with
        // integer options documented by Linux. No pointers cross the boundary.
        if unsafe { libc::prctl(option, argument, 0, 0, 0) } != 0 {
            return Err(format!("{label}: {}", std::io::Error::last_os_error()));
        }
    }
    #[cfg(any(feature = "avif", feature = "heic"))]
    viewr_seccomp::apply_production_c_decoder_policy()
        .map_err(|error| format!("C-decoder seccomp allowlist: {error}"))?;
    Ok(())
}

fn decode_input(format: &str, encoded: &[u8]) -> Result<DecodedOutput, String> {
    match format {
        "avif" => decode_avif(encoded),
        "heic" | "heif" => decode_heic(encoded),
        "cr2" | "nef" | "arw" | "dng" | "rw2" | "orf" | "raf" => decode_raw(encoded),
        _ => Err("unsupported worker format".into()),
    }
}

#[cfg(feature = "avif")]
fn decode_avif(encoded: &[u8]) -> Result<DecodedOutput, String> {
    avif::decode(encoded)
}

#[cfg(not(feature = "avif"))]
fn decode_avif(_encoded: &[u8]) -> Result<DecodedOutput, String> {
    Err("AVIF support requires building viewr-decode with --features avif (and libavif)".into())
}

#[cfg(feature = "heic")]
fn decode_heic(encoded: &[u8]) -> Result<DecodedOutput, String> {
    heic::decode(encoded)
}

#[cfg(not(feature = "heic"))]
fn decode_heic(_encoded: &[u8]) -> Result<DecodedOutput, String> {
    Err(
        "HEIC/HEIF support requires building viewr-decode with --features heic (and libheif)"
            .into(),
    )
}

/// Camera RAW is deferred from 1.0. `libraw-rs` is still pre-1.0, and the
/// isolated-worker bar is not met. Callers get a stable, honest error. See
/// `docs/STACK.md` Decision 9 and `docs/FORMATS.md`.
#[cfg(feature = "raw")]
fn decode_raw(_encoded: &[u8]) -> Result<DecodedOutput, String> {
    Err("camera RAW is deferred from 1.0 (feature raw reserved; see docs/FORMATS.md)".into())
}

#[cfg(not(feature = "raw"))]
fn decode_raw(_encoded: &[u8]) -> Result<DecodedOutput, String> {
    Err("camera RAW is deferred from 1.0; see docs/FORMATS.md".into())
}

#[cfg(test)]
mod tests {
    use super::{
        DecodedOutput, copy_strided_rgba, hand_run_exit, hand_run_notice, validate_decoded,
        worker_error,
    };
    use viewr_protocol::{MAX_DECODE_DIMENSION, MAX_DECODE_PIXELS};
    use viewr_protocol::{WorkerColorProfile, WorkerResponse};

    #[test]
    fn decoded_output_validation_checks_shape_and_limit() {
        let decoded = |width, height, rgba| DecodedOutput {
            width,
            height,
            rgba,
            color_profile: WorkerColorProfile::Unknown,
        };
        assert!(validate_decoded(decoded(2, 3, vec![0; 24])).is_ok());
        assert!(validate_decoded(decoded(0, 3, Vec::new())).is_err());
        assert!(validate_decoded(decoded(2, 3, vec![0; 23])).is_err());

        let height =
            u32::try_from(MAX_DECODE_PIXELS / u64::from(MAX_DECODE_DIMENSION) + 1).unwrap();
        assert!(validate_decoded(decoded(MAX_DECODE_DIMENSION, height, Vec::new())).is_err());
    }

    #[test]
    fn strided_rgba_copy_validates_layout_and_removes_padding() {
        let data = [
            1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 9, 10, 11, 12, 13, 14, 15, 16, 0, 0,
        ];
        assert_eq!(
            copy_strided_rgba(&data, 2, 2, 10).unwrap(),
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
        assert!(copy_strided_rgba(&data, 2, 2, 7).is_err());
        assert!(copy_strided_rgba(&data[..15], 2, 2, 8).is_err());
    }

    #[test]
    fn running_the_worker_by_hand_explains_itself_instead_of_waiting() {
        let argument = |value: &str| vec![value.to_owned()];
        for value in ["--help", "-h", "help", "--version", "-V", "version"] {
            assert_eq!(hand_run_exit(&argument(value), false), Some(0), "{value}");
            assert_eq!(hand_run_exit(&argument(value), true), Some(0), "{value}");
        }
        assert_eq!(hand_run_exit(&argument("photo.heic"), false), Some(2));
        assert_eq!(hand_run_exit(&argument("--decode"), false), Some(2));

        // An interactive terminal cannot speak the protocol, so do not block on it.
        assert_eq!(hand_run_exit(&[], true), Some(0));
        // A pipe from viewr is the only supported invocation.
        assert_eq!(hand_run_exit(&[], false), None);

        let notice = hand_run_notice();
        assert!(notice.starts_with(&format!("viewr-decode {}", super::VERSION)));
        assert!(notice.contains("not a command you run"));
        assert!(notice.contains("viewr doctor"));
        assert!(notice.ends_with('\n'));
    }

    #[test]
    fn worker_errors_are_bounded_and_single_line_safe() {
        let WorkerResponse::Error(message) = worker_error(format!("bad\n{}", "x".repeat(3000)))
        else {
            panic!("expected error response");
        };
        assert!(!message.contains('\n'));
        assert!(message.len() <= viewr_protocol::MAX_WORKER_ERROR_BYTES);
    }
}
