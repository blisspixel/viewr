//! Pure image-sizing, preview, and upload-selection policy for the GPU adapter.
//!
//! This module owns no device, queue, surface, texture, or event-loop state. The
//! native adapter in `gpu` consumes these validated decisions and remains the sole
//! owner of GPU resources.

use crate::color::{OutputColorTransform, WorkingColorEncoding};
use crate::decode::DecodedImage;
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
pub(crate) struct ImagePreview {
    rgba: Vec<u8>,
    spec: PreviewSpec,
    working_color: WorkingColorEncoding,
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
/// Returning `Ok(None)` means the generation was canceled between output rows.
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
        ImagePreview, MAX_GPU_BASE_PIXELS, PreviewSpec, mip_level_count, prepare_image_preview,
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
        assert_eq!(preview.rgba, [0, 0, 255, 128]);
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
