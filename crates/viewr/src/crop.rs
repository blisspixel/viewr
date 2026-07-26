//! Pure crop geometry and output calculations.

/// The aspect-ratio constraint to enforce when cropping.
///
/// Fixed ratios are data rather than enum variants so adding a preset never
/// requires another branch in the crop geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropRatio {
    /// Free-form cropping with no locked ratio.
    Free,
    /// Preserve the current image's original pixel aspect ratio.
    Original,
    /// Lock to an explicit width-to-height ratio.
    Fixed {
        /// Relative width component. Zero is treated as an unlocked ratio.
        width: u16,
        /// Relative height component. Zero is treated as an unlocked ratio.
        height: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CropHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl CropRatio {
    /// A square crop.
    pub const SQUARE: Self = Self::fixed(1, 1);
    /// Standard 3:2 landscape photography crop.
    pub const THREE_TWO: Self = Self::fixed(3, 2);
    /// Portrait orientation of [`Self::THREE_TWO`].
    pub const TWO_THREE: Self = Self::fixed(2, 3);
    /// Standard 4:3 landscape crop.
    pub const FOUR_THREE: Self = Self::fixed(4, 3);
    /// Portrait orientation of [`Self::FOUR_THREE`].
    pub const THREE_FOUR: Self = Self::fixed(3, 4);
    /// Common 8x10 and 16x20 landscape print crop.
    pub const FIVE_FOUR: Self = Self::fixed(5, 4);
    /// Portrait orientation of [`Self::FIVE_FOUR`].
    pub const FOUR_FIVE: Self = Self::fixed(4, 5);
    /// Standard 5:3 landscape crop.
    pub const FIVE_THREE: Self = Self::fixed(5, 3);
    /// Portrait orientation of [`Self::FIVE_THREE`].
    pub const THREE_FIVE: Self = Self::fixed(3, 5);
    /// Widescreen landscape crop.
    pub const SIXTEEN_NINE: Self = Self::fixed(16, 9);
    /// Portrait orientation of [`Self::SIXTEEN_NINE`].
    pub const NINE_SIXTEEN: Self = Self::fixed(9, 16);

    /// Construct an explicit width-to-height crop ratio.
    #[must_use]
    pub const fn fixed(width: u16, height: u16) -> Self {
        Self::Fixed { width, height }
    }

    /// Return explicit ratio components, if this is a fixed ratio.
    #[must_use]
    pub const fn components(self) -> Option<(u16, u16)> {
        match self {
            Self::Fixed { width, height } if width != 0 && height != 0 => Some((width, height)),
            Self::Free | Self::Original | Self::Fixed { .. } => None,
        }
    }

    /// A compact label suitable for the crop toolbar.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Free => "Free".to_owned(),
            Self::Original => "Original".to_owned(),
            Self::Fixed { width, height } => format!("{width}:{height}"),
        }
    }
}
const DEFAULT_CROP_MARGIN: f32 = 0.1;
const MINIMUM_CROP_SPAN: f32 = 0.02;

/// Convert a ratio chosen in visible/output orientation to the decoded source
/// axes used by crop geometry. A quarter turn swaps width and height.
pub(crate) fn crop_ratio_for_source(ratio: CropRatio, rotation_steps: i32) -> CropRatio {
    if rotation_steps.rem_euclid(2) == 0 {
        return ratio;
    }
    match ratio {
        CropRatio::Fixed { width, height } => CropRatio::fixed(height, width),
        CropRatio::Free | CropRatio::Original => ratio,
    }
}

/// Quantize a normalized crop selection through the exact exporter path.
/// Chrome and accessibility consumers use this seam so announced pixel bounds
/// cannot drift from the full-resolution crop that will be written.
pub(crate) fn quantized_crop_pixel_rect(
    rect: [f32; 4],
    width: u32,
    height: u32,
    ratio: CropRatio,
) -> Option<crate::edit::Rect> {
    crop_pixel_rect(rect, width, height, ratio)
}

fn crop_integer_ratio(image_size: (u32, u32), ratio: CropRatio) -> Option<(u32, u32)> {
    let (width, height) = match ratio {
        CropRatio::Original => image_size,
        CropRatio::Fixed { width, height } if width != 0 && height != 0 => {
            (u32::from(width), u32::from(height))
        }
        CropRatio::Free | CropRatio::Fixed { .. } => return None,
    };
    if width == 0 || height == 0 {
        return None;
    }
    let divisor = greatest_common_divisor(width, height);
    Some((width / divisor, height / divisor))
}

const fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

pub(crate) fn reduced_crop_ratio(width: u32, height: u32) -> Option<(u16, u16)> {
    if width == 0 || height == 0 {
        return None;
    }
    let divisor = greatest_common_divisor(width, height);
    let width = u16::try_from(width / divisor).ok()?;
    let height = u16::try_from(height / divisor).ok()?;
    Some((width, height))
}

pub(crate) fn crop_pixel_aspect(image_size: (u32, u32), ratio: CropRatio) -> Option<f32> {
    let (image_width, image_height) = image_size;
    if image_width == 0 || image_height == 0 {
        return None;
    }
    match ratio {
        CropRatio::Original => Some(image_width as f32 / image_height as f32),
        CropRatio::Fixed { width, height } if width != 0 && height != 0 => {
            Some(f32::from(width) / f32::from(height))
        }
        CropRatio::Free | CropRatio::Fixed { .. } => None,
    }
}

fn crop_uv_aspect(image_size: (u32, u32), ratio: CropRatio) -> Option<f32> {
    let (image_width, image_height) = image_size;
    let pixel_aspect = crop_pixel_aspect(image_size, ratio)?;
    Some(pixel_aspect * image_height as f32 / image_width as f32)
}

pub(crate) fn default_crop_rect(image_size: (u32, u32), ratio: CropRatio) -> [f32; 4] {
    fit_crop_rect_to_ratio(
        [
            DEFAULT_CROP_MARGIN,
            DEFAULT_CROP_MARGIN,
            1.0 - DEFAULT_CROP_MARGIN,
            1.0 - DEFAULT_CROP_MARGIN,
        ],
        image_size,
        ratio,
    )
}

pub(crate) fn fit_crop_rect_to_ratio(bounds: [f32; 4], image_size: (u32, u32), ratio: CropRatio) -> [f32; 4] {
    let left = bounds[0].min(bounds[2]).clamp(0.0, 1.0);
    let top = bounds[1].min(bounds[3]).clamp(0.0, 1.0);
    let right = bounds[0].max(bounds[2]).clamp(left, 1.0);
    let bottom = bounds[1].max(bounds[3]).clamp(top, 1.0);
    let Some(aspect) = crop_uv_aspect(image_size, ratio) else {
        return [left, top, right, bottom];
    };
    let available_width = right - left;
    let available_height = bottom - top;
    if available_width <= 0.0 || available_height <= 0.0 {
        return [left, top, right, bottom];
    }

    let (width, height) = if available_width / available_height > aspect {
        (available_height * aspect, available_height)
    } else {
        (available_width, available_width / aspect)
    };
    let center_x = (left + right) * 0.5;
    let center_y = (top + bottom) * 0.5;
    [
        center_x - width * 0.5,
        center_y - height * 0.5,
        center_x + width * 0.5,
        center_y + height * 0.5,
    ]
}

pub(crate) fn crop_handle_from_uv(rect: [f32; 4], point: (f32, f32)) -> CropHandle {
    fn nearest_zone(value: f32, start: f32, end: f32) -> i8 {
        let center = (start + end) * 0.5;
        let candidates = [
            ((value - start).abs(), -1),
            ((value - center).abs(), 0),
            ((value - end).abs(), 1),
        ];
        candidates
            .into_iter()
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map_or(0, |candidate| candidate.1)
    }

    let left = rect[0].min(rect[2]);
    let top = rect[1].min(rect[3]);
    let right = rect[0].max(rect[2]);
    let bottom = rect[1].max(rect[3]);
    match (
        nearest_zone(point.0, left, right),
        nearest_zone(point.1, top, bottom),
    ) {
        (-1, -1) => CropHandle::TopLeft,
        (0, -1) => CropHandle::Top,
        (1, -1) => CropHandle::TopRight,
        (1, 0) => CropHandle::Right,
        (1, 1) => CropHandle::BottomRight,
        (0, 1) => CropHandle::Bottom,
        (-1, 1) => CropHandle::BottomLeft,
        (-1, 0) => CropHandle::Left,
        // A handle center never maps to the middle, but choosing the nearest
        // horizontal edge is a deterministic fallback for malformed geometry.
        (0, 0) => {
            if (point.0 - left).abs() <= (right - point.0).abs() {
                CropHandle::Left
            } else {
                CropHandle::Right
            }
        }
        _ => CropHandle::Right,
    }
}

pub(crate) fn resize_crop_rect_from_pointer(
    rect: [f32; 4],
    image_size: (u32, u32),
    ratio: CropRatio,
    handle: CropHandle,
    pointer: (f32, f32),
) -> [f32; 4] {
    let normalized = [
        rect[0].min(rect[2]).clamp(0.0, 1.0),
        rect[1].min(rect[3]).clamp(0.0, 1.0),
        rect[0].max(rect[2]).clamp(0.0, 1.0),
        rect[1].max(rect[3]).clamp(0.0, 1.0),
    ];
    let pointer = (pointer.0.clamp(0.0, 1.0), pointer.1.clamp(0.0, 1.0));
    crop_uv_aspect(image_size, ratio).map_or_else(
        || resize_free_crop_rect(normalized, handle, pointer),
        |aspect| resize_locked_crop_rect(normalized, handle, pointer, aspect),
    )
}

fn resize_free_crop_rect(
    [left, top, right, bottom]: [f32; 4],
    handle: CropHandle,
    pointer: (f32, f32),
) -> [f32; 4] {
    let next_left = pointer.0.min(right - MINIMUM_CROP_SPAN);
    let next_right = pointer.0.max(left + MINIMUM_CROP_SPAN);
    let next_top = pointer.1.min(bottom - MINIMUM_CROP_SPAN);
    let next_bottom = pointer.1.max(top + MINIMUM_CROP_SPAN);
    match handle {
        CropHandle::TopLeft => [next_left, next_top, right, bottom],
        CropHandle::Top => [left, next_top, right, bottom],
        CropHandle::TopRight => [left, next_top, next_right, bottom],
        CropHandle::Right => [left, top, next_right, bottom],
        CropHandle::BottomRight => [left, top, next_right, next_bottom],
        CropHandle::Bottom => [left, top, right, next_bottom],
        CropHandle::BottomLeft => [next_left, top, right, next_bottom],
        CropHandle::Left => [next_left, top, right, bottom],
    }
}

fn resize_locked_crop_rect(
    rect: [f32; 4],
    handle: CropHandle,
    pointer: (f32, f32),
    aspect: f32,
) -> [f32; 4] {
    match handle {
        CropHandle::TopLeft => {
            locked_corner_crop((rect[2], rect[3]), pointer, (-1.0, -1.0), aspect)
        }
        CropHandle::TopRight => {
            locked_corner_crop((rect[0], rect[3]), pointer, (1.0, -1.0), aspect)
        }
        CropHandle::BottomRight => {
            locked_corner_crop((rect[0], rect[1]), pointer, (1.0, 1.0), aspect)
        }
        CropHandle::BottomLeft => {
            locked_corner_crop((rect[2], rect[1]), pointer, (-1.0, 1.0), aspect)
        }
        CropHandle::Left | CropHandle::Right => {
            locked_horizontal_edge_crop(rect, handle, pointer.0, aspect)
        }
        CropHandle::Top | CropHandle::Bottom => {
            locked_vertical_edge_crop(rect, handle, pointer.1, aspect)
        }
    }
}

fn fit_locked_extent(
    desired_width: f32,
    desired_height: f32,
    maximum_width: f32,
    maximum_height: f32,
    aspect: f32,
) -> (f32, f32) {
    let mut width = desired_width
        .max(desired_height * aspect)
        .max(MINIMUM_CROP_SPAN)
        .max(MINIMUM_CROP_SPAN * aspect);
    let mut height = width / aspect;
    let scale = (maximum_width / width)
        .min(maximum_height / height)
        .clamp(0.0, 1.0);
    width *= scale;
    height *= scale;
    (width, height)
}

fn locked_corner_crop(
    anchor: (f32, f32),
    pointer: (f32, f32),
    direction: (f32, f32),
    aspect: f32,
) -> [f32; 4] {
    let maximum_width = if direction.0 < 0.0 {
        anchor.0
    } else {
        1.0 - anchor.0
    };
    let maximum_height = if direction.1 < 0.0 {
        anchor.1
    } else {
        1.0 - anchor.1
    };
    let (width, height) = fit_locked_extent(
        (pointer.0 - anchor.0).abs(),
        (pointer.1 - anchor.1).abs(),
        maximum_width,
        maximum_height,
        aspect,
    );
    let (left, right) = if direction.0 < 0.0 {
        (anchor.0 - width, anchor.0)
    } else {
        (anchor.0, anchor.0 + width)
    };
    let (top, bottom) = if direction.1 < 0.0 {
        (anchor.1 - height, anchor.1)
    } else {
        (anchor.1, anchor.1 + height)
    };
    [left, top, right, bottom]
}

fn locked_horizontal_edge_crop(
    [left, top, right, bottom]: [f32; 4],
    handle: CropHandle,
    pointer_x: f32,
    aspect: f32,
) -> [f32; 4] {
    let center_y = (top + bottom) * 0.5;
    let (anchor_x, direction) = if handle == CropHandle::Left {
        (right, -1.0)
    } else {
        (left, 1.0)
    };
    let maximum_width = if direction < 0.0 {
        anchor_x
    } else {
        1.0 - anchor_x
    };
    let maximum_height = 2.0 * center_y.min(1.0 - center_y);
    let (width, height) = fit_locked_extent(
        (pointer_x - anchor_x).abs(),
        0.0,
        maximum_width,
        maximum_height,
        aspect,
    );
    let (left, right) = if direction < 0.0 {
        (anchor_x - width, anchor_x)
    } else {
        (anchor_x, anchor_x + width)
    };
    [
        left,
        center_y - height * 0.5,
        right,
        center_y + height * 0.5,
    ]
}

fn locked_vertical_edge_crop(
    [left, top, right, bottom]: [f32; 4],
    handle: CropHandle,
    pointer_y: f32,
    aspect: f32,
) -> [f32; 4] {
    let center_x = (left + right) * 0.5;
    let (anchor_y, direction) = if handle == CropHandle::Top {
        (bottom, -1.0)
    } else {
        (top, 1.0)
    };
    let maximum_width = 2.0 * center_x.min(1.0 - center_x);
    let maximum_height = if direction < 0.0 {
        anchor_y
    } else {
        1.0 - anchor_y
    };
    let (width, height) = fit_locked_extent(
        0.0,
        (pointer_y - anchor_y).abs(),
        maximum_width,
        maximum_height,
        aspect,
    );
    let (top, bottom) = if direction < 0.0 {
        (anchor_y - height, anchor_y)
    } else {
        (anchor_y, anchor_y + height)
    };
    [center_x - width * 0.5, top, center_x + width * 0.5, bottom]
}

pub(crate) fn adjust_crop_rect(
    rect: [f32; 4],
    image_size: (u32, u32),
    ratio: CropRatio,
    horizontal: f32,
    vertical: f32,
    resize: bool,
) -> [f32; 4] {
    let left = rect[0].min(rect[2]).clamp(0.0, 1.0);
    let top = rect[1].min(rect[3]).clamp(0.0, 1.0);
    let right = rect[0].max(rect[2]).clamp(left, 1.0);
    let bottom = rect[1].max(rect[3]).clamp(top, 1.0);
    let width = (right - left).clamp(MINIMUM_CROP_SPAN, 1.0);
    let height = (bottom - top).clamp(MINIMUM_CROP_SPAN, 1.0);

    if !resize {
        let next_left = (left + horizontal).clamp(0.0, 1.0 - width);
        let next_top = (top + vertical).clamp(0.0, 1.0 - height);
        return [next_left, next_top, next_left + width, next_top + height];
    }

    let mut next_width = (width + horizontal).clamp(MINIMUM_CROP_SPAN, 1.0);
    let mut next_height = (height + vertical).clamp(MINIMUM_CROP_SPAN, 1.0);
    if let Some(aspect) = crop_uv_aspect(image_size, ratio) {
        if horizontal.abs() >= vertical.abs() {
            next_height = next_width / aspect;
        } else {
            next_width = next_height * aspect;
        }
        if next_width < MINIMUM_CROP_SPAN {
            next_width = MINIMUM_CROP_SPAN;
            next_height = next_width / aspect;
        }
        if next_height < MINIMUM_CROP_SPAN {
            next_height = MINIMUM_CROP_SPAN;
            next_width = next_height * aspect;
        }
        let fit_scale = (1.0 / next_width).min(1.0 / next_height).min(1.0);
        next_width *= fit_scale;
        next_height *= fit_scale;
    }

    let center_x = (left + right) * 0.5;
    let center_y = (top + bottom) * 0.5;
    let next_left = (center_x - next_width * 0.5).clamp(0.0, 1.0 - next_width);
    let next_top = (center_y - next_height * 0.5).clamp(0.0, 1.0 - next_height);
    [
        next_left,
        next_top,
        next_left + next_width,
        next_top + next_height,
    ]
}

pub(crate) fn crop_keyboard_delta(
    horizontal: f32,
    vertical: f32,
    uv_matrix: [f32; 4],
    resize: bool,
) -> (f32, f32) {
    if !resize {
        return (
            uv_matrix[0] * horizontal + uv_matrix[2] * vertical,
            uv_matrix[1] * horizontal + uv_matrix[3] * vertical,
        );
    }

    // Resize is centered: right/down always grow and left/up always shrink.
    // Rotation selects the source axis, while flips must not reverse growth.
    if horizontal.abs() > f32::EPSILON {
        if uv_matrix[0].abs() >= uv_matrix[1].abs() {
            (horizontal, 0.0)
        } else {
            (0.0, horizontal)
        }
    } else if uv_matrix[2].abs() >= uv_matrix[3].abs() {
        (vertical, 0.0)
    } else {
        (0.0, vertical)
    }
}
fn nonnegative_floor_u32(value: f64) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    if value >= f64::from(u32::MAX) {
        return u32::MAX;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        value.floor() as u32
    }
}

fn nonnegative_ceil_u32(value: f64) -> u32 {
    nonnegative_floor_u32(value.ceil())
}

fn centered_crop_origin(center: f64, extent: u32, bound: u32) -> u32 {
    let maximum = bound.saturating_sub(extent);
    let origin = (center - f64::from(extent) * 0.5)
        .round()
        .clamp(0.0, f64::from(maximum));
    nonnegative_floor_u32(origin)
}

/// Convert a UV crop rect into one bounded pixel rectangle. Locked ratios
/// are quantized as whole multiples of their reduced integer components,
/// so the exported pixel dimensions keep the ratio exactly.
pub fn crop_pixel_rect(
    rect: [f32; 4],
    width: u32,
    height: u32,
    ratio: CropRatio,
) -> Option<crate::edit::Rect> {
    if width == 0 || height == 0 {
        return None;
    }
    let left = f64::from(rect[0].min(rect[2]).clamp(0.0, 1.0));
    let top = f64::from(rect[1].min(rect[3]).clamp(0.0, 1.0));
    let right = f64::from(rect[0].max(rect[2]).clamp(0.0, 1.0));
    let bottom = f64::from(rect[1].max(rect[3]).clamp(0.0, 1.0));

    let Some((ratio_width, ratio_height)) = crop_integer_ratio((width, height), ratio) else {
        let x = nonnegative_floor_u32(left * f64::from(width)).min(width);
        let y = nonnegative_floor_u32(top * f64::from(height)).min(height);
        let right = nonnegative_ceil_u32(right * f64::from(width)).min(width);
        let bottom = nonnegative_ceil_u32(bottom * f64::from(height)).min(height);
        let crop_width = right.saturating_sub(x);
        let crop_height = bottom.saturating_sub(y);
        return (crop_width != 0 && crop_height != 0).then_some(crate::edit::Rect {
            x,
            y,
            width: crop_width,
            height: crop_height,
        });
    };

    let maximum_scale = (width / ratio_width).min(height / ratio_height);
    if maximum_scale == 0 {
        return None;
    }
    let selected_width = (right - left) * f64::from(width);
    let selected_height = (bottom - top) * f64::from(height);
    let desired_scale = (selected_width / f64::from(ratio_width))
        .min(selected_height / f64::from(ratio_height))
        .round()
        .clamp(1.0, f64::from(maximum_scale));
    let scale = nonnegative_floor_u32(desired_scale).clamp(1, maximum_scale);
    let crop_width = ratio_width.checked_mul(scale)?;
    let crop_height = ratio_height.checked_mul(scale)?;
    let center_x = (left + right) * 0.5 * f64::from(width);
    let center_y = (top + bottom) * 0.5 * f64::from(height);
    let x = centered_crop_origin(center_x, crop_width, width);
    let y = centered_crop_origin(center_y, crop_height, height);
    Some(crate::edit::Rect {
        x,
        y,
        width: crop_width,
        height: crop_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rect_close(actual: [f32; 4], expected: [f32; 4]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                (actual - expected).abs() < 1e-5,
                "expected {expected}, got {actual}"
            );
        }
    }

    fn assert_pair_close(actual: (f32, f32), expected: (f32, f32)) {
        assert!((actual.0 - expected.0).abs() < 1e-5);
        assert!((actual.1 - expected.1).abs() < 1e-5);
    }

    #[test]
    fn default_free_crop_has_predictable_ten_percent_margins() {
        assert_rect_close(
            default_crop_rect((400, 200), CropRatio::Free),
            [0.1, 0.1, 0.9, 0.9],
        );
    }

    #[test]
    fn default_square_crop_accounts_for_non_square_pixels_in_uv_space() {
        assert_rect_close(
            default_crop_rect((400, 200), CropRatio::SQUARE),
            [0.3, 0.1, 0.7, 0.9],
        );
    }

    #[test]
    fn all_standard_crop_presets_hold_their_pixel_aspect() {
        let presets = [
            CropRatio::SQUARE,
            CropRatio::THREE_TWO,
            CropRatio::TWO_THREE,
            CropRatio::FOUR_THREE,
            CropRatio::THREE_FOUR,
            CropRatio::FIVE_FOUR,
            CropRatio::FOUR_FIVE,
            CropRatio::FIVE_THREE,
            CropRatio::THREE_FIVE,
            CropRatio::SIXTEEN_NINE,
            CropRatio::NINE_SIXTEEN,
        ];
        for image_size in [(4032, 3024), (3024, 4032), (1001, 667)] {
            for ratio in presets {
                let rect = default_crop_rect(image_size, ratio);
                let pixel_width = (rect[2] - rect[0]) * image_size.0 as f32;
                let pixel_height = (rect[3] - rect[1]) * image_size.1 as f32;
                let expected = crop_pixel_aspect(image_size, ratio).unwrap();
                assert!(
                    (pixel_width / pixel_height - expected).abs() < 1e-4,
                    "{} did not hold for {image_size:?}",
                    ratio.label()
                );
            }
        }
    }

    #[test]
    fn locked_crop_exports_exact_integer_ratios_on_odd_dimensions() {
        let presets = [
            CropRatio::SQUARE,
            CropRatio::THREE_TWO,
            CropRatio::TWO_THREE,
            CropRatio::FOUR_THREE,
            CropRatio::THREE_FOUR,
            CropRatio::FIVE_FOUR,
            CropRatio::FOUR_FIVE,
            CropRatio::FIVE_THREE,
            CropRatio::THREE_FIVE,
            CropRatio::SIXTEEN_NINE,
            CropRatio::NINE_SIXTEEN,
            CropRatio::fixed(7, 11),
        ];
        for image_size in [(101, 101), (1001, 667), (4031, 3023)] {
            for ratio in presets {
                let uv = default_crop_rect(image_size, ratio);
                let pixel = crop_pixel_rect(uv, image_size.0, image_size.1, ratio)
                    .unwrap_or_else(|| panic!("{} did not fit {image_size:?}", ratio.label()));
                let (ratio_width, ratio_height) = crop_integer_ratio(image_size, ratio).unwrap();
                assert_eq!(
                    u64::from(pixel.width) * u64::from(ratio_height),
                    u64::from(pixel.height) * u64::from(ratio_width),
                    "{} did not quantize exactly for {image_size:?}",
                    ratio.label()
                );
                assert!(pixel.x + pixel.width <= image_size.0);
                assert!(pixel.y + pixel.height <= image_size.1);
            }
        }

        let rect = default_crop_rect((101, 101), CropRatio::SIXTEEN_NINE);
        let pixel = crop_pixel_rect(rect, 101, 101, CropRatio::SIXTEEN_NINE).unwrap();
        assert_eq!((pixel.width, pixel.height), (80, 45));
    }

    #[test]
    fn crop_pixel_quantization_handles_free_edges_rotation_and_tiny_images() {
        let free = crop_pixel_rect([0.8, 0.8, 1.0, 1.0], 101, 101, CropRatio::Free).unwrap();
        assert_eq!(
            free,
            crate::edit::Rect {
                x: 80,
                y: 80,
                width: 21,
                height: 21
            }
        );

        let source_ratio = crop_ratio_for_source(CropRatio::SIXTEEN_NINE, 1);
        let rect = default_crop_rect((101, 151), source_ratio);
        let rotated = crop_pixel_rect(rect, 101, 151, source_ratio).unwrap();
        assert_eq!(u64::from(rotated.width) * 16, u64::from(rotated.height) * 9);
        assert!(
            crop_pixel_rect([0.0, 0.0, 1.0, 1.0], 15, 8, CropRatio::SIXTEEN_NINE).is_none()
        );
    }

    #[test]
    fn fixed_crop_ratio_tracks_the_visible_orientation() {
        let image_size = (4_000, 3_000);
        let source_ratio = crop_ratio_for_source(CropRatio::SIXTEEN_NINE, 1);
        assert_eq!(source_ratio, CropRatio::NINE_SIXTEEN);
        let rect = default_crop_rect(image_size, source_ratio);
        let source_width = (rect[2] - rect[0]) * image_size.0 as f32;
        let source_height = (rect[3] - rect[1]) * image_size.1 as f32;
        assert!((source_height / source_width - 16.0 / 9.0).abs() < 1e-4);
        assert_eq!(
            crop_ratio_for_source(CropRatio::Original, 1),
            CropRatio::Original
        );
        assert_eq!(
            crop_ratio_for_source(CropRatio::FOUR_FIVE, 2),
            CropRatio::FOUR_FIVE
        );
    }

    #[test]
    fn original_crop_ratio_tracks_each_image() {
        for image_size in [(4032, 3024), (3024, 4032), (1001, 667)] {
            let rect = default_crop_rect(image_size, CropRatio::Original);
            let pixel_width = (rect[2] - rect[0]) * image_size.0 as f32;
            let pixel_height = (rect[3] - rect[1]) * image_size.1 as f32;
            assert!(
                (pixel_width / pixel_height - image_size.0 as f32 / image_size.1 as f32).abs()
                    < 1e-4
            );
        }
    }

    #[test]
    fn image_dimensions_reduce_to_a_stable_custom_ratio() {
        assert_eq!(reduced_crop_ratio(4032, 3024), Some((4, 3)));
        assert_eq!(reduced_crop_ratio(3024, 4032), Some((3, 4)));
        assert_eq!(reduced_crop_ratio(0, 10), None);
    }

    #[test]
    fn crop_handle_hit_mapping_uses_source_geometry() {
        let rect = [0.2, 0.3, 0.8, 0.9];
        assert_eq!(crop_handle_from_uv(rect, (0.2, 0.3)), CropHandle::TopLeft);
        assert_eq!(crop_handle_from_uv(rect, (0.5, 0.3)), CropHandle::Top);
        assert_eq!(crop_handle_from_uv(rect, (0.8, 0.6)), CropHandle::Right);
        assert_eq!(
            crop_handle_from_uv(rect, (0.2, 0.9)),
            CropHandle::BottomLeft
        );
    }

    #[test]
    fn free_crop_handles_move_only_the_expected_edges() {
        let rect = [0.2, 0.2, 0.8, 0.8];
        assert_rect_close(
            resize_crop_rect_from_pointer(
                rect,
                (400, 300),
                CropRatio::Free,
                CropHandle::TopLeft,
                (0.1, 0.15),
            ),
            [0.1, 0.15, 0.8, 0.8],
        );
        assert_rect_close(
            resize_crop_rect_from_pointer(
                rect,
                (400, 300),
                CropRatio::Free,
                CropHandle::Right,
                (0.9, 0.4),
            ),
            [0.2, 0.2, 0.9, 0.8],
        );
    }

    #[test]
    fn every_locked_crop_handle_preserves_ratio_and_bounds() {
        let handles = [
            CropHandle::TopLeft,
            CropHandle::Top,
            CropHandle::TopRight,
            CropHandle::Right,
            CropHandle::BottomRight,
            CropHandle::Bottom,
            CropHandle::BottomLeft,
            CropHandle::Left,
        ];
        let image_size = (400, 300);
        let ratio = CropRatio::SIXTEEN_NINE;
        let initial = default_crop_rect(image_size, ratio);
        let expected = crop_pixel_aspect(image_size, ratio).unwrap();
        for handle in handles {
            for pointer in [(-0.2, -0.1), (0.15, 0.25), (0.95, 0.85), (1.2, 1.1)] {
                let rect =
                    resize_crop_rect_from_pointer(initial, image_size, ratio, handle, pointer);
                assert!(rect[0] >= -1e-6 && rect[1] >= -1e-6);
                assert!(rect[2] <= 1.0 + 1e-6 && rect[3] <= 1.0 + 1e-6);
                assert!(rect[2] > rect[0] && rect[3] > rect[1]);
                let pixel_width = (rect[2] - rect[0]) * image_size.0 as f32;
                let pixel_height = (rect[3] - rect[1]) * image_size.1 as f32;
                assert!(
                    (pixel_width / pixel_height - expected).abs() < 1e-4,
                    "{handle:?} produced {rect:?}"
                );
            }
        }
    }

    #[test]
    fn keyboard_crop_move_preserves_size_and_clamps_at_image_edge() {
        let moved = adjust_crop_rect(
            [0.1, 0.2, 0.5, 0.6],
            (400, 200),
            CropRatio::Free,
            -0.5,
            0.8,
            false,
        );
        assert_rect_close(moved, [0.0, 0.6, 0.4, 1.0]);
    }

    #[test]
    fn keyboard_crop_resize_preserves_locked_pixel_aspect() {
        let resized = adjust_crop_rect(
            [0.3, 0.1, 0.7, 0.9],
            (400, 200),
            CropRatio::SQUARE,
            0.1,
            0.0,
            true,
        );
        let pixel_width = (resized[2] - resized[0]) * 400.0;
        let pixel_height = (resized[3] - resized[1]) * 200.0;
        assert!((pixel_width / pixel_height - 1.0).abs() < 1e-5);
    }

    #[test]
    fn keyboard_crop_direction_tracks_rotation_and_flip_on_screen() {
        assert_pair_close(
            crop_keyboard_delta(1.0, 0.0, crate::view::uv_transform(1, false, false), false),
            (0.0, -1.0),
        );
        assert_pair_close(
            crop_keyboard_delta(1.0, 0.0, crate::view::uv_transform(0, true, false), false),
            (-1.0, 0.0),
        );
        assert_pair_close(
            crop_keyboard_delta(1.0, 0.0, crate::view::uv_transform(0, true, false), true),
            (1.0, 0.0),
        );
    }
}
