//! Non-destructive edits and export: cropping and saving to another format.
//!
//! Export re-encodes from raw pixels. By default any metadata in the source
//! file (EXIF, GPS, camera serials) is **dropped** — a deliberate privacy
//! property. Users can opt in to retain EXIF for a session via
//! [`SaveOptions::retain_exif`]. See `docs/PRIVACY.md`.

use std::path::Path;

use image::{DynamicImage, ImageFormat, RgbaImage};
use little_exif::metadata::Metadata;

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

/// Options for [`save_with_options`].
///
/// Defaults strip all metadata (privacy-first): `retain_exif` is `false`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveOptions {
    /// When `true`, copy EXIF tags from the source file into the output after
    /// encoding pixels. **Default is `false`** — metadata is removed.
    pub retain_exif: bool,
}

impl SaveOptions {
    /// Privacy default: re-encode pixels only, drop all metadata.
    #[must_use]
    pub const fn strip() -> Self {
        Self { retain_exif: false }
    }

    /// Copy EXIF from the source path into the destination when possible.
    #[must_use]
    pub const fn retain_exif() -> Self {
        Self { retain_exif: true }
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

/// Save `image` to `path`, stripping all metadata (EXIF, GPS, …).
///
/// Equivalent to [`save_with_options`] with [`SaveOptions::strip`] and no source.
///
/// # Errors
/// Returns [`Error::Encode`] if the pixel buffer is malformed, the extension is
/// not a recognized format, or the file cannot be encoded or written.
pub fn save(image: &DecodedImage, path: &Path) -> Result<(), Error> {
    save_with_options(image, path, None, SaveOptions::strip())
}

/// Save `image` to `dest`, optionally copying EXIF from `source`.
///
/// When `opts.retain_exif` is `false` (the default), the file contains **only
/// pixels** — no EXIF, GPS, or other sidecar metadata. When `true` and
/// `source` is provided, EXIF tags are best-effort copied from the source into
/// the freshly encoded destination (supported containers: JPEG, PNG, WebP,
/// TIFF, and related formats that `little_exif` understands).
///
/// # Errors
/// Returns [`Error::Encode`] on encode failure or if EXIF copy was requested
/// and could not be applied.
pub fn save_with_options(
    image: &DecodedImage,
    dest: &Path,
    source: Option<&Path>,
    opts: SaveOptions,
) -> Result<(), Error> {
    encode_pixels_only(image, dest)?;

    if opts.retain_exif {
        let Some(src) = source else {
            return Err(Error::Encode(
                "retain EXIF requested but no source path was provided".into(),
            ));
        };
        copy_exif(src, dest)?;
    }
    // When retain_exif is false: do nothing more. Pixel re-encode already
    // left the file free of source metadata by construction.
    Ok(())
}

/// Encode RGBA pixels to `path` with no metadata written.
fn encode_pixels_only(image: &DecodedImage, path: &Path) -> Result<(), Error> {
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

/// Whether `path`'s extension is a container that can carry EXIF via `little_exif`.
#[must_use]
pub fn path_supports_exif(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "tif" | "tiff" | "jxl" | "heic" | "heif" | "avif" | "hif"
    )
}

/// Copy EXIF from `source` into an already-encoded `dest`.
fn copy_exif(source: &Path, dest: &Path) -> Result<(), Error> {
    if !path_supports_exif(dest) {
        return Err(Error::Encode(
            "destination format cannot carry EXIF (use JPEG/PNG/WebP/TIFF, or turn retain off)"
                .into(),
        ));
    }
    if !path_supports_exif(source) {
        // Source has no EXIF container we can read — treat as success (nothing to copy).
        return Ok(());
    }

    let metadata = Metadata::new_from_path(source)
        .map_err(|e| Error::Encode(format!("could not read EXIF from source: {e}")))?;
    metadata
        .write_to_file(dest)
        .map_err(|e| Error::Encode(format!("could not write EXIF to output: {e}")))?;
    Ok(())
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
    use super::{Rect, SaveOptions, crop, path_supports_exif, save, save_with_options};
    use crate::decode::DecodedImage;
    use crate::ephemeral::TempWorkspace;
    use little_exif::exif_tag::ExifTag;
    use little_exif::metadata::Metadata;
    use std::path::Path;

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

    fn solid_rgba(w: u32, h: u32) -> DecodedImage {
        DecodedImage {
            rgba: vec![10, 20, 30, 255]
                .into_iter()
                .cycle()
                .take((w * h * 4) as usize)
                .collect(),
            width: w,
            height: h,
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
        let ws = TempWorkspace::new("edit_bad").unwrap();
        let path = ws.path().join("viewr_bad.png");
        assert!(save(&bad, &path).is_err());
    }

    #[test]
    fn saves_to_alpha_less_format_by_dropping_alpha() {
        let img = DecodedImage {
            rgba: vec![10, 20, 30, 255, 40, 50, 60, 255],
            width: 2,
            height: 1,
        };
        let ws = TempWorkspace::new("edit_jpeg").unwrap();
        let path = ws.path().join("viewr_jpeg.jpg");
        assert!(save(&img, &path).is_ok());
    }

    #[test]
    fn save_options_default_strips_metadata() {
        assert!(!SaveOptions::default().retain_exif);
        assert!(!SaveOptions::strip().retain_exif);
        assert!(SaveOptions::retain_exif().retain_exif);
    }

    #[test]
    fn path_supports_exif_for_common_containers() {
        assert!(path_supports_exif(Path::new("a.jpg")));
        assert!(path_supports_exif(Path::new("a.PNG")));
        assert!(path_supports_exif(Path::new("a.webp")));
        assert!(!path_supports_exif(Path::new("a.bmp")));
        assert!(!path_supports_exif(Path::new("a.gif")));
    }

    /// Build a JPEG with a known `ImageDescription`, then export with strip vs retain.
    #[test]
    fn default_save_strips_exif_retain_keeps_description() {
        let ws = TempWorkspace::new("edit_exif").unwrap();
        let source = ws.path().join("with_exif.jpg");
        let stripped = ws.path().join("stripped.jpg");
        let retained = ws.path().join("retained.jpg");

        let img = solid_rgba(8, 8);
        encode_and_stamp_description(&img, &source, "viewr-test-gps-should-not-leak");

        // Default / strip path: no retain, description must not appear.
        save_with_options(&img, &stripped, Some(&source), SaveOptions::strip()).unwrap();
        assert!(
            !file_has_description(&stripped, "viewr-test-gps-should-not-leak"),
            "default Save As must strip EXIF description"
        );

        // Opt-in retain: description must survive.
        save_with_options(&img, &retained, Some(&source), SaveOptions::retain_exif()).unwrap();
        assert!(
            file_has_description(&retained, "viewr-test-gps-should-not-leak"),
            "retain_exif must copy ImageDescription from source"
        );
    }

    fn encode_and_stamp_description(img: &DecodedImage, path: &Path, text: &str) {
        save(img, path).unwrap();
        let mut meta = Metadata::new();
        meta.set_tag(ExifTag::ImageDescription(text.to_string()));
        meta.write_to_file(path).unwrap();
    }

    fn file_has_description(path: &Path, expected: &str) -> bool {
        let Ok(meta) = Metadata::new_from_path(path) else {
            return false;
        };
        for tag in &meta {
            if let ExifTag::ImageDescription(s) = tag
                && s.contains(expected)
            {
                return true;
            }
        }
        false
    }
}
