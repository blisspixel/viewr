//! Turning an image file on disk into pixels the GPU can upload.
//!
//! Pure-Rust formats decode in-process (`image`, `jxl-oxide`, `resvg`). Formats
//! that need C-backed decoders (AVIF, HEIC, RAW) are delegated to
//! [`crate::sandbox`]. The shape of [`DecodedImage`] (owned RGBA8 plus
//! dimensions) is what the GPU wants either way.

use std::path::Path;

use crate::error::Error;

/// A decoded image in the form the renderer uploads: tightly packed RGBA8,
/// eight bits per channel, `width * height * 4` bytes, top row first.
pub struct DecodedImage {
    /// Row-major RGBA8 pixels, no padding.
    pub rgba: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl DecodedImage {
    /// Decode the image at `path`, choosing the format by content then extension.
    ///
    /// # Errors
    /// Returns [`Error::Decode`] if the file cannot be read or is not a supported,
    /// well-formed image.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        if crate::fs::is_worker_format(path) {
            return crate::sandbox::load_via_worker(path);
        }

        if ext == "jxl" {
            return Self::load_jxl(path);
        }

        if ext == "svg" {
            return Self::load_svg(path);
        }

        let decoded = image::open(path)
            .map_err(|e| Error::Decode(format!("{}: {e}", path.display())))?
            .into_rgba8();
        let (width, height) = decoded.dimensions();
        Ok(Self {
            rgba: decoded.into_raw(),
            width,
            height,
        })
    }

    fn load_jxl(path: &Path) -> Result<Self, Error> {
        let file = std::fs::File::open(path).map_err(|e| Error::Decode(e.to_string()))?;
        let jxl = jxl_oxide::integration::JxlDecoder::new(file)
            .map_err(|e| Error::Decode(format!("failed to init JXL decoder: {e}")))?;
        let rgba = image::DynamicImage::from_decoder(jxl)
            .map_err(|e| Error::Decode(format!("failed to decode JXL: {e}")))?
            .into_rgba8();
        let (width, height) = rgba.dimensions();
        Ok(Self {
            rgba: rgba.into_raw(),
            width,
            height,
        })
    }

    /// Render an SVG to RGBA8 with pure-Rust `resvg` / `usvg`.
    fn load_svg(path: &Path) -> Result<Self, Error> {
        let data = std::fs::read(path).map_err(|e| Error::Decode(e.to_string()))?;
        let options = resvg::usvg::Options::default();
        let tree = resvg::usvg::Tree::from_data(&data, &options)
            .map_err(|e| Error::Decode(format!("failed to parse SVG: {e}")))?;

        let size = tree.size();
        let width = positive_f32_to_px(size.width());
        let height = positive_f32_to_px(size.height());

        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| Error::Decode("SVG produced invalid pixel dimensions".into()))?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::default(),
            &mut pixmap.as_mut(),
        );

        Ok(Self {
            rgba: pixmap.take(),
            width,
            height,
        })
    }
}

/// Convert a non-negative CSS/SVG length to a whole pixel count of at least 1.
pub(crate) fn positive_f32_to_px(v: f32) -> u32 {
    let v = v.ceil().max(1.0);
    if v >= f32::from(u16::MAX) {
        // Cap absurd sizes; viewports larger than this are not useful for a still viewer.
        u32::from(u16::MAX)
    } else {
        // v is finite and >= 1.0 here, so the cast cannot be negative.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            v as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DecodedImage, positive_f32_to_px};
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("viewr_decode_unit_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn svg_renders_to_declared_size() {
        let dir = scratch("svg");
        let path = dir.join("box.svg");
        fs::write(
            &path,
            r##"<svg width="40" height="30" xmlns="http://www.w3.org/2000/svg">
                <rect width="40" height="30" fill="#ff0000"/>
            </svg>"##,
        )
        .unwrap();

        let img = DecodedImage::load(&path).expect("svg decode");
        assert_eq!(img.width, 40);
        assert_eq!(img.height, 30);
        assert_eq!(img.rgba.len(), 40 * 30 * 4);
        assert_eq!(img.rgba[0], 255);
        assert_eq!(img.rgba[3], 255);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_image_is_decode_error() {
        let dir = scratch("bad");
        let path = dir.join("x.txt");
        fs::write(&path, b"not an image").unwrap();
        assert!(DecodedImage::load(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn png_round_trip_dimensions() {
        let dir = scratch("png");
        let path = dir.join("g.png");
        let img = image::RgbImage::from_fn(8, 6, |x, y| {
            image::Rgb([(x * 20) as u8, (y * 30) as u8, 100])
        });
        img.save(&path).unwrap();
        let decoded = DecodedImage::load(&path).expect("png");
        assert_eq!((decoded.width, decoded.height), (8, 6));
        assert_eq!(decoded.rgba.len(), 8 * 6 * 4);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_decode_error() {
        let path = PathBuf::from("definitely_missing_viewr_image_xyz.png");
        assert!(DecodedImage::load(&path).is_err());
    }

    #[test]
    fn positive_f32_to_px_clamps_and_ceils() {
        assert_eq!(positive_f32_to_px(0.0), 1);
        assert_eq!(positive_f32_to_px(-3.0), 1);
        assert_eq!(positive_f32_to_px(1.1), 2);
        assert_eq!(positive_f32_to_px(10.0), 10);
        assert_eq!(
            positive_f32_to_px(f32::from(u16::MAX) + 100.0),
            u32::from(u16::MAX)
        );
    }

    #[test]
    fn sandboxed_extension_without_worker_is_error() {
        let dir = scratch("avif");
        let path = dir.join("x.avif");
        fs::write(&path, b"not really avif").unwrap();
        // Worker binary is absent in unit tests; path still routes through sandbox.
        assert!(DecodedImage::load(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
