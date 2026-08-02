//! Non-destructive edits and export: cropping and saving to another format.
//!
//! Export re-encodes from raw pixels. By default any metadata in the source
//! file (EXIF, GPS, camera serials) is **dropped** as a deliberate privacy
//! property. Users can opt in to retain EXIF for a session via
//! [`SaveOptions::retain_exif`]. See `docs/PRIVACY.md`.

use std::io::{BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use image::{ColorType, ImageFormat};
use little_exif::exif_tag::ExifTag;
use little_exif::filetype::FileExtension;
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

enum DestinationState {
    Absent,
    Present(crate::fs::ImageSource),
}

/// Destination identity captured immediately after the native overwrite
/// decision and rechecked immediately before commit.
pub(crate) struct SaveDestination {
    path: PathBuf,
    parent_path: PathBuf,
    parent: crate::fs::DirectorySource,
    state: DestinationState,
}

impl SaveDestination {
    fn capture(path: &Path) -> Result<Self, Error> {
        let file_name = path
            .file_name()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| Error::Encode("Save As destination requires a filename".into()))?;
        let requested_parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent_path = requested_parent
            .canonicalize()
            .map_err(|_| Error::Encode("could not verify the Save As destination parent".into()))?;
        let parent = crate::fs::DirectorySource::open(&parent_path)
            .map_err(|_| Error::Encode("could not verify the Save As destination parent".into()))?;
        let path = parent_path.join(file_name);
        let state = match parent.open_image(file_name) {
            Ok(source) if source.matches_path(&path) == crate::fs::ImageSourceMatch::Same => {
                DestinationState::Present(source)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => DestinationState::Absent,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
                return Err(Error::Encode(
                    "Save As destination must be an ordinary file path".into(),
                ));
            }
            Ok(_) | Err(_) => {
                if std::fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_file()) {
                    return Err(Error::Encode(
                        "Save As destination must be an ordinary file path".into(),
                    ));
                }
                return Err(Error::Encode(
                    "could not verify the Save As destination".into(),
                ));
            }
        };
        if !parent.matches_path(&parent_path) {
            return Err(Error::Encode(
                "Save As destination parent changed during confirmation".into(),
            ));
        }
        Ok(Self {
            path,
            parent_path,
            parent,
            state,
        })
    }

    /// Return whether the captured destination names an existing file and
    /// therefore requires an app-owned overwrite confirmation.
    #[must_use]
    pub(crate) const fn requires_overwrite_confirmation(&self) -> bool {
        matches!(&self.state, DestinationState::Present(_))
    }

    /// Revalidate the exact existing file after the user confirms overwrite.
    ///
    /// The save transaction performs the same check again before commit. This
    /// earlier check binds consent to the object captured after the native file
    /// dialog instead of trusting what happened to occupy that path later.
    pub(crate) fn confirm_overwrite(&self) -> Result<(), Error> {
        if !self.requires_overwrite_confirmation() {
            return Err(Error::Encode(
                "Save As overwrite confirmation did not name an existing file".into(),
            ));
        }
        self.verify_unchanged().map_err(|_| {
            Error::Encode(
                "Save As destination changed before overwrite confirmation; nothing was replaced"
                    .into(),
            )
        })
    }

    fn verify_parent_unchanged(&self) -> Result<(), Error> {
        if self.parent.matches_path(&self.parent_path) {
            Ok(())
        } else {
            Err(Error::Encode(
                "Save As destination parent changed after confirmation; nothing was replaced"
                    .into(),
            ))
        }
    }

    fn verify_unchanged(&self) -> Result<(), Error> {
        self.verify_parent_unchanged()?;
        let unchanged = match &self.state {
            DestinationState::Absent => std::fs::symlink_metadata(&self.path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            DestinationState::Present(source) => {
                source.matches_path(&self.path) == crate::fs::ImageSourceMatch::Same
            }
        };
        if unchanged {
            Ok(())
        } else {
            Err(Error::Encode(
                "Save As destination changed after confirmation; nothing was replaced".into(),
            ))
        }
    }

    fn aliases(&self, source: &crate::fs::ImageSource) -> bool {
        match &self.state {
            DestinationState::Absent => false,
            DestinationState::Present(destination) => destination.same_object(source),
        }
    }
}

enum ExportMetadataSource<'a> {
    Path(&'a Path),
    Accepted(&'a crate::fs::ImageSource),
}

impl ExportMetadataSource<'_> {
    fn load_bounded(&self) -> Option<Metadata> {
        match self {
            Self::Path(path) => crate::image_info::load_bounded_metadata(path),
            Self::Accepted(source) => crate::image_info::load_bounded_metadata_from_source(source),
        }
    }

    fn version_is_current(&self) -> bool {
        match self {
            Self::Path(_) => true,
            Self::Accepted(source) => source.version_is_current(),
        }
    }
}

pub(crate) fn prepare_save_destination(path: &Path) -> Result<SaveDestination, Error> {
    SaveDestination::capture(path)
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
    match crop_while(src, rect, || false)? {
        Some(image) => Ok(image),
        None => unreachable!("a crop without a cancellation source cannot be cancelled"),
    }
}

/// Crop while observing a cooperative cancellation flag before allocation and
/// between copied rows. A cancelled crop returns no partial image.
pub(crate) fn crop_cancellable(
    src: &DecodedImage,
    rect: Rect,
    cancel: &AtomicBool,
) -> Result<Option<DecodedImage>, Error> {
    crop_while(src, rect, || cancel.load(Ordering::Acquire))
}

fn crop_while(
    src: &DecodedImage,
    rect: Rect,
    mut is_cancelled: impl FnMut() -> bool,
) -> Result<Option<DecodedImage>, Error> {
    if is_cancelled() {
        return Ok(None);
    }
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
    if is_cancelled() {
        return Ok(None);
    }
    let bottom =
        r.y.checked_add(r.height)
            .ok_or_else(|| Error::Encode("crop rectangle exceeds image dimensions".to_owned()))?;
    for row in r.y..bottom {
        if is_cancelled() {
            return Ok(None);
        }
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
    if is_cancelled() {
        return Ok(None);
    }
    Ok(Some(DecodedImage {
        rgba,
        width: r.width,
        height: r.height,
        color_profile: src.color_profile,
        working_color: src.working_color,
    }))
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
    let destination = prepare_save_destination(dest)?;
    save_to_destination(
        image,
        &destination,
        source,
        None,
        source.map(ExportMetadataSource::Path),
        opts,
    )
}

pub(crate) fn save_with_accepted_source(
    image: &DecodedImage,
    destination: &SaveDestination,
    source_path: &Path,
    source: Option<&crate::fs::ImageSource>,
    opts: SaveOptions,
) -> Result<MetadataDisposition, Error> {
    save_to_destination(
        image,
        destination,
        Some(source_path),
        source,
        source.map(ExportMetadataSource::Accepted),
        opts,
    )
}

fn save_to_destination(
    image: &DecodedImage,
    destination: &SaveDestination,
    source_path: Option<&Path>,
    accepted_source: Option<&crate::fs::ImageSource>,
    metadata_source: Option<ExportMetadataSource<'_>>,
    opts: SaveOptions,
) -> Result<MetadataDisposition, Error> {
    let dest = &destination.path;
    if accepted_source.is_some_and(|source| destination.aliases(source))
        || source_path.is_some_and(|source| paths_refer_to_same_file(source, dest))
    {
        return Err(Error::Encode(
            "Save As destination must differ from the open source file".into(),
        ));
    }
    let (format, expected_len) = validate_export(image, dest)?;
    let mut retained_metadata = if opts.retain_exif {
        let Some(source) = metadata_source else {
            return Err(Error::Encode(
                "retain EXIF requested but no accepted source was available".into(),
            ));
        };
        if !path_supports_exif(dest) {
            return Err(Error::Encode(
                "destination format cannot carry EXIF (use JPEG, PNG, or WebP, or turn retain off)"
                    .into(),
            ));
        }
        if !source.version_is_current() {
            return Err(Error::Encode(
                "open source changed before metadata could be retained; nothing was saved".into(),
            ));
        }
        let metadata = source.load_bounded();
        if !source.version_is_current() {
            return Err(Error::Encode(
                "open source changed while metadata was read; nothing was saved".into(),
            ));
        }
        metadata.map(|mut metadata| {
            normalize_export_metadata(&mut metadata, image.width, image.height);
            metadata
        })
    } else {
        None
    };

    destination.verify_parent_unchanged()?;
    let suffix = dest
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| Error::Encode("output filename requires a supported extension".into()))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".viewr-save-")
        .suffix(&format!(".{suffix}"))
        .tempfile_in(&destination.parent_path)
        .map_err(|_| {
            Error::Encode("could not create a temporary output beside the destination".into())
        })?;
    destination.verify_parent_unchanged()?;

    let metadata_disposition = if retained_metadata.is_some() {
        MetadataDisposition::Retained
    } else if opts.retain_exif {
        MetadataDisposition::NotPresent
    } else {
        MetadataDisposition::Stripped
    };
    encode_pixels_only(image, temporary.as_file_mut(), dest, format, expected_len)?;
    if let Some(metadata) = retained_metadata.as_mut() {
        write_metadata_to_open_file(metadata, temporary.as_file_mut(), format)?;
    }
    commit_temporary(destination, temporary)?;
    Ok(metadata_disposition)
}

fn commit_temporary(
    destination: &SaveDestination,
    temporary: tempfile::NamedTempFile,
) -> Result<(), Error> {
    commit_temporary_with_hook(destination, temporary, || {})
}

fn commit_temporary_with_hook(
    destination: &SaveDestination,
    temporary: tempfile::NamedTempFile,
    before_install: impl FnOnce(),
) -> Result<(), Error> {
    destination.verify_unchanged()?;
    if !crate::fs::regular_path_matches_file(temporary.path(), temporary.as_file()) {
        return Err(Error::Encode(
            "temporary output changed before commit; nothing was replaced".into(),
        ));
    }
    before_install();
    match &destination.state {
        DestinationState::Absent => temporary
            .persist_noclobber(&destination.path)
            .map_err(|_| {
                Error::Encode(
                    "could not install the completed output without overwriting a new file".into(),
                )
            })
            .map(|_| ()),
        DestinationState::Present(_) => {
            #[cfg(target_os = "windows")]
            {
                let temporary = temporary.into_temp_path();
                crate::fs::replace_file(&destination.path, &temporary, None).map_err(|_| {
                    Error::Encode(
                        "could not replace the destination with the completed output".into(),
                    )
                })?;
                Ok(())
            }
            #[cfg(not(target_os = "windows"))]
            {
                temporary.persist(&destination.path).map_err(|_| {
                    Error::Encode(
                        "could not replace the destination with the completed output".into(),
                    )
                })?;
                Ok(())
            }
        }
    }
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

/// Encode RGBA pixels to an already-open staging file with no metadata written.
fn encode_pixels_only(
    image: &DecodedImage,
    writer: &mut (impl Write + Seek),
    output_path: &Path,
    format: ImageFormat,
    expected_len: usize,
) -> Result<(), Error> {
    let name = output_path
        .file_name()
        .map_or_else(|| "output".into(), |s| s.to_string_lossy().into_owned());
    let encode = |e: image::ImageError| Error::Encode(format!("{name}: {e}"));
    let mut writer = BufWriter::new(writer);
    let encoded = if supports_alpha(format) {
        image::write_buffer_with_format(
            &mut writer,
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
        image::write_buffer_with_format(
            &mut writer,
            &rgb,
            image.width,
            image.height,
            ColorType::Rgb8,
            format,
        )
        .map_err(encode)
    };
    encoded?;
    writer
        .flush()
        .map_err(|_| Error::Encode("could not flush the temporary output".into()))
}

fn write_metadata_to_open_file(
    metadata: &Metadata,
    file: &mut std::fs::File,
    format: ImageFormat,
) -> Result<(), Error> {
    let file_type = match format {
        ImageFormat::Jpeg => FileExtension::JPEG,
        ImageFormat::Png => FileExtension::PNG {
            as_zTXt_chunk: true,
        },
        ImageFormat::WebP => FileExtension::WEBP,
        _ => {
            return Err(Error::Encode("destination format cannot carry EXIF".into()));
        }
    };
    file.rewind()
        .map_err(|_| Error::Encode("could not read the temporary output for EXIF".into()))?;
    let encoded_len = usize::try_from(
        file.metadata()
            .map_err(|_| Error::Encode("could not inspect the temporary output".into()))?
            .len(),
    )
    .map_err(|_| Error::Encode("temporary output is too large to retain EXIF".into()))?;
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(encoded_len)
        .map_err(|error| Error::Encode(format!("could not allocate EXIF output: {error}")))?;
    file.read_to_end(&mut encoded)
        .map_err(|_| Error::Encode("could not read the temporary output for EXIF".into()))?;
    metadata
        .write_to_vec(&mut encoded, file_type)
        .map_err(|_| Error::Encode("could not write EXIF to the temporary output".into()))?;
    file.set_len(0)
        .map_err(|_| Error::Encode("could not rewrite the temporary EXIF output".into()))?;
    file.rewind()
        .map_err(|_| Error::Encode("could not rewrite the temporary EXIF output".into()))?;
    file.write_all(&encoded)
        .and_then(|()| file.flush())
        .map_err(|_| Error::Encode("could not rewrite the temporary EXIF output".into()))
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
        MetadataDisposition, PixelTransform, Rect, SaveOptions, commit_temporary_with_hook, crop,
        crop_cancellable, crop_while, encode_pixels_only, path_supports_exif,
        prepare_save_destination, save, save_with_accepted_source, save_with_options,
    };
    #[cfg(unix)]
    use super::{commit_temporary, write_metadata_to_open_file};
    use crate::color::WorkingColorEncoding;
    use crate::decode::DecodedImage;
    use crate::ephemeral::TempWorkspace;
    use image::ImageFormat;
    use little_exif::exif_tag::ExifTag;
    #[cfg(unix)]
    use little_exif::filetype::FileExtension;
    use little_exif::metadata::Metadata;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

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
    fn cancellable_crop_stops_before_allocation_and_between_rows() {
        let image = solid_rgba(16, 16);
        let rect = Rect {
            x: 0,
            y: 0,
            width: 16,
            height: 16,
        };
        let cancel = AtomicBool::new(true);
        assert!(crop_cancellable(&image, rect, &cancel).unwrap().is_none());

        let checks = std::cell::Cell::new(0usize);
        let cancelled = crop_while(&image, rect, || {
            let next = checks.get() + 1;
            checks.set(next);
            next >= 5
        })
        .unwrap();
        assert!(cancelled.is_none());
        assert_eq!(checks.get(), 5);
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

    #[test]
    fn save_rejects_a_destination_created_after_confirmation() {
        let ws = TempWorkspace::new("edit_destination_created").unwrap();
        let destination_path = ws.path().join("copy.png");
        let destination = prepare_save_destination(&destination_path).unwrap();
        std::fs::write(&destination_path, b"valuable replacement").unwrap();

        let result = save_with_accepted_source(
            &solid_rgba(3, 2),
            &destination,
            Path::new("source.png"),
            None,
            SaveOptions::strip(),
        );

        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&destination_path).unwrap(),
            b"valuable replacement"
        );
    }

    #[test]
    fn absent_destination_never_clobbers_a_file_created_after_final_verification() {
        let ws = TempWorkspace::new("edit_destination_noclobber").unwrap();
        let destination_path = ws.path().join("copy.png");
        let destination = prepare_save_destination(&destination_path).unwrap();
        let mut temporary = tempfile::Builder::new()
            .prefix(".viewr-save-")
            .suffix(".png")
            .tempfile_in(ws.path())
            .unwrap();
        let image = solid_rgba(3, 2);
        encode_pixels_only(
            &image,
            temporary.as_file_mut(),
            &destination_path,
            ImageFormat::Png,
            image.rgba.len(),
        )
        .unwrap();

        let result = commit_temporary_with_hook(&destination, temporary, || {
            std::fs::write(&destination_path, b"concurrent valuable file").unwrap();
        });

        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&destination_path).unwrap(),
            b"concurrent valuable file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn staging_path_substitution_cannot_redirect_pixels_or_exif() {
        use std::io::{Read, Seek};
        use std::os::unix::fs::symlink;

        let ws = TempWorkspace::new("edit_staging_identity").unwrap();
        let destination_path = ws.path().join("copy.jpg");
        let victim = ws.path().join("victim.txt");
        std::fs::write(&victim, b"valuable victim").unwrap();
        let destination = prepare_save_destination(&destination_path).unwrap();
        let mut temporary = tempfile::Builder::new()
            .prefix(".viewr-save-")
            .suffix(".jpg")
            .tempfile_in(ws.path())
            .unwrap();
        let image = solid_rgba(3, 2);
        encode_pixels_only(
            &image,
            temporary.as_file_mut(),
            &destination_path,
            ImageFormat::Jpeg,
            image.rgba.len(),
        )
        .unwrap();
        let mut metadata = Metadata::new();
        metadata.set_tag(ExifTag::ImageDescription("retained-handle".into()));
        write_metadata_to_open_file(&metadata, temporary.as_file_mut(), ImageFormat::Jpeg).unwrap();
        let mut retained = temporary.as_file().try_clone().unwrap();
        let temporary_path = temporary.path().to_owned();
        std::fs::remove_file(&temporary_path).unwrap();
        symlink(&victim, &temporary_path).unwrap();

        let error = commit_temporary(&destination, temporary).unwrap_err();

        assert_eq!(std::fs::read(&victim).unwrap(), b"valuable victim");
        assert!(error.to_string().contains("temporary output changed"));
        retained.rewind().unwrap();
        let mut encoded = Vec::new();
        retained.read_to_end(&mut encoded).unwrap();
        assert_eq!(
            image::load_from_memory_with_format(&encoded, ImageFormat::Jpeg)
                .unwrap()
                .to_rgba8()
                .dimensions(),
            (3, 2)
        );
        let metadata = Metadata::new_from_vec(&encoded, FileExtension::JPEG).unwrap();
        assert!(metadata.into_iter().any(|tag| {
            matches!(tag, ExifTag::ImageDescription(value) if value.contains("retained-handle"))
        }));
    }

    #[test]
    fn save_destination_rejects_a_non_file_entry() {
        let ws = TempWorkspace::new("edit_destination_directory").unwrap();
        let destination = ws.path().join("folder.png");
        std::fs::create_dir(&destination).unwrap();

        let error = prepare_save_destination(&destination)
            .err()
            .expect("a directory cannot be a Save As destination");

        assert_eq!(
            error.to_string(),
            "could not save image: Save As destination must be an ordinary file path"
        );
    }

    #[test]
    fn overwrite_confirmation_is_bound_to_the_captured_existing_file() {
        let ws = TempWorkspace::new("edit_overwrite_confirmation").unwrap();
        let destination_path = ws.path().join("copy.png");
        let displaced = ws.path().join("displaced.png");

        let absent = prepare_save_destination(&destination_path).unwrap();
        assert!(!absent.requires_overwrite_confirmation());

        save(&solid_rgba(1, 1), &destination_path).unwrap();
        let present = prepare_save_destination(&destination_path).unwrap();
        assert!(present.requires_overwrite_confirmation());
        present.confirm_overwrite().unwrap();

        std::fs::rename(&destination_path, &displaced).unwrap();
        std::fs::write(&destination_path, b"unconfirmed replacement").unwrap();
        let error = present.confirm_overwrite().unwrap_err();

        assert_eq!(
            error.to_string(),
            "could not save image: Save As destination changed before overwrite confirmation; nothing was replaced"
        );
        assert_eq!(
            std::fs::read(&destination_path).unwrap(),
            b"unconfirmed replacement"
        );
    }

    #[test]
    fn save_rejects_a_destination_replaced_after_confirmation() {
        let ws = TempWorkspace::new("edit_destination_replaced").unwrap();
        let destination_path = ws.path().join("copy.png");
        let displaced = ws.path().join("displaced.png");
        save(&solid_rgba(1, 1), &destination_path).unwrap();
        let destination = prepare_save_destination(&destination_path).unwrap();
        std::fs::rename(&destination_path, &displaced).unwrap();
        std::fs::write(&destination_path, b"valuable replacement").unwrap();

        let result = save_with_accepted_source(
            &solid_rgba(3, 2),
            &destination,
            Path::new("source.png"),
            None,
            SaveOptions::strip(),
        );

        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&destination_path).unwrap(),
            b"valuable replacement"
        );
    }

    #[test]
    fn save_replaces_the_exact_confirmed_destination() {
        let ws = TempWorkspace::new("edit_destination_confirmed").unwrap();
        let destination_path = ws.path().join("copy.png");
        save(&solid_rgba(1, 1), &destination_path).unwrap();
        let destination = prepare_save_destination(&destination_path).unwrap();

        save_with_accepted_source(
            &solid_rgba(3, 2),
            &destination,
            Path::new("source.png"),
            None,
            SaveOptions::strip(),
        )
        .unwrap();

        assert_eq!(
            image::open(destination_path)
                .unwrap()
                .to_rgba8()
                .dimensions(),
            (3, 2)
        );
    }

    #[test]
    fn save_rejects_a_replaced_destination_parent() {
        let ws = TempWorkspace::new("edit_destination_parent").unwrap();
        let parent = ws.path().join("selected");
        let displaced_parent = ws.path().join("displaced");
        std::fs::create_dir(&parent).unwrap();
        let destination_path = parent.join("copy.png");
        let destination = prepare_save_destination(&destination_path).unwrap();
        std::fs::rename(&parent, &displaced_parent).unwrap();
        std::fs::create_dir(&parent).unwrap();

        let error = save_with_accepted_source(
            &solid_rgba(3, 2),
            &destination,
            Path::new("source.png"),
            None,
            SaveOptions::strip(),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "could not save image: Save As destination parent changed after confirmation; nothing was replaced"
        );
        assert!(!parent.join("copy.png").exists());
        assert!(!displaced_parent.join("copy.png").exists());
    }

    #[test]
    fn save_rejects_an_alias_of_the_accepted_source_after_rename() {
        let ws = TempWorkspace::new("edit_accepted_source_alias").unwrap();
        let source_path = ws.path().join("source.png");
        let accepted_alias = ws.path().join("accepted.png");
        let image = solid_rgba(3, 2);
        save(&image, &source_path).unwrap();
        let source = crate::fs::ImageSource::open(&source_path).unwrap();
        std::fs::rename(&source_path, &accepted_alias).unwrap();
        save(&solid_rgba(1, 1), &source_path).unwrap();
        let accepted_bytes = std::fs::read(&accepted_alias).unwrap();
        let destination = prepare_save_destination(&accepted_alias).unwrap();

        let error = save_with_accepted_source(
            &image,
            &destination,
            &source_path,
            Some(&source),
            SaveOptions::strip(),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "could not save image: Save As destination must differ from the open source file"
        );
        assert_eq!(std::fs::read(&accepted_alias).unwrap(), accepted_bytes);
    }

    #[cfg(unix)]
    #[test]
    fn save_rejects_a_retargeted_symlink_sources_accepted_object() {
        use std::os::unix::fs::symlink;

        let ws = TempWorkspace::new("edit_symlink_source_alias").unwrap();
        let accepted_path = ws.path().join("accepted.png");
        let source_path = ws.path().join("source.png");
        let image = solid_rgba(3, 2);
        save(&image, &accepted_path).unwrap();
        symlink(&accepted_path, &source_path).unwrap();
        let source = crate::fs::ImageSource::open(&source_path).unwrap();
        let accepted_bytes = std::fs::read(&accepted_path).unwrap();
        std::fs::remove_file(&source_path).unwrap();
        save(&solid_rgba(1, 1), &source_path).unwrap();
        let destination = prepare_save_destination(&accepted_path).unwrap();

        let error = save_with_accepted_source(
            &image,
            &destination,
            &source_path,
            Some(&source),
            SaveOptions::strip(),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "could not save image: Save As destination must differ from the open source file"
        );
        assert_eq!(std::fs::read(&accepted_path).unwrap(), accepted_bytes);
    }

    #[test]
    fn retained_exif_fails_closed_after_the_accepted_source_is_renamed() {
        let ws = TempWorkspace::new("edit_accepted_metadata_source").unwrap();
        let source_path = ws.path().join("source.jpg");
        let original_path = ws.path().join("original.jpg");
        let destination_path = ws.path().join("copy.jpg");
        let image = solid_rgba(3, 2);
        encode_and_stamp_description(&image, &source_path, "accepted-original");
        let source = crate::fs::ImageSource::open(&source_path).unwrap();
        std::fs::rename(&source_path, &original_path).unwrap();
        encode_and_stamp_description(&image, &source_path, "replacement-metadata");
        let destination = prepare_save_destination(&destination_path).unwrap();

        let error = save_with_accepted_source(
            &image,
            &destination,
            &source_path,
            Some(&source),
            SaveOptions::retain_exif(),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "could not save image: open source changed before metadata could be retained; nothing was saved"
        );
        assert!(!destination_path.exists());
    }

    #[test]
    fn retained_exif_rejects_an_in_place_source_rewrite() {
        let ws = TempWorkspace::new("edit_accepted_source_version").unwrap();
        let source_path = ws.path().join("source.jpg");
        let replacement_path = ws.path().join("replacement.jpg");
        let destination_path = ws.path().join("copy.jpg");
        let image = solid_rgba(3, 2);
        encode_and_stamp_description(&image, &source_path, "accepted-original");
        let source = crate::fs::ImageSource::open(&source_path).unwrap();
        encode_and_stamp_description(&image, &replacement_path, "replacement-metadata");
        std::fs::write(&source_path, std::fs::read(&replacement_path).unwrap()).unwrap();
        let destination = prepare_save_destination(&destination_path).unwrap();

        let error = save_with_accepted_source(
            &image,
            &destination,
            &source_path,
            Some(&source),
            SaveOptions::retain_exif(),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "could not save image: open source changed before metadata could be retained; nothing was saved"
        );
        assert!(!destination_path.exists());
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
