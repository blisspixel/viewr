//! Isolated decode worker for formats that need C-backed libraries.
//!
//! Protocol (line-oriented over stdin/stdout):
//! 1. Parent writes a UTF-8 path, then newline.
//! 2. Worker replies `SHM <os_id> <width> <height>` and waits for `ACK`, or
//!    `ERR <message>`.
//! 3. Pixel buffer is tightly packed RGBA8 in the shared-memory region.
//!
//! Feature flags gate optional system libraries so the default workspace build
//! never requires libheif / libavif / libraw on CI hosts.

#![allow(unsafe_code)] // shared-memory mapping until a safe abstraction exists
#![allow(missing_docs)] // binary entry; module docs live above

use shared_memory::ShmemConf;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let mut lines = stdin.lock().lines();

    while let Some(Ok(path_str)) = lines.next() {
        let path_str = path_str.trim();
        if path_str.is_empty() {
            continue;
        }

        let path = PathBuf::from(path_str);

        match decode_file(&path) {
            Ok((width, height, rgba)) => {
                let size = rgba.len();
                match ShmemConf::new().size(size).create() {
                    Ok(shmem) => {
                        // # Safety
                        // Region is freshly created with `size` bytes; we own it
                        // until ACK. Parent opens by os_id and copies out.
                        let slice = unsafe { std::slice::from_raw_parts_mut(shmem.as_ptr(), size) };
                        slice.copy_from_slice(&rgba);

                        let name = shmem.get_os_id();
                        let _ = writeln!(stdout, "SHM {name} {width} {height}");
                        let _ = stdout.flush();

                        if let Some(Ok(ack)) = lines.next() {
                            let _ = ack.trim() == "ACK";
                        }
                    }
                    Err(e) => {
                        let _ = writeln!(stdout, "ERR failed to create shmem: {e}");
                        let _ = stdout.flush();
                    }
                }
            }
            Err(e) => {
                let _ = writeln!(stdout, "ERR {e}");
                let _ = stdout.flush();
            }
        }
    }
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
        other => Err(format!("unsupported worker format: {other}")),
    }
}

#[cfg(feature = "avif")]
fn decode_avif(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    use image::GenericImageView;
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).map_err(|e| e.to_string())?;

    let img = libavif_image::read(&data).map_err(|e| e.to_string())?;
    let rgba = img.into_rgba8();
    let (width, height) = rgba.dimensions();
    Ok((width, height, rgba.into_raw()))
}

#[cfg(not(feature = "avif"))]
fn decode_avif(_path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    Err("AVIF support requires building viewr-decode with --features avif (and libavif)".into())
}

#[cfg(feature = "heic")]
fn decode_heic(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let path_str = path.to_str().ok_or("invalid path encoding")?;
    let ctx = libheif_rs::HeifContext::read_from_file(path_str).map_err(|e| e.to_string())?;
    let handle = ctx.primary_image_handle().map_err(|e| e.to_string())?;
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

    let width = plane.width;
    let height = plane.height;
    let stride = plane.stride;
    let data = plane.data;

    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    if stride == width as usize * 4 {
        rgba.extend_from_slice(data);
    } else {
        for y in 0..height as usize {
            let start = y * stride;
            let end = start + width as usize * 4;
            rgba.extend_from_slice(&data[start..end]);
        }
    }
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
