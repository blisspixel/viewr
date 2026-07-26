//! Turning an image file on disk into pixels the GPU can upload.
//!
//! Pure-Rust formats decode in-process (`image`, `jxl-oxide`, `resvg`). Formats
//! that need C-backed decoders (AVIF, HEIC, RAW) are delegated to
//! [`crate::sandbox`]. Decoder-owned source pixels cross one explicit color
//! normalization boundary before becoming a [`DecodedImage`] in the renderer's
//! typed working encoding.

use std::io::{self, BufRead, BufReader, Read, Seek};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::color::WorkingColorEncoding;
use crate::error::Error;

pub(crate) use viewr_protocol::MAX_DECODE_DIMENSION;
const MAX_SVG_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ICC_PROFILE_BYTES: usize = jxl_color::icc::MAX_EMBEDDED_ICC_BYTES as usize;
const MAX_PNG_ICC_CHUNK_BYTES: usize = MAX_ICC_PROFILE_BYTES + 64 * 1024;
// PNG EXIF written as an ImageMagick-compatible raw profile is hex encoded,
// making a bounded 2 MiB TIFF payload slightly larger than 4 MiB. Keep room
// for that interoperable representation while bounding all retained text.
const MAX_PNG_TEXT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CONTAINER_CHUNKS: usize = 4096;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const JXL_CODESTREAM_SIGNATURE: &[u8] = b"\xff\x0a";
const JXL_CONTAINER_SIGNATURE: &[u8] = b"\0\0\0\x0cJXL \r\n\x87\n";
const MAX_CONCURRENT_FILE_DECODES: usize = 2;
const BACKGROUND_DECODE_QUEUE_CAPACITY: usize = 8;

#[derive(Clone, Copy)]
pub(crate) struct DecodeGeneration<'a> {
    current: Option<&'a AtomicU64>,
    expected: u64,
}

impl DecodeGeneration<'_> {
    pub(crate) const fn unconditional() -> Self {
        Self {
            current: None,
            expected: 0,
        }
    }

    pub(crate) const fn tracked(current: &AtomicU64, expected: u64) -> DecodeGeneration<'_> {
        DecodeGeneration {
            current: Some(current),
            expected,
        }
    }

    pub(crate) fn is_current(self) -> bool {
        self.current
            .is_none_or(|current| current.load(Ordering::Acquire) == self.expected)
    }

    pub(crate) fn ensure_current(self) -> io::Result<()> {
        if self.is_current() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "image decode request was superseded",
            ))
        }
    }
}

/// Reader boundary that turns generation changes into cooperative decoder
/// cancellation. Decoders observe the interruption at their next read or seek;
/// the outer request then discards that expected error instead of surfacing it.
struct GenerationReader<'a, R> {
    inner: R,
    generation: DecodeGeneration<'a>,
}

impl<'a, R> GenerationReader<'a, R> {
    const fn new(inner: R, generation: DecodeGeneration<'a>) -> Self {
        Self { inner, generation }
    }
}

impl<R: Read> Read for GenerationReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.generation.ensure_current()?;
        let read = self.inner.read(buffer)?;
        self.generation.ensure_current()?;
        Ok(read)
    }
}

impl<R: BufRead> BufRead for GenerationReader<'_, R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.generation.ensure_current()?;
        self.inner.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.inner.consume(amount);
    }
}

impl<R: Seek> Seek for GenerationReader<'_, R> {
    fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
        self.generation.ensure_current()?;
        let position = self.inner.seek(position)?;
        self.generation.ensure_current()?;
        Ok(position)
    }
}

type DecodeJob = Box<dyn FnOnce() + Send + 'static>;

#[derive(Default)]
struct LatestJobQueue {
    job: std::sync::Mutex<Option<DecodeJob>>,
    ready: std::sync::Condvar,
}

impl LatestJobQueue {
    fn replace(&self, job: DecodeJob) {
        *self
            .job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(job);
        self.ready.notify_one();
    }

    fn take(&self) -> DecodeJob {
        let mut job = self
            .job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(job) = job.take() {
                return job;
            }
            job = self
                .ready
                .wait(job)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

struct DecodeExecutor {
    foreground: std::sync::Arc<LatestJobQueue>,
    auxiliary: std::sync::Arc<LatestJobQueue>,
    presentation: std::sync::Arc<LatestJobQueue>,
    background: std::sync::mpsc::SyncSender<DecodeJob>,
    _threads: Vec<std::thread::JoinHandle<()>>,
}

impl DecodeExecutor {
    fn new() -> Result<Self, String> {
        let (background, background_rx) =
            std::sync::mpsc::sync_channel::<DecodeJob>(BACKGROUND_DECODE_QUEUE_CAPACITY);
        let background_rx = std::sync::Arc::new(std::sync::Mutex::new(background_rx));
        let mut threads = Vec::with_capacity(MAX_CONCURRENT_FILE_DECODES * 2);
        for index in 0..MAX_CONCURRENT_FILE_DECODES {
            let receiver = std::sync::Arc::clone(&background_rx);
            let thread = std::thread::Builder::new()
                .name(format!("viewr-decode-background-{index}"))
                .spawn(move || {
                    loop {
                        let job = receiver
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .recv();
                        let Ok(job) = job else {
                            break;
                        };
                        job();
                    }
                })
                .map_err(|error| format!("failed to start background decoder: {error}"))?;
            threads.push(thread);
        }

        let foreground = std::sync::Arc::new(LatestJobQueue::default());
        for index in 0..MAX_CONCURRENT_FILE_DECODES {
            let foreground_rx = std::sync::Arc::clone(&foreground);
            let thread = std::thread::Builder::new()
                .name(format!("viewr-decode-foreground-{index}"))
                .spawn(move || {
                    loop {
                        foreground_rx.take()();
                    }
                })
                .map_err(|error| format!("failed to start foreground decoder: {error}"))?;
            threads.push(thread);
        }

        let auxiliary = std::sync::Arc::new(LatestJobQueue::default());
        let auxiliary_rx = std::sync::Arc::clone(&auxiliary);
        threads.push(
            std::thread::Builder::new()
                .name("viewr-decode-current-details".into())
                .spawn(move || {
                    loop {
                        auxiliary_rx.take()();
                    }
                })
                .map_err(|error| format!("failed to start current-image decoder: {error}"))?,
        );

        let presentation = std::sync::Arc::new(LatestJobQueue::default());
        let presentation_rx = std::sync::Arc::clone(&presentation);
        threads.push(
            std::thread::Builder::new()
                .name("viewr-image-preview".into())
                .spawn(move || {
                    loop {
                        presentation_rx.take()();
                    }
                })
                .map_err(|error| format!("failed to start image preview worker: {error}"))?,
        );

        Ok(Self {
            foreground,
            auxiliary,
            presentation,
            background,
            _threads: threads,
        })
    }
}

fn decode_executor() -> Result<&'static DecodeExecutor, Error> {
    static EXECUTOR: std::sync::OnceLock<Result<DecodeExecutor, String>> =
        std::sync::OnceLock::new();
    EXECUTOR
        .get_or_init(DecodeExecutor::new)
        .as_ref()
        .map_err(|message| Error::Decode(message.clone()))
}

/// Queue a replace-latest foreground decode without creating per-open threads.
pub(crate) fn schedule_foreground_decode(job: impl FnOnce() + Send + 'static) -> Result<(), Error> {
    decode_executor()?.foreground.replace(Box::new(job));
    Ok(())
}

/// Queue replace-latest animation and metadata work for the current image.
/// This queue cannot reject the final selection when speculative work is full.
pub(crate) fn schedule_current_image_details(
    job: impl FnOnce() + Send + 'static,
) -> Result<(), Error> {
    decode_executor()?.auxiliary.replace(Box::new(job));
    Ok(())
}

/// Queue replace-latest preview preparation without blocking decode or UI work.
pub(crate) fn schedule_image_preview(job: impl FnOnce() + Send + 'static) -> Result<(), Error> {
    decode_executor()?.presentation.replace(Box::new(job));
    Ok(())
}

/// Queue bounded speculative work, returning false when the queue is saturated.
pub(crate) fn schedule_background_decode(job: impl FnOnce() + Send + 'static) -> bool {
    decode_executor()
        .is_ok_and(|executor| try_schedule_background(&executor.background, Box::new(job)))
}

/// Run decode work under the shared background-priority concurrency gate.
pub(crate) fn with_background_decode_permit<T>(work: impl FnOnce() -> T) -> T {
    let _permit = acquire_decode_permit(DecodePriority::Background);
    work()
}

fn try_schedule_background(
    sender: &std::sync::mpsc::SyncSender<DecodeJob>,
    job: DecodeJob,
) -> bool {
    sender.try_send(job).is_ok()
}

#[derive(Default)]
struct DecodeGateState {
    active: usize,
    waiting_foreground: usize,
}

struct DecodeGate {
    state: std::sync::Mutex<DecodeGateState>,
    available: std::sync::Condvar,
}

struct DecodePermit(std::sync::Arc<DecodeGate>);

#[derive(Clone, Copy)]
enum DecodePriority {
    Foreground,
    Background,
}

impl Drop for DecodePermit {
    fn drop(&mut self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active = state.active.saturating_sub(1);
        self.0.available.notify_all();
    }
}

fn acquire_decode_permit(priority: DecodePriority) -> DecodePermit {
    static GATE: std::sync::OnceLock<std::sync::Arc<DecodeGate>> = std::sync::OnceLock::new();
    let gate = GATE.get_or_init(|| {
        std::sync::Arc::new(DecodeGate {
            state: std::sync::Mutex::new(DecodeGateState::default()),
            available: std::sync::Condvar::new(),
        })
    });
    acquire_decode_permit_from(std::sync::Arc::clone(gate), priority)
}

fn acquire_decode_permit_from(
    gate: std::sync::Arc<DecodeGate>,
    priority: DecodePriority,
) -> DecodePermit {
    let mut state = gate
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if matches!(priority, DecodePriority::Foreground) {
        state.waiting_foreground += 1;
        while state.active >= MAX_CONCURRENT_FILE_DECODES {
            state = gate
                .available
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.waiting_foreground = state.waiting_foreground.saturating_sub(1);
    } else {
        while state.active >= MAX_CONCURRENT_FILE_DECODES || state.waiting_foreground > 0 {
            state = gate
                .available
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
    state.active += 1;
    drop(state);
    DecodePermit(gate)
}

/// Decoder-owned RGBA8 pixels that have not entered the working color space.
pub(crate) struct SourceImage {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl SourceImage {
    pub(crate) fn new(rgba: Vec<u8>, width: u32, height: u32) -> Result<Self, Error> {
        let expected_size = validate_dimensions(width, height)?;
        if rgba.len() != expected_size {
            return Err(Error::Decode(
                "decoder returned an invalid RGBA buffer".into(),
            ));
        }
        Ok(Self {
            rgba,
            width,
            height,
        })
    }

    fn from_dynamic_image(decoded: image::DynamicImage) -> Result<Self, Error> {
        let width = decoded.width();
        let height = decoded.height();
        Self::new(decoded.into_rgba8().into_raw(), width, height)
    }

    fn into_working(self, color_profile: ColorProfileStatus) -> DecodedImage {
        DecodedImage {
            rgba: self.rgba,
            width: self.width,
            height: self.height,
            color_profile,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        }
    }
}

/// A decoded image in the form the renderer uploads: tightly packed RGBA8 sRGB,
/// eight bits per channel, `width * height * 4` bytes, top row first.
pub struct DecodedImage {
    /// Row-major RGBA8 pixels, no padding.
    pub rgba: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How embedded color metadata was handled before the pixels reached the GPU.
    pub color_profile: ColorProfileStatus,
    /// Complete color-space and storage interpretation for `rgba`.
    pub working_color: WorkingColorEncoding,
}

/// Color-management result attached to decoded pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorProfileStatus {
    /// No usable embedded color profile was present, so standard sRGB is assumed.
    #[default]
    AssumedSrgb,
    /// Embedded metadata explicitly identifies standard sRGB component values.
    TaggedSrgb,
    /// An embedded RGB ICC profile was converted into sRGB.
    ConvertedToSrgb,
    /// Embedded color metadata was present but could not be converted safely.
    EmbeddedProfileFallback,
    /// An isolated decoder could not establish the pixel stream's color space.
    UnknownWorkerProfileFallback,
}

impl ColorProfileStatus {
    /// Short, user-facing description for Image Information.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::AssumedSrgb => "sRGB",
            Self::TaggedSrgb => "Embedded color metadata: sRGB",
            Self::ConvertedToSrgb => "Embedded ICC converted to sRGB",
            Self::EmbeddedProfileFallback => "Embedded color metadata unavailable; sRGB fallback",
            Self::UnknownWorkerProfileFallback => "Worker color space unknown; sRGB fallback",
        }
    }
}

pub(crate) struct ColorNormalizer {
    transform: Option<std::sync::Arc<moxcms::Transform8BitExecutor>>,
    status: ColorProfileStatus,
}

impl ColorNormalizer {
    pub(crate) fn assumed_srgb() -> Self {
        Self::without_transform(ColorProfileStatus::AssumedSrgb)
    }

    pub(crate) fn tagged_srgb() -> Self {
        Self::without_transform(ColorProfileStatus::TaggedSrgb)
    }

    pub(crate) fn unsupported_profile() -> Self {
        Self::without_transform(ColorProfileStatus::EmbeddedProfileFallback)
    }

    pub(crate) fn unknown_worker_profile() -> Self {
        Self::without_transform(ColorProfileStatus::UnknownWorkerProfileFallback)
    }

    fn without_transform(status: ColorProfileStatus) -> Self {
        Self {
            transform: None,
            status,
        }
    }

    pub(crate) fn from_decoder(decoder: &mut impl image::ImageDecoder) -> Self {
        match decoder.icc_profile() {
            Ok(Some(profile)) => Self::from_icc_profile(&profile),
            Ok(None) => Self::assumed_srgb(),
            Err(_) => Self::unsupported_profile(),
        }
    }

    pub(crate) fn from_icc_profile(profile: &[u8]) -> Self {
        if profile.len() > MAX_ICC_PROFILE_BYTES {
            return Self::fallback();
        }
        let Ok(source) = moxcms::ColorProfile::new_from_slice(profile) else {
            return Self::fallback();
        };
        Self::from_color_profile(&source)
    }

    fn from_color_profile(source: &moxcms::ColorProfile) -> Self {
        if source.color_space != moxcms::DataColorSpace::Rgb {
            return Self::fallback();
        }
        let destination = moxcms::ColorProfile::new_srgb();
        let Ok(transform) = source.create_transform_8bit(
            moxcms::Layout::Rgba,
            &destination,
            moxcms::Layout::Rgba,
            moxcms::TransformOptions::default(),
        ) else {
            return Self::fallback();
        };
        Self {
            transform: Some(transform),
            status: ColorProfileStatus::ConvertedToSrgb,
        }
    }

    fn fallback() -> Self {
        Self::unsupported_profile()
    }

    pub(crate) fn normalize(&self, source: SourceImage) -> Result<DecodedImage, Error> {
        self.normalize_if_current(source, DecodeGeneration::unconditional())
            .map_err(|error| Error::Decode(error.to_string()))
    }

    pub(crate) fn normalize_if_current(
        &self,
        source: SourceImage,
        generation: DecodeGeneration<'_>,
    ) -> io::Result<DecodedImage> {
        self.normalize_with_check(source, || generation.ensure_current())
    }

    pub(crate) fn normalize_while_current(
        &self,
        source: SourceImage,
        is_current: &impl Fn() -> bool,
    ) -> Result<Option<DecodedImage>, Error> {
        match self.normalize_with_check(source, || {
            if is_current() {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "image decode request was superseded",
                ))
            }
        }) {
            Ok(image) => Ok(Some(image)),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(None),
            Err(error) => Err(Error::Decode(error.to_string())),
        }
    }

    fn normalize_with_check(
        &self,
        mut source: SourceImage,
        ensure_current: impl Fn() -> io::Result<()>,
    ) -> io::Result<DecodedImage> {
        ensure_current()?;
        let Some(transform) = self.transform.as_ref() else {
            return Ok(source.into_working(self.status));
        };
        let Some(row_bytes) = usize::try_from(source.width)
            .ok()
            .and_then(|width| width.checked_mul(4))
        else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "source row size exceeds this platform",
            ));
        };
        if row_bytes == 0 || !source.rgba.len().is_multiple_of(row_bytes) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "source pixels do not match their dimensions",
            ));
        }
        let mut converted = Vec::new();
        converted
            .try_reserve_exact(row_bytes)
            .map_err(|_| io::Error::other("not enough memory for one converted image row"))?;
        converted.resize(row_bytes, 0);
        for row in source.rgba.chunks_exact_mut(row_bytes) {
            ensure_current()?;
            transform.transform(row, &mut converted).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "embedded ICC transform failed")
            })?;
            row.copy_from_slice(&converted);
        }
        ensure_current()?;
        Ok(source.into_working(self.status))
    }
}

impl DecodedImage {
    /// Decode the image at `path`, choosing the format by content then extension.
    ///
    /// # Errors
    /// Returns [`Error::Decode`] if the file cannot be read or is not a supported,
    /// well-formed image.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let _permit = acquire_decode_permit(DecodePriority::Foreground);
        Self::load_file(path)
    }

    /// Decode a speculative thumbnail or neighbor without overtaking foreground work.
    pub(crate) fn load_background(path: &Path) -> Result<Self, Error> {
        let _permit = acquire_decode_permit(DecodePriority::Background);
        Self::load_file(path)
    }

    /// Decode only if `generation` still identifies the latest foreground request.
    pub(crate) fn load_if_current(
        path: &Path,
        current_generation: &AtomicU64,
        generation: u64,
    ) -> Result<Option<Self>, Error> {
        let _permit = acquire_decode_permit(DecodePriority::Foreground);
        Self::load_file_if_current(
            path,
            DecodeGeneration::tracked(current_generation, generation),
        )
    }

    fn load_file(path: &Path) -> Result<Self, Error> {
        Self::load_file_if_current(path, DecodeGeneration::unconditional())?.ok_or_else(|| {
            Error::Decode("unconditional image decode stopped before completion".into())
        })
    }

    fn load_file_if_current(
        path: &Path,
        generation: DecodeGeneration<'_>,
    ) -> Result<Option<Self>, Error> {
        if !generation.is_current() {
            return Ok(None);
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        let result = if crate::fs::is_worker_format(path) {
            crate::sandbox::load_via_worker(path, generation)
        } else if ext == "jxl" {
            Self::load_jxl(path, generation)
        } else if ext == "svg" {
            Self::load_svg(path, generation)
        } else {
            Self::load_core_image(path, generation)
        };

        if !generation.is_current() {
            return Ok(None);
        }
        result.map(Some)
    }

    fn load_core_image(path: &Path, generation: DecodeGeneration<'_>) -> Result<Self, Error> {
        let file = std::fs::File::open(path)
            .map_err(|error| Error::Decode(format!("open/decode failed: {error}")))?;
        let mut reader = GenerationReader::new(BufReader::new(file), generation);
        enforce_embedded_metadata_limits(&mut reader)?;
        reader
            .rewind()
            .map_err(|error| Error::Decode(format!("open/decode failed: {error}")))?;
        // Never embed full filesystem paths in error strings (privacy / logs).
        let mut reader = image::ImageReader::new(reader);
        if let Ok(format) = image::ImageFormat::from_path(path) {
            reader.set_format(format);
        }
        let reader = reader
            .with_guessed_format()
            .map_err(|error| Error::Decode(format!("open/decode failed: {error}")))?;
        Self::decode_image_reader(reader, generation)
    }

    /// Decode image bytes already in memory (no temp file, no path on disk).
    ///
    /// Used by doctor / default benchmark so product diagnostics leave **zero**
    /// debris under the system temp directory.
    ///
    /// # Errors
    /// Returns [`Error::Decode`] if the bytes are not a supported, well-formed image.
    pub fn load_from_memory(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.starts_with(JXL_CODESTREAM_SIGNATURE) || bytes.starts_with(JXL_CONTAINER_SIGNATURE)
        {
            return Self::decode_jxl(
                std::io::Cursor::new(bytes),
                DecodeGeneration::unconditional(),
            );
        }

        // SVG is not handled by `image::load_from_memory`; sniff the payload.
        let trimmed = bytes
            .iter()
            .position(|&b| !b.is_ascii_whitespace())
            .map_or(bytes, |i| &bytes[i..]);
        if trimmed.starts_with(b"<svg")
            || trimmed.starts_with(b"<?xml")
            || trimmed.starts_with(b"<SVG")
        {
            return Self::load_svg_bytes(trimmed);
        }

        enforce_embedded_metadata_limits(std::io::Cursor::new(bytes))?;

        let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| Error::Decode(format!("decode failed: {e}")))?;
        Self::decode_image_reader(reader, DecodeGeneration::unconditional())
    }

    /// Decode bytes using the decoder associated with a core file extension.
    ///
    /// Unlike [`Self::load_from_memory`], this does not depend on a recognizable
    /// file signature. It is useful for non-seekable callers that receive the
    /// original filename separately and for exercising every decoder with
    /// malformed fuzz input.
    ///
    /// # Errors
    /// Returns [`Error::Decode`] when `extension` is not an enabled core format,
    /// or when the bytes are not a well-formed image of that format.
    pub fn load_from_memory_with_extension(bytes: &[u8], extension: &str) -> Result<Self, Error> {
        let extension = extension
            .strip_prefix('.')
            .unwrap_or(extension)
            .to_ascii_lowercase();
        if !crate::fs::CORE_EXTENSIONS.contains(&extension.as_str()) {
            return Err(Error::Decode("unsupported core image format".into()));
        }
        match extension.as_str() {
            "jxl" => Self::decode_jxl(
                std::io::Cursor::new(bytes),
                DecodeGeneration::unconditional(),
            ),
            "svg" => Self::load_svg_bytes(bytes),
            _ => {
                enforce_embedded_metadata_limits(std::io::Cursor::new(bytes))?;
                let format = image::ImageFormat::from_extension(&extension)
                    .ok_or_else(|| Error::Decode("unsupported core image format".into()))?;
                let reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
                Self::decode_image_reader(reader, DecodeGeneration::unconditional())
            }
        }
    }

    fn decode_image_reader<R>(
        mut reader: image::ImageReader<R>,
        generation: DecodeGeneration<'_>,
    ) -> Result<Self, Error>
    where
        R: BufRead + Seek,
    {
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_DECODE_DIMENSION);
        limits.max_image_height = Some(MAX_DECODE_DIMENSION);
        limits.max_alloc = Some(viewr_protocol::MAX_RGBA_BYTES);
        reader.limits(limits);

        let mut decoder = reader
            .into_decoder()
            .map_err(|e| Error::Decode(format!("decode failed: {e}")))?;
        validate_decoder_allocation(&decoder)?;
        let color_normalizer = ColorNormalizer::from_decoder(&mut decoder);
        let orientation = image::ImageDecoder::orientation(&mut decoder)
            .map_err(|e| Error::Decode(format!("could not read image orientation: {e}")))?;
        let mut dynamic_image = image::DynamicImage::from_decoder(decoder)
            .map_err(|e| Error::Decode(format!("decode failed: {e}")))?;
        dynamic_image.apply_orientation(orientation);
        color_normalizer
            .normalize_if_current(SourceImage::from_dynamic_image(dynamic_image)?, generation)
            .map_err(|error| Error::Decode(error.to_string()))
    }

    fn load_jxl(path: &Path, generation: DecodeGeneration<'_>) -> Result<Self, Error> {
        let file = std::fs::File::open(path).map_err(|e| Error::Decode(e.to_string()))?;
        Self::decode_jxl(
            GenerationReader::new(BufReader::new(file), generation),
            generation,
        )
    }

    fn decode_jxl(reader: impl Read, generation: DecodeGeneration<'_>) -> Result<Self, Error> {
        let mut jxl = jxl_oxide::integration::JxlDecoder::new(reader)
            .map_err(|e| Error::Decode(format!("failed to init JXL decoder: {e}")))?;
        validate_decoder_allocation(&jxl)?;
        let color_normalizer = ColorNormalizer::from_decoder(&mut jxl);
        let decoded = image::DynamicImage::from_decoder(jxl)
            .map_err(|e| Error::Decode(format!("failed to decode JXL: {e}")))?;
        color_normalizer
            .normalize_if_current(SourceImage::from_dynamic_image(decoded)?, generation)
            .map_err(|error| Error::Decode(error.to_string()))
    }

    /// Render an SVG to RGBA8 with pure-Rust `resvg` / `usvg`.
    fn load_svg(path: &Path, generation: DecodeGeneration<'_>) -> Result<Self, Error> {
        let file = std::fs::File::open(path).map_err(|e| Error::Decode(e.to_string()))?;
        let reader = GenerationReader::new(BufReader::new(file), generation);
        let mut data = Vec::new();
        reader
            .take(MAX_SVG_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(|e| Error::Decode(e.to_string()))?;
        generation
            .ensure_current()
            .map_err(|error| Error::Decode(error.to_string()))?;
        if data.len() as u64 > MAX_SVG_BYTES {
            return Err(Error::Decode("SVG input exceeds safety limit".into()));
        }
        Self::load_svg_bytes(&data)
    }

    /// Render SVG markup already held in memory (no temp file).
    fn load_svg_bytes(data: &[u8]) -> Result<Self, Error> {
        if data.len() as u64 > MAX_SVG_BYTES {
            return Err(Error::Decode("SVG input exceeds safety limit".into()));
        }
        let options = resvg::usvg::Options::default();
        let tree = resvg::usvg::Tree::from_data(data, &options)
            .map_err(|e| Error::Decode(format!("failed to parse SVG: {e}")))?;

        let size = tree.size();
        let width = positive_f32_to_px(size.width());
        let height = positive_f32_to_px(size.height());
        let expected_size = validate_dimensions(width, height)?;

        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| Error::Decode("SVG produced invalid pixel dimensions".into()))?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::default(),
            &mut pixmap.as_mut(),
        );

        let rgba = pixmap.take();
        if rgba.len() != expected_size {
            return Err(Error::Decode(
                "SVG renderer returned an invalid RGBA buffer".into(),
            ));
        }
        ColorNormalizer::assumed_srgb().normalize(SourceImage::new(rgba, width, height)?)
    }
}

pub(crate) fn enforce_embedded_metadata_limits(mut reader: impl Read + Seek) -> Result<(), Error> {
    let mut signature = [0_u8; 12];
    let signature_len = reader.read(&mut signature).unwrap_or(0);
    reader
        .rewind()
        .map_err(|error| Error::Decode(format!("could not inspect color metadata: {error}")))?;
    if signature_len >= PNG_SIGNATURE.len() && &signature[..PNG_SIGNATURE.len()] == PNG_SIGNATURE {
        return enforce_png_icc_limit(&mut reader);
    }
    if signature_len >= 12 && &signature[..4] == b"RIFF" && &signature[8..12] == b"WEBP" {
        return enforce_webp_icc_limit(&mut reader);
    }
    Ok(())
}

fn enforce_png_icc_limit(reader: &mut (impl Read + Seek)) -> Result<(), Error> {
    let mut signature = [0_u8; 8];
    if reader.read_exact(&mut signature).is_err() || signature != *PNG_SIGNATURE {
        return Ok(());
    }

    let mut saw_icc = false;
    let mut saw_exif = false;
    let mut text_bytes = 0_u64;
    for _ in 0..MAX_CONTAINER_CHUNKS {
        let mut header = [0_u8; 8];
        if reader.read_exact(&mut header).is_err() {
            return Ok(());
        }
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let kind = [header[4], header[5], header[6], header[7]];
        if matches!(&kind, b"tEXt" | b"zTXt" | b"iTXt") {
            text_bytes = text_bytes
                .checked_add(u64::from(length))
                .ok_or_else(|| Error::Decode("PNG text metadata length overflowed".into()))?;
            if u64::from(length) > MAX_PNG_TEXT_BYTES || text_bytes > MAX_PNG_TEXT_BYTES {
                return Err(Error::Decode(
                    "PNG text metadata exceeds safety limit".into(),
                ));
            }
        }
        if &kind == b"eXIf" {
            if saw_exif {
                return Err(Error::Decode(
                    "PNG contains multiple embedded EXIF payloads".into(),
                ));
            }
            saw_exif = true;
            if u64::from(length) > crate::image_info::MAX_EXIF_BYTES {
                return Err(Error::Decode(
                    "PNG EXIF payload exceeds safety limit".into(),
                ));
            }
        }
        if &kind == b"iCCP" {
            if saw_icc {
                return Err(Error::Decode(
                    "PNG contains multiple embedded ICC profiles".into(),
                ));
            }
            saw_icc = true;
            let length = usize::try_from(length)
                .map_err(|_| Error::Decode("PNG ICC chunk exceeds safety limit".into()))?;
            if length > MAX_PNG_ICC_CHUNK_BYTES {
                return Err(Error::Decode("PNG ICC chunk exceeds safety limit".into()));
            }
            let mut payload = Vec::new();
            payload
                .try_reserve_exact(length)
                .map_err(|_| Error::Decode("not enough memory for bounded PNG ICC data".into()))?;
            payload.resize(length, 0);
            if reader.read_exact(&mut payload).is_err() {
                return Ok(());
            }
            validate_compressed_png_icc(&payload)?;
        } else if reader
            .seek(std::io::SeekFrom::Current(i64::from(length)))
            .is_err()
        {
            return Ok(());
        }

        if reader.seek(std::io::SeekFrom::Current(4)).is_err() {
            return Ok(());
        }
        if &kind == b"IEND" {
            return Ok(());
        }
    }
    Err(Error::Decode(
        "PNG contains too many chunks before image data".into(),
    ))
}

fn enforce_webp_icc_limit(reader: &mut (impl Read + Seek)) -> Result<(), Error> {
    let file_len = reader
        .seek(std::io::SeekFrom::End(0))
        .map_err(|error| Error::Decode(format!("could not inspect WebP metadata: {error}")))?;
    if file_len < 12 {
        return Ok(());
    }
    reader
        .seek(std::io::SeekFrom::Start(12))
        .map_err(|error| Error::Decode(format!("could not inspect WebP metadata: {error}")))?;

    let mut saw_icc = false;
    let mut saw_exif = false;
    for _ in 0..MAX_CONTAINER_CHUNKS {
        let mut header = [0_u8; 8];
        if reader.read_exact(&mut header).is_err() {
            return Ok(());
        }
        let kind = &header[..4];
        let length = u64::from(u32::from_le_bytes([
            header[4], header[5], header[6], header[7],
        ]));
        if kind == b"ICCP" {
            if saw_icc {
                return Err(Error::Decode(
                    "WebP contains multiple embedded ICC profiles".into(),
                ));
            }
            saw_icc = true;
            if length > u64::try_from(MAX_ICC_PROFILE_BYTES).unwrap_or(u64::MAX) {
                return Err(Error::Decode(
                    "WebP ICC profile exceeds safety limit".into(),
                ));
            }
        }
        if kind == b"EXIF" {
            if saw_exif {
                return Err(Error::Decode(
                    "WebP contains multiple embedded EXIF payloads".into(),
                ));
            }
            saw_exif = true;
            if length > crate::image_info::MAX_EXIF_BYTES {
                return Err(Error::Decode(
                    "WebP EXIF payload exceeds safety limit".into(),
                ));
            }
        }

        let padded = length
            .checked_add(length % 2)
            .ok_or_else(|| Error::Decode("WebP chunk length overflowed".into()))?;
        let position = reader
            .stream_position()
            .map_err(|error| Error::Decode(format!("could not inspect WebP metadata: {error}")))?;
        let Some(next) = position.checked_add(padded) else {
            return Err(Error::Decode("WebP chunk length overflowed".into()));
        };
        if next > file_len {
            return Ok(());
        }
        reader
            .seek(std::io::SeekFrom::Start(next))
            .map_err(|error| Error::Decode(format!("could not inspect WebP metadata: {error}")))?;
    }
    Err(Error::Decode("WebP contains too many chunks".into()))
}

fn validate_compressed_png_icc(payload: &[u8]) -> Result<(), Error> {
    let Some(name_end) = payload.iter().position(|byte| *byte == 0) else {
        return Ok(());
    };
    let Some((&compression_method, compressed)) = payload[name_end + 1..].split_first() else {
        return Ok(());
    };
    if compression_method != 0 {
        return Ok(());
    }

    let decoder = flate2::read::ZlibDecoder::new(compressed);
    let limit = u64::try_from(MAX_ICC_PROFILE_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bounded = decoder.take(limit);
    let mut profile = Vec::new();
    profile
        .try_reserve_exact(MAX_ICC_PROFILE_BYTES.min(64 * 1024))
        .map_err(|_| Error::Decode("not enough memory for bounded PNG ICC data".into()))?;
    if bounded.read_to_end(&mut profile).is_err() {
        return Ok(());
    }
    if profile.len() > MAX_ICC_PROFILE_BYTES {
        return Err(Error::Decode("PNG ICC profile exceeds safety limit".into()));
    }
    Ok(())
}

fn validate_dimensions(width: u32, height: u32) -> Result<usize, Error> {
    viewr_protocol::checked_rgba_len(width, height)
        .map_err(|error| Error::Decode(error.to_string()))
}

fn validate_decoder_allocation(decoder: &impl image::ImageDecoder) -> Result<(), Error> {
    let (width, height) = decoder.dimensions();
    validate_dimensions(width, height)?;
    if decoder.total_bytes() > viewr_protocol::MAX_RGBA_BYTES {
        return Err(Error::Decode(
            "decoder output allocation exceeds safety limit".into(),
        ));
    }
    Ok(())
}

/// Convert a non-negative CSS/SVG length to a whole pixel count of at least 1.
pub(crate) fn positive_f32_to_px(v: f32) -> u32 {
    let v = v.ceil().max(1.0);
    if v >= MAX_DECODE_DIMENSION as f32 {
        // Cap absurd sizes; viewports larger than this are not useful for a still viewer.
        MAX_DECODE_DIMENSION
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
    use super::{
        ColorNormalizer, ColorProfileStatus, DecodeGate, DecodeGateState, DecodeGeneration,
        DecodePriority, DecodedImage, GenerationReader, LatestJobQueue,
        MAX_CONCURRENT_FILE_DECODES, MAX_DECODE_DIMENSION, MAX_ICC_PROFILE_BYTES, PNG_SIGNATURE,
        SourceImage, acquire_decode_permit_from, positive_f32_to_px, try_schedule_background,
        validate_dimensions,
    };
    use crate::color::WorkingColorEncoding;
    use crate::ephemeral::TempWorkspace;
    use little_exif::exif_tag::ExifTag;
    use little_exif::metadata::Metadata;
    use std::fs;
    use std::io::{Cursor, Read, Write};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    use std::time::{Duration, Instant};
    use viewr_protocol::MAX_DECODE_PIXELS;

    #[test]
    fn svg_renders_to_declared_size() {
        let ws = TempWorkspace::new("decode_svg").unwrap();
        let path = ws.path().join("box.svg");
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
    }

    #[test]
    fn non_image_is_decode_error() {
        let ws = TempWorkspace::new("decode_bad").unwrap();
        let path = ws.path().join("x.txt");
        fs::write(&path, b"not an image").unwrap();
        assert!(DecodedImage::load(&path).is_err());
    }

    #[test]
    fn png_round_trip_dimensions() {
        let ws = TempWorkspace::new("decode_png").unwrap();
        let path = ws.path().join("g.png");
        let img = image::RgbImage::from_fn(8, 6, |x, y| {
            image::Rgb([(x * 20) as u8, (y * 30) as u8, 100])
        });
        img.save(&path).unwrap();
        let decoded = DecodedImage::load(&path).expect("png");
        assert_eq!((decoded.width, decoded.height), (8, 6));
        assert_eq!(decoded.rgba.len(), 8 * 6 * 4);
        assert_eq!(decoded.color_profile, ColorProfileStatus::AssumedSrgb);
        assert_eq!(decoded.working_color, WorkingColorEncoding::SRGB_RGBA8);
    }

    #[test]
    fn display_p3_pixels_are_converted_to_srgb_without_changing_alpha() {
        let normalizer =
            ColorNormalizer::from_color_profile(&moxcms::ColorProfile::new_display_p3());
        let original = vec![210, 120, 35, 17, 40, 180, 210, 231];
        let source = SourceImage::new(original.clone(), 1, 2).unwrap();
        let image = normalizer.normalize(source).unwrap();

        assert_eq!(image.color_profile, ColorProfileStatus::ConvertedToSrgb);
        assert_eq!(image.working_color, WorkingColorEncoding::SRGB_RGBA8);
        assert_ne!(image.rgba, original);
        assert_eq!(image.rgba[3], 17);
        assert_eq!(image.rgba[7], 231);
        assert_eq!(
            [
                ColorProfileStatus::AssumedSrgb.label(),
                ColorProfileStatus::TaggedSrgb.label(),
                ColorProfileStatus::ConvertedToSrgb.label(),
                ColorProfileStatus::EmbeddedProfileFallback.label(),
                ColorProfileStatus::UnknownWorkerProfileFallback.label(),
            ],
            [
                "sRGB",
                "Embedded color metadata: sRGB",
                "Embedded ICC converted to sRGB",
                "Embedded color metadata unavailable; sRGB fallback",
                "Worker color space unknown; sRGB fallback",
            ]
        );

        assert!(SourceImage::new(vec![10, 20, 30, 255], 0, 1).is_err());
    }

    #[test]
    fn color_normalization_checks_cancellation_between_rows() {
        let normalizer =
            ColorNormalizer::from_color_profile(&moxcms::ColorProfile::new_display_p3());
        let source = SourceImage::new(vec![120; 4 * 4], 1, 4).unwrap();
        let current = AtomicU64::new(7);
        let checks = std::cell::Cell::new(0_u8);

        let error = normalizer
            .normalize_with_check(source, || {
                let next = checks.get() + 1;
                checks.set(next);
                if next == 3 {
                    current.store(8, std::sync::atomic::Ordering::Release);
                }
                DecodeGeneration::tracked(&current, 7).ensure_current()
            })
            .err()
            .expect("superseding a multi-row transform must cancel it");

        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(checks.get(), 3);
    }

    #[test]
    fn invalid_or_incompatible_icc_profiles_fall_back_without_mutating_pixels() {
        for normalizer in [
            ColorNormalizer::from_icc_profile(b"not an ICC profile"),
            ColorNormalizer::from_color_profile(&moxcms::ColorProfile::new_gray_with_gamma(2.2)),
        ] {
            let image = normalizer
                .normalize(SourceImage::new(vec![10, 20, 30, 40], 1, 1).unwrap())
                .unwrap();
            assert_eq!(
                image.color_profile,
                ColorProfileStatus::EmbeddedProfileFallback
            );
            assert_eq!(image.rgba, vec![10, 20, 30, 40]);
        }

        let oversized_profile = vec![0; MAX_ICC_PROFILE_BYTES + 1];
        let image = ColorNormalizer::from_icc_profile(&oversized_profile)
            .normalize(SourceImage::new(vec![10, 20, 30, 40], 1, 1).unwrap())
            .unwrap();
        assert_eq!(
            image.color_profile,
            ColorProfileStatus::EmbeddedProfileFallback
        );
        assert_eq!(image.rgba, vec![10, 20, 30, 40]);
    }

    #[test]
    fn jpeg_xl_initialization_uses_the_shared_embedded_icc_limit() {
        fn varint(mut value: u64) -> Vec<u8> {
            let mut encoded = Vec::new();
            loop {
                let mut byte = (value & 0x7f) as u8;
                value >>= 7;
                if value != 0 {
                    byte |= 0x80;
                }
                encoded.push(byte);
                if value == 0 {
                    return encoded;
                }
            }
        }

        assert_eq!(MAX_ICC_PROFILE_BYTES, 10 * 1024 * 1024);
        let mut hostile = varint(u64::try_from(MAX_ICC_PROFILE_BYTES).unwrap() + 1);
        hostile.push(0); // Empty command stream.
        let error = jxl_color::icc::decode_icc(&hostile).unwrap_err();
        assert!(error.to_string().contains("ICC output_size too large"));

        let expanding_commands = 4_096_usize;
        let mut amplified = varint(129);
        amplified.extend(varint(u64::try_from(expanding_commands + 1).unwrap()));
        amplified.push(0); // No tag table.
        amplified.extend(std::iter::repeat_n(16, expanding_commands));
        amplified.extend([0; 128]);
        let error = jxl_color::icc::decode_icc(&amplified).unwrap_err();
        assert!(error.to_string().contains("exceeds declared output_size"));
    }

    #[test]
    fn oversized_compressed_png_icc_is_rejected_before_decoder_materialization() {
        let encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        let mut encoder = encoder;
        encoder
            .write_all(&vec![0_u8; MAX_ICC_PROFILE_BYTES + 1])
            .unwrap();
        let compressed = encoder.finish().unwrap();
        let mut payload = b"viewr-test\0\0".to_vec();
        payload.extend_from_slice(&compressed);
        let mut png = PNG_SIGNATURE.to_vec();
        png.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
        png.extend_from_slice(b"iCCP");
        png.extend_from_slice(&payload);
        png.extend_from_slice(&[0; 4]);

        let Err(error) = DecodedImage::load_from_memory(&png) else {
            panic!("oversized ICC profile was accepted");
        };
        assert!(
            error
                .to_string()
                .contains("ICC profile exceeds safety limit")
        );
    }

    #[test]
    fn duplicate_png_icc_chunks_are_rejected_before_repeated_inflation() {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"small profile").unwrap();
        let mut payload = b"viewr-test\0\0".to_vec();
        payload.extend_from_slice(&encoder.finish().unwrap());
        let mut png = PNG_SIGNATURE.to_vec();
        for _ in 0..2 {
            png.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
            png.extend_from_slice(b"iCCP");
            png.extend_from_slice(&payload);
            png.extend_from_slice(&[0; 4]);
        }

        let Err(error) = DecodedImage::load_from_memory(&png) else {
            panic!("duplicate PNG ICC profiles were accepted");
        };
        assert!(error.to_string().contains("multiple embedded ICC profiles"));
    }

    #[test]
    fn webp_icc_headers_are_bounded_before_decoder_allocation() {
        let mut oversized = b"RIFF\0\0\0\0WEBP".to_vec();
        oversized.extend_from_slice(b"ICCP");
        oversized.extend_from_slice(&u32::MAX.to_le_bytes());
        let Err(error) = DecodedImage::load_from_memory(&oversized) else {
            panic!("oversized WebP ICC profile was accepted");
        };
        assert!(
            error
                .to_string()
                .contains("ICC profile exceeds safety limit")
        );

        let mut duplicate = b"RIFF\0\0\0\0WEBP".to_vec();
        for _ in 0..2 {
            duplicate.extend_from_slice(b"ICCP");
            duplicate.extend_from_slice(&0_u32.to_le_bytes());
        }
        let Err(error) = DecodedImage::load_from_memory(&duplicate) else {
            panic!("duplicate WebP ICC profiles were accepted");
        };
        assert!(error.to_string().contains("multiple embedded ICC profiles"));

        let mut oversized_exif = b"RIFF\0\0\0\0WEBP".to_vec();
        oversized_exif.extend_from_slice(b"EXIF");
        oversized_exif.extend_from_slice(
            &u32::try_from(crate::image_info::MAX_EXIF_BYTES + 1)
                .unwrap()
                .to_le_bytes(),
        );
        let Err(error) = DecodedImage::load_from_memory(&oversized_exif) else {
            panic!("oversized WebP EXIF payload was accepted");
        };
        assert!(
            error
                .to_string()
                .contains("EXIF payload exceeds safety limit")
        );
    }

    #[test]
    fn png_exif_headers_are_bounded_before_orientation_materialization() {
        let mut oversized = PNG_SIGNATURE.to_vec();
        oversized.extend_from_slice(
            &u32::try_from(crate::image_info::MAX_EXIF_BYTES + 1)
                .unwrap()
                .to_be_bytes(),
        );
        oversized.extend_from_slice(b"eXIf");
        let Err(error) = DecodedImage::load_from_memory(&oversized) else {
            panic!("oversized PNG EXIF payload was accepted");
        };
        assert!(
            error
                .to_string()
                .contains("EXIF payload exceeds safety limit")
        );

        let mut duplicate = PNG_SIGNATURE.to_vec();
        for _ in 0..2 {
            duplicate.extend_from_slice(&0_u32.to_be_bytes());
            duplicate.extend_from_slice(b"eXIf");
            duplicate.extend_from_slice(&[0; 4]);
        }
        let Err(error) = DecodedImage::load_from_memory(&duplicate) else {
            panic!("duplicate PNG EXIF payloads were accepted");
        };
        assert!(
            error
                .to_string()
                .contains("multiple embedded EXIF payloads")
        );
    }

    #[test]
    fn png_text_and_post_image_metadata_are_bounded_before_decoder_allocation() {
        for position in [b"IHDR".as_slice(), b"IDAT".as_slice()] {
            let mut png = PNG_SIGNATURE.to_vec();
            png.extend_from_slice(&0_u32.to_be_bytes());
            png.extend_from_slice(position);
            png.extend_from_slice(&[0; 4]);
            png.extend_from_slice(
                &u32::try_from(super::MAX_PNG_TEXT_BYTES + 1)
                    .unwrap()
                    .to_be_bytes(),
            );
            png.extend_from_slice(b"tEXt");
            let Err(error) = DecodedImage::load_from_memory(&png) else {
                panic!("oversized PNG text metadata was accepted");
            };
            assert!(
                error
                    .to_string()
                    .contains("text metadata exceeds safety limit")
            );
        }

        let mut png = PNG_SIGNATURE.to_vec();
        png.extend_from_slice(&0_u32.to_be_bytes());
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&[0; 4]);
        png.extend_from_slice(
            &u32::try_from(crate::image_info::MAX_EXIF_BYTES + 1)
                .unwrap()
                .to_be_bytes(),
        );
        png.extend_from_slice(b"eXIf");
        let Err(error) = DecodedImage::load_from_memory(&png) else {
            panic!("oversized post-IDAT PNG EXIF payload was accepted");
        };
        assert!(
            error
                .to_string()
                .contains("EXIF payload exceeds safety limit")
        );
    }

    #[test]
    fn jpeg_exif_orientation_is_applied_for_all_eight_values() {
        let ws = TempWorkspace::new("decode_orientation").unwrap();
        let base_path = ws.path().join("base.jpg");
        let source = image::RgbImage::from_fn(7, 5, |x, y| {
            image::Rgb([
                (x * 31 + y * 7) as u8,
                (y * 43 + x * 5) as u8,
                (x * 17 + y * 29) as u8,
            ])
        });
        source.save(&base_path).unwrap();

        // Decode once without orientation so the expected images use exactly
        // the same JPEG pixels. Writing EXIF does not alter the JPEG scan data.
        let base = DecodedImage::load(&base_path).unwrap();

        for exif_value in 1..=8u8 {
            let oriented_path = ws.path().join(format!("orientation-{exif_value}.jpg"));
            fs::copy(&base_path, &oriented_path).unwrap();
            let mut metadata = Metadata::new();
            metadata.set_tag(ExifTag::Orientation(vec![u16::from(exif_value)]));
            metadata.write_to_file(&oriented_path).unwrap();

            let mut expected = image::DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(base.width, base.height, base.rgba.clone()).unwrap(),
            );
            expected
                .apply_orientation(image::metadata::Orientation::from_exif(exif_value).unwrap());
            let expected = expected.into_rgba8();

            let decoded = DecodedImage::load(&oriented_path).unwrap();
            assert_eq!(
                (decoded.width, decoded.height),
                expected.dimensions(),
                "orientation {exif_value} dimensions"
            );
            assert_eq!(
                decoded.rgba,
                expected.into_raw(),
                "orientation {exif_value} pixels"
            );

            let bytes = fs::read(&oriented_path).unwrap();
            let memory_decoded = DecodedImage::load_from_memory(&bytes).unwrap();
            assert_eq!(
                (memory_decoded.width, memory_decoded.height),
                (decoded.width, decoded.height),
                "orientation {exif_value} memory dimensions"
            );
            assert_eq!(
                memory_decoded.rgba, decoded.rgba,
                "orientation {exif_value} memory pixels"
            );
        }
    }

    #[test]
    fn missing_file_is_decode_error() {
        let path = PathBuf::from("definitely_missing_viewr_image_xyz.png");
        assert!(DecodedImage::load(&path).is_err());
    }

    #[test]
    fn load_from_memory_png_and_svg() {
        use std::io::Cursor;
        let rgb = image::RgbImage::from_pixel(4, 3, image::Rgb([1, 2, 3]));
        let mut png = Cursor::new(Vec::new());
        rgb.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let img = DecodedImage::load_from_memory(png.get_ref()).expect("png mem");
        assert_eq!((img.width, img.height), (4, 3));

        let svg = br##"<svg width="12" height="8" xmlns="http://www.w3.org/2000/svg">
            <rect width="12" height="8" fill="#00ff00"/>
        </svg>"##;
        let s = DecodedImage::load_from_memory(svg).expect("svg mem");
        assert_eq!((s.width, s.height), (12, 8));
    }

    #[test]
    fn load_from_memory_routes_jxl_signatures_to_bounded_decoder() {
        for bytes in [
            super::JXL_CODESTREAM_SIGNATURE,
            super::JXL_CONTAINER_SIGNATURE,
        ] {
            let error = DecodedImage::load_from_memory(bytes).err().unwrap();
            assert!(error.to_string().contains("JXL decoder"));
        }
    }

    #[test]
    fn memory_extension_dispatch_reaches_every_declared_core_decoder() {
        for extension in crate::fs::CORE_EXTENSIONS {
            let error = DecodedImage::load_from_memory_with_extension(b"malformed", extension)
                .err()
                .unwrap();
            let crate::Error::Decode(message) = error else {
                panic!("memory decode returned a non-decode error")
            };
            assert_ne!(message, "unsupported core image format", "{extension}");
        }
        assert!(DecodedImage::load_from_memory_with_extension(b"data", "unknown").is_err());
    }

    #[test]
    fn memory_extension_dispatch_decodes_extension_only_format() {
        use std::io::Cursor;
        let rgb = image::RgbImage::from_pixel(3, 2, image::Rgb([4, 5, 6]));
        let mut tga = Cursor::new(Vec::new());
        rgb.write_to(&mut tga, image::ImageFormat::Tga).unwrap();

        let decoded = DecodedImage::load_from_memory_with_extension(tga.get_ref(), ".TGA")
            .expect("TGA decode selected by extension");
        assert_eq!((decoded.width, decoded.height), (3, 2));
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
    fn dimension_validation_rejects_zero_and_excessive_outputs() {
        assert_eq!(validate_dimensions(4, 3).unwrap(), 48);
        assert!(validate_dimensions(0, 3).is_err());
        assert!(validate_dimensions(3, 0).is_err());
        assert!(validate_dimensions(MAX_DECODE_DIMENSION + 1, 1).is_err());

        let height =
            u32::try_from(MAX_DECODE_PIXELS / u64::from(MAX_DECODE_DIMENSION) + 1).unwrap();
        assert!(validate_dimensions(MAX_DECODE_DIMENSION, height).is_err());
    }

    #[test]
    fn oversized_core_header_is_rejected_before_pixel_allocation() {
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&54_u32.to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54_u32.to_le_bytes());
        bmp.extend_from_slice(&40_u32.to_le_bytes());
        bmp.extend_from_slice(&50_000_i32.to_le_bytes());
        bmp.extend_from_slice(&50_000_i32.to_le_bytes());
        bmp.extend_from_slice(&1_u16.to_le_bytes());
        bmp.extend_from_slice(&24_u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);

        let Err(error) = DecodedImage::load_from_memory_with_extension(&bmp, "bmp") else {
            panic!("oversized BMP header was accepted");
        };
        assert!(
            error.to_string().contains("dimensions exceed safety limit"),
            "unexpected oversized-header error: {error}"
        );
    }

    #[test]
    fn oversized_svg_is_rejected_before_raster_allocation() {
        let svg = br#"<svg width="20000" height="20000" xmlns="http://www.w3.org/2000/svg"/>"#;
        let error = DecodedImage::load_from_memory(svg).err().unwrap();
        assert!(error.to_string().contains("safety limit"));
    }

    #[test]
    fn superseded_load_is_cancelled_before_file_access() {
        let generation = AtomicU64::new(2);
        let result = DecodedImage::load_if_current(
            std::path::Path::new("path-that-must-not-be-opened.png"),
            &generation,
            1,
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn generation_reader_interrupts_a_decode_after_supersession() {
        let generation = AtomicU64::new(7);
        let mut reader = GenerationReader::new(
            Cursor::new(b"still-decoding"),
            DecodeGeneration::tracked(&generation, 7),
        );
        let mut first = [0_u8; 1];
        reader.read_exact(&mut first).unwrap();
        assert_eq!(first, [b's']);

        generation.store(8, std::sync::atomic::Ordering::Release);
        let error = reader.read(&mut first).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    }

    #[test]
    fn sandboxed_extension_without_worker_is_error() {
        let ws = TempWorkspace::new("decode_avif").unwrap();
        let path = ws.path().join("x.avif");
        fs::write(&path, b"not really avif").unwrap();
        // Worker binary is absent in unit tests; path still routes through sandbox.
        assert!(DecodedImage::load(&path).is_err());
    }

    #[test]
    fn foreground_queue_replaces_work_that_has_not_started() {
        let queue = LatestJobQueue::default();
        let (completed, receiver) = mpsc::channel();
        let first = completed.clone();
        queue.replace(Box::new(move || first.send(1).unwrap()));
        queue.replace(Box::new(move || completed.send(2).unwrap()));

        queue.take()();
        assert_eq!(receiver.recv().unwrap(), 2);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn background_queue_rejects_work_when_capacity_is_exhausted() {
        let (sender, _receiver) = mpsc::sync_channel(2);
        assert!(try_schedule_background(&sender, Box::new(|| {})));
        assert!(try_schedule_background(&sender, Box::new(|| {})));
        assert!(!try_schedule_background(&sender, Box::new(|| {})));
    }

    #[test]
    fn decode_gate_caps_concurrency_and_prioritizes_foreground_waiters() {
        let gate = Arc::new(DecodeGate {
            state: Mutex::new(DecodeGateState::default()),
            available: Condvar::new(),
        });
        let first_background =
            acquire_decode_permit_from(Arc::clone(&gate), DecodePriority::Background);
        let second_background =
            acquire_decode_permit_from(Arc::clone(&gate), DecodePriority::Background);
        assert_eq!(
            gate.state.lock().unwrap().active,
            MAX_CONCURRENT_FILE_DECODES
        );

        let (acquired, acquisition_order) = mpsc::channel();
        let (release_foreground, foreground_release) = mpsc::channel();
        let foreground_gate = Arc::clone(&gate);
        let foreground = std::thread::spawn(move || {
            let _permit = acquire_decode_permit_from(foreground_gate, DecodePriority::Foreground);
            acquired.send("foreground").unwrap();
            foreground_release.recv().unwrap();
        });

        let deadline = Instant::now() + Duration::from_secs(1);
        while gate.state.lock().unwrap().waiting_foreground == 0 {
            assert!(
                Instant::now() < deadline,
                "foreground did not reach decode gate"
            );
            std::thread::yield_now();
        }

        let background_gate = Arc::clone(&gate);
        let (background_acquired, background_order) = mpsc::channel();
        let background = std::thread::spawn(move || {
            let _permit = acquire_decode_permit_from(background_gate, DecodePriority::Background);
            background_acquired.send("background").unwrap();
        });

        drop(first_background);
        assert_eq!(
            acquisition_order
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            "foreground"
        );
        assert!(background_order.try_recv().is_err());

        release_foreground.send(()).unwrap();
        assert_eq!(
            background_order
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            "background"
        );
        drop(second_background);
        foreground.join().unwrap();
        background.join().unwrap();
    }
}
