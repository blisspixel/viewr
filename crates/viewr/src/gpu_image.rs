//! Pure image-sizing, preview, and upload-selection policy for the GPU adapter.
//!
//! This module owns no device, queue, surface, texture, or event-loop state. The
//! native adapter in `gpu` consumes these validated decisions and remains the sole
//! owner of GPU resources.

use std::borrow::Cow;

use crate::color::{OutputColorTransform, WorkingColorEncoding};
use crate::decode::{ColorProfileStatus, DecodedImage};
use crate::error::Error;

/// Maximum number of RGBA pixels retained in the base GPU image texture.
///
/// A complete mip chain adds at most one third again, so this caps the image
/// allocation at roughly 341 MiB while preserving full-resolution CPU pixels for
/// export. Typical 60-megapixel camera images still fit without a proxy.
pub(crate) const MAX_GPU_BASE_PIXELS: u64 = 64 * 1024 * 1024;

/// The explicit GUI probe uses a lower limit so ordinary CI hardware exercises
/// the asynchronous preview path without allocating a hostile-size fixture.
pub(crate) const PERFORMANCE_PROBE_GPU_BASE_PIXELS: u64 = 1024 * 1024;

/// Dimensions selected for one bounded GPU preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PreviewSpec {
    width: u32,
    height: u32,
    source_size: (u32, u32),
}

/// A complete, validated preview prepared away from the event thread.
#[derive(Clone)]
pub(crate) struct ImagePreview {
    rgba: Vec<u8>,
    spec: PreviewSpec,
    working_color: WorkingColorEncoding,
}

impl ImagePreview {
    /// Retain a prepared display image as decoded pixels for a transient collage.
    pub(crate) fn into_decoded_image(self, color_profile: ColorProfileStatus) -> DecodedImage {
        DecodedImage {
            rgba: self.rgba,
            width: self.spec.width,
            height: self.spec.height,
            color_profile,
            working_color: self.working_color,
            decode_proof: None,
        }
    }
}

pub(crate) struct ImageUpload<'a> {
    pub(crate) rgba: &'a [u8],
    pub(crate) size: (u32, u32),
    pub(crate) full_resolution: bool,
}

/// Largest aspect-preserving dimensions that fit texture and pixel limits.
#[must_use]
fn texture_dimensions(source: (u32, u32), max_dim: u32, max_pixels: u64) -> (u32, u32) {
    let (width, height) = source;
    let source_pixels = u64::from(width).saturating_mul(u64::from(height));
    if width <= max_dim && height <= max_dim && source_pixels <= max_pixels {
        return source;
    }
    let longest = u64::from(width.max(height));
    let dimensions_at = |long_edge: u64| {
        let scaled = |edge: u32| {
            u32::try_from(u64::from(edge).saturating_mul(long_edge) / longest)
                .unwrap_or(max_dim)
                .max(1)
        };
        (scaled(width), scaled(height))
    };

    let mut low = 1_u64;
    let mut high = longest.min(u64::from(max_dim.max(1)));
    let mut best = dimensions_at(1);
    while low <= high {
        let middle = low + (high - low) / 2;
        let candidate = dimensions_at(middle);
        let pixels = u64::from(candidate.0).saturating_mul(u64::from(candidate.1));
        if pixels <= max_pixels {
            best = candidate;
            low = middle.saturating_add(1);
        } else {
            high = middle.saturating_sub(1);
        }
    }
    best
}

/// Number of levels needed to reduce the longest edge to one pixel.
#[must_use]
pub(crate) fn mip_level_count(size: (u32, u32)) -> u32 {
    size.0.max(size.1).max(1).ilog2() + 1
}

pub(crate) fn preview_spec(
    source: (u32, u32),
    max_dim: u32,
    max_pixels: u64,
) -> Option<PreviewSpec> {
    let target = texture_dimensions(source, max_dim, max_pixels);
    (target != source).then_some(PreviewSpec {
        width: target.0,
        height: target.1,
        source_size: source,
    })
}

/// Build a linear-light, alpha-correct area preview with bounded allocation.
///
/// The returned bytes stay straight sRGB RGBA. Fully transparent RGB does not
/// contribute. Returning `Ok(None)` means the generation was canceled between
/// output rows.
pub(crate) fn prepare_image_preview(
    image: &DecodedImage,
    spec: PreviewSpec,
    is_cancelled: impl Fn() -> bool,
) -> Result<Option<ImagePreview>, Error> {
    if image.working_color != WorkingColorEncoding::SRGB_RGBA8 {
        return Err(Error::Gpu(
            "image preview does not support this working color encoding".into(),
        ));
    }
    if spec.source_size != (image.width, image.height) || spec.width == 0 || spec.height == 0 {
        return Err(Error::Gpu(
            "image preview dimensions are inconsistent".into(),
        ));
    }
    let source_len = viewr_protocol::checked_rgba_len(image.width, image.height)
        .map_err(|error| Error::Gpu(error.to_string()))?;
    if image.rgba.len() != source_len {
        return Err(Error::Gpu(
            "image preview source does not match its dimensions".into(),
        ));
    }
    let output_len = viewr_protocol::checked_rgba_len(spec.width, spec.height)
        .map_err(|error| Error::Gpu(error.to_string()))?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(output_len)
        .map_err(|error| Error::Gpu(format!("could not allocate image preview: {error}")))?;
    rgba.resize(output_len, 0);

    let source_width = f64::from(image.width);
    let source_height = f64::from(image.height);
    let target_width = f64::from(spec.width);
    let target_height = f64::from(spec.height);
    let linear = srgb_decode_table();

    for target_y in 0..spec.height {
        if is_cancelled() {
            return Ok(None);
        }
        let source_top = f64::from(target_y) * source_height / target_height;
        let source_bottom = f64::from(target_y + 1) * source_height / target_height;
        let first_y = nonnegative_floor_to_u32(source_top);
        let last_y = nonnegative_ceil_to_u32(source_bottom).min(image.height);
        for target_x in 0..spec.width {
            let source_left = f64::from(target_x) * source_width / target_width;
            let source_right = f64::from(target_x + 1) * source_width / target_width;
            let first_x = nonnegative_floor_to_u32(source_left);
            let last_x = nonnegative_ceil_to_u32(source_right).min(image.width);
            let mut total_weight = 0.0_f32;
            let mut alpha_sum = 0.0_f32;
            let mut red_sum = 0.0_f32;
            let mut green_sum = 0.0_f32;
            let mut blue_sum = 0.0_f32;

            for source_y in first_y..last_y {
                let vertical = (source_bottom.min(f64::from(source_y + 1))
                    - source_top.max(f64::from(source_y))) as f32;
                for source_x in first_x..last_x {
                    let horizontal = (source_right.min(f64::from(source_x + 1))
                        - source_left.max(f64::from(source_x)))
                        as f32;
                    let weight = horizontal * vertical;
                    let offset = usize::try_from(
                        (u64::from(source_y) * u64::from(image.width) + u64::from(source_x)) * 4,
                    )
                    .map_err(|_| Error::Gpu("image preview offset overflowed".into()))?;
                    let pixel = &image.rgba[offset..offset + 4];
                    let alpha = f32::from(pixel[3]) / 255.0;
                    let premultiplied_weight = weight * alpha;
                    total_weight += weight;
                    alpha_sum += premultiplied_weight;
                    red_sum += linear[usize::from(pixel[0])] * premultiplied_weight;
                    green_sum += linear[usize::from(pixel[1])] * premultiplied_weight;
                    blue_sum += linear[usize::from(pixel[2])] * premultiplied_weight;
                }
            }

            let output_offset = usize::try_from(
                (u64::from(target_y) * u64::from(spec.width) + u64::from(target_x)) * 4,
            )
            .map_err(|_| Error::Gpu("image preview output offset overflowed".into()))?;
            let output = &mut rgba[output_offset..output_offset + 4];
            if alpha_sum > 0.0 {
                output[0] = linear_to_srgb_byte(red_sum / alpha_sum);
                output[1] = linear_to_srgb_byte(green_sum / alpha_sum);
                output[2] = linear_to_srgb_byte(blue_sum / alpha_sum);
            }
            output[3] = unit_to_byte(alpha_sum / total_weight.max(f32::EPSILON));
        }
    }

    Ok(Some(ImagePreview {
        rgba,
        spec,
        working_color: image.working_color,
    }))
}

pub(crate) fn select_image_upload<'a>(
    image: &'a DecodedImage,
    prepared: Option<&'a ImagePreview>,
    required: Option<PreviewSpec>,
    output: OutputColorTransform,
) -> Result<ImageUpload<'a>, Error> {
    let source_len = viewr_protocol::checked_rgba_len(image.width, image.height)
        .map_err(|error| Error::Gpu(error.to_string()))?;
    if image.rgba.len() != source_len {
        return Err(Error::Gpu(
            "image pixels do not match their declared dimensions".into(),
        ));
    }
    if !output.accepts(image.working_color) {
        return Err(Error::Gpu(
            "image working color does not match the display output transform".into(),
        ));
    }
    match (required, prepared) {
        (None, None) => Ok(ImageUpload {
            rgba: &image.rgba,
            size: (image.width, image.height),
            full_resolution: true,
        }),
        (Some(required), Some(preview)) if preview.spec == required => {
            let expected = viewr_protocol::checked_rgba_len(required.width, required.height)
                .map_err(|error| Error::Gpu(error.to_string()))?;
            if preview.rgba.len() != expected {
                return Err(Error::Gpu(
                    "prepared preview does not match its dimensions".into(),
                ));
            }
            if preview.working_color != image.working_color {
                return Err(Error::Gpu(
                    "prepared preview color encoding does not match its source".into(),
                ));
            }
            Ok(ImageUpload {
                rgba: &preview.rgba,
                size: (required.width, required.height),
                full_resolution: false,
            })
        }
        (Some(_), None) => Err(Error::Gpu(
            "image requires a background-prepared GPU preview".into(),
        )),
        (None | Some(_), Some(_)) => Err(Error::Gpu("prepared image preview is stale".into())),
    }
}

/// Bytes ready for one image texture, without mutating the stored image.
pub(crate) struct TextureUpload<'a> {
    /// `true` when `rgba` is premultiplied linear RGBA8 for an `Rgba8Unorm` texture.
    pub(crate) premultiplied: bool,
    /// Straight sRGB when `premultiplied` is false. The borrow is the caller's buffer.
    pub(crate) rgba: Cow<'a, [u8]>,
}

/// Patch bytes for a texture that already has a representation, or a request to
/// upload the whole image because a straight sRGB texture cannot take new alpha.
pub(crate) enum PatchUploadBytes<'a> {
    /// Write these bytes into the existing texture.
    Inline(Cow<'a, [u8]>),
    /// The patch introduces transparency into an opaque sRGB texture.
    NeedsFullUpload,
}

/// Choose the display texture for straight sRGB working pixels.
///
/// Opaque buffers are borrowed unchanged. Any alpha byte below 255 selects a
/// new premultiplied linear buffer. The input is not modified.
pub(crate) fn prepare_texture_upload(straight_srgb: &[u8]) -> Result<TextureUpload<'_>, Error> {
    if !straight_srgb.len().is_multiple_of(4) {
        return Err(Error::Gpu("display pixels are not packed RGBA".to_owned()));
    }
    if !crate::image_info::rgba_has_transparency(straight_srgb) {
        return Ok(TextureUpload {
            premultiplied: false,
            rgba: Cow::Borrowed(straight_srgb),
        });
    }
    Ok(TextureUpload {
        premultiplied: true,
        rgba: Cow::Owned(premultiply_linear_rgba8(straight_srgb)?),
    })
}

/// Encode one Spot Heal patch for the texture representation already in use.
pub(crate) fn encode_patch_for_texture(
    straight_srgb: &[u8],
    premultiplied_texture: bool,
) -> Result<PatchUploadBytes<'_>, Error> {
    if !straight_srgb.len().is_multiple_of(4) {
        return Err(Error::Gpu("patch pixels are not packed RGBA".to_owned()));
    }
    if premultiplied_texture {
        return Ok(PatchUploadBytes::Inline(Cow::Owned(
            premultiply_linear_rgba8(straight_srgb)?,
        )));
    }
    if crate::image_info::rgba_has_transparency(straight_srgb) {
        return Ok(PatchUploadBytes::NeedsFullUpload);
    }
    Ok(PatchUploadBytes::Inline(Cow::Borrowed(straight_srgb)))
}

/// Premultiply straight sRGB RGBA8 into linear light.
///
/// RGB is decoded with the sRGB transfer, multiplied by alpha, and stored as
/// 8-bit linear. Alpha is copied unchanged. A fully transparent pixel stores
/// zero RGB, so its source color contributes nothing.
pub(crate) fn premultiply_linear_rgba8(straight_srgb: &[u8]) -> Result<Vec<u8>, Error> {
    if !straight_srgb.len().is_multiple_of(4) {
        return Err(Error::Gpu("display pixels are not packed RGBA".to_owned()));
    }
    let mut out = Vec::new();
    out.try_reserve_exact(straight_srgb.len())
        .map_err(|error| {
            Error::Gpu(format!("could not allocate premultiplied display: {error}"))
        })?;
    out.resize(straight_srgb.len(), 0);
    let linear = srgb_decode_table();
    for (src, dst) in straight_srgb
        .as_chunks::<4>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<4>().0)
    {
        let alpha = f32::from(src[3]) / 255.0;
        dst[0] = unit_to_byte(linear[usize::from(src[0])] * alpha);
        dst[1] = unit_to_byte(linear[usize::from(src[1])] * alpha);
        dst[2] = unit_to_byte(linear[usize::from(src[2])] * alpha);
        dst[3] = src[3];
    }
    Ok(out)
}

fn srgb_decode_table() -> &'static [f32; 256] {
    static TABLE: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let encoded = index as f32 / 255.0;
            if encoded <= 0.040_45 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}

fn linear_to_srgb_byte(linear: f32) -> u8 {
    const STEPS: usize = 4096;
    static TABLE: std::sync::OnceLock<[u8; STEPS + 1]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let linear = index as f32 / STEPS as f32;
            let encoded = if linear <= 0.003_130_8 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            };
            unit_to_byte(encoded)
        })
    });
    let index = (linear.clamp(0.0, 1.0) * STEPS as f32).round();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        table[index as usize]
    }
}

fn unit_to_byte(value: f32) -> u8 {
    let rounded = (value.clamp(0.0, 1.0) * 255.0).round();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        rounded as u8
    }
}

fn nonnegative_ceil_to_u32(value: f64) -> u32 {
    let value = value.ceil().clamp(0.0, f64::from(u32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        value as u32
    }
}

fn nonnegative_floor_to_u32(value: f64) -> u32 {
    let value = value.floor().clamp(0.0, f64::from(u32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        value as u32
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ImagePreview, MAX_GPU_BASE_PIXELS, PatchUploadBytes, PreviewSpec, encode_patch_for_texture,
        mip_level_count, premultiply_linear_rgba8, prepare_image_preview, prepare_texture_upload,
        preview_spec, select_image_upload, texture_dimensions,
    };
    use crate::color::{OutputColorTransform, WorkingColorEncoding};
    use crate::decode::{ColorProfileStatus, DecodedImage};

    fn decoded_image(
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        working_color: WorkingColorEncoding,
    ) -> DecodedImage {
        DecodedImage {
            rgba,
            width,
            height,
            color_profile: ColorProfileStatus::AssumedSrgb,
            working_color,
            decode_proof: None,
        }
    }

    #[test]
    fn oversized_texture_dimensions_preserve_the_complete_aspect() {
        assert_eq!(
            texture_dimensions((20_000, 10_000), 8_192, MAX_GPU_BASE_PIXELS),
            (8_192, 4_096)
        );
        assert_eq!(
            texture_dimensions((10_000, 20_000), 8_192, MAX_GPU_BASE_PIXELS),
            (4_096, 8_192)
        );
        assert_eq!(
            texture_dimensions((4_000, 3_000), 8_192, MAX_GPU_BASE_PIXELS),
            (4_000, 3_000)
        );
        assert_eq!(
            texture_dimensions((1, 65_535), 8_192, MAX_GPU_BASE_PIXELS),
            (1, 8_192)
        );
        assert_eq!(
            texture_dimensions((12_000, 12_000), 16_384, MAX_GPU_BASE_PIXELS),
            (8_192, 8_192)
        );
        let bounded = texture_dimensions((16_000, 8_000), 16_384, MAX_GPU_BASE_PIXELS);
        assert!(bounded.0.abs_diff(bounded.1.saturating_mul(2)) <= 1);
        assert!(u64::from(bounded.0) * u64::from(bounded.1) <= MAX_GPU_BASE_PIXELS);
        assert!(preview_spec((4_000, 3_000), 8_192, MAX_GPU_BASE_PIXELS).is_none());
    }

    #[test]
    fn image_upload_selection_enforces_source_and_preview_contracts() {
        let image = decoded_image(vec![7; 16], 2, 2, WorkingColorEncoding::SRGB_RGBA8);
        let direct =
            select_image_upload(&image, None, None, OutputColorTransform::SRGB_TO_SRGB).unwrap();
        assert_eq!(direct.size, (2, 2));
        assert!(direct.full_resolution);
        assert_eq!(direct.rgba, image.rgba);

        let required = PreviewSpec {
            width: 1,
            height: 1,
            source_size: (2, 2),
        };
        assert!(
            select_image_upload(
                &image,
                None,
                Some(required),
                OutputColorTransform::SRGB_TO_SRGB,
            )
            .is_err()
        );

        let preview = ImagePreview {
            rgba: vec![9; 4],
            spec: required,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        };
        let reduced = select_image_upload(
            &image,
            Some(&preview),
            Some(required),
            OutputColorTransform::SRGB_TO_SRGB,
        )
        .unwrap();
        assert_eq!(reduced.size, (1, 1));
        assert!(!reduced.full_resolution);
        assert_eq!(reduced.rgba, preview.rgba);
        assert!(
            select_image_upload(
                &image,
                Some(&preview),
                None,
                OutputColorTransform::SRGB_TO_SRGB,
            )
            .is_err()
        );

        let stale_preview = ImagePreview {
            rgba: vec![9; 8],
            spec: PreviewSpec {
                width: 2,
                height: 1,
                source_size: (2, 2),
            },
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        };
        assert!(
            select_image_upload(
                &image,
                Some(&stale_preview),
                Some(required),
                OutputColorTransform::SRGB_TO_SRGB,
            )
            .is_err()
        );

        let malformed_preview = ImagePreview {
            rgba: vec![0; 3],
            spec: required,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        };
        assert!(
            select_image_upload(
                &image,
                Some(&malformed_preview),
                Some(required),
                OutputColorTransform::SRGB_TO_SRGB,
            )
            .is_err()
        );

        let wrong_color_preview = ImagePreview {
            rgba: vec![0; 4],
            spec: required,
            working_color: WorkingColorEncoding::DISPLAY_P3_RGBA8,
        };
        assert!(
            select_image_upload(
                &image,
                Some(&wrong_color_preview),
                Some(required),
                OutputColorTransform::SRGB_TO_SRGB,
            )
            .is_err()
        );

        let unsupported = decoded_image(vec![7; 16], 2, 2, WorkingColorEncoding::DISPLAY_P3_RGBA8);
        assert!(
            select_image_upload(&unsupported, None, None, OutputColorTransform::SRGB_TO_SRGB)
                .is_err()
        );

        let malformed = decoded_image(vec![7; 15], 2, 2, WorkingColorEncoding::SRGB_RGBA8);
        assert!(
            select_image_upload(&malformed, None, None, OutputColorTransform::SRGB_TO_SRGB)
                .is_err()
        );
    }

    #[test]
    fn preview_area_filter_is_linear_light_and_alpha_correct() {
        let image = decoded_image(
            vec![
                0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
            ],
            2,
            2,
            WorkingColorEncoding::SRGB_RGBA8,
        );
        let spec = preview_spec((2, 2), 2, 1).unwrap();
        let preview = prepare_image_preview(&image, spec, || false)
            .unwrap()
            .unwrap();
        assert_eq!(preview.rgba, [188, 188, 188, 255]);
        assert_eq!(preview.working_color, WorkingColorEncoding::SRGB_RGBA8);

        let alpha = decoded_image(
            vec![255, 0, 0, 0, 0, 0, 255, 255],
            2,
            1,
            WorkingColorEncoding::SRGB_RGBA8,
        );
        let spec = preview_spec((2, 1), 2, 1).unwrap();
        let preview = prepare_image_preview(&alpha, spec, || false)
            .unwrap()
            .unwrap();
        // Fully transparent red must not tint the opaque blue. A straight
        // average of those bytes would be about [128, 0, 128, 128].
        assert_eq!(preview.rgba, [0, 0, 255, 128]);
        assert_ne!(preview.rgba, [128, 0, 128, 128]);
        assert_ne!(preview.rgba, [127, 0, 127, 128]);

        let edge = decoded_image(
            vec![255, 0, 0, 128, 0, 0, 255, 255],
            2,
            1,
            WorkingColorEncoding::SRGB_RGBA8,
        );
        let edge_preview = prepare_image_preview(&edge, spec, || false)
            .unwrap()
            .unwrap();
        let expected = linear_edge_reference();
        for (actual, target) in edge_preview.rgba.iter().zip(expected) {
            assert!(
                actual.abs_diff(target) <= 1,
                "partial edge {actual} drifted from linear reference {target}; got {:?}",
                edge_preview.rgba
            );
        }
        assert!(
            edge_preview.rgba[0].abs_diff(127) > 20,
            "partial edge collapsed to a straight RGB average: {:?}",
            edge_preview.rgba
        );
    }

    /// Independent IEC transfer for a half-covered red pixel beside opaque blue.
    fn linear_edge_reference() -> [u8; 4] {
        fn srgb_to_linear(byte: u8) -> f64 {
            let encoded = f64::from(byte) / 255.0;
            if encoded <= 0.040_45 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            }
        }
        fn linear_to_srgb(linear: f64) -> u8 {
            let encoded = if linear <= 0.003_130_8 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
            }
        }
        let alpha0 = 128.0 / 255.0;
        let alpha1 = 1.0;
        let total_weight = 2.0;
        let alpha_sum = alpha0 + alpha1;
        let red = srgb_to_linear(255) * alpha0;
        let blue = srgb_to_linear(255) * alpha1;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let alpha = ((alpha_sum / total_weight).clamp(0.0, 1.0) * 255.0).round() as u8;
        [
            linear_to_srgb(red / alpha_sum),
            0,
            linear_to_srgb(blue / alpha_sum),
            alpha,
        ]
    }

    #[test]
    fn premultiply_matches_linear_light_and_leaves_opaque_bytes_unchanged() {
        let transparent_red = [255_u8, 0, 0, 0];
        let cleared = premultiply_linear_rgba8(&transparent_red).unwrap();
        assert_eq!(cleared, [0, 0, 0, 0]);

        let edge = [128_u8, 64, 200, 128];
        let encoded = premultiply_linear_rgba8(&edge).unwrap();
        assert_eq!(encoded[3], 128);
        let alpha = 128.0 / 255.0;
        let expect = |byte: u8| {
            let encoded_channel = f64::from(byte) / 255.0;
            let linear = if encoded_channel <= 0.040_45 {
                encoded_channel / 12.92
            } else {
                ((encoded_channel + 0.055) / 1.055).powf(2.4)
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                (linear * alpha * 255.0).round() as u8
            }
        };
        for (actual, source) in encoded[..3].iter().zip(edge[..3].iter().copied()) {
            let target = expect(source);
            assert!(
                actual.abs_diff(target) <= 1,
                "channel {source} stored {actual}, reference {target}"
            );
        }

        let source = [255_u8, 0, 0, 0, 10, 20, 30, 255];
        let original = source;
        let combined = premultiply_linear_rgba8(&source).unwrap();
        assert_eq!(source, original);
        assert_eq!(&combined[..4], &[0, 0, 0, 0]);
        assert_eq!(combined[7], 255);

        let opaque = [128_u8, 128, 128, 255, 4, 5, 6, 255];
        let upload = prepare_texture_upload(&opaque).unwrap();
        assert!(!upload.premultiplied);
        assert!(matches!(upload.rgba, std::borrow::Cow::Borrowed(_)));
        assert_eq!(&*upload.rgba, &opaque);

        let mixed = [255_u8, 0, 0, 0, 0, 0, 255, 255];
        let upload = prepare_texture_upload(&mixed).unwrap();
        assert!(upload.premultiplied);
        assert_eq!(&upload.rgba[..4], &[0, 0, 0, 0]);
        assert_eq!(&upload.rgba[4..], &[0, 0, 255, 255]);
        assert!(prepare_texture_upload(&[1, 2, 3]).is_err());
        assert!(premultiply_linear_rgba8(&[1, 2, 3]).is_err());

        let opaque_patch = [8_u8, 8, 8, 255];
        match encode_patch_for_texture(&opaque_patch, false).unwrap() {
            PatchUploadBytes::Inline(bytes) => assert_eq!(&*bytes, &opaque_patch),
            PatchUploadBytes::NeedsFullUpload => panic!("opaque patch fits a straight texture"),
        }
        assert!(matches!(
            encode_patch_for_texture(&transparent_red, false).unwrap(),
            PatchUploadBytes::NeedsFullUpload
        ));
        match encode_patch_for_texture(&transparent_red, true).unwrap() {
            PatchUploadBytes::Inline(bytes) => assert_eq!(&*bytes, &[0, 0, 0, 0]),
            PatchUploadBytes::NeedsFullUpload => panic!("premultiplied texture accepts the patch"),
        }
        let gray = [128_u8, 128, 128, 255];
        match encode_patch_for_texture(&gray, true).unwrap() {
            PatchUploadBytes::Inline(bytes) => {
                assert_eq!(bytes[3], 255);
                assert_ne!(
                    bytes[0], 128,
                    "opaque gray in a linear texture stays sRGB-encoded"
                );
            }
            PatchUploadBytes::NeedsFullUpload => {
                panic!("premultiplied texture accepts opaque gray")
            }
        }
    }

    #[test]
    fn preview_preparation_is_generation_cancellable_and_validates_source() {
        let image = decoded_image(vec![0; 4 * 4 * 4], 4, 4, WorkingColorEncoding::SRGB_RGBA8);
        let spec = preview_spec((4, 4), 4, 4).unwrap();
        let cancellation_checks = std::cell::Cell::new(0_u32);
        assert!(
            prepare_image_preview(&image, spec, || {
                let prior = cancellation_checks.get();
                cancellation_checks.set(prior + 1);
                prior == 1
            })
            .unwrap()
            .is_none()
        );
        assert_eq!(cancellation_checks.get(), 2);

        let malformed = decoded_image(vec![0; 3], 4, 4, WorkingColorEncoding::SRGB_RGBA8);
        assert!(prepare_image_preview(&malformed, spec, || false).is_err());

        let unsupported = decoded_image(
            vec![0; 4 * 4 * 4],
            4,
            4,
            WorkingColorEncoding::DISPLAY_P3_RGBA8,
        );
        assert!(prepare_image_preview(&unsupported, spec, || false).is_err());

        let wrong_source = PreviewSpec {
            source_size: (3, 4),
            ..spec
        };
        assert!(prepare_image_preview(&image, wrong_source, || false).is_err());
        let zero_output = PreviewSpec { width: 0, ..spec };
        assert!(prepare_image_preview(&image, zero_output, || false).is_err());
    }

    #[test]
    fn mip_chain_reaches_one_pixel_on_the_longest_edge() {
        assert_eq!(mip_level_count((1, 1)), 1);
        assert_eq!(mip_level_count((2, 1)), 2);
        assert_eq!(mip_level_count((3, 2)), 2);
        assert_eq!(mip_level_count((4_000, 3_000)), 12);
        assert_eq!(mip_level_count((8_192, 4_096)), 14);
        assert_eq!(mip_level_count((0, 0)), 1);
    }
}
