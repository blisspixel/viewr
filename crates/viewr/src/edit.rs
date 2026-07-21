//! Non-destructive edits and export: cropping and saving to another format.
//!
//! Export re-encodes from raw pixels, which means any metadata in the source
//! file (EXIF, GPS, camera serials) is dropped by default. That is a deliberate
//! privacy property, not an accident: your location does not ride along inside a
//! photo you save. See `docs/PRIVACY.md`.

use std::path::Path;

use image::{DynamicImage, ImageFormat, RgbaImage};

use crate::decode::DecodedImage;
use crate::error::Error;

/// A pixel rectangle, used to describe a crop region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left edge, in pixels from the image's left.
    pub x: u32,
    /// Top edge, in pixels from the image's top.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Rect {
    /// Return this rectangle clipped so it lies entirely within a `bounds`
    /// (width, height) image. The origin is clamped first, then the extent, so
    /// the result never reads outside the image. A rectangle that starts past
    /// the image edge collapses to zero area.
    #[must_use]
    pub fn clamped_to(self, bounds: (u32, u32)) -> Rect {
        let x = self.x.min(bounds.0);
        let y = self.y.min(bounds.1);
        Rect {
            x,
            y,
            width: self.width.min(bounds.0 - x),
            height: self.height.min(bounds.1 - y),
        }
    }
}

/// Crop `src` to `rect`, returning a new image. The rectangle is clamped to the
/// source bounds first, so out-of-range rectangles are safe and never panic.
#[must_use]
pub fn crop(src: &DecodedImage, rect: Rect) -> DecodedImage {
    let r = rect.clamped_to((src.width, src.height));
    let row_bytes = (r.width * 4) as usize;
    let mut rgba = Vec::with_capacity(row_bytes * r.height as usize);
    for row in r.y..r.y + r.height {
        let start = ((row * src.width + r.x) * 4) as usize;
        rgba.extend_from_slice(&src.rgba[start..start + row_bytes]);
    }
    DecodedImage {
        rgba,
        width: r.width,
        height: r.height,
    }
}

/// Save `image` to `path`, choosing the output format from the file extension.
/// Formats without an alpha channel (JPEG, PNM, HDR) receive an RGB copy so that
/// saving as any format "just works". Metadata is not written: the file contains
/// only pixels.
///
/// # Errors
/// Returns [`Error::Encode`] if the pixel buffer is malformed, the extension is
/// not a recognized format, or the file cannot be encoded or written.
pub fn save(image: &DecodedImage, path: &Path) -> Result<(), Error> {
    let buffer = RgbaImage::from_raw(image.width, image.height, image.rgba.clone())
        .ok_or_else(|| Error::Encode("pixel buffer does not match dimensions".to_string()))?;
    // Filename only — never the full path — so encode errors are safe in logs/toasts.
    let name = path
        .file_name()
        .map_or_else(|| "output".into(), |s| s.to_string_lossy().into_owned());
    let encode = |e: image::ImageError| Error::Encode(format!("{name}: {e}"));

    let format = ImageFormat::from_path(path).map_err(encode)?;
    let image = DynamicImage::ImageRgba8(buffer);
    if supports_alpha(format) {
        image.save(path).map_err(encode)
    } else {
        DynamicImage::ImageRgb8(image.into_rgb8())
            .save(path)
            .map_err(encode)
    }
}

/// Whether `format`'s encoder accepts an alpha channel. The few that do not get
/// an RGB conversion before saving.
fn supports_alpha(format: ImageFormat) -> bool {
    !matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::Pnm | ImageFormat::Hdr
    )
}

#[cfg(test)]
mod tests {
    use super::{Rect, crop, save};
    use crate::decode::DecodedImage;

    /// A 4x4 image where each pixel's red channel encodes its (x + y*4) index,
    /// so crops can be checked precisely.
    fn indexed_4x4() -> DecodedImage {
        let mut rgba = Vec::new();
        for i in 0..16u8 {
            rgba.extend_from_slice(&[i, 0, 0, 255]);
        }
        DecodedImage {
            rgba,
            width: 4,
            height: 4,
        }
    }

    #[test]
    fn clamp_keeps_rect_inside_bounds() {
        let r = Rect {
            x: 3,
            y: 3,
            width: 10,
            height: 10,
        }
        .clamped_to((4, 4));
        assert_eq!(
            r,
            Rect {
                x: 3,
                y: 3,
                width: 1,
                height: 1
            }
        );
    }

    #[test]
    fn clamp_origin_past_edge_collapses_to_zero() {
        let r = Rect {
            x: 9,
            y: 9,
            width: 5,
            height: 5,
        }
        .clamped_to((4, 4));
        assert_eq!(r.width, 0);
        assert_eq!(r.height, 0);
    }

    #[test]
    fn crop_extracts_the_right_pixels() {
        // Take the 2x2 block at (1,1): indices 5,6 / 9,10.
        let out = crop(
            &indexed_4x4(),
            Rect {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            },
        );
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 2);
        let reds: Vec<u8> = out.rgba.chunks_exact(4).map(|p| p[0]).collect();
        assert_eq!(reds, vec![5, 6, 9, 10]);
    }

    #[test]
    fn crop_out_of_range_is_clamped_not_panicking() {
        let out = crop(
            &indexed_4x4(),
            Rect {
                x: 2,
                y: 2,
                width: 100,
                height: 100,
            },
        );
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 2);
    }

    #[test]
    fn save_rejects_malformed_buffer() {
        let bad = DecodedImage {
            rgba: vec![0, 0, 0],
            width: 4,
            height: 4,
        };
        let ws = crate::ephemeral::TempWorkspace::new("edit_bad").unwrap();
        let path = ws.path().join("viewr_bad.png");
        assert!(save(&bad, &path).is_err());
        // ws drops → cleans any partial write
    }

    #[test]
    fn saves_to_alpha_less_format_by_dropping_alpha() {
        // Regression: JPEG has no alpha channel; saving RGBA pixels must still
        // succeed rather than error.
        let img = DecodedImage {
            rgba: vec![10, 20, 30, 255, 40, 50, 60, 255],
            width: 2,
            height: 1,
        };
        let ws = crate::ephemeral::TempWorkspace::new("edit_jpeg").unwrap();
        let path = ws.path().join("viewr_jpeg.jpg");
        assert!(save(&img, &path).is_ok());
        // ws drops → removes the jpeg
    }
}
