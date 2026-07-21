//! Turning an image file on disk into pixels the GPU can upload.
//!
//! Pure-Rust formats decode in-process (`image`, `jxl-oxide`, `resvg`). Formats
//! that need C-backed decoders (AVIF, HEIC, RAW) are delegated to
//! [`crate::sandbox`]. The shape of [`DecodedImage`] (owned RGBA8 plus
//! dimensions) is what the GPU wants either way.

use std::io::{BufRead, Read, Seek};
use std::path::Path;

use crate::error::Error;

pub(crate) use viewr_protocol::MAX_DECODE_DIMENSION;
const MAX_SVG_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CONCURRENT_FILE_DECODES: usize = 2;
const BACKGROUND_DECODE_QUEUE_CAPACITY: usize = 8;

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

        Ok(Self {
            foreground,
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

/// Queue bounded speculative work, returning false when the queue is saturated.
pub(crate) fn schedule_background_decode(job: impl FnOnce() + Send + 'static) -> bool {
    decode_executor()
        .is_ok_and(|executor| try_schedule_background(&executor.background, Box::new(job)))
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
        current_generation: &std::sync::atomic::AtomicU64,
        generation: u64,
    ) -> Result<Option<Self>, Error> {
        let _permit = acquire_decode_permit(DecodePriority::Foreground);
        if current_generation.load(std::sync::atomic::Ordering::Acquire) != generation {
            return Ok(None);
        }
        Self::load_file(path).map(Some)
    }

    fn load_file(path: &Path) -> Result<Self, Error> {
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

        // Never embed full filesystem paths in error strings (privacy / logs).
        let reader = image::ImageReader::open(path)
            .and_then(image::ImageReader::with_guessed_format)
            .map_err(|e| Error::Decode(format!("open/decode failed: {e}")))?;
        Self::decode_image_reader(reader)
    }

    /// Decode image bytes already in memory (no temp file, no path on disk).
    ///
    /// Used by doctor / default benchmark so product diagnostics leave **zero**
    /// debris under the system temp directory.
    ///
    /// # Errors
    /// Returns [`Error::Decode`] if the bytes are not a supported, well-formed image.
    pub fn load_from_memory(bytes: &[u8]) -> Result<Self, Error> {
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

        let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| Error::Decode(format!("decode failed: {e}")))?;
        Self::decode_image_reader(reader)
    }

    fn decode_image_reader<R>(mut reader: image::ImageReader<R>) -> Result<Self, Error>
    where
        R: BufRead + Seek,
    {
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_DECODE_DIMENSION);
        limits.max_image_height = Some(MAX_DECODE_DIMENSION);
        limits.max_alloc = Some(viewr_protocol::MAX_RGBA_BYTES);
        reader.limits(limits);

        let decoded = reader
            .decode()
            .map_err(|e| Error::Decode(format!("decode failed: {e}")))?;
        Self::from_dynamic_image(decoded)
    }

    fn from_dynamic_image(decoded: image::DynamicImage) -> Result<Self, Error> {
        let width = decoded.width();
        let height = decoded.height();
        let expected_size = validate_dimensions(width, height)?;
        let rgba = decoded.into_rgba8().into_raw();
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

    fn load_jxl(path: &Path) -> Result<Self, Error> {
        let file = std::fs::File::open(path).map_err(|e| Error::Decode(e.to_string()))?;
        let jxl = jxl_oxide::integration::JxlDecoder::new(file)
            .map_err(|e| Error::Decode(format!("failed to init JXL decoder: {e}")))?;
        let (width, height) = image::ImageDecoder::dimensions(&jxl);
        validate_dimensions(width, height)?;
        let decoded = image::DynamicImage::from_decoder(jxl)
            .map_err(|e| Error::Decode(format!("failed to decode JXL: {e}")))?;
        Self::from_dynamic_image(decoded)
    }

    /// Render an SVG to RGBA8 with pure-Rust `resvg` / `usvg`.
    fn load_svg(path: &Path) -> Result<Self, Error> {
        let file = std::fs::File::open(path).map_err(|e| Error::Decode(e.to_string()))?;
        let mut data = Vec::new();
        file.take(MAX_SVG_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(|e| Error::Decode(e.to_string()))?;
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
        Ok(Self {
            rgba,
            width,
            height,
        })
    }
}

fn validate_dimensions(width: u32, height: u32) -> Result<usize, Error> {
    viewr_protocol::checked_rgba_len(width, height)
        .map_err(|error| Error::Decode(error.to_string()))
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
        DecodeGate, DecodeGateState, DecodePriority, DecodedImage, LatestJobQueue,
        MAX_CONCURRENT_FILE_DECODES, MAX_DECODE_DIMENSION, acquire_decode_permit_from,
        positive_f32_to_px, try_schedule_background, validate_dimensions,
    };
    use crate::ephemeral::TempWorkspace;
    use std::fs;
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
