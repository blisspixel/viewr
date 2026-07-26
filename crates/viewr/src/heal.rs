//! Focused, local spot healing over a bounded region of interest.
//!
//! The default implementation is deterministic patch-based inpainting. It is
//! deliberately independent of a model runtime so viewr remains a complete
//! viewer without optional local-intelligence components. The app shares decoded
//! pixels with a short-lived worker, which prepares only the bounded working
//! region and drops the full image before running the path-free repair.

use std::collections::VecDeque;
use std::error::Error as StdError;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::decode::DecodedImage;
use crate::edit::Rect;

const CHANNELS: usize = 4;
const ORTHOGONAL_COST: i32 = 3;
const DIAGONAL_COST: i32 = 4;
const UNREACHABLE_DISTANCE: i32 = i32::MAX / 4;
const MAX_WORKING_PIXELS: u64 = 4 * 1024 * 1024;
const MAX_RASTER_WORK: u64 = 16 * 1024 * 1024;
const MAX_BOUNDARY_SAMPLES: usize = 2_048;
const MAX_PATCH_CANDIDATES: usize = 8;
const MAX_TONE_ADJUSTMENT: i16 = 64;

/// Maximum number of sparse input points retained for one repair gesture.
pub const MAX_STROKE_POINTS: usize = 16_384;

/// Smallest supported brush radius, in source-image pixels.
pub const MIN_BRUSH_RADIUS: u32 = 2;
/// Largest supported brush radius, in source-image pixels.
pub const MAX_BRUSH_RADIUS: u32 = 256;
/// Default feather amount as a percentage of brush radius.
pub const DEFAULT_FEATHER_PERCENT: u8 = 35;
/// Largest accepted feather amount as a percentage of brush radius.
pub const MAX_FEATHER_PERCENT: u8 = 100;

/// One image-space point in a spot-heal stroke.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokePoint {
    /// Horizontal source-image coordinate.
    pub x: f32,
    /// Vertical source-image coordinate.
    pub y: f32,
}

/// A rectangular RGBA patch that can be applied without touching other pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagePatch {
    /// Exact destination rectangle in source-image pixels.
    pub bounds: Rect,
    /// Tightly packed RGBA8 pixels for `bounds`.
    pub rgba: Vec<u8>,
}

/// Result of a ranked spot-heal repair.
///
/// `candidate_count` is zero when the bounded inpainting fallback was needed.
/// Otherwise `candidate_index` identifies the selected source patch and can be
/// advanced to offer a deterministic alternate source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpotHealResult {
    /// Exact pixels to apply to the source image.
    pub patch: ImagePatch,
    /// Zero-based selected source-patch rank.
    pub candidate_index: usize,
    /// Number of distinct source patches retained by the bounded search.
    pub candidate_count: usize,
}

/// Bounded in-memory undo and redo history for pixel patches.
///
/// History never writes a sidecar or cache. Oldest entries are discarded when
/// the configured byte ceiling would be exceeded.
pub struct PatchHistory {
    undo: VecDeque<ImagePatch>,
    redo: VecDeque<ImagePatch>,
    undo_bytes: usize,
    redo_bytes: usize,
    byte_limit: usize,
}

impl PatchHistory {
    /// Create an empty history with a strict combined undo/redo byte ceiling.
    #[must_use]
    pub fn new(byte_limit: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            undo_bytes: 0,
            redo_bytes: 0,
            byte_limit,
        }
    }

    /// Remove every in-memory edit record.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.undo_bytes = 0;
        self.redo_bytes = 0;
    }

    /// Record the inverse patch returned by [`apply_patch`].
    pub fn record(&mut self, inverse: ImagePatch) {
        self.redo.clear();
        self.redo_bytes = 0;
        push_bounded(
            &mut self.undo,
            &mut self.undo_bytes,
            inverse,
            self.byte_limit,
        );
    }

    /// Whether an edit can currently be undone.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether an undone edit can currently be reapplied.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo one edit. Returns `false` when history is empty.
    ///
    /// # Errors
    /// Returns [`HealError`] if the current image no longer matches the patch.
    pub fn undo(&mut self, image: &mut DecodedImage) -> Result<bool, HealError> {
        self.undo_patch(image).map(|patch| patch.is_some())
    }

    /// Undo one edit and return the exact patch applied to the image.
    ///
    /// The returned patch allows a renderer to update only the changed texture
    /// region. Returns `None` when history is empty.
    ///
    /// # Errors
    /// Returns [`HealError`] if the current image no longer matches the patch.
    pub fn undo_patch(
        &mut self,
        image: &mut DecodedImage,
    ) -> Result<Option<ImagePatch>, HealError> {
        let Some(patch) = self.undo.pop_back() else {
            return Ok(None);
        };
        self.undo_bytes = self.undo_bytes.saturating_sub(patch.rgba.len());
        match apply_patch(image, &patch) {
            Ok(inverse) => {
                push_bounded(
                    &mut self.redo,
                    &mut self.redo_bytes,
                    inverse,
                    self.byte_limit,
                );
                Ok(Some(patch))
            }
            Err(error) => {
                self.undo_bytes = self.undo_bytes.saturating_add(patch.rgba.len());
                self.undo.push_back(patch);
                Err(error)
            }
        }
    }

    /// Reapply one undone edit. Returns `false` when redo history is empty.
    ///
    /// # Errors
    /// Returns [`HealError`] if the current image no longer matches the patch.
    pub fn redo(&mut self, image: &mut DecodedImage) -> Result<bool, HealError> {
        self.redo_patch(image).map(|patch| patch.is_some())
    }

    /// Reapply one undone edit and return the exact patch applied to the image.
    ///
    /// The returned patch allows a renderer to update only the changed texture
    /// region. Returns `None` when redo history is empty.
    ///
    /// # Errors
    /// Returns [`HealError`] if the current image no longer matches the patch.
    pub fn redo_patch(
        &mut self,
        image: &mut DecodedImage,
    ) -> Result<Option<ImagePatch>, HealError> {
        let Some(patch) = self.redo.pop_back() else {
            return Ok(None);
        };
        self.redo_bytes = self.redo_bytes.saturating_sub(patch.rgba.len());
        match apply_patch(image, &patch) {
            Ok(inverse) => {
                push_bounded(
                    &mut self.undo,
                    &mut self.undo_bytes,
                    inverse,
                    self.byte_limit,
                );
                Ok(Some(patch))
            }
            Err(error) => {
                self.redo_bytes = self.redo_bytes.saturating_add(patch.rgba.len());
                self.redo.push_back(patch);
                Err(error)
            }
        }
    }
}

fn push_bounded(
    history: &mut VecDeque<ImagePatch>,
    used_bytes: &mut usize,
    patch: ImagePatch,
    byte_limit: usize,
) {
    let patch_bytes = patch.rgba.len();
    if patch_bytes > byte_limit {
        history.clear();
        *used_bytes = 0;
        return;
    }
    while used_bytes.saturating_add(patch_bytes) > byte_limit {
        let Some(discarded) = history.pop_front() else {
            break;
        };
        *used_bytes = used_bytes.saturating_sub(discarded.rgba.len());
    }
    history.push_back(patch);
    *used_bytes = used_bytes.saturating_add(patch_bytes);
}

/// A fully prepared, path-free spot-heal operation.
///
/// Preparation copies a bounded working region. The worker drops its shared
/// source image immediately afterward, so running the job does not retain or
/// access the full image.
#[derive(Clone)]
pub struct SpotHealJob {
    origin: (u32, u32),
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    points: Vec<StrokePoint>,
    damage_bounds: LocalRect,
    feather_radius: u32,
    search_radius: u32,
    brush_radius: u32,
}

/// Failures that prevent a safe, bounded repair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HealError {
    /// Source dimensions and RGBA byte count disagree.
    InvalidImageBuffer,
    /// A coordinate or brush radius is outside the accepted contract.
    InvalidStroke,
    /// The gesture would require too much raster work for an interactive edit.
    StrokeTooComplex,
    /// The bounded working area exceeds the spot-heal memory ceiling.
    WorkingAreaTooLarge,
    /// The patch dimensions or byte count do not match the destination.
    InvalidPatch,
    /// No unmasked source pixels exist from which to repair the stroke.
    NoSourcePixels,
    /// The caller canceled a repair that was no longer relevant.
    Cancelled,
}

impl fmt::Display for HealError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidImageBuffer => "image dimensions do not match the RGBA buffer",
            Self::InvalidStroke => "spot-heal stroke is invalid",
            Self::StrokeTooComplex => "spot-heal stroke is too long; use shorter strokes",
            Self::WorkingAreaTooLarge => "spot-heal stroke is too large; use shorter strokes",
            Self::InvalidPatch => "image patch does not fit the destination image",
            Self::NoSourcePixels => "spot heal needs clean pixels around the painted area",
            Self::Cancelled => "spot heal was canceled",
        };
        formatter.write_str(message)
    }
}

impl StdError for HealError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl LocalRect {
    fn right(self) -> u32 {
        self.x.saturating_add(self.width)
    }

    fn bottom(self) -> u32 {
        self.y.saturating_add(self.height)
    }

    fn expanded(self, padding: u32, width: u32, height: u32) -> Self {
        let x = self.x.saturating_sub(padding);
        let y = self.y.saturating_sub(padding);
        let right = self.right().saturating_add(padding).min(width);
        let bottom = self.bottom().saturating_add(padding).min(height);
        Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }

    fn translated(self, dx: i32, dy: i32) -> Option<Self> {
        Some(Self {
            x: self.x.checked_add_signed(dx)?,
            y: self.y.checked_add_signed(dy)?,
            width: self.width,
            height: self.height,
        })
    }

    fn is_inside(self, width: u32, height: u32) -> bool {
        self.width != 0
            && self.height != 0
            && self
                .x
                .checked_add(self.width)
                .is_some_and(|right| right <= width)
            && self
                .y
                .checked_add(self.height)
                .is_some_and(|bottom| bottom <= height)
    }

    fn overlaps(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

impl SpotHealJob {
    /// Prepare a bounded repair from an image-space stroke.
    ///
    /// Returns `Ok(None)` when every point lies outside the image. The brush
    /// radius must be between [`MIN_BRUSH_RADIUS`] and [`MAX_BRUSH_RADIUS`].
    ///
    /// # Errors
    /// Returns [`HealError`] for malformed pixels, non-finite coordinates,
    /// excessive strokes, invalid brush size, or a working region above the
    /// fixed memory ceiling.
    pub fn prepare(
        image: &DecodedImage,
        points: &[StrokePoint],
        brush_radius: u32,
    ) -> Result<Option<Self>, HealError> {
        Self::prepare_with_feather(image, points, brush_radius, DEFAULT_FEATHER_PERCENT)
    }

    /// Prepare a bounded repair with an explicit feather percentage.
    ///
    /// A value of zero limits blending to the painted mask. A value of 100
    /// feathers outward by one brush radius.
    ///
    /// # Errors
    /// Returns [`HealError`] for the same malformed inputs as [`Self::prepare`]
    /// or when `feather_percent` is greater than [`MAX_FEATHER_PERCENT`].
    pub fn prepare_with_feather(
        image: &DecodedImage,
        points: &[StrokePoint],
        brush_radius: u32,
        feather_percent: u8,
    ) -> Result<Option<Self>, HealError> {
        validate_image(image)?;
        if !(MIN_BRUSH_RADIUS..=MAX_BRUSH_RADIUS).contains(&brush_radius)
            || feather_percent > MAX_FEATHER_PERCENT
            || points.is_empty()
            || points.len() > MAX_STROKE_POINTS
            || points.iter().any(|point| {
                !point.x.is_finite()
                    || !point.y.is_finite()
                    || point.x.abs() > image.width as f32 + MAX_BRUSH_RADIUS as f32
                    || point.y.abs() > image.height as f32 + MAX_BRUSH_RADIUS as f32
            })
        {
            return Err(HealError::InvalidStroke);
        }

        let Some(global_damage) = stroke_bounds(points, brush_radius, image.width, image.height)
        else {
            return Ok(None);
        };
        let feather_radius = brush_radius
            .saturating_mul(u32::from(feather_percent))
            .saturating_add(50)
            / 100;
        let search_radius = brush_radius.saturating_mul(5).clamp(32, 256);
        let working_padding = feather_radius.saturating_add(search_radius);
        let global_working = global_damage.expanded(working_padding, image.width, image.height);
        let working_pixels = u64::from(global_working.width) * u64::from(global_working.height);
        if working_pixels > MAX_WORKING_PIXELS {
            return Err(HealError::WorkingAreaTooLarge);
        }

        let local_points: Vec<StrokePoint> = points
            .iter()
            .map(|point| StrokePoint {
                x: point.x - global_working.x as f32,
                y: point.y - global_working.y as f32,
            })
            .collect();
        validate_raster_work(&local_points, brush_radius)?;
        let rgba = extract_region(image, global_working);
        let damage_bounds = LocalRect {
            x: global_damage.x - global_working.x,
            y: global_damage.y - global_working.y,
            width: global_damage.width,
            height: global_damage.height,
        };

        Ok(Some(Self {
            origin: (global_working.x, global_working.y),
            width: global_working.width,
            height: global_working.height,
            rgba,
            points: local_points,
            damage_bounds,
            feather_radius,
            search_radius,
            brush_radius,
        }))
    }

    /// Run deterministic patch matching and return only the changed rectangle.
    ///
    /// # Errors
    /// Returns [`HealError::NoSourcePixels`] if the stroke leaves no clean
    /// pixels from which the bounded fallback can reconstruct the area.
    pub fn run(self) -> Result<ImagePatch, HealError> {
        self.run_inner(0, None).map(|result| result.patch)
    }

    /// Run a repair that can be canceled when its image or tool mode changes.
    ///
    /// # Errors
    /// Returns [`HealError::Cancelled`] after `cancel` becomes true, or the same
    /// reconstruction errors as [`Self::run`].
    pub fn run_cancellable(self, cancel: &AtomicBool) -> Result<ImagePatch, HealError> {
        self.run_inner(0, Some(cancel)).map(|result| result.patch)
    }

    /// Run a repair using one of the ranked, spatially distinct source patches.
    ///
    /// The requested rank wraps within the available candidates. This makes a
    /// repeated Refresh Source action deterministic. When no source patch fits,
    /// the result reports zero candidates and uses bounded directional
    /// inpainting instead.
    ///
    /// # Errors
    /// Returns the same reconstruction and cancellation errors as [`Self::run`].
    pub fn run_ranked(&self, candidate_index: usize) -> Result<SpotHealResult, HealError> {
        self.run_inner(candidate_index, None)
    }

    /// Run a ranked repair that can be canceled when its image or tool mode
    /// changes.
    ///
    /// # Errors
    /// Returns [`HealError::Cancelled`] after `cancel` becomes true, or the same
    /// reconstruction errors as [`Self::run_ranked`].
    pub fn run_ranked_cancellable(
        &self,
        candidate_index: usize,
        cancel: &AtomicBool,
    ) -> Result<SpotHealResult, HealError> {
        self.run_inner(candidate_index, Some(cancel))
    }

    fn run_inner(
        &self,
        candidate_index: usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<SpotHealResult, HealError> {
        check_cancelled(cancel)?;
        let mut mask = vec![0_u8; (u64::from(self.width) * u64::from(self.height)) as usize];
        rasterize_stroke(
            &mut mask,
            self.width,
            self.height,
            &self.points,
            self.brush_radius,
            cancel,
        )?;
        let coverage =
            feather_coverage(&mask, self.width, self.height, self.feather_radius, cancel)?;
        let affected = self
            .damage_bounds
            .expanded(self.feather_radius, self.width, self.height);
        let candidates = ranked_patch_offsets(
            &self.rgba,
            &mask,
            &coverage,
            self.width,
            self.height,
            affected,
            self.brush_radius,
            self.search_radius,
            cancel,
        )?;
        let (patch_rgba, selected_index) = if candidates.is_empty() {
            let repaired =
                directional_boundary_fill(&self.rgba, &coverage, self.width, self.height, cancel)?;
            (
                composite_region(
                    &self.rgba, &repaired, &coverage, self.width, affected, cancel,
                )?,
                0,
            )
        } else {
            let selected_index = candidate_index % candidates.len();
            let candidate = candidates[selected_index];
            (
                composite_shifted_region(
                    &self.rgba, &coverage, self.width, affected, candidate, cancel,
                )?,
                selected_index,
            )
        };
        check_cancelled(cancel)?;
        Ok(SpotHealResult {
            patch: ImagePatch {
                bounds: Rect {
                    x: self.origin.0 + affected.x,
                    y: self.origin.1 + affected.y,
                    width: affected.width,
                    height: affected.height,
                },
                rgba: patch_rgba,
            },
            candidate_index: selected_index,
            candidate_count: candidates.len(),
        })
    }
}

fn check_cancelled(cancel: Option<&AtomicBool>) -> Result<(), HealError> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        Err(HealError::Cancelled)
    } else {
        Ok(())
    }
}

/// Apply `patch` and return the exact pixels it replaced for undo.
///
/// # Errors
/// Returns [`HealError::InvalidImageBuffer`] for malformed destination pixels,
/// or [`HealError::InvalidPatch`] when the patch is malformed or out of bounds.
pub fn apply_patch(image: &mut DecodedImage, patch: &ImagePatch) -> Result<ImagePatch, HealError> {
    validate_image(image)?;
    let bounds = LocalRect {
        x: patch.bounds.x,
        y: patch.bounds.y,
        width: patch.bounds.width,
        height: patch.bounds.height,
    };
    let expected = rgba_len(bounds.width, bounds.height).ok_or(HealError::InvalidPatch)?;
    if !bounds.is_inside(image.width, image.height) || patch.rgba.len() != expected {
        return Err(HealError::InvalidPatch);
    }

    let previous = extract_region(image, bounds);
    copy_region_into(&mut image.rgba, image.width, bounds, &patch.rgba);
    Ok(ImagePatch {
        bounds: patch.bounds,
        rgba: previous,
    })
}

fn validate_image(image: &DecodedImage) -> Result<(), HealError> {
    let expected = rgba_len(image.width, image.height).ok_or(HealError::InvalidImageBuffer)?;
    if image.width == 0 || image.height == 0 || image.rgba.len() != expected {
        return Err(HealError::InvalidImageBuffer);
    }
    Ok(())
}

fn rgba_len(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    usize::try_from(pixels.checked_mul(CHANNELS as u64)?).ok()
}

fn stroke_bounds(
    points: &[StrokePoint],
    radius: u32,
    width: u32,
    height: u32,
) -> Option<LocalRect> {
    let radius = radius as f32;
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0_u32;
    let mut max_y = 0_u32;
    let mut found = false;
    for point in points {
        if point.x + radius < 0.0
            || point.y + radius < 0.0
            || point.x - radius >= width as f32
            || point.y - radius >= height as f32
        {
            continue;
        }
        found = true;
        let left = nonnegative_floor_u32((point.x - radius).max(0.0));
        let top = nonnegative_floor_u32((point.y - radius).max(0.0));
        let right = nonnegative_ceil_u32((point.x + radius).max(0.0))
            .saturating_add(1)
            .min(width);
        let bottom = nonnegative_ceil_u32((point.y + radius).max(0.0))
            .saturating_add(1)
            .min(height);
        min_x = min_x.min(left);
        min_y = min_y.min(top);
        max_x = max_x.max(right);
        max_y = max_y.max(bottom);
    }
    if found {
        Some(LocalRect {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        })
    } else {
        None
    }
}

fn extract_region(image: &DecodedImage, bounds: LocalRect) -> Vec<u8> {
    extract_local_region(&image.rgba, image.width, bounds)
}

fn extract_local_region(src: &[u8], source_width: u32, bounds: LocalRect) -> Vec<u8> {
    let row_bytes = bounds.width as usize * CHANNELS;
    let mut output = vec![0_u8; row_bytes * bounds.height as usize];
    for row in 0..bounds.height as usize {
        let source_start =
            ((bounds.y as usize + row) * source_width as usize + bounds.x as usize) * CHANNELS;
        let destination_start = row * row_bytes;
        output[destination_start..destination_start + row_bytes]
            .copy_from_slice(&src[source_start..source_start + row_bytes]);
    }
    output
}

fn copy_region_into(dst: &mut [u8], destination_width: u32, bounds: LocalRect, src: &[u8]) {
    let row_bytes = bounds.width as usize * CHANNELS;
    for row in 0..bounds.height as usize {
        let destination_start =
            ((bounds.y as usize + row) * destination_width as usize + bounds.x as usize) * CHANNELS;
        let source_start = row * row_bytes;
        dst[destination_start..destination_start + row_bytes]
            .copy_from_slice(&src[source_start..source_start + row_bytes]);
    }
}

fn validate_raster_work(points: &[StrokePoint], radius: u32) -> Result<(), HealError> {
    let diameter = u64::from(radius).saturating_mul(2).saturating_add(3);
    let work_per_stamp = diameter.saturating_mul(diameter);
    let spacing = (radius as f32 * 0.35).max(1.0);
    let mut stamps = u64::from(points.len() == 1);
    for pair in points.windows(2) {
        let distance = (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y);
        let steps = (distance / spacing).max(1.0).ceil();
        if !steps.is_finite() || steps > MAX_RASTER_WORK as f32 {
            return Err(HealError::StrokeTooComplex);
        }
        let step_count = u64::from(nonnegative_ceil_u32(steps));
        stamps = stamps.saturating_add(step_count + 1);
        if stamps.saturating_mul(work_per_stamp) > MAX_RASTER_WORK {
            return Err(HealError::StrokeTooComplex);
        }
    }
    Ok(())
}

fn rasterize_stroke(
    mask: &mut [u8],
    width: u32,
    height: u32,
    points: &[StrokePoint],
    radius: u32,
    cancel: Option<&AtomicBool>,
) -> Result<(), HealError> {
    validate_raster_work(points, radius)?;
    if points.len() == 1 {
        check_cancelled(cancel)?;
        paint_circle(mask, width, height, points[0], radius);
        return Ok(());
    }
    for pair in points.windows(2) {
        let dx = pair[1].x - pair[0].x;
        let dy = pair[1].y - pair[0].y;
        let distance = dx.hypot(dy);
        let spacing = (radius as f32 * 0.35).max(1.0);
        let steps = nonnegative_ceil_u32((distance / spacing).max(1.0));
        for step in 0..=steps {
            check_cancelled(cancel)?;
            let amount = step as f32 / steps as f32;
            paint_circle(
                mask,
                width,
                height,
                StrokePoint {
                    x: pair[0].x + dx * amount,
                    y: pair[0].y + dy * amount,
                },
                radius,
            );
        }
    }
    Ok(())
}

fn paint_circle(mask: &mut [u8], width: u32, height: u32, center: StrokePoint, radius: u32) {
    let radius_f = radius as f32;
    let left = nonnegative_floor_u32((center.x - radius_f).max(0.0));
    let top = nonnegative_floor_u32((center.y - radius_f).max(0.0));
    let right = nonnegative_ceil_u32((center.x + radius_f).max(0.0))
        .saturating_add(1)
        .min(width);
    let bottom = nonnegative_ceil_u32((center.y + radius_f).max(0.0))
        .saturating_add(1)
        .min(height);
    let radius_squared = radius_f * radius_f;
    for y in top..bottom {
        for x in left..right {
            let dx = x as f32 + 0.5 - center.x;
            let dy = y as f32 + 0.5 - center.y;
            if dx.mul_add(dx, dy * dy) <= radius_squared {
                mask[(y * width + x) as usize] = 255;
            }
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers prove the finite value is non-negative and bounded by image dimensions"
)]
fn nonnegative_floor_u32(value: f32) -> u32 {
    debug_assert!(value.is_finite() && value >= 0.0 && value <= u32::MAX as f32);
    value.floor() as u32
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers prove the finite value is non-negative and bounded by image dimensions"
)]
fn nonnegative_ceil_u32(value: f32) -> u32 {
    debug_assert!(value.is_finite() && value >= 0.0 && value <= u32::MAX as f32);
    value.ceil() as u32
}

fn feather_coverage(
    mask: &[u8],
    width: u32,
    height: u32,
    feather: u32,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<u8>, HealError> {
    if feather == 0 {
        check_cancelled(cancel)?;
        return Ok(mask.to_vec());
    }
    let width = width as usize;
    let height = height as usize;
    let mut distance: Vec<i32> = mask
        .iter()
        .map(|value| if *value == 0 { UNREACHABLE_DISTANCE } else { 0 })
        .collect();

    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if index.is_multiple_of(0x1000) {
                check_cancelled(cancel)?;
            }
            if distance[index] == 0 {
                continue;
            }
            let mut best = distance[index];
            if x > 0 {
                best = best.min(distance[index - 1].saturating_add(ORTHOGONAL_COST));
            }
            if y > 0 {
                best = best.min(distance[index - width].saturating_add(ORTHOGONAL_COST));
                if x > 0 {
                    best = best.min(distance[index - width - 1].saturating_add(DIAGONAL_COST));
                }
                if x + 1 < width {
                    best = best.min(distance[index - width + 1].saturating_add(DIAGONAL_COST));
                }
            }
            distance[index] = best;
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let index = y * width + x;
            if index.is_multiple_of(0x1000) {
                check_cancelled(cancel)?;
            }
            if distance[index] == 0 {
                continue;
            }
            let mut best = distance[index];
            if x + 1 < width {
                best = best.min(distance[index + 1].saturating_add(ORTHOGONAL_COST));
            }
            if y + 1 < height {
                best = best.min(distance[index + width].saturating_add(ORTHOGONAL_COST));
                if x + 1 < width {
                    best = best.min(distance[index + width + 1].saturating_add(DIAGONAL_COST));
                }
                if x > 0 {
                    best = best.min(distance[index + width - 1].saturating_add(DIAGONAL_COST));
                }
            }
            distance[index] = best;
        }
    }

    let span = i32::try_from(feather)
        .unwrap_or(i32::MAX / ORTHOGONAL_COST)
        .saturating_mul(ORTHOGONAL_COST);
    let coverage = distance
        .iter()
        .zip(mask)
        .map(|(distance, mask)| {
            if *mask != 0 {
                255
            } else if *distance >= span {
                0
            } else {
                let numerator = (span - *distance).saturating_mul(255) + span / 2;
                u8::try_from(numerator / span).unwrap_or(255)
            }
        })
        .collect();
    check_cancelled(cancel)?;
    Ok(coverage)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PatchCandidate {
    score: u64,
    dx: i32,
    dy: i32,
    adjustment: [i16; 3],
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn best_patch_offset(
    rgba: &[u8],
    mask: &[u8],
    coverage: &[u8],
    width: u32,
    height: u32,
    affected: LocalRect,
    brush_radius: u32,
    search_radius: u32,
    cancel: Option<&AtomicBool>,
) -> Result<Option<(i32, i32)>, HealError> {
    Ok(ranked_patch_offsets(
        rgba,
        mask,
        coverage,
        width,
        height,
        affected,
        brush_radius,
        search_radius,
        cancel,
    )?
    .first()
    .map(|candidate| (candidate.dx, candidate.dy)))
}

#[allow(clippy::too_many_arguments)]
fn ranked_patch_offsets(
    rgba: &[u8],
    mask: &[u8],
    coverage: &[u8],
    width: u32,
    height: u32,
    affected: LocalRect,
    brush_radius: u32,
    search_radius: u32,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<PatchCandidate>, HealError> {
    let boundary = boundary_samples(mask, coverage, width);
    if boundary.is_empty() {
        return Ok(Vec::new());
    }
    let step =
        i32::try_from((brush_radius / 8).clamp(2, 8)).map_err(|_| HealError::InvalidStroke)?;
    let search = i32::try_from(search_radius).map_err(|_| HealError::InvalidStroke)?;
    let minimum_shift =
        i32::try_from(brush_radius.saturating_add(2)).map_err(|_| HealError::InvalidStroke)?;
    let step_size = usize::try_from(step).map_err(|_| HealError::InvalidStroke)?;
    let mut values: Vec<i32> = (-search..=search).step_by(step_size).collect();
    if values.last().copied() != Some(search) {
        values.push(search);
    }
    if !values.contains(&0) {
        values.push(0);
        values.sort_unstable();
    }

    let mut candidates = Vec::new();
    for &dy in &values {
        for &dx in &values {
            check_cancelled(cancel)?;
            if dx == 0 && dy == 0 || dx.abs().max(dy.abs()) < minimum_shift {
                continue;
            }
            let Some(source) = affected.translated(dx, dy) else {
                continue;
            };
            if !source.is_inside(width, height) {
                continue;
            }
            if !translated_boundary_is_clean(&boundary, mask, width, height, dx, dy) {
                continue;
            }

            if source.overlaps(affected)
                && !shifted_covered_source_is_clean(mask, coverage, width, affected, dx, dy)
            {
                continue;
            }

            let adjustment = boundary_tone_adjustment(rgba, &boundary, width, dx, dy);
            let mut score =
                patch_match_score(rgba, mask, &boundary, width, height, dx, dy, adjustment);
            let distance_x = u64::from(dx.unsigned_abs());
            let distance_y = u64::from(dy.unsigned_abs());
            let distance = distance_x * distance_x + distance_y * distance_y;
            score = score.saturating_add(
                distance.saturating_mul(u64::try_from(boundary.len()).unwrap_or(u64::MAX)) / 16,
            );
            candidates.push(PatchCandidate {
                score,
                dx,
                dy,
                adjustment,
            });
        }
    }

    candidates.sort_unstable_by_key(|candidate| {
        let distance = i64::from(candidate.dx) * i64::from(candidate.dx)
            + i64::from(candidate.dy) * i64::from(candidate.dy);
        (candidate.score, distance, candidate.dy, candidate.dx)
    });
    let minimum_separation =
        i64::from((brush_radius / 2).max(u32::try_from(step * 2).unwrap_or(0)));
    let minimum_separation_squared = minimum_separation * minimum_separation;
    let mut distinct = Vec::with_capacity(MAX_PATCH_CANDIDATES);
    for candidate in candidates {
        let is_distinct = distinct.iter().all(|selected: &PatchCandidate| {
            let dx = i64::from(candidate.dx - selected.dx);
            let dy = i64::from(candidate.dy - selected.dy);
            dx * dx + dy * dy >= minimum_separation_squared
        });
        if is_distinct {
            distinct.push(candidate);
            if distinct.len() == MAX_PATCH_CANDIDATES {
                break;
            }
        }
    }
    Ok(distinct)
}

fn translated_boundary_is_clean(
    boundary: &[(u32, u32)],
    mask: &[u8],
    width: u32,
    height: u32,
    dx: i32,
    dy: i32,
) -> bool {
    boundary.iter().all(|&(x, y)| {
        let Some(source_x) = x.checked_add_signed(dx) else {
            return false;
        };
        let Some(source_y) = y.checked_add_signed(dy) else {
            return false;
        };
        source_x < width && source_y < height && mask[(source_y * width + source_x) as usize] == 0
    })
}

fn boundary_tone_adjustment(
    rgba: &[u8],
    boundary: &[(u32, u32)],
    width: u32,
    dx: i32,
    dy: i32,
) -> [i16; 3] {
    let mut histograms = [[0_u16; 511]; 3];
    for &(x, y) in boundary {
        let source_x = x.checked_add_signed(dx).unwrap_or(x);
        let source_y = y.checked_add_signed(dy).unwrap_or(y);
        let destination_byte = (y * width + x) as usize * CHANNELS;
        let source_byte = (source_y * width + source_x) as usize * CHANNELS;
        for channel in 0..3 {
            let difference = i16::from(rgba[destination_byte + channel])
                - i16::from(rgba[source_byte + channel]);
            histograms[channel][usize::try_from(difference + 255).unwrap_or(0)] += 1;
        }
    }
    let middle = boundary.len() / 2;
    histograms.map(|histogram| {
        let mut cumulative = 0_usize;
        let median = histogram
            .iter()
            .position(|count| {
                cumulative += usize::from(*count);
                cumulative > middle
            })
            .map_or(0, |index| i16::try_from(index).unwrap_or(255) - 255);
        median.clamp(-MAX_TONE_ADJUSTMENT, MAX_TONE_ADJUSTMENT)
    })
}

#[allow(clippy::too_many_arguments)]
fn patch_match_score(
    rgba: &[u8],
    mask: &[u8],
    boundary: &[(u32, u32)],
    width: u32,
    height: u32,
    dx: i32,
    dy: i32,
    adjustment: [i16; 3],
) -> u64 {
    let mut score = 0_u64;
    for &(x, y) in boundary {
        let source_x = x.checked_add_signed(dx).unwrap_or(x);
        let source_y = y.checked_add_signed(dy).unwrap_or(y);
        let destination_byte = (y * width + x) as usize * CHANNELS;
        let source_byte = (source_y * width + source_x) as usize * CHANNELS;
        for channel in 0..3 {
            let adapted =
                (i16::from(rgba[source_byte + channel]) + adjustment[channel]).clamp(0, 255);
            let difference = i32::from(rgba[destination_byte + channel]) - i32::from(adapted);
            score = score.saturating_add(robust_squared(difference, 64));
        }

        if let (Some(destination_gradient), Some(source_gradient)) = (
            clean_luma_gradient(rgba, mask, width, height, x, y),
            clean_luma_gradient(rgba, mask, width, height, source_x, source_y),
        ) {
            let difference_x = destination_gradient.0 - source_gradient.0;
            let difference_y = destination_gradient.1 - source_gradient.1;
            score = score
                .saturating_add(robust_squared(difference_x, 96).saturating_mul(2))
                .saturating_add(robust_squared(difference_y, 96).saturating_mul(2));
        }
    }
    score
}

fn robust_squared(value: i32, limit: i32) -> u64 {
    let magnitude = value.unsigned_abs().min(limit.unsigned_abs());
    u64::from(magnitude) * u64::from(magnitude)
}

fn clean_luma_gradient(
    rgba: &[u8],
    mask: &[u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
) -> Option<(i32, i32)> {
    let center_index = (y * width + x) as usize;
    if mask[center_index] != 0 {
        return None;
    }
    let center = luma(rgba, width, x, y);
    let sample =
        |x: u32, y: u32| (mask[(y * width + x) as usize] == 0).then(|| luma(rgba, width, x, y));
    let left = x.checked_sub(1).and_then(|x| sample(x, y));
    let right = (x + 1 < width).then(|| sample(x + 1, y)).flatten();
    let top = y.checked_sub(1).and_then(|y| sample(x, y));
    let bottom = (y + 1 < height).then(|| sample(x, y + 1)).flatten();
    let gradient_x = axis_gradient(left, Some(center), right);
    let gradient_y = axis_gradient(top, Some(center), bottom);
    (left.is_some() || right.is_some() || top.is_some() || bottom.is_some())
        .then_some((gradient_x, gradient_y))
}

fn luma(rgba: &[u8], width: u32, x: u32, y: u32) -> i32 {
    let byte = (y * width + x) as usize * CHANNELS;
    (54 * i32::from(rgba[byte])
        + 183 * i32::from(rgba[byte + 1])
        + 19 * i32::from(rgba[byte + 2])
        + 128)
        / 256
}

fn shifted_covered_source_is_clean(
    mask: &[u8],
    coverage: &[u8],
    width: u32,
    affected: LocalRect,
    dx: i32,
    dy: i32,
) -> bool {
    for y in affected.y..affected.bottom() {
        for x in affected.x..affected.right() {
            let destination = (y * width + x) as usize;
            if coverage[destination] == 0 {
                continue;
            }
            let Some(source_x) = x.checked_add_signed(dx) else {
                return false;
            };
            let Some(source_y) = y.checked_add_signed(dy) else {
                return false;
            };
            let source = (source_y * width + source_x) as usize;
            if mask[source] != 0 {
                return false;
            }
        }
    }
    true
}

fn boundary_samples(mask: &[u8], coverage: &[u8], width: u32) -> Vec<(u32, u32)> {
    let height = u32::try_from(mask.len())
        .unwrap_or(u32::MAX)
        .checked_div(width)
        .unwrap_or(0);
    let sample_count = mask
        .iter()
        .zip(coverage)
        .enumerate()
        .filter(|(index, (mask_value, coverage_value))| {
            **mask_value == 0
                && (**coverage_value > 0 || touches_mask(mask, width, height, *index as u32))
        })
        .count();
    let stride = sample_count.div_ceil(MAX_BOUNDARY_SAMPLES).max(1);
    mask.iter()
        .zip(coverage)
        .enumerate()
        .filter(|(index, (mask_value, coverage_value))| {
            **mask_value == 0
                && (**coverage_value > 0 || touches_mask(mask, width, height, *index as u32))
        })
        .step_by(stride)
        .take(MAX_BOUNDARY_SAMPLES)
        .map(|(index, _)| (index as u32 % width, index as u32 / width))
        .collect()
}

fn touches_mask(mask: &[u8], width: u32, height: u32, index: u32) -> bool {
    let x = index % width;
    let y = index / width;
    neighbors(width, height, x, y)
        .iter()
        .any(|(x, y)| mask[(*y * width + *x) as usize] != 0)
}

fn composite_shifted_region(
    rgba: &[u8],
    coverage: &[u8],
    width: u32,
    bounds: LocalRect,
    candidate: PatchCandidate,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<u8>, HealError> {
    let mut output = Vec::with_capacity(bounds.width as usize * bounds.height as usize * CHANNELS);
    let mut pixels = 0_usize;
    for y in bounds.y..bounds.bottom() {
        for x in bounds.x..bounds.right() {
            if pixels.is_multiple_of(0x1000) {
                check_cancelled(cancel)?;
            }
            pixels = pixels.saturating_add(1);
            let source_x = x
                .checked_add_signed(candidate.dx)
                .ok_or(HealError::NoSourcePixels)?;
            let source_y = y
                .checked_add_signed(candidate.dy)
                .ok_or(HealError::NoSourcePixels)?;
            let destination = (y * width + x) as usize;
            let source = (source_y * width + source_x) as usize;
            let mut adapted = [0_u8; CHANNELS];
            for channel in 0..3 {
                adapted[channel] = (i16::from(rgba[source * CHANNELS + channel])
                    + candidate.adjustment[channel])
                    .clamp(0, 255)
                    .try_into()
                    .unwrap_or(0);
            }
            adapted[3] = rgba[source * CHANNELS + 3];
            append_blended_pixel(
                &mut output,
                &rgba[destination * CHANNELS..destination * CHANNELS + CHANNELS],
                &adapted,
                coverage[destination],
            );
        }
    }
    Ok(output)
}

fn directional_boundary_fill(
    rgba: &[u8],
    coverage: &[u8],
    width: u32,
    height: u32,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<u8>, HealError> {
    let mut output = rgba.to_vec();
    let mut known: Vec<bool> = coverage.iter().map(|value| *value == 0).collect();
    if !known.iter().any(|value| *value) {
        return Err(HealError::NoSourcePixels);
    }
    let distance = distance_from_known(&known, width, height, cancel)?;
    let mut unknown: Vec<(i32, usize)> = known
        .iter()
        .enumerate()
        .filter(|(_, value)| !**value)
        .map(|(index, _)| (distance[index], index))
        .collect();
    unknown.sort_unstable();

    let mut cursor = 0;
    while cursor < unknown.len() {
        let band_distance = unknown[cursor].0;
        let band_end = unknown[cursor..]
            .iter()
            .position(|entry| entry.0 != band_distance)
            .map_or(unknown.len(), |offset| cursor + offset);
        let mut band_values = Vec::with_capacity(band_end - cursor);
        for &(_, index) in &unknown[cursor..band_end] {
            if index.is_multiple_of(0x1000) {
                check_cancelled(cancel)?;
            }
            let x = index as u32 % width;
            let y = index as u32 / width;
            let Some(pixel) =
                directional_pixel_estimate(&output, &known, &distance, width, height, x, y)
            else {
                return Err(HealError::NoSourcePixels);
            };
            band_values.push((index, pixel));
        }
        for (index, pixel) in band_values {
            output[index * CHANNELS..index * CHANNELS + CHANNELS].copy_from_slice(&pixel);
            known[index] = true;
        }
        cursor = band_end;
    }
    check_cancelled(cancel)?;
    Ok(output)
}

fn distance_from_known(
    known: &[bool],
    width: u32,
    height: u32,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<i32>, HealError> {
    let width_usize = width as usize;
    let height_usize = height as usize;
    let mut distance: Vec<i32> = known
        .iter()
        .map(|value| if *value { 0 } else { UNREACHABLE_DISTANCE })
        .collect();
    for y in 0..height_usize {
        for x in 0..width_usize {
            let index = y * width_usize + x;
            if index.is_multiple_of(0x1000) {
                check_cancelled(cancel)?;
            }
            let mut best = distance[index];
            if x > 0 {
                best = best.min(distance[index - 1].saturating_add(ORTHOGONAL_COST));
            }
            if y > 0 {
                best = best.min(distance[index - width_usize].saturating_add(ORTHOGONAL_COST));
                if x > 0 {
                    best =
                        best.min(distance[index - width_usize - 1].saturating_add(DIAGONAL_COST));
                }
                if x + 1 < width_usize {
                    best =
                        best.min(distance[index - width_usize + 1].saturating_add(DIAGONAL_COST));
                }
            }
            distance[index] = best;
        }
    }
    for y in (0..height_usize).rev() {
        for x in (0..width_usize).rev() {
            let index = y * width_usize + x;
            let mut best = distance[index];
            if x + 1 < width_usize {
                best = best.min(distance[index + 1].saturating_add(ORTHOGONAL_COST));
            }
            if y + 1 < height_usize {
                best = best.min(distance[index + width_usize].saturating_add(ORTHOGONAL_COST));
                if x + 1 < width_usize {
                    best =
                        best.min(distance[index + width_usize + 1].saturating_add(DIAGONAL_COST));
                }
                if x > 0 {
                    best =
                        best.min(distance[index + width_usize - 1].saturating_add(DIAGONAL_COST));
                }
            }
            distance[index] = best;
        }
    }
    if distance.contains(&UNREACHABLE_DISTANCE) {
        return Err(HealError::NoSourcePixels);
    }
    Ok(distance)
}

#[allow(clippy::too_many_arguments)]
fn directional_pixel_estimate(
    rgba: &[u8],
    known: &[bool],
    distance: &[i32],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
) -> Option<[u8; CHANNELS]> {
    let adjacent = neighbors(width, height, x, y);
    let mut weighted = [0_i64; CHANNELS];
    let mut total_weight = 0_i64;
    for &(neighbor_x, neighbor_y) in adjacent.iter() {
        let neighbor = (neighbor_y * width + neighbor_x) as usize;
        if !known[neighbor] {
            continue;
        }
        let dx = i32::try_from(x).ok()? - i32::try_from(neighbor_x).ok()?;
        let dy = i32::try_from(y).ok()? - i32::try_from(neighbor_y).ok()?;
        let spatial_weight = if dx == 0 || dy == 0 { 12 } else { 8 };
        let level_weight =
            1 + i64::from(distance[neighbor].abs_diff(distance[(y * width + x) as usize]));
        let weight = spatial_weight / level_weight.max(1);
        if weight == 0 {
            continue;
        }
        for channel in 0..CHANNELS {
            let value = i32::from(rgba[neighbor * CHANNELS + channel]);
            let (gradient_x, gradient_y) =
                known_channel_gradient(rgba, known, width, height, neighbor_x, neighbor_y, channel);
            let estimate = value
                .saturating_add(gradient_x.saturating_mul(dx))
                .saturating_add(gradient_y.saturating_mul(dy))
                .clamp(0, 255);
            weighted[channel] = weighted[channel].saturating_add(i64::from(estimate) * weight);
        }
        total_weight += weight;
    }
    if total_weight == 0 {
        return None;
    }
    Some(weighted.map(|value| {
        ((value + total_weight / 2) / total_weight)
            .clamp(0, 255)
            .try_into()
            .unwrap_or(0)
    }))
}

#[allow(clippy::too_many_arguments)]
fn known_channel_gradient(
    rgba: &[u8],
    known: &[bool],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    channel: usize,
) -> (i32, i32) {
    let center = sample_known_channel(rgba, known, width, x, y, channel);
    let left = x
        .checked_sub(1)
        .and_then(|x| sample_known_channel(rgba, known, width, x, y, channel));
    let right = (x + 1 < width)
        .then(|| sample_known_channel(rgba, known, width, x + 1, y, channel))
        .flatten();
    let top = y
        .checked_sub(1)
        .and_then(|y| sample_known_channel(rgba, known, width, x, y, channel));
    let bottom = (y + 1 < height)
        .then(|| sample_known_channel(rgba, known, width, x, y + 1, channel))
        .flatten();
    (
        axis_gradient(left, center, right),
        axis_gradient(top, center, bottom),
    )
}

fn sample_known_channel(
    rgba: &[u8],
    known: &[bool],
    width: u32,
    x: u32,
    y: u32,
    channel: usize,
) -> Option<i32> {
    let pixel = (y * width + x) as usize;
    known[pixel].then(|| i32::from(rgba[pixel * CHANNELS + channel]))
}

fn axis_gradient(before: Option<i32>, center: Option<i32>, after: Option<i32>) -> i32 {
    match (before, center, after) {
        (Some(before), _, Some(after)) => (after - before) / 2,
        (Some(before), Some(center), None) => center - before,
        (None, Some(center), Some(after)) => after - center,
        _ => 0,
    }
}

struct Neighbors {
    points: [(u32, u32); 8],
    len: usize,
}

impl Neighbors {
    fn iter(&self) -> impl Iterator<Item = &(u32, u32)> {
        self.points[..self.len].iter()
    }
}

fn neighbors(width: u32, height: u32, x: u32, y: u32) -> Neighbors {
    let mut result = Neighbors {
        points: [(0, 0); 8],
        len: 0,
    };
    for dy in -1_i32..=1 {
        for dx in -1_i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let Some(neighbor_x) = x.checked_add_signed(dx) else {
                continue;
            };
            let Some(neighbor_y) = y.checked_add_signed(dy) else {
                continue;
            };
            if neighbor_x < width && neighbor_y < height {
                result.points[result.len] = (neighbor_x, neighbor_y);
                result.len += 1;
            }
        }
    }
    result
}

fn composite_region(
    original: &[u8],
    repaired: &[u8],
    coverage: &[u8],
    width: u32,
    bounds: LocalRect,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<u8>, HealError> {
    let mut output = Vec::with_capacity(bounds.width as usize * bounds.height as usize * CHANNELS);
    let mut pixels = 0_usize;
    for y in bounds.y..bounds.bottom() {
        for x in bounds.x..bounds.right() {
            if pixels.is_multiple_of(0x1000) {
                check_cancelled(cancel)?;
            }
            pixels = pixels.saturating_add(1);
            let pixel = (y * width + x) as usize;
            let byte = pixel * CHANNELS;
            append_blended_pixel(
                &mut output,
                &original[byte..byte + CHANNELS],
                &repaired[byte..byte + CHANNELS],
                coverage[pixel],
            );
        }
    }
    Ok(output)
}

fn append_blended_pixel(output: &mut Vec<u8>, original: &[u8], repaired: &[u8], alpha: u8) {
    let alpha = u32::from(alpha);
    let inverse = 255 - alpha;
    for channel in 0..CHANNELS {
        output.push(
            ((u32::from(original[channel]) * inverse + u32::from(repaired[channel]) * alpha + 127)
                / 255) as u8,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CHANNELS, HealError, ImagePatch, LocalRect, MAX_STROKE_POINTS, MIN_BRUSH_RADIUS,
        PatchHistory, SpotHealJob, StrokePoint, apply_patch, best_patch_offset,
        boundary_tone_adjustment, directional_boundary_fill, feather_coverage, patch_match_score,
        ranked_patch_offsets, rasterize_stroke,
    };
    use crate::color::WorkingColorEncoding;
    use crate::decode::DecodedImage;
    use crate::edit::Rect;
    use std::sync::atomic::AtomicBool;

    fn patterned_image(width: u32, height: u32) -> DecodedImage {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[
                    (x * 3 % 251) as u8,
                    (y * 5 % 241) as u8,
                    ((x + y) * 2 % 239) as u8,
                    255,
                ]);
            }
        }
        DecodedImage {
            rgba,
            width,
            height,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        }
    }

    fn pixel(image: &DecodedImage, x: u32, y: u32) -> [u8; 4] {
        let index = ((y * image.width + x) * 4) as usize;
        image.rgba[index..index + 4].try_into().unwrap()
    }

    #[test]
    fn preparation_rejects_malformed_images_and_strokes() {
        let malformed = DecodedImage {
            rgba: vec![0; 3],
            width: 2,
            height: 2,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        };
        assert!(matches!(
            SpotHealJob::prepare(&malformed, &[StrokePoint { x: 1.0, y: 1.0 }], 4),
            Err(HealError::InvalidImageBuffer)
        ));
        let overflowing_dimensions = DecodedImage {
            rgba: Vec::new(),
            width: u32::MAX,
            height: u32::MAX,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        };
        assert!(matches!(
            SpotHealJob::prepare(
                &overflowing_dimensions,
                &[StrokePoint { x: 1.0, y: 1.0 }],
                4,
            ),
            Err(HealError::InvalidImageBuffer)
        ));
        let image = patterned_image(32, 32);
        assert!(matches!(
            SpotHealJob::prepare(
                &image,
                &[StrokePoint {
                    x: f32::NAN,
                    y: 1.0
                }],
                4
            ),
            Err(HealError::InvalidStroke)
        ));
        assert!(matches!(
            SpotHealJob::prepare(&image, &[StrokePoint { x: 1.0, y: 1.0 }], 1),
            Err(HealError::InvalidStroke)
        ));
    }

    #[test]
    fn fully_outside_stroke_is_a_noop() {
        let image = patterned_image(32, 32);
        assert!(
            SpotHealJob::prepare(
                &image,
                &[StrokePoint {
                    x: -100.0,
                    y: -100.0,
                }],
                4,
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn oversized_working_region_is_rejected_before_copying_pixels() {
        let image = DecodedImage {
            rgba: vec![0; 2_100 * 2_100 * 4],
            width: 2_100,
            height: 2_100,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        };
        let result = SpotHealJob::prepare(
            &image,
            &[
                StrokePoint { x: 2.0, y: 2.0 },
                StrokePoint {
                    x: 2_097.0,
                    y: 2_097.0,
                },
            ],
            2,
        );
        assert!(matches!(result, Err(HealError::WorkingAreaTooLarge)));
    }

    #[test]
    fn excessive_raster_work_is_rejected_before_the_worker_runs() {
        let image = patterned_image(128, 128);
        let points: Vec<StrokePoint> = (0..MAX_STROKE_POINTS)
            .map(|index| StrokePoint {
                x: if index % 2 == 0 { 10.0 } else { 118.0 },
                y: 64.0,
            })
            .collect();
        assert!(matches!(
            SpotHealJob::prepare(&image, &points, MIN_BRUSH_RADIUS),
            Err(HealError::StrokeTooComplex)
        ));
    }

    #[test]
    fn canceled_job_returns_without_a_patch() {
        let image = patterned_image(128, 128);
        let job = SpotHealJob::prepare(&image, &[StrokePoint { x: 64.0, y: 64.0 }], 9)
            .unwrap()
            .unwrap();
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            job.run_cancellable(&cancel),
            Err(HealError::Cancelled)
        ));
    }

    #[test]
    fn fully_masked_image_reports_that_no_source_pixels_exist() {
        let image = patterned_image(16, 16);
        let job = SpotHealJob::prepare(&image, &[StrokePoint { x: 8.0, y: 8.0 }], 16)
            .unwrap()
            .unwrap();
        assert!(matches!(job.run(), Err(HealError::NoSourcePixels)));
    }

    #[test]
    fn rasterized_polyline_has_no_gaps() {
        let mut mask = vec![0_u8; 80 * 20];
        rasterize_stroke(
            &mut mask,
            80,
            20,
            &[
                StrokePoint { x: 5.0, y: 10.0 },
                StrokePoint { x: 75.0, y: 10.0 },
            ],
            3,
            None,
        )
        .unwrap();
        assert!((5..=75).all(|x| mask[10 * 80 + x] == 255));
    }

    #[test]
    fn feather_is_full_in_mask_and_decreases_outside() {
        let mut mask = vec![0_u8; 12];
        mask[0] = 255;
        let coverage = feather_coverage(&mask, 12, 1, 3, None).unwrap();
        assert_eq!(&coverage[..5], &[255, 170, 85, 0, 0]);
    }

    #[test]
    fn zero_feather_is_exactly_the_painted_mask() {
        let mask = vec![0, 255, 0, 255, 0];
        assert_eq!(feather_coverage(&mask, 5, 1, 0, None).unwrap(), mask);
    }

    #[test]
    fn zero_feather_still_uses_ranked_patch_matching() {
        let image = patterned_image(96, 96);
        let result =
            SpotHealJob::prepare_with_feather(&image, &[StrokePoint { x: 48.0, y: 48.0 }], 8, 0)
                .unwrap()
                .unwrap()
                .run_ranked(0)
                .unwrap();
        assert!(result.candidate_count > 0);
    }

    #[test]
    fn candidate_scoring_rewards_matching_edges_after_tone_adaptation() {
        let width = 15;
        let height = 7;
        let mut rgba = vec![100_u8; width * height * CHANNELS];
        for pixel in rgba.chunks_exact_mut(CHANNELS) {
            pixel[3] = 255;
        }
        let set_gray = |rgba: &mut [u8], x: usize, y: usize, value: u8| {
            let byte = (y * width + x) * CHANNELS;
            rgba[byte..byte + 3].fill(value);
        };
        for (x, value) in [(2, 20), (3, 100), (4, 180), (6, 20), (7, 100), (8, 180)] {
            set_gray(&mut rgba, x, 3, value);
        }
        let mask = vec![0_u8; width * height];
        let boundary = [(7_u32, 3_u32)];
        let matching_adjustment = boundary_tone_adjustment(&rgba, &boundary, width as u32, -4, 0);
        let matching = patch_match_score(
            &rgba,
            &mask,
            &boundary,
            width as u32,
            height as u32,
            -4,
            0,
            matching_adjustment,
        );
        let flat_adjustment = boundary_tone_adjustment(&rgba, &boundary, width as u32, 4, 0);
        let flat = patch_match_score(
            &rgba,
            &mask,
            &boundary,
            width as u32,
            height as u32,
            4,
            0,
            flat_adjustment,
        );
        assert!(matching < flat);
    }

    #[test]
    fn tone_adjustment_uses_a_robust_bounded_median() {
        let width = 8;
        let mut rgba = vec![0_u8; width * CHANNELS];
        for x in 0..width {
            let byte = x * CHANNELS;
            rgba[byte..byte + CHANNELS].copy_from_slice(&[50, 60, 70, 255]);
        }
        for x in 4..8 {
            let byte = x * CHANNELS;
            rgba[byte..byte + CHANNELS].copy_from_slice(&[100, 120, 140, 255]);
        }
        let boundary = [(4, 0), (5, 0), (6, 0), (7, 0)];
        assert_eq!(
            boundary_tone_adjustment(&rgba, &boundary, width as u32, -4, 0),
            [50, 60, 64]
        );
    }

    #[test]
    fn ranked_sources_are_distinct_and_repeatable() {
        let width = 80;
        let height = 48;
        let affected = LocalRect {
            x: 34,
            y: 18,
            width: 12,
            height: 12,
        };
        let rgba = vec![128; width * height * CHANNELS];
        let mut mask = vec![0_u8; width * height];
        for y in affected.y..affected.bottom() {
            for x in affected.x..affected.right() {
                mask[(y * width as u32 + x) as usize] = 255;
            }
        }
        let coverage = feather_coverage(&mask, width as u32, height as u32, 3, None).unwrap();
        let first = ranked_patch_offsets(
            &rgba,
            &mask,
            &coverage,
            width as u32,
            height as u32,
            affected,
            6,
            24,
            None,
        )
        .unwrap();
        let second = ranked_patch_offsets(
            &rgba,
            &mask,
            &coverage,
            width as u32,
            height as u32,
            affected,
            6,
            24,
            None,
        )
        .unwrap();
        assert!(first.len() > 1);
        assert_eq!(first, second);
        assert!(
            first
                .windows(2)
                .all(|pair| (pair[0].dx, pair[0].dy) != (pair[1].dx, pair[1].dy))
        );
    }

    #[test]
    fn directional_fallback_continues_a_linear_ramp() {
        let width = 6;
        let height = 3;
        let mut rgba = Vec::with_capacity(width * height * CHANNELS);
        for _y in 0..height {
            for x in 0..width {
                let value = (x * 40) as u8;
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        let mut coverage = vec![0_u8; width * height];
        for y in 0..height {
            coverage[y * width + 2] = 255;
            coverage[y * width + 3] = 255;
        }
        let repaired =
            directional_boundary_fill(&rgba, &coverage, width as u32, height as u32, None).unwrap();
        for y in 0..height {
            assert!(repaired[(y * width + 2) * CHANNELS].abs_diff(80) <= 2);
            assert!(repaired[(y * width + 3) * CHANNELS].abs_diff(120) <= 2);
        }
    }

    #[test]
    fn spot_heal_replaces_a_defect_and_changes_only_its_patch() {
        let mut image = patterned_image(128, 128);
        let original_rgba = image.rgba.clone();
        for y in 58..70 {
            for x in 58..70 {
                let index = ((y * image.width + x) * 4) as usize;
                image.rgba[index..index + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let damaged_origin = pixel(&image, 0, 0);
        let job = SpotHealJob::prepare(&image, &[StrokePoint { x: 64.0, y: 64.0 }], 9)
            .unwrap()
            .unwrap();
        let patch = job.run().unwrap();
        let undo = apply_patch(&mut image, &patch).unwrap();

        assert_ne!(pixel(&image, 64, 64), [255, 0, 0, 255]);
        assert_eq!(pixel(&image, 0, 0), damaged_origin);
        assert_eq!(undo.bounds, patch.bounds);
        assert!(
            patch.bounds.x <= 58
                && patch.bounds.y <= 58
                && patch.bounds.x + patch.bounds.width > 69
                && patch.bounds.y + patch.bounds.height > 69
        );
        assert_ne!(
            image.rgba, original_rgba,
            "repair need not recreate hidden pixels exactly"
        );
    }

    #[test]
    fn edge_spot_is_repaired_without_out_of_bounds_access() {
        let mut image = patterned_image(64, 64);
        for y in 0..5 {
            for x in 0..5 {
                let index = ((y * image.width + x) * 4) as usize;
                image.rgba[index..index + 4].copy_from_slice(&[255, 0, 255, 255]);
            }
        }
        let job = SpotHealJob::prepare(&image, &[StrokePoint { x: 1.0, y: 1.0 }], 6)
            .unwrap()
            .unwrap();
        let patch = job.run().unwrap();
        apply_patch(&mut image, &patch).unwrap();
        assert_ne!(pixel(&image, 1, 1), [255, 0, 255, 255]);
    }

    #[test]
    fn repair_is_deterministic() {
        let image = patterned_image(96, 96);
        let points = [
            StrokePoint { x: 40.0, y: 40.0 },
            StrokePoint { x: 52.0, y: 46.0 },
        ];
        let first = SpotHealJob::prepare(&image, &points, 7)
            .unwrap()
            .unwrap()
            .run()
            .unwrap();
        let second = SpotHealJob::prepare(&image, &points, 7)
            .unwrap()
            .unwrap()
            .run()
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn ranked_repair_refreshes_to_an_alternate_and_wraps() {
        let mut image = patterned_image(128, 96);
        for y in 42..54 {
            for x in 58..70 {
                let byte = ((y * image.width + x) * 4) as usize;
                image.rgba[byte..byte + 4].copy_from_slice(&[255, 0, 255, 255]);
            }
        }
        let job = SpotHealJob::prepare(&image, &[StrokePoint { x: 64.0, y: 48.0 }], 9)
            .unwrap()
            .unwrap();
        let first = job.run_ranked(0).unwrap();
        assert!(first.candidate_count > 1);
        let second = job.run_ranked(1).unwrap();
        let wrapped = job.run_ranked(first.candidate_count).unwrap();
        assert_eq!(first.candidate_index, 0);
        assert_eq!(second.candidate_index, 1);
        assert_ne!(first.patch, second.patch);
        assert_eq!(first, wrapped);
    }

    #[test]
    fn patch_search_finds_the_nearest_non_overlapping_source() {
        let width = 64;
        let height = 32;
        let affected = LocalRect {
            x: 24,
            y: 8,
            width: 8,
            height: 8,
        };
        let rgba = vec![128; (width * height * CHANNELS as u32) as usize];
        let mut mask = vec![0_u8; (width * height) as usize];
        for y in affected.y..affected.bottom() {
            for x in affected.x..affected.right() {
                mask[(y * width + x) as usize] = 255;
            }
        }
        let coverage = feather_coverage(&mask, width, height, 2, None).unwrap();

        let offset = best_patch_offset(
            &rgba, &mask, &coverage, width, height, affected, 3, 20, None,
        )
        .unwrap();

        assert_eq!(offset, Some((-10, 0)));
    }

    #[test]
    fn applying_the_inverse_patch_restores_every_byte() {
        let mut image = patterned_image(16, 12);
        let original_rgba = image.rgba.clone();
        let patch = ImagePatch {
            bounds: Rect {
                x: 3,
                y: 4,
                width: 2,
                height: 3,
            },
            rgba: vec![200; 2 * 3 * 4],
        };
        let inverse = apply_patch(&mut image, &patch).unwrap();
        assert_ne!(image.rgba, original_rgba);
        let redo = apply_patch(&mut image, &inverse).unwrap();
        assert_eq!(image.rgba, original_rgba);
        assert_eq!(redo, patch);
    }

    #[test]
    fn invalid_patch_is_rejected_without_mutating_image() {
        let mut image = patterned_image(8, 8);
        let original_rgba = image.rgba.clone();
        let patch = ImagePatch {
            bounds: Rect {
                x: 7,
                y: 7,
                width: 2,
                height: 2,
            },
            rgba: vec![0; 16],
        };
        assert_eq!(
            apply_patch(&mut image, &patch),
            Err(HealError::InvalidPatch)
        );
        let overflowing_patch = ImagePatch {
            bounds: Rect {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 1,
            },
            rgba: vec![0; 8],
        };
        assert_eq!(
            apply_patch(&mut image, &overflowing_patch),
            Err(HealError::InvalidPatch)
        );
        assert_eq!(image.rgba, original_rgba);
    }

    #[test]
    fn bounded_history_undoes_redoes_and_discards_redo_on_new_edit() {
        let mut image = patterned_image(8, 8);
        let original = image.rgba.clone();
        let first = ImagePatch {
            bounds: Rect {
                x: 1,
                y: 1,
                width: 1,
                height: 1,
            },
            rgba: vec![250, 1, 2, 255],
        };
        let mut history = PatchHistory::new(32);
        history.record(apply_patch(&mut image, &first).unwrap());
        let edited = image.rgba.clone();
        assert!(history.can_undo());
        assert!(history.undo(&mut image).unwrap());
        assert_eq!(image.rgba, original);
        assert!(history.can_redo());
        assert!(history.redo(&mut image).unwrap());
        assert_eq!(image.rgba, edited);

        assert!(history.undo(&mut image).unwrap());
        history.record(apply_patch(&mut image, &first).unwrap());
        assert!(!history.can_redo());
    }

    #[test]
    fn history_enforces_its_byte_ceiling() {
        let mut history = PatchHistory::new(4);
        history.record(ImagePatch {
            bounds: Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            rgba: vec![1; 4],
        });
        history.record(ImagePatch {
            bounds: Rect {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            },
            rgba: vec![2; 4],
        });
        assert_eq!(history.undo.len(), 1);

        history.record(ImagePatch {
            bounds: Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            },
            rgba: vec![3; 8],
        });
        assert!(!history.can_undo());
    }

    #[test]
    fn local_rect_overlap_uses_half_open_edges() {
        let left = LocalRect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        };
        let touching = LocalRect {
            x: 4,
            y: 0,
            width: 4,
            height: 4,
        };
        let overlapping = LocalRect {
            x: 3,
            y: 3,
            width: 2,
            height: 2,
        };
        assert!(!left.overlaps(touching));
        assert!(left.overlaps(overlapping));
    }
}
