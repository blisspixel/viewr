//! Background thumbnail generation for the filmstrip.
//!
//! Thumbs are pure CPU work (`image` resize) so they can be unit-tested without a
//! GPU. The app owns the async queue and egui texture upload.

use std::path::{Path, PathBuf};

/// Target edge length for filmstrip thumbnails (pixels).
pub const THUMB_EDGE: u32 = 72;

/// RGBA8 thumbnail ready for egui upload.
#[derive(Clone, Debug)]
pub struct ThumbRgba {
    /// Source path this thumb was generated from.
    pub path: PathBuf,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Tightly packed RGBA8 rows.
    pub rgba: Vec<u8>,
}

/// Decode `path` and downscale to fit inside a `THUMB_EDGE` box.
///
/// Worker formats may fail if the helper is missing; callers treat that as a
/// miss and show a placeholder.
///
/// # Errors
/// Returns a human-readable reason when decode or resize fails.
pub fn generate_thumb(path: &Path) -> Result<ThumbRgba, String> {
    let decoded = crate::decode::DecodedImage::load_background(path).map_err(|e| e.to_string())?;
    if decoded.width == 0 || decoded.height == 0 {
        return Err("empty image".into());
    }
    let img = image::RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
        .ok_or_else(|| "invalid rgba buffer".to_string())?;

    let (tw, th) = fit_size(decoded.width, decoded.height, THUMB_EDGE);
    let resized = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
    Ok(ThumbRgba {
        path: path.to_path_buf(),
        width: resized.width(),
        height: resized.height(),
        rgba: resized.into_raw(),
    })
}

/// Scale `(w, h)` to fit inside a square of `edge` while preserving aspect ratio.
#[must_use]
pub fn fit_size(w: u32, h: u32, edge: u32) -> (u32, u32) {
    if w == 0 || h == 0 || edge == 0 {
        return (1, 1);
    }
    let wf = f64::from(w);
    let hf = f64::from(h);
    let e = f64::from(edge);
    let scale = (e / wf).min(e / hf).min(1.0);
    // Positive after max(1.0); clamp into u32 range without sign-loss noise.
    let tw = f64_to_px((wf * scale).round().max(1.0));
    let th = f64_to_px((hf * scale).round().max(1.0));
    (tw, th)
}

fn f64_to_px(v: f64) -> u32 {
    let v = v.clamp(1.0, f64::from(u32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        v as u32
    }
}

#[cfg(test)]
mod tests {
    use super::{THUMB_EDGE, fit_size, generate_thumb};

    #[test]
    fn fit_size_preserves_aspect_and_caps_edge() {
        assert_eq!(fit_size(200, 100, 50), (50, 25));
        assert_eq!(fit_size(100, 200, 50), (25, 50));
        // Already small: do not upscale.
        assert_eq!(fit_size(20, 10, 50), (20, 10));
    }

    #[test]
    fn generate_thumb_from_png() {
        let ws = crate::ephemeral::TempWorkspace::new("thumb").unwrap();
        let path = ws.path().join("big.png");
        image::RgbImage::from_fn(120, 80, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 40])
        })
        .save(&path)
        .unwrap();
        let thumb = generate_thumb(&path).expect("thumb");
        assert!(thumb.width <= THUMB_EDGE);
        assert!(thumb.height <= THUMB_EDGE);
        assert_eq!(thumb.rgba.len(), (thumb.width * thumb.height * 4) as usize);
    }
}
