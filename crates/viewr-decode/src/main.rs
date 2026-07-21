//! Isolated decode worker for formats that need C-backed libraries.
//!
//! Protocol (versioned framing over stdin and stdout):
//! 1. Parent writes a length-prefixed native path frame.
//! 2. Worker replies with a typed pixel-stream or bounded error frame.
//! 3. A successful frame is followed by exactly `width * height * 4` RGBA8 bytes.
//! 4. After receiving the pixel stream, the parent sends an ACK frame.
//!
//! Feature flags gate optional system libraries so the default workspace build
//! never requires libheif / libavif / libraw on CI hosts.

#![allow(missing_docs)] // binary entry; module docs live above

#[cfg(any(feature = "avif", feature = "heic", test))]
use std::io::Read;
use std::io::{BufReader, Write};
use std::path::Path;
use viewr_protocol::WorkerResponse;

#[cfg(any(feature = "avif", feature = "heic"))]
const MAX_WORKER_INPUT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_WORKER_ERROR_BYTES: usize = 2048;

fn main() {
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
        let path = match viewr_protocol::read_path_request(&mut input) {
            Ok(Some(path)) => path,
            Ok(None) => break,
            Err(error) => {
                let _ = send_response(
                    &mut stdout,
                    &worker_error(format!("invalid request: {error}")),
                );
                break;
            }
        };

        match decode_file(&path)
            .and_then(|(width, height, rgba)| validate_decoded(width, height, rgba))
        {
            Ok((width, height, rgba)) => {
                if !send_response(&mut stdout, &WorkerResponse::PixelStream { width, height }) {
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
        if bounded.len() + character.len_utf8() > MAX_WORKER_ERROR_BYTES {
            break;
        }
        bounded.push(character);
    }
    if bounded.is_empty() {
        bounded.push_str("worker decode failed");
    }
    WorkerResponse::Error(bounded)
}

fn validate_decoded(width: u32, height: u32, rgba: Vec<u8>) -> Result<(u32, u32, Vec<u8>), String> {
    let expected_size =
        viewr_protocol::checked_rgba_len(width, height).map_err(|error| error.to_string())?;
    if expected_size != rgba.len() {
        return Err("decoder returned an invalid RGBA buffer".into());
    }
    Ok((width, height, rgba))
}

#[cfg(any(feature = "avif", feature = "heic"))]
fn read_bounded_input(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let initial_capacity = file
        .metadata()
        .ok()
        .and_then(|metadata| usize::try_from(metadata.len()).ok())
        .map_or(0, |length| {
            length.min(usize::try_from(MAX_WORKER_INPUT_BYTES).unwrap_or(usize::MAX))
        });
    read_bounded(file, initial_capacity, MAX_WORKER_INPUT_BYTES)
}

#[cfg(any(feature = "avif", feature = "heic", test))]
fn read_bounded(
    reader: impl std::io::Read,
    initial_capacity: usize,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    let mut data = Vec::with_capacity(initial_capacity);
    reader
        .take(max_bytes + 1)
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())?;
    if data.len() as u64 > max_bytes {
        return Err("encoded input exceeds worker safety limit".into());
    }
    Ok(data)
}

#[cfg(any(feature = "heic", test))]
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

    let mut rgba = Vec::with_capacity(expected_size);
    for row in data.chunks_exact(stride).take(rows) {
        rgba.extend_from_slice(&row[..row_bytes]);
    }
    validate_decoded(width, height, rgba).map(|(_, _, rgba)| rgba)
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
    Ok(())
}

fn decode_file(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "avif" => decode_avif(path),
        "heic" | "heif" => decode_heic(path),
        "cr2" | "nef" | "arw" | "dng" | "rw2" | "orf" | "raf" => decode_raw(path),
        _ => Err("unsupported worker format".into()),
    }
}

#[cfg(feature = "avif")]
fn decode_avif(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    use image::GenericImageView;

    let data = read_bounded_input(path)?;

    let img = libavif_image::read(&data).map_err(|e| e.to_string())?;
    let (width, height) = img.dimensions();
    viewr_protocol::checked_rgba_len(width, height).map_err(|error| error.to_string())?;
    Ok((width, height, img.into_rgba8().into_raw()))
}

#[cfg(not(feature = "avif"))]
fn decode_avif(_path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    Err("AVIF support requires building viewr-decode with --features avif (and libavif)".into())
}

#[cfg(feature = "heic")]
fn decode_heic(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let data = read_bounded_input(path)?;
    let ctx = libheif_rs::HeifContext::read_from_bytes(&data).map_err(|e| e.to_string())?;
    let handle = ctx.primary_image_handle().map_err(|e| e.to_string())?;
    viewr_protocol::checked_rgba_len(handle.width(), handle.height())
        .map_err(|error| error.to_string())?;
    let options = libheif_rs::DecodingOptions::new()
        .ok_or_else(|| "failed to create HEIF decoding options".to_string())?;
    let image = handle
        .decode(
            libheif_rs::ColorSpace::Rgb(libheif_rs::RgbChroma::Rgba),
            options,
        )
        .map_err(|e| e.to_string())?;
    let planes = image.planes();
    let plane = planes
        .interleaved
        .ok_or_else(|| "no interleaved plane found".to_string())?;

    let width = image.width();
    let height = image.height();
    if plane.width != width || plane.height != height {
        return Err("decoder returned inconsistent RGBA plane dimensions".into());
    }
    let rgba = copy_strided_rgba(plane.data, width, height, plane.stride)?;
    Ok((width, height, rgba))
}

#[cfg(not(feature = "heic"))]
fn decode_heic(_path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    Err(
        "HEIC/HEIF support requires building viewr-decode with --features heic (and libheif)"
            .into(),
    )
}

/// Camera RAW is deliberately deferred: `libraw-rs` is immature and heavy.
/// Callers get a stable, honest error until a pure-Rust or well-packaged
/// backend is chosen (tracked in docs/ROADMAP.md Phase 6 residuals).
#[cfg(feature = "raw")]
fn decode_raw(_path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    Err(
        "camera RAW decoding is not implemented yet (feature raw reserved; see docs/FORMATS.md)"
            .into(),
    )
}

#[cfg(not(feature = "raw"))]
fn decode_raw(_path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    Err("camera RAW is deferred; see docs/FORMATS.md (build with --features raw when ready)".into())
}

#[cfg(test)]
mod tests {
    use super::{copy_strided_rgba, read_bounded, validate_decoded, worker_error};
    use viewr_protocol::WorkerResponse;
    use viewr_protocol::{MAX_DECODE_DIMENSION, MAX_DECODE_PIXELS};

    #[test]
    fn decoded_output_validation_checks_shape_and_limit() {
        assert!(validate_decoded(2, 3, vec![0; 24]).is_ok());
        assert!(validate_decoded(0, 3, Vec::new()).is_err());
        assert!(validate_decoded(2, 3, vec![0; 23]).is_err());

        let height =
            u32::try_from(MAX_DECODE_PIXELS / u64::from(MAX_DECODE_DIMENSION) + 1).unwrap();
        assert!(validate_decoded(MAX_DECODE_DIMENSION, height, Vec::new()).is_err());
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
    fn worker_errors_are_bounded_and_single_line_safe() {
        let WorkerResponse::Error(message) = worker_error(format!("bad\n{}", "x".repeat(3000)))
        else {
            panic!("expected error response");
        };
        assert!(!message.contains('\n'));
        assert!(message.len() <= super::MAX_WORKER_ERROR_BYTES);
    }

    #[test]
    fn bounded_input_detects_growth_past_the_limit() {
        assert_eq!(read_bounded(&b"abcd"[..], 0, 4).unwrap(), b"abcd");
        assert!(read_bounded(&b"abcde"[..], 0, 4).is_err());
    }
}
