//! Narrow, bounded ownership layer around the libavif decode API.
//!
//! # Safety
//!
//! Every C allocation has one owning wrapper and one matching destructor. Input,
//! ICC, and pixel pointers are borrowed only while their owners are live. All
//! dimensions, strides, and byte lengths are checked against protocol limits
//! before a Rust slice is formed or output memory is copied.

#![allow(unsafe_code)] // all libavif pointer ownership is confined and reviewed here

use std::ffi::CStr;
use std::ptr::NonNull;

use libavif_sys as ffi;
use viewr_protocol::{CicpColor, WorkerColorProfile};

use super::{DecodedOutput, copy_strided_rgba};

const MAX_DECODER_THREADS: i32 = 4;

pub(super) fn decode(encoded: &[u8]) -> Result<DecodedOutput, String> {
    let image = AvifImage::new()?;
    let mut decoder = AvifDecoder::new()?;
    decoder.configure()?;
    check_result(
        // SAFETY: both wrappers own live libavif objects. `encoded` remains
        // borrowed for the complete synchronous call, and its byte length is
        // supplied exactly. libavif accepts a dangling slice pointer at size 0.
        unsafe {
            ffi::avifDecoderReadMemory(
                decoder.raw.as_ptr(),
                image.raw.as_ptr(),
                encoded.as_ptr(),
                encoded.len(),
            )
        },
        "AVIF decode",
    )?;

    let decoded_image = image.get();
    viewr_protocol::checked_rgba_len(decoded_image.width, decoded_image.height)
        .map_err(|error| error.to_string())?;
    let color_profile = color_profile(decoded_image)?;
    let rgba = RgbPixels::decode(decoded_image)?;

    Ok(DecodedOutput {
        width: decoded_image.width,
        height: decoded_image.height,
        rgba,
        color_profile,
    })
}

fn color_profile(image: &ffi::avifImage) -> Result<WorkerColorProfile, String> {
    if image.icc.size != 0 {
        if image.icc.size > viewr_protocol::MAX_COLOR_PROFILE_BYTES {
            return Err("AVIF ICC profile exceeds worker safety limit".into());
        }
        let Some(data) = NonNull::new(image.icc.data) else {
            return Err("AVIF decoder returned an invalid ICC profile".into());
        };
        let mut profile = Vec::new();
        profile
            .try_reserve_exact(image.icc.size)
            .map_err(|_| "not enough memory for AVIF ICC profile".to_string())?;
        // SAFETY: libavif owns `icc.size` initialized bytes at the non-null
        // pointer for the lifetime of `image`; this function only copies them.
        let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr(), image.icc.size) };
        profile.extend_from_slice(bytes);
        return Ok(WorkerColorProfile::Icc(profile));
    }

    if matches!(image.colorPrimaries, 0 | 2) || matches!(image.transferCharacteristics, 0 | 2) {
        return Ok(WorkerColorProfile::Unknown);
    }
    Ok(WorkerColorProfile::Cicp(CicpColor {
        color_primaries: image.colorPrimaries,
        transfer_characteristics: image.transferCharacteristics,
        matrix_coefficients: image.matrixCoefficients,
        full_range: image.yuvRange == ffi::AVIF_RANGE_FULL,
    }))
}

struct AvifImage {
    raw: NonNull<ffi::avifImage>,
}

impl AvifImage {
    fn new() -> Result<Self, String> {
        // SAFETY: the returned allocation is either null or uniquely owned and
        // released by this wrapper's Drop implementation.
        let raw = NonNull::new(unsafe { ffi::avifImageCreateEmpty() })
            .ok_or_else(|| "failed to allocate AVIF image".to_string())?;
        Ok(Self { raw })
    }

    fn get(&self) -> &ffi::avifImage {
        // SAFETY: `raw` stays live and immovable until Drop, and this shared
        // reference cannot outlive the wrapper.
        unsafe { self.raw.as_ref() }
    }
}

impl Drop for AvifImage {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the live libavif image.
        unsafe { ffi::avifImageDestroy(self.raw.as_ptr()) };
    }
}

struct AvifDecoder {
    raw: NonNull<ffi::avifDecoder>,
}

impl AvifDecoder {
    fn new() -> Result<Self, String> {
        // SAFETY: the returned allocation is either null or uniquely owned and
        // released by this wrapper's Drop implementation.
        let raw = NonNull::new(unsafe { ffi::avifDecoderCreate() })
            .ok_or_else(|| "failed to allocate AVIF decoder".to_string())?;
        Ok(Self { raw })
    }

    fn configure(&mut self) -> Result<(), String> {
        // SAFETY: this wrapper has unique access to a live decoder, and every
        // assigned value is within the range documented by libavif 1.0.
        let decoder = unsafe { self.raw.as_mut() };
        decoder.maxThreads = MAX_DECODER_THREADS;
        decoder.requestedSource = ffi::AVIF_DECODER_SOURCE_PRIMARY_ITEM;
        decoder.allowProgressive = 0;
        decoder.allowIncremental = 0;
        decoder.ignoreExif = 1;
        decoder.ignoreXMP = 1;
        decoder.imageSizeLimit = u32::try_from(viewr_protocol::MAX_DECODE_PIXELS)
            .map_err(|_| "protocol pixel limit does not fit libavif".to_string())?;
        decoder.imageDimensionLimit = viewr_protocol::MAX_DECODE_DIMENSION;
        decoder.imageCountLimit = 1;
        decoder.strictFlags = ffi::AVIF_STRICT_ENABLED;
        Ok(())
    }
}

impl Drop for AvifDecoder {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the live libavif decoder.
        unsafe { ffi::avifDecoderDestroy(self.raw.as_ptr()) };
    }
}

struct RgbPixels {
    raw: ffi::avifRGBImage,
}

impl RgbPixels {
    fn decode(image: &ffi::avifImage) -> Result<Vec<u8>, String> {
        let mut pixels = Self {
            raw: ffi::avifRGBImage::default(),
        };
        // SAFETY: both pointers name initialized live objects. libavif writes
        // only the RGB descriptor fields and does not retain either pointer.
        unsafe { ffi::avifRGBImageSetDefaults(&raw mut pixels.raw, image) };
        pixels.raw.depth = 8;
        pixels.raw.format = ffi::AVIF_RGB_FORMAT_RGBA;
        pixels.raw.maxThreads = MAX_DECODER_THREADS;
        check_result(
            // SAFETY: `pixels.raw` is initialized from `image`, uniquely
            // borrowed, and any successful allocation is released by Drop.
            unsafe { ffi::avifRGBImageAllocatePixels(&raw mut pixels.raw) },
            "AVIF RGB allocation",
        )?;
        check_result(
            // SAFETY: the image remains live, the destination was allocated by
            // libavif, and both dimensions were validated before allocation.
            unsafe { ffi::avifImageYUVToRGB(image, &raw mut pixels.raw) },
            "AVIF RGB conversion",
        )?;

        let stride = usize::try_from(pixels.raw.rowBytes)
            .map_err(|_| "AVIF decoder returned an invalid RGBA stride".to_string())?;
        let rows = usize::try_from(image.height)
            .map_err(|_| "AVIF decoder returned an invalid RGBA height".to_string())?;
        let length = stride
            .checked_mul(rows)
            .ok_or_else(|| "AVIF decoder returned an invalid RGBA buffer".to_string())?;
        let Some(data) = NonNull::new(pixels.raw.pixels) else {
            return Err("AVIF decoder returned a missing RGBA buffer".into());
        };
        // SAFETY: a successful libavif allocation contains `rowBytes` bytes for
        // every decoded row and remains owned by `pixels` through this copy.
        let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr(), length) };
        copy_strided_rgba(bytes, image.width, image.height, stride)
    }
}

impl Drop for RgbPixels {
    fn drop(&mut self) {
        if !self.raw.pixels.is_null() {
            // SAFETY: libavif allocated this descriptor's pixels exactly once;
            // this wrapper owns them and frees them exactly once.
            unsafe { ffi::avifRGBImageFreePixels(&raw mut self.raw) };
        }
    }
}

fn check_result(result: ffi::avifResult, operation: &str) -> Result<(), String> {
    if result == ffi::AVIF_RESULT_OK {
        return Ok(());
    }
    // SAFETY: libavif returns a process-lifetime NUL-terminated string for each
    // result code. A defensive null check avoids dereferencing a bad pointer.
    let detail = unsafe {
        let message = ffi::avifResultToString(result);
        if message.is_null() {
            format!("error code {result}")
        } else {
            CStr::from_ptr(message).to_string_lossy().into_owned()
        }
    };
    Err(format!("{operation} failed: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::{color_profile, decode};
    use libavif_sys as ffi;
    use viewr_protocol::{CicpColor, WorkerColorProfile};

    #[test]
    fn tagged_avif_decodes_pixels_and_cicp_together() {
        let source = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([(x * 70) as u8, (y * 90) as u8, 40, 255])
        }));
        let mut encoded = libavif_image::save(&source)
            .expect("encode AVIF fixture")
            .as_slice()
            .to_vec();
        let nclx = encoded
            .windows(4)
            .position(|window| window == b"nclx")
            .expect("AVIF fixture contains CICP metadata");
        encoded[nclx + 4..nclx + 11].copy_from_slice(&[0, 1, 0, 13, 0, 1, 0x80]);
        let reference = libavif_image::read(&encoded)
            .expect("decode AVIF reference pixels")
            .into_rgba8()
            .into_raw();

        let decoded = decode(&encoded).expect("decode tagged AVIF");
        assert_eq!((decoded.width, decoded.height), (3, 2));
        assert_eq!(decoded.rgba, reference);
        assert_eq!(
            decoded.color_profile,
            WorkerColorProfile::Cicp(CicpColor {
                color_primaries: 1,
                transfer_characteristics: 13,
                matrix_coefficients: 1,
                full_range: true,
            })
        );
    }

    #[test]
    fn avif_color_metadata_prefers_bounded_icc_then_cicp() {
        let profile_bytes = [1_u8, 2, 3, 4];
        let mut image = ffi::avifImage {
            icc: ffi::avifRWData {
                data: profile_bytes.as_ptr().cast_mut(),
                size: profile_bytes.len(),
            },
            colorPrimaries: 9,
            transferCharacteristics: 16,
            matrixCoefficients: 9,
            yuvRange: ffi::AVIF_RANGE_LIMITED,
            ..ffi::avifImage::default()
        };
        assert_eq!(
            color_profile(&image).unwrap(),
            WorkerColorProfile::Icc(profile_bytes.to_vec())
        );

        image.icc = ffi::avifRWData::default();
        assert_eq!(
            color_profile(&image).unwrap(),
            WorkerColorProfile::Cicp(CicpColor {
                color_primaries: 9,
                transfer_characteristics: 16,
                matrix_coefficients: 9,
                full_range: false,
            })
        );
    }

    #[test]
    fn avif_color_metadata_rejects_oversized_or_ambiguous_profiles() {
        let mut marker = 0_u8;
        let oversized = ffi::avifImage {
            icc: ffi::avifRWData {
                data: &raw mut marker,
                size: viewr_protocol::MAX_COLOR_PROFILE_BYTES + 1,
            },
            ..ffi::avifImage::default()
        };
        assert!(
            color_profile(&oversized)
                .unwrap_err()
                .contains("exceeds worker safety limit")
        );

        for (primaries, transfer) in [(0, 13), (2, 13), (1, 0), (1, 2)] {
            let image = ffi::avifImage {
                colorPrimaries: primaries,
                transferCharacteristics: transfer,
                ..ffi::avifImage::default()
            };
            assert_eq!(color_profile(&image).unwrap(), WorkerColorProfile::Unknown);
        }
    }
}
