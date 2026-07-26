//! Non-destructive edits and export: cropping and saving to another format.
//!
//! Export re-encodes from raw pixels. By default any metadata in the source
//! file (EXIF, GPS, camera serials) is **dropped** as a deliberate privacy
//! property. Users can opt in to retain EXIF for a session via
//! [`SaveOptions::retain_exif`]. See `docs/PRIVACY.md`.

use std::path::Path;

use image::{ColorType, ImageFormat};
use little_exif::exif_tag::ExifTag;
use little_exif::metadata::Metadata;

use crate::color::WorkingColorEncoding;
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

/// Lossless quarter-turn and axis-flip operations applied by the viewer.
///
/// Flips use source-image axes and are followed by the clockwise rotation. This
/// is the same sampling order as the GPU preview, so exported pixels match the
/// visible image exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PixelTransform {
    quarter_turns: i32,
    flip_horizontal: bool,
    flip_vertical: bool,
}

impl PixelTransform {
    /// Construct a transform from the viewer's current rotation and flip state.
    #[must_use]
    pub const fn new(quarter_turns: i32, flip_horizontal: bool, flip_vertical: bool) -> Self {
        Self {
            quarter_turns,
            flip_horizontal,
            flip_vertical,
        }
    }

    /// Return whether applying this transform would leave every pixel unchanged.
    #[must_use]
    pub fn is_identity(self) -> bool {
        self.quarter_turns.rem_euclid(4) == 0 && !self.flip_horizontal && !self.flip_vertical
    }

    /// Return the output dimensions after the quarter-turn rotation.
    #[must_use]
    pub fn output_size(self, source: (u32, u32)) -> (u32, u32) {
        if self.quarter_turns.rem_euclid(2) == 0 {
            source
        } else {
            (source.1, source.0)
        }
    }

    /// Materialize the transform into a tightly packed RGBA8 image.
    ///
    /// # Errors
    /// Returns [`Error::Encode`] when the source buffer does not match its
    /// dimensions or the output allocation cannot be reserved.
    pub fn apply(self, source: &DecodedImage) -> Result<DecodedImage, Error> {
        let source_len = rgba_len(source.width, source.height)?;
        if source.rgba.len() != source_len {
            return Err(Error::Encode(
                "pixel buffer does not match dimensions".to_owned(),
            ));
        }

        let output_size = self.output_size((source.width, source.height));
        let output_len = rgba_len(output_size.0, output_size.1)?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(output_len).map_err(|error| {
            Error::Encode(format!("could not allocate transformed image: {error}"))
        })?;
        rgba.resize(output_len, 0);

        let rotation = self.quarter_turns.rem_euclid(4);
        for output_y in 0..output_size.1 {
            for output_x in 0..output_size.0 {
                let (mut source_x, mut source_y) = match rotation {
                    1 => (output_y, source.height - 1 - output_x),
                    2 => (source.width - 1 - output_x, source.height - 1 - output_y),
                    3 => (source.width - 1 - output_y, output_x),
                    _ => (output_x, output_y),
                };
                if self.flip_horizontal {
                    source_x = source.width - 1 - source_x;
                }
                if self.flip_vertical {
                    source_y = source.height - 1 - source_y;
                }

                let source_offset = pixel_offset(source_x, source_y, source.width)?;
                let output_offset = pixel_offset(output_x, output_y, output_size.0)?;
                rgba[output_offset..output_offset + 4]
                    .copy_from_slice(&source.rgba[source_offset..source_offset + 4]);
            }
        }

        Ok(DecodedImage {
            rgba,
            width: output_size.0,
            height: output_size.1,
            color_profile: source.color_profile,
            working_color: source.working_color,
        })
    }
}

fn rgba_len(width: u32, height: u32) -> Result<usize, Error> {
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| Error::Encode("image dimensions overflow the pixel buffer".to_owned()))?;
    usize::try_from(bytes)
        .map_err(|_| Error::Encode("image dimensions exceed this platform's limits".to_owned()))
}

fn pixel_offset(x: u32, y: u32, width: u32) -> Result<usize, Error> {
    let offset = (u64::from(y) * u64::from(width) + u64::from(x)) * 4;
    usize::try_from(offset)
        .map_err(|_| Error::Encode("pixel offset exceeds this platform's limits".to_owned()))
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
    /// encoding pixels. **Default is `false`**, so metadata is removed.
    pub retain_exif: bool,
}

/// What happened to source EXIF during a completed export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataDisposition {
    /// Retention was disabled, so the output intentionally contains only pixels.
    Stripped,
    /// Supported source EXIF was found and written to the output.
    Retained,
    /// Retention was requested, but the source had no supported EXIF payload.
    NotPresent,
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
///
/// # Errors
/// Returns [`Error::Encode`] when the source buffer does not match its declared
/// dimensions or the cropped buffer cannot be allocated.
pub fn crop(src: &DecodedImage, rect: Rect) -> Result<DecodedImage, Error> {
    let source_len = rgba_len(src.width, src.height)?;
    if src.rgba.len() != source_len {
        return Err(Error::Encode(
            "pixel buffer does not match dimensions".to_owned(),
        ));
    }

    let r = rect.clamped_to((src.width, src.height));
    let output_len = rgba_len(r.width, r.height)?;
    let row_bytes = rgba_len(r.width, 1)?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(output_len)
        .map_err(|error| Error::Encode(format!("could not allocate cropped image: {error}")))?;
    let bottom =
        r.y.checked_add(r.height)
            .ok_or_else(|| Error::Encode("crop rectangle exceeds image dimensions".to_owned()))?;
    for row in r.y..bottom {
        let start = pixel_offset(r.x, row, src.width)?;
        let end = start
            .checked_add(row_bytes)
            .ok_or_else(|| Error::Encode("crop row exceeds the pixel buffer".to_owned()))?;
        let source_row = src
            .rgba
            .get(start..end)
            .ok_or_else(|| Error::Encode("crop row exceeds the pixel buffer".to_owned()))?;
        rgba.extend_from_slice(source_row);
    }
    Ok(DecodedImage {
        rgba,
        width: r.width,
        height: r.height,
        color_profile: src.color_profile,
        working_color: src.working_color,
    })
}

/// Save `image` to `path`, stripping all metadata (EXIF, GPS, …).
///
/// Equivalent to [`save_with_options`] with [`SaveOptions::strip`] and no source.
///
/// # Errors
/// Returns [`Error::Encode`] if the pixel buffer is malformed, the extension is
/// not a recognized format, or the file cannot be encoded or written.
pub fn save(image: &DecodedImage, path: &Path) -> Result<(), Error> {
    save_with_options(image, path, None, SaveOptions::strip()).map(|_| ())
}

/// Save `image` to `dest`, optionally copying EXIF from `source`.
///
/// When `opts.retain_exif` is `false` (the default), the file contains **only
/// pixels**: no EXIF, GPS, or other sidecar metadata. When `true` and
/// `source` is provided, EXIF tags are best-effort copied from the source into
/// the freshly encoded destination (supported containers: JPEG, PNG, and WebP).
/// The return value distinguishes intentional stripping, successful retention,
/// and a source with no supported EXIF payload.
///
/// # Errors
/// Returns [`Error::Encode`] on encode failure or if EXIF copy was requested
/// and could not be applied.
pub fn save_with_options(
    image: &DecodedImage,
    dest: &Path,
    source: Option<&Path>,
    opts: SaveOptions,
) -> Result<MetadataDisposition, Error> {
    if source.is_some_and(|source| paths_refer_to_same_file(source, dest)) {
        return Err(Error::Encode(
            "Save As destination must differ from the open source file".into(),
        ));
    }
    let (format, expected_len) = validate_export(image, dest)?;
    let mut retained_metadata = if opts.retain_exif {
        let Some(src) = source else {
            return Err(Error::Encode(
                "retain EXIF requested but no source path was provided".into(),
            ));
        };
        if !path_supports_exif(dest) {
            return Err(Error::Encode(
                "destination format cannot carry EXIF (use JPEG, PNG, or WebP, or turn retain off)"
                    .into(),
            ));
        }
        crate::image_info::load_bounded_metadata(src).map(|mut metadata| {
            normalize_export_metadata(&mut metadata, image.width, image.height);
            metadata
        })
    } else {
        None
    };

    let parent = dest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let suffix = dest
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| Error::Encode("output filename requires a supported extension".into()))?;
    let temporary = tempfile::Builder::new()
        .prefix(".viewr-save-")
        .suffix(&format!(".{suffix}"))
        .tempfile_in(parent)
        .map_err(|_| {
            Error::Encode("could not create a temporary output beside the destination".into())
        })?
        .into_temp_path();

    let metadata_disposition = if retained_metadata.is_some() {
        MetadataDisposition::Retained
    } else if opts.retain_exif {
        MetadataDisposition::NotPresent
    } else {
        MetadataDisposition::Stripped
    };
    encode_pixels_only(image, &temporary, format, expected_len)?;
    if let Some(metadata) = retained_metadata.as_mut() {
        metadata
            .write_to_file(&temporary)
            .map_err(|_| Error::Encode("could not write EXIF to the temporary output".into()))?;
    }
    temporary.persist(dest).map_err(|_| {
        Error::Encode("could not replace the destination with the completed output".into())
    })?;
    Ok(metadata_disposition)
}

fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    same_file::is_same_file(left, right).unwrap_or(left == right)
}

fn validate_export(image: &DecodedImage, path: &Path) -> Result<(ImageFormat, usize), Error> {
    if image.working_color != WorkingColorEncoding::SRGB_RGBA8 {
        return Err(Error::Encode(
            "export does not support this working color encoding".into(),
        ));
    }
    let expected_len = rgba_len(image.width, image.height)?;
    if image.rgba.len() != expected_len {
        return Err(Error::Encode(
            "pixel buffer does not match dimensions".to_owned(),
        ));
    }
    // Filename only, never the full path, so encode errors are safe in logs/toasts.
    let name = path
        .file_name()
        .map_or_else(|| "output".into(), |s| s.to_string_lossy().into_owned());
    let encode = |e: image::ImageError| Error::Encode(format!("{name}: {e}"));
    let format = ImageFormat::from_path(path).map_err(encode)?;
    Ok((format, expected_len))
}

/// Encode RGBA pixels to `path` with no metadata written.
fn encode_pixels_only(
    image: &DecodedImage,
    path: &Path,
    format: ImageFormat,
    expected_len: usize,
) -> Result<(), Error> {
    let name = path
        .file_name()
        .map_or_else(|| "output".into(), |s| s.to_string_lossy().into_owned());
    let encode = |e: image::ImageError| Error::Encode(format!("{name}: {e}"));
    if supports_alpha(format) {
        image::save_buffer_with_format(
            path,
            &image.rgba,
            image.width,
            image.height,
            ColorType::Rgba8,
            format,
        )
        .map_err(encode)
    } else {
        let pixels = expected_len / 4;
        let rgb_len = pixels
            .checked_mul(3)
            .ok_or_else(|| Error::Encode("RGB export dimensions overflow".to_owned()))?;
        let mut rgb = Vec::new();
        rgb.try_reserve_exact(rgb_len)
            .map_err(|error| Error::Encode(format!("could not allocate RGB export: {error}")))?;
        for pixel in image.rgba.chunks_exact(4) {
            rgb.extend_from_slice(&pixel[..3]);
        }
        image::save_buffer_with_format(
            path,
            &rgb,
            image.width,
            image.height,
            ColorType::Rgb8,
            format,
        )
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
    matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp")
}

/// Normalize tags whose meaning changes after Viewr has decoded, oriented, or
/// cropped the pixels. Descriptive, camera, and GPS metadata remains intact.
fn normalize_export_metadata(metadata: &mut Metadata, width: u32, height: u32) {
    for tag in [
        ExifTag::Orientation(Vec::new()),
        ExifTag::ImageWidth(Vec::new()),
        ExifTag::ImageHeight(Vec::new()),
        ExifTag::ExifImageWidth(Vec::new()),
        ExifTag::ExifImageHeight(Vec::new()),
        ExifTag::ThumbnailOffset(Vec::new(), Vec::new()),
        ExifTag::ThumbnailLength(Vec::new()),
    ] {
        metadata.remove_tag(tag);
    }

    metadata.set_tag(ExifTag::Orientation(vec![1]));
    metadata.set_tag(ExifTag::ImageWidth(vec![width]));
    metadata.set_tag(ExifTag::ImageHeight(vec![height]));
    metadata.set_tag(ExifTag::ExifImageWidth(vec![width]));
    metadata.set_tag(ExifTag::ExifImageHeight(vec![height]));
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
    use super::{
        MetadataDisposition, PixelTransform, Rect, SaveOptions, crop, path_supports_exif, save,
        save_with_options,
    };
    use crate::color::WorkingColorEncoding;
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
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        }
    }

    fn indexed_3x2() -> DecodedImage {
        let mut rgba = Vec::new();
        for index in 1..=6u8 {
            rgba.extend_from_slice(&[index, 0, 0, 255]);
        }
        DecodedImage {
            rgba,
            width: 3,
            height: 2,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        }
    }

    fn red_channels(image: &DecodedImage) -> Vec<u8> {
        image.rgba.chunks_exact(4).map(|pixel| pixel[0]).collect()
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
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
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
        )
        .unwrap();
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 2);
        assert_eq!(out.working_color, WorkingColorEncoding::SRGB_RGBA8);
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
        )
        .unwrap();
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 2);
    }

    #[test]
    fn crop_rejects_a_malformed_source_without_indexing_it() {
        let malformed = DecodedImage {
            rgba: vec![0; 15],
            width: 2,
            height: 2,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        };
        assert!(
            crop(
                &malformed,
                Rect {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn pixel_transform_matches_clockwise_gpu_preview() {
        let source = indexed_3x2();
        let clockwise = PixelTransform::new(1, false, false).apply(&source).unwrap();
        assert_eq!((clockwise.width, clockwise.height), (2, 3));
        assert_eq!(clockwise.working_color, source.working_color);
        assert_eq!(red_channels(&clockwise), vec![4, 1, 5, 2, 6, 3]);

        let counterclockwise = PixelTransform::new(-1, false, false)
            .apply(&source)
            .unwrap();
        assert_eq!(red_channels(&counterclockwise), vec![3, 6, 2, 5, 1, 4]);
    }

    #[test]
    fn pixel_transform_applies_source_axis_flips_before_rotation() {
        let source = indexed_3x2();
        let horizontal = PixelTransform::new(0, true, false).apply(&source).unwrap();
        assert_eq!(red_channels(&horizontal), vec![3, 2, 1, 6, 5, 4]);

        let horizontal_then_clockwise = PixelTransform::new(1, true, false).apply(&source).unwrap();
        assert_eq!(
            red_channels(&horizontal_then_clockwise),
            vec![6, 3, 5, 2, 4, 1]
        );

        let vertical = PixelTransform::new(0, false, true).apply(&source).unwrap();
        assert_eq!(red_channels(&vertical), vec![4, 5, 6, 1, 2, 3]);
    }

    #[test]
    fn pixel_transform_normalizes_turns_and_rejects_malformed_input() {
        let source = indexed_3x2();
        assert!(PixelTransform::new(4, false, false).is_identity());
        assert_eq!(
            PixelTransform::new(5, false, false).output_size((3, 2)),
            (2, 3)
        );
        assert_eq!(
            red_channels(&PixelTransform::new(2, false, false).apply(&source).unwrap()),
            vec![6, 5, 4, 3, 2, 1]
        );

        let malformed = DecodedImage {
            rgba: vec![0; 3],
            width: 2,
            height: 2,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        };
        assert!(PixelTransform::default().apply(&malformed).is_err());
    }

    #[test]
    fn save_rejects_malformed_buffer() {
        let bad = DecodedImage {
            rgba: vec![0, 0, 0],
            width: 4,
            height: 4,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        };
        let ws = TempWorkspace::new("edit_bad").unwrap();
        let path = ws.path().join("viewr_bad.png");
        assert!(save(&bad, &path).is_err());
    }

    #[test]
    fn save_rejects_an_unsupported_working_encoding_before_touching_destination() {
        let mut image = solid_rgba(1, 1);
        image.working_color = WorkingColorEncoding::DISPLAY_P3_RGBA8;
        let workspace = TempWorkspace::new("edit_working_color").unwrap();
        let destination = workspace.path().join("unsupported.png");

        let error = save(&image, &destination).unwrap_err();

        assert!(error.to_string().contains("working color"));
        assert!(!destination.exists());
    }

    #[test]
    fn saves_to_alpha_less_format_by_dropping_alpha() {
        let img = DecodedImage {
            rgba: vec![10, 20, 30, 255, 40, 50, 60, 255],
            width: 2,
            height: 1,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
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
        assert!(!path_supports_exif(Path::new("a.tiff")));
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
        assert_eq!(
            save_with_options(&img, &stripped, Some(&source), SaveOptions::strip()).unwrap(),
            MetadataDisposition::Stripped
        );
        assert!(
            !file_has_description(&stripped, "viewr-test-gps-should-not-leak"),
            "default Save As must strip EXIF description"
        );

        // Opt-in retain: description must survive.
        assert_eq!(
            save_with_options(&img, &retained, Some(&source), SaveOptions::retain_exif()).unwrap(),
            MetadataDisposition::Retained
        );
        assert!(
            file_has_description(&retained, "viewr-test-gps-should-not-leak"),
            "retain_exif must copy ImageDescription from source"
        );
    }

    #[test]
    fn retention_reports_when_the_source_contains_no_supported_exif() {
        let ws = TempWorkspace::new("edit_no_exif").unwrap();
        let source = ws.path().join("source.png");
        let destination = ws.path().join("copy.png");
        let image = solid_rgba(3, 2);
        save(&image, &source).unwrap();

        let disposition = save_with_options(
            &image,
            &destination,
            Some(&source),
            SaveOptions::retain_exif(),
        )
        .unwrap();

        assert_eq!(disposition, MetadataDisposition::NotPresent);
        assert!(destination.is_file());
    }

    #[test]
    fn retained_exif_is_normalized_to_exported_pixels() {
        let ws = TempWorkspace::new("edit_exif_dimensions").unwrap();
        let source = ws.path().join("source.jpg");
        let retained = ws.path().join("retained.jpg");
        let source_image = solid_rgba(8, 6);
        save(&source_image, &source).unwrap();

        let mut source_metadata = Metadata::new();
        source_metadata.set_tag(ExifTag::ImageDescription("kept-description".into()));
        source_metadata.set_tag(ExifTag::Orientation(vec![6]));
        source_metadata.set_tag(ExifTag::ImageWidth(vec![8]));
        source_metadata.set_tag(ExifTag::ImageHeight(vec![6]));
        source_metadata.set_tag(ExifTag::ExifImageWidth(vec![8]));
        source_metadata.set_tag(ExifTag::ExifImageHeight(vec![6]));
        source_metadata.write_to_file(&source).unwrap();

        let exported = solid_rgba(3, 2);
        save_with_options(
            &exported,
            &retained,
            Some(&source),
            SaveOptions::retain_exif(),
        )
        .unwrap();

        let retained_metadata = Metadata::new_from_path(&retained).unwrap();
        assert_eq!(tag_u16(&retained_metadata, 0x0112), vec![1]);
        assert_eq!(tag_u32(&retained_metadata, 0x0100), vec![3]);
        assert_eq!(tag_u32(&retained_metadata, 0x0101), vec![2]);
        assert_eq!(tag_u32(&retained_metadata, 0xa002), vec![3]);
        assert_eq!(tag_u32(&retained_metadata, 0xa003), vec![2]);
        assert!(file_has_description(&retained, "kept-description"));
    }

    #[test]
    fn save_as_rejects_the_open_source_before_changing_it() {
        let ws = TempWorkspace::new("edit_same_path").unwrap();
        let source = ws.path().join("source.jpg");
        let image = solid_rgba(8, 6);
        encode_and_stamp_description(&image, &source, "source-must-survive");
        let before = std::fs::read(&source).unwrap();

        let result = save_with_options(
            &solid_rgba(3, 2),
            &source,
            Some(&source),
            SaveOptions::retain_exif(),
        );

        assert!(result.is_err());
        assert_eq!(std::fs::read(&source).unwrap(), before);
        assert!(file_has_description(&source, "source-must-survive"));
    }

    #[test]
    fn retain_preconditions_fail_before_touching_the_destination() {
        let ws = TempWorkspace::new("edit_retain_preconditions").unwrap();
        let source = ws.path().join("source.jpg");
        let unsupported = ws.path().join("existing.bmp");
        let missing_source = ws.path().join("existing.png");
        let image = solid_rgba(3, 2);
        encode_and_stamp_description(&image, &source, "keep-me");
        std::fs::write(&unsupported, b"existing bmp bytes").unwrap();
        std::fs::write(&missing_source, b"existing png bytes").unwrap();

        assert!(
            save_with_options(
                &image,
                &unsupported,
                Some(&source),
                SaveOptions::retain_exif(),
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&unsupported).unwrap(), b"existing bmp bytes");
        assert!(
            save_with_options(&image, &missing_source, None, SaveOptions::retain_exif(),).is_err()
        );
        assert_eq!(
            std::fs::read(&missing_source).unwrap(),
            b"existing png bytes"
        );
        assert!(std::fs::read_dir(ws.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".viewr-save-")
        }));
    }

    #[test]
    fn retained_exif_is_detected_by_content_after_source_rename() {
        let ws = TempWorkspace::new("edit_content_exif").unwrap();
        let original = ws.path().join("source.jpg");
        let renamed = ws.path().join("source.bmp");
        let retained = ws.path().join("retained.jpg");
        let image = solid_rgba(3, 2);
        encode_and_stamp_description(&image, &original, "content-detected");
        std::fs::rename(&original, &renamed).unwrap();

        save_with_options(
            &image,
            &retained,
            Some(&renamed),
            SaveOptions::retain_exif(),
        )
        .unwrap();

        assert!(file_has_description(&retained, "content-detected"));
    }

    #[test]
    fn retained_exif_ignores_an_oversized_source_payload_safely() {
        let ws = TempWorkspace::new("edit_bounded_exif").unwrap();
        let source = ws.path().join("hostile.png");
        let destination = ws.path().join("safe.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&(3_u32 * 1024 * 1024).to_be_bytes());
        bytes.extend_from_slice(b"eXIf");
        std::fs::write(&source, bytes).unwrap();

        let image = solid_rgba(3, 2);
        save_with_options(
            &image,
            &destination,
            Some(&source),
            SaveOptions::retain_exif(),
        )
        .unwrap();
        assert_eq!(
            image::open(destination).unwrap().to_rgba8().dimensions(),
            (3, 2)
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

    fn tag_u16(metadata: &Metadata, hex: u16) -> Vec<u16> {
        metadata
            .into_iter()
            .find_map(|tag| match tag {
                ExifTag::Orientation(values) if tag.as_u16() == hex => Some(values.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn tag_u32(metadata: &Metadata, hex: u16) -> Vec<u32> {
        metadata
            .into_iter()
            .find_map(|tag| match tag {
                ExifTag::ImageWidth(values)
                | ExifTag::ImageHeight(values)
                | ExifTag::ExifImageWidth(values)
                | ExifTag::ExifImageHeight(values)
                    if tag.as_u16() == hex =>
                {
                    Some(values.clone())
                }
                _ => None,
            })
            .unwrap_or_default()
    }
}
