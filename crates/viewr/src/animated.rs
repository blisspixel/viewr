//! Bounded animated-image decoding and deterministic playback timing.
//!
//! Frames stay in memory only for the current image. Decoding runs through the
//! same foreground-priority gate as still images and never creates a disk cache.

use std::fs::File;
use std::io::{BufReader, Seek};
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use image::{AnimationDecoder, ImageDecoder};

use crate::decode::{ColorNormalizer, DecodedImage, SourceImage};
use crate::error::Error;

const MAX_ANIMATION_BYTES: usize = 256 * 1024 * 1024;
const MAX_ANIMATION_FRAMES: usize = 1_000;
const MIN_FRAME_DELAY: Duration = Duration::from_millis(10);
const MAX_FRAME_DELAY: Duration = Duration::from_hours(1);

/// One fully composited animation frame.
pub(crate) struct AnimationFrame {
    image: Arc<DecodedImage>,
    delay: Duration,
}

/// Loop behavior carried by the image container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnimationRepeat {
    Infinite,
    Finite(NonZeroU32),
}

/// A bounded, fully decoded animation ready for UI-thread playback.
pub(crate) struct DecodedAnimation {
    frames: Vec<AnimationFrame>,
    repeat: AnimationRepeat,
}

impl DecodedAnimation {
    /// Decode animation frames for GIF, animated WebP, or APNG. A still image
    /// or a supported container with only one frame returns `Ok(None)`.
    pub(crate) fn load_background_if_current(
        path: &Path,
        current_generation: &AtomicU64,
        generation: u64,
    ) -> Result<Option<Self>, Error> {
        crate::decode::with_background_decode_permit(|| {
            Self::load_with_cancellation(path, &|| {
                current_generation.load(Ordering::Acquire) == generation
            })
        })
    }

    fn load_with_cancellation(
        path: &Path,
        is_current: &impl Fn() -> bool,
    ) -> Result<Option<Self>, Error> {
        if !is_current() {
            return Ok(None);
        }
        let format = image::ImageReader::open(path)
            .and_then(image::ImageReader::with_guessed_format)
            .map_err(|error| Error::Decode(format!("animation format detection failed: {error}")))?
            .format();
        if !is_current() {
            return Ok(None);
        }
        match format {
            Some(image::ImageFormat::Gif) => Self::decode_gif(path, is_current),
            Some(image::ImageFormat::WebP) => Self::decode_webp(path, is_current),
            Some(image::ImageFormat::Png) => Self::decode_apng(path, is_current),
            _ => Ok(None),
        }
    }

    fn decode_gif(path: &Path, is_current: &impl Fn() -> bool) -> Result<Option<Self>, Error> {
        let reader =
            BufReader::new(File::open(path).map_err(|error| Error::Decode(error.to_string()))?);
        let mut decoder = image::codecs::gif::GifDecoder::new(reader)
            .map_err(|error| animation_decode_error(&error))?;
        decoder
            .set_limits(animation_limits())
            .map_err(|error| animation_decode_error(&error))?;
        validate_animation_canvas(decoder.dimensions())?;
        let color_normalizer = ColorNormalizer::from_decoder(&mut decoder);
        let orientation = decoder
            .orientation()
            .map_err(|error| animation_decode_error(&error))?;
        collect_frames(decoder, orientation, &color_normalizer, is_current)
    }

    fn decode_webp(path: &Path, is_current: &impl Fn() -> bool) -> Result<Option<Self>, Error> {
        let mut reader =
            BufReader::new(File::open(path).map_err(|error| Error::Decode(error.to_string()))?);
        crate::decode::enforce_embedded_metadata_limits(&mut reader)?;
        reader
            .rewind()
            .map_err(|error| Error::Decode(error.to_string()))?;
        let mut decoder = image::codecs::webp::WebPDecoder::new(reader)
            .map_err(|error| animation_decode_error(&error))?;
        decoder
            .set_limits(animation_limits())
            .map_err(|error| animation_decode_error(&error))?;
        validate_animation_canvas(decoder.dimensions())?;
        let color_normalizer = ColorNormalizer::from_decoder(&mut decoder);
        let orientation = decoder
            .orientation()
            .map_err(|error| animation_decode_error(&error))?;
        collect_frames(decoder, orientation, &color_normalizer, is_current)
    }

    fn decode_apng(path: &Path, is_current: &impl Fn() -> bool) -> Result<Option<Self>, Error> {
        let mut reader =
            BufReader::new(File::open(path).map_err(|error| Error::Decode(error.to_string()))?);
        crate::decode::enforce_embedded_metadata_limits(&mut reader)?;
        reader
            .rewind()
            .map_err(|error| Error::Decode(error.to_string()))?;
        let mut decoder = image::codecs::png::PngDecoder::with_limits(reader, animation_limits())
            .map_err(|error| animation_decode_error(&error))?;
        validate_animation_canvas(decoder.dimensions())?;
        if !decoder
            .is_apng()
            .map_err(|error| animation_decode_error(&error))?
        {
            return Ok(None);
        }
        let color_normalizer = ColorNormalizer::from_decoder(&mut decoder);
        let orientation = decoder
            .orientation()
            .map_err(|error| animation_decode_error(&error))?;
        let decoder = decoder
            .apng()
            .map_err(|error| animation_decode_error(&error))?;
        collect_frames(decoder, orientation, &color_normalizer, is_current)
    }
}

fn animation_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(viewr_protocol::MAX_DECODE_DIMENSION);
    limits.max_image_height = Some(viewr_protocol::MAX_DECODE_DIMENSION);
    limits.max_alloc = Some(u64::try_from(MAX_ANIMATION_BYTES).unwrap_or(u64::MAX));
    limits
}

fn validate_animation_canvas((width, height): (u32, u32)) -> Result<(), Error> {
    let bytes = viewr_protocol::checked_rgba_len(width, height)
        .map_err(|error| Error::Decode(error.to_string()))?;
    if bytes > MAX_ANIMATION_BYTES {
        return Err(Error::Decode(format!(
            "animation canvas exceeds the {} MiB playback limit",
            MAX_ANIMATION_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

fn animation_decode_error(error: &image::ImageError) -> Error {
    Error::Decode(format!("animation decode failed: {error}"))
}

fn collect_frames<'a>(
    decoder: impl AnimationDecoder<'a>,
    orientation: image::metadata::Orientation,
    color_normalizer: &ColorNormalizer,
    is_current: &impl Fn() -> bool,
) -> Result<Option<DecodedAnimation>, Error> {
    let repeat = match decoder.loop_count() {
        image::metadata::LoopCount::Infinite => AnimationRepeat::Infinite,
        image::metadata::LoopCount::Finite(count) => AnimationRepeat::Finite(count),
    };
    let mut frames = Vec::new();
    let mut decoded_bytes = 0usize;
    let mut expected_dimensions = None;

    let mut decoded_frames = decoder.into_frames();
    loop {
        if !is_current() {
            return Ok(None);
        }
        let Some(frame_result) = decoded_frames.next() else {
            break;
        };
        if frames.len() == MAX_ANIMATION_FRAMES {
            return Err(Error::Decode(format!(
                "animation exceeds the {MAX_ANIMATION_FRAMES}-frame safety limit"
            )));
        }
        let frame = frame_result.map_err(|error| animation_decode_error(&error))?;
        let delay = normalized_frame_delay(frame.delay());
        let mut image = image::DynamicImage::ImageRgba8(frame.into_buffer());
        image.apply_orientation(orientation);
        let buffer = image.into_rgba8();
        let (width, height) = buffer.dimensions();
        let expected_bytes = viewr_protocol::checked_rgba_len(width, height)
            .map_err(|error| Error::Decode(error.to_string()))?;
        if expected_dimensions.is_some_and(|dimensions| dimensions != (width, height)) {
            return Err(Error::Decode(
                "animation frames returned inconsistent dimensions".into(),
            ));
        }
        expected_dimensions = Some((width, height));
        decoded_bytes = decoded_bytes
            .checked_add(expected_bytes)
            .ok_or_else(|| Error::Decode("animation byte count overflowed".into()))?;
        if decoded_bytes > MAX_ANIMATION_BYTES {
            return Err(Error::Decode(format!(
                "animation exceeds the {} MiB playback limit",
                MAX_ANIMATION_BYTES / (1024 * 1024)
            )));
        }
        let rgba = buffer.into_raw();
        if rgba.len() != expected_bytes {
            return Err(Error::Decode(
                "animation frame returned an invalid RGBA buffer".into(),
            ));
        }
        let Some(image) = color_normalizer
            .normalize_while_current(SourceImage::new(rgba, width, height)?, is_current)?
        else {
            return Ok(None);
        };
        frames.push(AnimationFrame {
            image: Arc::new(image),
            delay,
        });
    }

    if frames.len() <= 1 {
        Ok(None)
    } else {
        Ok(Some(DecodedAnimation { frames, repeat }))
    }
}

fn normalized_frame_delay(delay: image::Delay) -> Duration {
    let (numerator, denominator) = delay.numer_denom_ms();
    let denominator = u128::from(denominator.max(1));
    let nanos = u128::from(numerator)
        .saturating_mul(1_000_000)
        .checked_div(denominator)
        .unwrap_or(0);
    let nanos = u64::try_from(nanos).unwrap_or(u64::MAX);
    Duration::from_nanos(nanos).clamp(MIN_FRAME_DELAY, MAX_FRAME_DELAY)
}

/// Mutable playback cursor with an explicit wake deadline.
pub(crate) struct AnimationPlayback {
    animation: DecodedAnimation,
    frame_index: usize,
    completed_cycles: u32,
    playing: bool,
    next_frame_at: Option<Instant>,
}

impl AnimationPlayback {
    pub(crate) fn new(animation: DecodedAnimation, now: Instant) -> Self {
        debug_assert!(animation.frames.len() > 1);
        let next_frame_at = now.checked_add(animation.frames[0].delay);
        Self {
            animation,
            frame_index: 0,
            completed_cycles: 0,
            playing: true,
            next_frame_at,
        }
    }

    pub(crate) fn current_image(&self) -> Arc<DecodedImage> {
        Arc::clone(&self.animation.frames[self.frame_index].image)
    }

    pub(crate) const fn frame_index(&self) -> usize {
        self.frame_index
    }

    pub(crate) fn frame_count(&self) -> usize {
        self.animation.frames.len()
    }

    pub(crate) const fn is_playing(&self) -> bool {
        self.playing
    }

    pub(crate) const fn next_deadline(&self) -> Option<Instant> {
        self.next_frame_at
    }

    pub(crate) fn toggle(&mut self, now: Instant) {
        if self.playing {
            self.playing = false;
            self.next_frame_at = None;
            return;
        }
        if self.finished() {
            self.frame_index = 0;
            self.completed_cycles = 0;
        }
        self.playing = true;
        self.next_frame_at = now.checked_add(self.current_delay());
    }

    pub(crate) fn pause(&mut self) {
        self.playing = false;
        self.next_frame_at = None;
    }

    /// Advance every due frame, returning whether the displayed frame changed.
    pub(crate) fn advance(&mut self, now: Instant) -> bool {
        if !self.playing {
            return false;
        }
        let mut changed = false;
        let max_steps = self.animation.frames.len().saturating_mul(2).max(1);
        for _ in 0..max_steps {
            let Some(deadline) = self.next_frame_at else {
                break;
            };
            if deadline > now {
                break;
            }
            changed |= self.advance_one();
            if self.playing {
                self.next_frame_at = deadline.checked_add(self.current_delay());
            } else {
                self.next_frame_at = None;
                break;
            }
        }
        if self.next_frame_at.is_some_and(|deadline| deadline <= now) {
            self.next_frame_at = now.checked_add(self.current_delay());
        }
        changed
    }

    fn advance_one(&mut self) -> bool {
        if self.frame_index + 1 < self.animation.frames.len() {
            self.frame_index += 1;
            return true;
        }
        self.completed_cycles = self.completed_cycles.saturating_add(1);
        if self.finished() {
            self.playing = false;
            return false;
        }
        self.frame_index = 0;
        true
    }

    fn current_delay(&self) -> Duration {
        self.animation.frames[self.frame_index].delay
    }

    fn finished(&self) -> bool {
        match self.animation.repeat {
            AnimationRepeat::Infinite => false,
            AnimationRepeat::Finite(total) => self.completed_cycles >= total.get(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::ColorProfileStatus;
    use crate::ephemeral::TempWorkspace;

    fn frame(id: u8, delay_ms: u64) -> AnimationFrame {
        AnimationFrame {
            image: Arc::new(DecodedImage {
                rgba: vec![id, 0, 0, 255],
                width: 1,
                height: 1,
                color_profile: ColorProfileStatus::AssumedSrgb,
                working_color: crate::color::WorkingColorEncoding::SRGB_RGBA8,
            }),
            delay: Duration::from_millis(delay_ms),
        }
    }

    fn playback(repeat: AnimationRepeat, now: Instant) -> AnimationPlayback {
        AnimationPlayback::new(
            DecodedAnimation {
                frames: vec![frame(1, 20), frame(2, 30)],
                repeat,
            },
            now,
        )
    }

    #[test]
    fn gif_loader_decodes_timing_and_composited_frames() {
        let workspace = TempWorkspace::new("animated_gif").unwrap();
        let path = workspace.path().join("two-frames.gif");
        let file = File::create(&path).unwrap();
        let mut encoder = image::codecs::gif::GifEncoder::new(file);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Infinite)
            .unwrap();
        let frames = [
            image::Frame::from_parts(
                image::RgbaImage::from_pixel(2, 1, image::Rgba([255, 0, 0, 255])),
                0,
                0,
                image::Delay::from_numer_denom_ms(20, 1),
            ),
            image::Frame::from_parts(
                image::RgbaImage::from_pixel(2, 1, image::Rgba([0, 0, 255, 255])),
                0,
                0,
                image::Delay::from_numer_denom_ms(30, 1),
            ),
        ];
        encoder.encode_frames(frames).unwrap();
        drop(encoder);

        let animation = DecodedAnimation::load_with_cancellation(&path, &|| true)
            .unwrap()
            .unwrap();
        assert_eq!(animation.frames.len(), 2);
        assert_eq!(animation.frames[0].delay, Duration::from_millis(20));
        assert_eq!(animation.frames[1].delay, Duration::from_millis(30));
        assert_eq!(animation.frames[0].image.rgba[0], 255);
        assert_eq!(animation.frames[1].image.rgba[2], 255);
        assert_eq!(animation.repeat, AnimationRepeat::Infinite);
    }

    #[test]
    fn still_png_is_not_reported_as_animation() {
        let workspace = TempWorkspace::new("animated_still").unwrap();
        let path = workspace.path().join("still.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        assert!(
            DecodedAnimation::load_with_cancellation(&path, &|| true)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn animation_is_detected_by_content_after_a_misleading_rename() {
        let workspace = TempWorkspace::new("animated_content_detection").unwrap();
        let gif_path = workspace.path().join("two-frames.gif");
        let renamed_path = workspace.path().join("two-frames.png");
        let file = File::create(&gif_path).unwrap();
        let mut encoder = image::codecs::gif::GifEncoder::new(file);
        encoder
            .encode_frames([
                image::Frame::new(image::RgbaImage::from_pixel(
                    1,
                    1,
                    image::Rgba([1, 2, 3, 255]),
                )),
                image::Frame::new(image::RgbaImage::from_pixel(
                    1,
                    1,
                    image::Rgba([4, 5, 6, 255]),
                )),
            ])
            .unwrap();
        drop(encoder);
        std::fs::rename(gif_path, &renamed_path).unwrap();

        let animation = DecodedAnimation::load_with_cancellation(&renamed_path, &|| true)
            .unwrap()
            .unwrap();
        assert_eq!(animation.frames.len(), 2);
    }

    #[test]
    fn superseded_animation_stops_between_frames() {
        let workspace = TempWorkspace::new("animated_frame_cancellation").unwrap();
        let path = workspace.path().join("three-frames.gif");
        let file = File::create(&path).unwrap();
        let mut encoder = image::codecs::gif::GifEncoder::new(file);
        encoder
            .encode_frames((0..3).map(|value| {
                image::Frame::new(image::RgbaImage::from_pixel(
                    2,
                    2,
                    image::Rgba([value, 0, 0, 255]),
                ))
            }))
            .unwrap();
        drop(encoder);

        let checks = std::cell::Cell::new(0_u8);
        let animation = DecodedAnimation::load_with_cancellation(&path, &|| {
            let next = checks.get() + 1;
            checks.set(next);
            next < 4
        })
        .unwrap();
        assert!(animation.is_none());
        assert_eq!(checks.get(), 4);
    }

    #[test]
    fn frame_delay_is_safe_and_fractional() {
        assert_eq!(
            normalized_frame_delay(image::Delay::from_numer_denom_ms(0, 1)),
            MIN_FRAME_DELAY
        );
        assert_eq!(
            normalized_frame_delay(image::Delay::from_numer_denom_ms(25, 2)),
            Duration::from_micros(12_500)
        );
        assert_eq!(
            normalized_frame_delay(image::Delay::from_numer_denom_ms(u32::MAX, 1)),
            MAX_FRAME_DELAY
        );
    }

    #[test]
    fn animation_canvas_is_bounded_before_frame_allocation() {
        assert!(validate_animation_canvas((8_192, 8_192)).is_ok());
        let error = validate_animation_canvas((9_000, 8_000)).unwrap_err();
        assert!(error.to_string().contains("playback limit"));
    }

    #[test]
    fn playback_advances_pauses_and_restarts() {
        let now = Instant::now();
        let mut playback = playback(AnimationRepeat::Finite(NonZeroU32::new(1).unwrap()), now);
        assert_eq!(playback.current_image().rgba[0], 1);
        assert!(!playback.advance(now + Duration::from_millis(19)));
        assert!(playback.advance(now + Duration::from_millis(20)));
        assert_eq!(playback.frame_index(), 1);
        assert!(!playback.advance(now + Duration::from_millis(50)));
        assert!(!playback.is_playing());
        playback.toggle(now + Duration::from_millis(60));
        assert!(playback.is_playing());
        assert_eq!(playback.frame_index(), 0);
        playback.toggle(now + Duration::from_millis(61));
        assert!(!playback.is_playing());
        assert_eq!(playback.next_deadline(), None);
    }

    #[test]
    fn infinite_playback_resynchronizes_after_a_long_stall() {
        let now = Instant::now();
        let mut playback = playback(AnimationRepeat::Infinite, now);
        assert!(playback.advance(now + Duration::from_secs(10)));
        assert!(playback.is_playing());
        assert!(
            playback
                .next_deadline()
                .is_some_and(|deadline| deadline > now)
        );
    }
}
