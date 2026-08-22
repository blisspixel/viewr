//! Bounded TIFF page and ICO frame navigation.
//!
//! Documents are current-image RAM state, never auto-played, and never a disk
//! cache. Pages may differ in size. Decoding uses the same generation, source
//! identity, byte, and count ceilings as animation.

use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use image::ImageDecoder;
use image::metadata::Orientation;

use crate::decode::{ColorNormalizer, DecodedImage, SourceImage};
use crate::error::Error;

const MAX_PAGE_BYTES: usize = 256 * 1024 * 1024;
const MAX_PAGES: usize = 1_000;
const ICO_HEADER_LEN: u64 = 6;
const ICO_ENTRY_LEN: u64 = 16;
const ICO_TYPE_ICON: u16 = 1;

/// Container that exposes identifiable still pages instead of timed frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageKind {
    Tiff,
    Ico,
}

impl PageKind {
    #[must_use]
    pub(crate) const fn noun(self) -> &'static str {
        match self {
            Self::Tiff => "Page",
            Self::Ico => "Icon",
        }
    }
}

struct PageFrame {
    image: Arc<DecodedImage>,
}

/// A bounded, fully decoded multi-page document ready for UI-thread stepping.
pub(crate) struct DecodedPages {
    frames: Vec<PageFrame>,
    kind: PageKind,
}

impl DecodedPages {
    /// Decode TIFF pages or ICO frames. A still container returns `Ok(None)`.
    pub(crate) fn load_background_if_current(
        path: &Path,
        source: &crate::fs::ImageSource,
        current_generation: &AtomicU64,
        generation: u64,
    ) -> Result<Option<Self>, Error> {
        Self::load_background_if_current_with_hook(
            path,
            source,
            current_generation,
            generation,
            || {},
        )
    }

    fn load_background_if_current_with_hook(
        path: &Path,
        source: &crate::fs::ImageSource,
        current_generation: &AtomicU64,
        generation: u64,
        before_final_version_check: impl FnOnce(),
    ) -> Result<Option<Self>, Error> {
        let is_current = || current_generation.load(Ordering::Acquire) == generation;
        if !source.version_is_current_while(is_current) {
            return Ok(None);
        }
        let file = source
            .clone_for_decode()
            .map_err(|error| Error::Decode(error.to_string()))?;
        let pages = crate::decode::with_background_decode_permit(|| {
            Self::load_file_with_cancellation(path, file, &is_current)
        })?;
        before_final_version_check();
        if !source.version_is_current_while(is_current) {
            return Ok(None);
        }
        Ok(pages)
    }

    #[cfg(test)]
    fn load_with_cancellation(
        path: &Path,
        is_current: &impl Fn() -> bool,
    ) -> Result<Option<Self>, Error> {
        let source =
            crate::fs::ImageSource::open(path).map_err(|error| Error::Decode(error.to_string()))?;
        let file = source
            .clone_for_decode()
            .map_err(|error| Error::Decode(error.to_string()))?;
        Self::load_file_with_cancellation(path, file, is_current)
    }

    fn load_file_with_cancellation<R>(
        _path: &Path,
        mut file: R,
        is_current: &impl Fn() -> bool,
    ) -> Result<Option<Self>, Error>
    where
        R: Read + Seek,
    {
        if !is_current() {
            return Ok(None);
        }
        let format = image::ImageReader::new(BufReader::new(&mut file))
            .with_guessed_format()
            .map_err(|error| Error::Decode(format!("page format detection failed: {error}")))?
            .format();
        file.rewind()
            .map_err(|error| Error::Decode(error.to_string()))?;
        if !is_current() {
            return Ok(None);
        }
        match format {
            Some(image::ImageFormat::Tiff) => Self::decode_tiff(file, is_current),
            Some(image::ImageFormat::Ico) => Self::decode_ico(file, is_current),
            _ => Ok(None),
        }
    }

    fn decode_tiff(
        file: impl Read + Seek,
        is_current: &impl Fn() -> bool,
    ) -> Result<Option<Self>, Error> {
        let mut decoder = tiff::decoder::Decoder::new(BufReader::new(file))
            .map_err(|error| page_decode_error(&error.to_string()))?
            .with_limits(tiff_limits());
        let mut frames = Vec::new();
        let mut decoded_bytes = 0usize;
        loop {
            if !is_current() {
                return Ok(None);
            }
            if frames.len() == MAX_PAGES {
                return Err(Error::Decode(format!(
                    "document exceeds the {MAX_PAGES}-page safety limit"
                )));
            }
            let image = decode_current_tiff_page(&mut decoder, is_current)?;
            let Some(image) = image else {
                return Ok(None);
            };
            decoded_bytes = add_decoded_bytes(decoded_bytes, &image)?;
            frames.push(PageFrame {
                image: Arc::new(image),
            });
            if !decoder.more_images() {
                break;
            }
            decoder
                .next_image()
                .map_err(|error| page_decode_error(&error.to_string()))?;
        }
        Ok(finish_pages(frames, PageKind::Tiff))
    }

    fn decode_ico(
        file: impl Read + Seek,
        is_current: &impl Fn() -> bool,
    ) -> Result<Option<Self>, Error> {
        let mut reader = BufReader::new(file);
        let entries = read_ico_entries(&mut reader)?;
        let mut frames = Vec::with_capacity(entries.len());
        let mut decoded_bytes = 0usize;
        for entry in entries {
            if !is_current() {
                return Ok(None);
            }
            let image_bytes = read_ico_image_bytes(&mut reader, entry)?;
            let synthetic = synthetic_single_entry_ico(entry, &image_bytes)?;
            let mut decoder = image::codecs::ico::IcoDecoder::new(Cursor::new(synthetic))
                .map_err(|error| page_decode_error(&error.to_string()))?;
            decoder
                .set_limits(image_limits())
                .map_err(|error| page_decode_error(&error.to_string()))?;
            validate_page_canvas(decoder.dimensions())?;
            let color_normalizer = ColorNormalizer::from_decoder(&mut decoder);
            let orientation = decoder
                .orientation()
                .map_err(|error| page_decode_error(&error.to_string()))?;
            let mut image = image::DynamicImage::from_decoder(decoder)
                .map_err(|error| page_decode_error(&error.to_string()))?;
            image.apply_orientation(orientation);
            let Some(image) = normalize_dynamic_image(image, &color_normalizer, is_current)? else {
                return Ok(None);
            };
            decoded_bytes = add_decoded_bytes(decoded_bytes, &image)?;
            frames.push(PageFrame {
                image: Arc::new(image),
            });
        }
        Ok(finish_pages(frames, PageKind::Ico))
    }

    #[must_use]
    pub(crate) const fn kind(&self) -> PageKind {
        self.kind
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    fn frame_image(&self, index: usize) -> Arc<DecodedImage> {
        Arc::clone(&self.frames[index].image)
    }

    #[must_use]
    fn index_matching(&self, width: u32, height: u32) -> usize {
        if let Some(index) = self
            .frames
            .iter()
            .position(|frame| frame.image.width == width && frame.image.height == height)
        {
            return index;
        }
        self.frames
            .iter()
            .enumerate()
            .max_by_key(|(_, frame)| u64::from(frame.image.width) * u64::from(frame.image.height))
            .map_or(0, |(index, _)| index)
    }
}

/// Mutable still-page cursor. Timed playback is never started.
pub(crate) struct PageCursor {
    pages: DecodedPages,
    index: usize,
}

impl PageCursor {
    #[must_use]
    pub(crate) fn new(pages: DecodedPages) -> Self {
        debug_assert!(pages.frames.len() > 1);
        Self { pages, index: 0 }
    }

    pub(crate) fn select_matching(&mut self, width: u32, height: u32) {
        self.index = self.pages.index_matching(width, height);
    }

    #[must_use]
    pub(crate) fn current_image(&self) -> Arc<DecodedImage> {
        self.pages.frame_image(self.index)
    }

    #[must_use]
    pub(crate) const fn index(&self) -> usize {
        self.index
    }

    #[must_use]
    pub(crate) fn count(&self) -> usize {
        self.pages.len()
    }

    #[must_use]
    pub(crate) const fn kind(&self) -> PageKind {
        self.pages.kind()
    }

    #[must_use]
    pub(crate) fn can_step(&self, delta: isize) -> bool {
        stepped_index(self.index, delta, self.pages.len()).is_some()
    }

    pub(crate) fn step(&mut self, delta: isize) -> bool {
        let Some(next) = stepped_index(self.index, delta, self.pages.len()) else {
            return false;
        };
        self.index = next;
        true
    }

    #[must_use]
    pub(crate) fn position_copy(&self) -> String {
        format!(
            "{} {} of {}",
            self.kind().noun(),
            self.index.saturating_add(1),
            self.count()
        )
    }

    /// Visible identity. ICO also reports the current pixel size.
    #[must_use]
    pub(crate) fn visible_copy(&self) -> String {
        match self.kind() {
            PageKind::Tiff => self.position_copy(),
            PageKind::Ico => {
                let image = self.current_image();
                format!(
                    "{} · {}×{}",
                    self.position_copy(),
                    image.width,
                    image.height
                )
            }
        }
    }

    #[must_use]
    pub(crate) fn accessibility_copy(&self) -> String {
        let image = self.current_image();
        format!(
            "{}, {} by {}",
            self.position_copy(),
            image.width,
            image.height
        )
    }
}

/// Visible reason page or frame stepping is refused during an in-memory edit.
#[must_use]
pub(crate) const fn edit_blocks_page_step_copy() -> &'static str {
    "Finish or discard the current edit before changing pages."
}

fn finish_pages(frames: Vec<PageFrame>, kind: PageKind) -> Option<DecodedPages> {
    if frames.len() <= 1 {
        None
    } else {
        Some(DecodedPages { frames, kind })
    }
}

fn tiff_limits() -> tiff::decoder::Limits {
    let mut limits = tiff::decoder::Limits::default();
    limits.decoding_buffer_size = MAX_PAGE_BYTES;
    limits.intermediate_buffer_size = MAX_PAGE_BYTES;
    limits.ifd_value_size = MAX_PAGE_BYTES;
    limits
}

fn image_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(viewr_protocol::MAX_DECODE_DIMENSION);
    limits.max_image_height = Some(viewr_protocol::MAX_DECODE_DIMENSION);
    limits.max_alloc = Some(u64::try_from(MAX_PAGE_BYTES).unwrap_or(u64::MAX));
    limits
}

fn validate_page_canvas((width, height): (u32, u32)) -> Result<(), Error> {
    let bytes = viewr_protocol::checked_rgba_len(width, height)
        .map_err(|error| Error::Decode(error.to_string()))?;
    if bytes > MAX_PAGE_BYTES {
        return Err(Error::Decode(format!(
            "document canvas exceeds the {} MiB playback limit",
            MAX_PAGE_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

fn add_decoded_bytes(decoded_bytes: usize, image: &DecodedImage) -> Result<usize, Error> {
    let expected_bytes = viewr_protocol::checked_rgba_len(image.width, image.height)
        .map_err(|error| Error::Decode(error.to_string()))?;
    let total = decoded_bytes
        .checked_add(expected_bytes)
        .ok_or_else(|| Error::Decode("document byte count overflowed".into()))?;
    if total > MAX_PAGE_BYTES {
        return Err(Error::Decode(format!(
            "document exceeds the {} MiB playback limit",
            MAX_PAGE_BYTES / (1024 * 1024)
        )));
    }
    Ok(total)
}

fn decode_current_tiff_page<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    is_current: &impl Fn() -> bool,
) -> Result<Option<DecodedImage>, Error> {
    let dimensions = decoder
        .dimensions()
        .map_err(|error| page_decode_error(&error.to_string()))?;
    validate_page_canvas(dimensions)?;
    let color_normalizer = tiff_color_normalizer(decoder);
    let orientation = tiff_orientation(decoder);
    let mut dynamic = tiff_page_to_dynamic(decoder)?;
    dynamic.apply_orientation(orientation);
    normalize_dynamic_image(dynamic, &color_normalizer, is_current)
}

fn tiff_color_normalizer<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
) -> ColorNormalizer {
    decoder
        .get_tag_u8_vec(tiff::tags::Tag::IccProfile)
        .ok()
        .map_or_else(ColorNormalizer::assumed_srgb, |profile| {
            ColorNormalizer::from_icc_profile(&profile)
        })
}

fn tiff_orientation<R: Read + Seek>(decoder: &mut tiff::decoder::Decoder<R>) -> Orientation {
    decoder
        .find_tag(tiff::tags::Tag::Orientation)
        .ok()
        .flatten()
        .and_then(|value| value.into_u16().ok())
        .and_then(|value| u8::try_from(value).ok())
        .and_then(Orientation::from_exif)
        .unwrap_or(Orientation::NoTransforms)
}

fn tiff_page_to_dynamic<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
) -> Result<image::DynamicImage, Error> {
    let (width, height) = decoder
        .dimensions()
        .map_err(|error| page_decode_error(&error.to_string()))?;
    let color = decoder
        .colortype()
        .map_err(|error| page_decode_error(&error.to_string()))?;
    let samples = decoder
        .read_image()
        .map_err(|error| page_decode_error(&error.to_string()))?;
    match (color, samples) {
        (tiff::ColorType::RGB(8), tiff::decoder::DecodingResult::U8(buffer)) => {
            dynamic_rgb8(width, height, buffer)
        }
        (tiff::ColorType::RGBA(8), tiff::decoder::DecodingResult::U8(buffer)) => {
            dynamic_rgba8(width, height, buffer)
        }
        (tiff::ColorType::Gray(8), tiff::decoder::DecodingResult::U8(buffer)) => {
            dynamic_gray8(width, height, buffer)
        }
        (tiff::ColorType::GrayA(8), tiff::decoder::DecodingResult::U8(buffer)) => {
            dynamic_gray_alpha8(width, height, buffer)
        }
        (tiff::ColorType::RGB(16), tiff::decoder::DecodingResult::U16(buffer)) => {
            dynamic_rgb16(width, height, buffer)
        }
        (tiff::ColorType::RGBA(16), tiff::decoder::DecodingResult::U16(buffer)) => {
            dynamic_rgba16(width, height, buffer)
        }
        (tiff::ColorType::Gray(16), tiff::decoder::DecodingResult::U16(buffer)) => {
            dynamic_gray16(width, height, buffer)
        }
        (tiff::ColorType::GrayA(16), tiff::decoder::DecodingResult::U16(buffer)) => {
            dynamic_gray_alpha16(width, height, buffer)
        }
        (tiff::ColorType::CMYK(8), tiff::decoder::DecodingResult::U8(buffer)) => {
            dynamic_cmyk8(width, height, &buffer)
        }
        (other, _) => Err(Error::Decode(format!(
            "TIFF page color type {other:?} is not supported"
        ))),
    }
}

fn dynamic_rgb8(width: u32, height: u32, buffer: Vec<u8>) -> Result<image::DynamicImage, Error> {
    image::RgbImage::from_raw(width, height, buffer)
        .map(image::DynamicImage::ImageRgb8)
        .ok_or_else(|| Error::Decode("TIFF page returned an invalid RGB buffer".into()))
}

fn dynamic_rgba8(width: u32, height: u32, buffer: Vec<u8>) -> Result<image::DynamicImage, Error> {
    image::RgbaImage::from_raw(width, height, buffer)
        .map(image::DynamicImage::ImageRgba8)
        .ok_or_else(|| Error::Decode("TIFF page returned an invalid RGBA buffer".into()))
}

fn dynamic_gray8(width: u32, height: u32, buffer: Vec<u8>) -> Result<image::DynamicImage, Error> {
    image::GrayImage::from_raw(width, height, buffer)
        .map(image::DynamicImage::ImageLuma8)
        .ok_or_else(|| Error::Decode("TIFF page returned an invalid gray buffer".into()))
}

fn dynamic_gray_alpha8(
    width: u32,
    height: u32,
    buffer: Vec<u8>,
) -> Result<image::DynamicImage, Error> {
    image::ImageBuffer::<image::LumaA<u8>, _>::from_raw(width, height, buffer)
        .map(image::DynamicImage::ImageLumaA8)
        .ok_or_else(|| Error::Decode("TIFF page returned an invalid gray-alpha buffer".into()))
}

fn dynamic_rgb16(width: u32, height: u32, buffer: Vec<u16>) -> Result<image::DynamicImage, Error> {
    image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(width, height, buffer)
        .map(image::DynamicImage::ImageRgb16)
        .ok_or_else(|| Error::Decode("TIFF page returned an invalid 16-bit RGB buffer".into()))
}

fn dynamic_rgba16(width: u32, height: u32, buffer: Vec<u16>) -> Result<image::DynamicImage, Error> {
    image::ImageBuffer::<image::Rgba<u16>, _>::from_raw(width, height, buffer)
        .map(image::DynamicImage::ImageRgba16)
        .ok_or_else(|| Error::Decode("TIFF page returned an invalid 16-bit RGBA buffer".into()))
}

fn dynamic_gray16(width: u32, height: u32, buffer: Vec<u16>) -> Result<image::DynamicImage, Error> {
    image::ImageBuffer::<image::Luma<u16>, _>::from_raw(width, height, buffer)
        .map(image::DynamicImage::ImageLuma16)
        .ok_or_else(|| Error::Decode("TIFF page returned an invalid 16-bit gray buffer".into()))
}

fn dynamic_gray_alpha16(
    width: u32,
    height: u32,
    buffer: Vec<u16>,
) -> Result<image::DynamicImage, Error> {
    image::ImageBuffer::<image::LumaA<u16>, _>::from_raw(width, height, buffer)
        .map(image::DynamicImage::ImageLumaA16)
        .ok_or_else(|| {
            Error::Decode("TIFF page returned an invalid 16-bit gray-alpha buffer".into())
        })
}

fn dynamic_cmyk8(width: u32, height: u32, buffer: &[u8]) -> Result<image::DynamicImage, Error> {
    let pixel_count = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| Error::Decode("TIFF CMYK page dimensions overflowed".into()))?;
    let expected = pixel_count
        .checked_mul(4)
        .ok_or_else(|| Error::Decode("TIFF CMYK page byte count overflowed".into()))?;
    if buffer.len() != expected {
        return Err(Error::Decode(
            "TIFF page returned an invalid CMYK buffer".into(),
        ));
    }
    let mut rgb = Vec::new();
    rgb.try_reserve(pixel_count.saturating_mul(3))
        .map_err(|_| Error::Decode("TIFF CMYK page could not allocate an RGB buffer".into()))?;
    for pixel in buffer.chunks_exact(4) {
        rgb.extend_from_slice(&cmyk8_to_rgb([pixel[0], pixel[1], pixel[2], pixel[3]]));
    }
    dynamic_rgb8(width, height, rgb)
}

fn cmyk8_to_rgb(cmyk: [u8; 4]) -> [u8; 3] {
    let inverse_black = u16::from(u8::MAX.saturating_sub(cmyk[3]));
    let convert = |channel: u8| {
        let product = u16::from(u8::MAX.saturating_sub(channel)).saturating_mul(inverse_black);
        u8::try_from(product / 255).unwrap_or(u8::MAX)
    };
    [convert(cmyk[0]), convert(cmyk[1]), convert(cmyk[2])]
}

fn normalize_dynamic_image(
    image: image::DynamicImage,
    color_normalizer: &ColorNormalizer,
    is_current: &impl Fn() -> bool,
) -> Result<Option<DecodedImage>, Error> {
    let buffer = image.into_rgba8();
    let (width, height) = buffer.dimensions();
    let expected_bytes = viewr_protocol::checked_rgba_len(width, height)
        .map_err(|error| Error::Decode(error.to_string()))?;
    let rgba = buffer.into_raw();
    if rgba.len() != expected_bytes {
        return Err(Error::Decode(
            "document page returned an invalid RGBA buffer".into(),
        ));
    }
    color_normalizer.normalize_while_current(SourceImage::new(rgba, width, height)?, is_current)
}

fn page_decode_error(error: &str) -> Error {
    Error::Decode(format!("page decode failed: {error}"))
}

#[derive(Clone, Copy)]
struct IcoEntry {
    width: u8,
    height: u8,
    color_count: u8,
    reserved: u8,
    num_color_planes: u16,
    bits_per_pixel: u16,
    image_length: u32,
    image_offset: u32,
}

fn read_ico_entries<R: Read + Seek>(reader: &mut R) -> Result<Vec<IcoEntry>, Error> {
    let file_len = reader
        .seek(SeekFrom::End(0))
        .map_err(|error| Error::Decode(error.to_string()))?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| Error::Decode(error.to_string()))?;
    let mut header = [0_u8; ICO_HEADER_LEN as usize];
    reader
        .read_exact(&mut header)
        .map_err(|error| Error::Decode(error.to_string()))?;
    let count = usize::from(u16::from_le_bytes([header[4], header[5]]));
    if count == 0 {
        return Err(Error::Decode("ICO directory contains no image".into()));
    }
    if count > MAX_PAGES {
        return Err(Error::Decode(format!(
            "document exceeds the {MAX_PAGES}-page safety limit"
        )));
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let mut raw = [0_u8; ICO_ENTRY_LEN as usize];
        reader
            .read_exact(&mut raw)
            .map_err(|error| Error::Decode(error.to_string()))?;
        let entry = IcoEntry {
            width: raw[0],
            height: raw[1],
            color_count: raw[2],
            reserved: raw[3],
            num_color_planes: u16::from_le_bytes([raw[4], raw[5]]),
            bits_per_pixel: u16::from_le_bytes([raw[6], raw[7]]),
            image_length: u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]),
            image_offset: u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]),
        };
        let start = u64::from(entry.image_offset);
        let length = u64::from(entry.image_length);
        let end = start
            .checked_add(length)
            .ok_or_else(|| Error::Decode("ICO image offset overflowed".into()))?;
        if length == 0 || end > file_len {
            return Err(Error::Decode("ICO image data is outside the file".into()));
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn read_ico_image_bytes<R: Read + Seek>(reader: &mut R, entry: IcoEntry) -> Result<Vec<u8>, Error> {
    reader
        .seek(SeekFrom::Start(u64::from(entry.image_offset)))
        .map_err(|error| Error::Decode(error.to_string()))?;
    let length = usize::try_from(entry.image_length)
        .map_err(|_| Error::Decode("ICO image is larger than addressable memory".into()))?;
    if length > MAX_PAGE_BYTES {
        return Err(Error::Decode(format!(
            "document exceeds the {} MiB playback limit",
            MAX_PAGE_BYTES / (1024 * 1024)
        )));
    }
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| Error::Decode(error.to_string()))?;
    Ok(bytes)
}

fn synthetic_single_entry_ico(entry: IcoEntry, image_bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let image_offset = u32::try_from(ICO_HEADER_LEN + ICO_ENTRY_LEN)
        .expect("ICO header plus one entry fits in u32");
    let image_length = u32::try_from(image_bytes.len())
        .map_err(|_| Error::Decode("ICO image is larger than addressable memory".into()))?;
    let total = usize::try_from(image_offset)
        .ok()
        .and_then(|offset| offset.checked_add(image_bytes.len()))
        .ok_or_else(|| Error::Decode("ICO image is larger than addressable memory".into()))?;
    let mut encoded = Vec::new();
    encoded
        .try_reserve(total)
        .map_err(|_| Error::Decode("ICO image could not allocate a decode buffer".into()))?;
    encoded.extend_from_slice(&0_u16.to_le_bytes());
    encoded.extend_from_slice(&ICO_TYPE_ICON.to_le_bytes());
    encoded.extend_from_slice(&1_u16.to_le_bytes());
    encoded.push(entry.width);
    encoded.push(entry.height);
    encoded.push(entry.color_count);
    encoded.push(entry.reserved);
    encoded.extend_from_slice(&entry.num_color_planes.to_le_bytes());
    encoded.extend_from_slice(&entry.bits_per_pixel.to_le_bytes());
    encoded.extend_from_slice(&image_length.to_le_bytes());
    encoded.extend_from_slice(&image_offset.to_le_bytes());
    encoded.extend_from_slice(image_bytes);
    Ok(encoded)
}

fn stepped_index(index: usize, delta: isize, count: usize) -> Option<usize> {
    let next = index.checked_add_signed(delta)?;
    if next == index || next >= count {
        None
    } else {
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::ColorProfileStatus;
    use crate::ephemeral::TempWorkspace;
    use std::fs::File;
    use std::io::Write;

    fn write_rgb_tiff(path: &Path, pages: &[(u32, u32, [u8; 3])]) {
        let file = File::create(path).unwrap();
        let mut encoder = tiff::encoder::TiffEncoder::new(file).unwrap();
        for &(width, height, color) in pages {
            let mut data = vec![0_u8; usize::try_from(width * height * 3).unwrap()];
            for pixel in data.chunks_exact_mut(3) {
                pixel.copy_from_slice(&color);
            }
            encoder
                .write_image::<tiff::encoder::colortype::RGB8>(width, height, &data)
                .unwrap();
        }
    }

    fn write_rgba_ico(path: &Path, frames: &[(u32, u32, [u8; 4])]) {
        let encoded: Vec<_> = frames
            .iter()
            .map(|&(width, height, color)| {
                let pixels = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
                image::codecs::ico::IcoFrame::as_png(
                    &pixels.into_raw(),
                    width,
                    height,
                    image::ExtendedColorType::Rgba8,
                )
                .unwrap()
            })
            .collect();
        image::codecs::ico::IcoEncoder::new(File::create(path).unwrap())
            .encode_images(&encoded)
            .unwrap();
    }

    #[test]
    fn two_page_tiff_is_identifiable_and_never_plays() {
        let workspace = TempWorkspace::new("pages_tiff").unwrap();
        let path = workspace.path().join("two-page.tiff");
        write_rgb_tiff(&path, &[(2, 1, [255, 0, 0]), (3, 2, [0, 0, 255])]);

        let pages = DecodedPages::load_with_cancellation(&path, &|| true)
            .unwrap()
            .unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages.kind(), PageKind::Tiff);
        assert_eq!(pages.frame_image(0).width, 2);
        assert_eq!(pages.frame_image(0).height, 1);
        assert_eq!(pages.frame_image(0).rgba[0], 255);
        assert_eq!(pages.frame_image(1).width, 3);
        assert_eq!(pages.frame_image(1).height, 2);
        assert_eq!(pages.frame_image(1).rgba[2], 255);

        let mut cursor = PageCursor::new(pages);
        cursor.select_matching(3, 2);
        assert_eq!(cursor.index(), 1);
        assert_eq!(cursor.position_copy(), "Page 2 of 2");
        assert!(cursor.accessibility_copy().contains("3 by 2"));
        assert!(cursor.can_step(-1));
        assert!(!cursor.can_step(1));
        assert!(cursor.step(-1));
        assert_eq!(cursor.index(), 0);
        assert!(!cursor.step(-1));
    }

    #[test]
    fn single_page_tiff_is_not_a_navigator() {
        let workspace = TempWorkspace::new("pages_still_tiff").unwrap();
        let path = workspace.path().join("one-page.tiff");
        write_rgb_tiff(&path, &[(2, 2, [1, 2, 3])]);
        assert!(
            DecodedPages::load_with_cancellation(&path, &|| true)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn ico_frames_keep_directory_order_and_match_the_largest_still() {
        let workspace = TempWorkspace::new("pages_ico").unwrap();
        let path = workspace.path().join("sizes.ico");
        write_rgba_ico(
            &path,
            &[(16, 16, [255, 0, 0, 255]), (32, 32, [0, 0, 255, 255])],
        );

        let pages = DecodedPages::load_with_cancellation(&path, &|| true)
            .unwrap()
            .unwrap();
        assert_eq!(pages.kind(), PageKind::Ico);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages.frame_image(0).width, 16);
        assert_eq!(pages.frame_image(1).width, 32);
        assert_eq!(pages.index_matching(32, 32), 1);
        assert_eq!(pages.index_matching(8, 8), 1);

        let mut cursor = PageCursor::new(pages);
        cursor.select_matching(32, 32);
        assert_eq!(cursor.position_copy(), "Icon 2 of 2");
        assert_eq!(cursor.visible_copy(), "Icon 2 of 2 · 32×32");
        assert!(cursor.step(-1));
        assert_eq!(cursor.current_image().width, 16);
    }

    #[test]
    fn pages_are_detected_by_content_after_a_misleading_rename() {
        let workspace = TempWorkspace::new("pages_rename").unwrap();
        let tiff_path = workspace.path().join("two-page.tiff");
        let renamed = workspace.path().join("two-page.png");
        write_rgb_tiff(&tiff_path, &[(1, 1, [9, 8, 7]), (1, 1, [1, 2, 3])]);
        std::fs::rename(tiff_path, &renamed).unwrap();
        let pages = DecodedPages::load_with_cancellation(&renamed, &|| true)
            .unwrap()
            .unwrap();
        assert_eq!(pages.len(), 2);
    }

    #[test]
    fn superseded_page_decode_stops_between_pages() {
        let workspace = TempWorkspace::new("pages_cancel").unwrap();
        let path = workspace.path().join("three-page.tiff");
        write_rgb_tiff(
            &path,
            &[(2, 2, [1, 0, 0]), (2, 2, [2, 0, 0]), (2, 2, [3, 0, 0])],
        );
        let checks = std::cell::Cell::new(0_u8);
        let pages = DecodedPages::load_with_cancellation(&path, &|| {
            let next = checks.get() + 1;
            checks.set(next);
            next < 4
        })
        .unwrap();
        assert!(pages.is_none());
        assert!(checks.get() >= 4);
    }

    #[test]
    fn page_cursor_does_not_wrap() {
        let pages = DecodedPages {
            frames: vec![
                PageFrame {
                    image: Arc::new(DecodedImage {
                        rgba: vec![1, 0, 0, 255],
                        width: 1,
                        height: 1,
                        color_profile: ColorProfileStatus::AssumedSrgb,
                        working_color: crate::color::WorkingColorEncoding::SRGB_RGBA8,
                    }),
                },
                PageFrame {
                    image: Arc::new(DecodedImage {
                        rgba: vec![2, 0, 0, 255],
                        width: 1,
                        height: 1,
                        color_profile: ColorProfileStatus::AssumedSrgb,
                        working_color: crate::color::WorkingColorEncoding::SRGB_RGBA8,
                    }),
                },
            ],
            kind: PageKind::Tiff,
        };
        let mut cursor = PageCursor::new(pages);
        assert!(!cursor.step(-1));
        assert!(cursor.step(1));
        assert!(!cursor.step(1));
        assert_eq!(cursor.index(), 1);
    }

    #[test]
    fn still_png_is_not_reported_as_pages() {
        let workspace = TempWorkspace::new("pages_png").unwrap();
        let path = workspace.path().join("still.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        assert!(
            DecodedPages::load_with_cancellation(&path, &|| true)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn truncated_ico_directory_is_an_explicit_error() {
        let workspace = TempWorkspace::new("pages_bad_ico").unwrap();
        let path = workspace.path().join("truncated.ico");
        let mut file = File::create(&path).unwrap();
        file.write_all(&[0, 0, 1, 0, 2, 0, 16, 16]).unwrap();
        let Err(error) = DecodedPages::load_with_cancellation(&path, &|| true) else {
            panic!("truncated ICO must fail closed");
        };
        let message = error.to_string();
        assert!(
            message.contains("page decode failed") || message.contains("failed to fill"),
            "unexpected truncated ICO error: {message}"
        );
    }

    #[test]
    fn edit_block_copy_is_stable() {
        assert_eq!(
            edit_blocks_page_step_copy(),
            "Finish or discard the current edit before changing pages."
        );
    }
}
