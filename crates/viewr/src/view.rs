//! Pure geometry for placing an image inside the viewport.
//!
//! Kept separate from the GPU code so the sizing math is unit-tested without a
//! device or a window. The renderer turns a [`Placement`] into the vertex-shader
//! transform.

/// How the current image sits in the viewport, expressed in normalized device
/// coordinates: `scale` is the image's half-extent as a fraction of the viewport
/// (1.0 fills the axis), `offset` shifts its center (0.0 is centered).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Half-extent along x and y as a fraction of the viewport.
    pub scale: [f32; 2],
    /// Center offset along x and y in normalized device coordinates.
    pub offset: [f32; 2],
    /// 2x2 matrix for UV rotation and flipping.
    pub uv_matrix: [f32; 4],
    /// Crop rect [x0, y0, x1, y1] in UV space. Width <= 0 disables crop preview.
    pub crop_rect: [f32; 4],
}

/// Physical-pixel space reserved by persistent application chrome.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportInsets {
    /// Reserved pixels at the left edge.
    pub left: f32,
    /// Reserved pixels at the right edge.
    pub right: f32,
    /// Reserved pixels at the top edge.
    pub top: f32,
    /// Reserved pixels at the bottom edge.
    pub bottom: f32,
}

/// Integer physical-pixel rectangle available to the image renderer.
///
/// The rectangle uses wgpu's top-left origin and can be passed directly to a
/// render-pass scissor. Its dimensions are always non-zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalViewport {
    /// Left edge in physical pixels.
    pub x: u32,
    /// Top edge in physical pixels.
    pub y: u32,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

impl PhysicalViewport {
    /// Convert this physical rectangle to logical UI coordinates.
    #[must_use]
    pub fn logical_bounds(self, scale_factor: f64) -> Option<[f32; 4]> {
        if !scale_factor.is_finite() || scale_factor <= 0.0 {
            return None;
        }
        let scale = scale_factor as f32;
        Some([
            self.x as f32 / scale,
            self.y as f32 / scale,
            self.x.saturating_add(self.width) as f32 / scale,
            self.y.saturating_add(self.height) as f32 / scale,
        ])
    }

    /// Intersect this rectangle with a physical render-target extent.
    #[must_use]
    pub fn intersect(self, extent: (u32, u32)) -> Option<Self> {
        let left = self.x.min(extent.0);
        let top = self.y.min(extent.1);
        let right = self.x.saturating_add(self.width).min(extent.0);
        let bottom = self.y.saturating_add(self.height).min(extent.1);
        let width = right.saturating_sub(left);
        let height = bottom.saturating_sub(top);
        (width > 0 && height > 0).then_some(Self {
            x: left,
            y: top,
            width,
            height,
        })
    }
}

/// Resolve chrome insets to an image-safe physical-pixel scissor rectangle.
///
/// Inner edges round inward so fractional scale factors cannot leak even one
/// image pixel under docked chrome. A fully consumed viewport returns `None`.
#[must_use]
#[allow(
    clippy::cast_sign_loss,
    reason = "all float edges are clamped to non-negative viewport bounds before conversion"
)]
pub fn safe_viewport_rect(
    viewport: (u32, u32),
    insets: ViewportInsets,
) -> Option<PhysicalViewport> {
    let (width, height) = viewport;
    if width == 0 || height == 0 {
        return None;
    }

    let width_f = width as f32;
    let height_f = height as f32;
    let left = insets.left.max(0.0).min(width_f).ceil() as u32;
    let top = insets.top.max(0.0).min(height_f).ceil() as u32;
    let right = (width_f - insets.right.max(0.0))
        .clamp(left as f32, width_f)
        .floor() as u32;
    let bottom = (height_f - insets.bottom.max(0.0))
        .clamp(top as f32, height_f)
        .floor() as u32;

    let available_width = right.saturating_sub(left);
    let available_height = bottom.saturating_sub(top);
    (available_width > 0 && available_height > 0).then_some(PhysicalViewport {
        x: left,
        y: top,
        width: available_width,
        height: available_height,
    })
}

/// Convert a physical-pixel rectangle to logical UI coordinates.
#[must_use]
pub fn physical_rect_to_logical(rect: [f32; 4], scale_factor: f64) -> Option<[f32; 4]> {
    if !scale_factor.is_finite()
        || scale_factor <= 0.0
        || !rect.iter().all(|value| value.is_finite())
    {
        return None;
    }
    let scale = scale_factor as f32;
    Some(rect.map(|value| value / scale))
}

/// Build the source-UV transform for quarter-turn rotation and axis flips.
///
/// The matrix is stored column-major as `[m00, m10, m01, m11]`, matching the
/// shader uniform and the CPU hit-testing path.
#[must_use]
pub fn uv_transform(rotation_steps: i32, flip_h: bool, flip_v: bool) -> [f32; 4] {
    let mut matrix = match rotation_steps.rem_euclid(4) {
        1 => [0.0, -1.0, 1.0, 0.0],
        2 => [-1.0, 0.0, 0.0, -1.0],
        3 => [0.0, 1.0, -1.0, 0.0],
        _ => [1.0, 0.0, 0.0, 1.0],
    };
    if flip_h {
        matrix[0] = -matrix[0];
        matrix[2] = -matrix[2];
    }
    if flip_v {
        matrix[1] = -matrix[1];
        matrix[3] = -matrix[3];
    }
    matrix
}

/// The largest scale fit will apply, in physical display pixels per source pixel.
///
/// Fit shrinks a large image and leaves a small one alone. Enlarging a 64 by 64
/// source to fill a 1000 pixel viewport is arithmetically honest and visually
/// wrong: the result is a soft interpolated wall that no longer reads as a small
/// image. Established viewers leave a small image at actual size and let the
/// player enlarge it deliberately, so fit stops at one source pixel per physical
/// display pixel, the same 100 percent the zoom readout reports. Explicit zoom
/// is unaffected and still reaches its own limits.
const MAX_FIT_SCALE: f32 = 1.0;

/// Scale the image to fit entirely within the viewport, preserving aspect ratio
/// and centering it. The longer-relative axis touches the edges; the other is
/// letterboxed, and a source smaller than the viewport rests at actual size
/// rather than being enlarged. A zero viewport or image dimension yields a
/// hidden placement rather than a division by zero.
#[must_use]
pub fn fit_to_window(viewport: (u32, u32), image: (u32, u32), rotated90: bool) -> Placement {
    fit_to_viewport(viewport, image, rotated90, ViewportInsets::default())
}

/// Fit an image inside the window area that remains after persistent chrome.
///
/// Scale and offset remain expressed against the full window because the GPU
/// shader renders into that target. Invalid or fully consumed viewports yield a
/// hidden placement.
#[must_use]
pub fn fit_to_viewport(
    viewport: (u32, u32),
    image: (u32, u32),
    rotated90: bool,
    insets: ViewportInsets,
) -> Placement {
    fit_to_viewport_with_limit(viewport, image, rotated90, insets, MAX_FIT_SCALE)
}

fn fit_to_viewport_with_limit(
    viewport: (u32, u32),
    image: (u32, u32),
    rotated90: bool,
    insets: ViewportInsets,
    max_scale: f32,
) -> Placement {
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    let (iw, ih) = if rotated90 {
        (image.1 as f32, image.0 as f32)
    } else {
        (image.0 as f32, image.1 as f32)
    };

    let left = insets.left.max(0.0).min(vw);
    let right = (vw - insets.right.max(0.0)).max(left);
    let top = insets.top.max(0.0).min(vh);
    let bottom = (vh - insets.bottom.max(0.0)).max(top);
    let available_width = right - left;
    let available_height = bottom - top;

    if vw <= 0.0
        || vh <= 0.0
        || iw <= 0.0
        || ih <= 0.0
        || available_width <= 0.0
        || available_height <= 0.0
    {
        return Placement {
            scale: [0.0, 0.0],
            offset: [0.0, 0.0],
            uv_matrix: [1.0, 0.0, 0.0, 1.0],
            crop_rect: [0.0, 0.0, 0.0, 0.0],
        };
    }
    let s = (available_width / iw)
        .min(available_height / ih)
        .min(max_scale);
    let center_x = (left + right) * 0.5;
    let center_y = (top + bottom) * 0.5;
    Placement {
        scale: [(iw * s) / vw, (ih * s) / vh],
        offset: [center_x / vw * 2.0 - 1.0, 1.0 - center_y / vh * 2.0],
        uv_matrix: [1.0, 0.0, 0.0, 1.0],
        crop_rect: [0.0, 0.0, 0.0, 0.0],
    }
}

/// Fit a complete image inside an arbitrary physical-pixel collage tile.
///
/// Unlike single-photo Fit, this may enlarge a small source because the tile is
/// already derived from that photo's aspect ratio. It never crops or distorts.
#[must_use]
pub fn fit_to_physical_viewport(
    target: (u32, u32),
    image: (u32, u32),
    viewport: PhysicalViewport,
    rotated90: bool,
) -> Placement {
    let right = target
        .0
        .saturating_sub(viewport.x.saturating_add(viewport.width)) as f32;
    let bottom = target
        .1
        .saturating_sub(viewport.y.saturating_add(viewport.height)) as f32;
    fit_to_viewport_with_limit(
        target,
        image,
        rotated90,
        ViewportInsets {
            left: viewport.x as f32,
            right,
            top: viewport.y as f32,
            bottom,
        },
        f32::MAX,
    )
}

/// Physical display pixels occupied by one source-image pixel at fit.
///
/// A value of `1.0` is actual size. Rotation swaps the source axes before the
/// scale is derived. Invalid geometry yields `0.0`.
#[must_use]
pub fn fit_pixel_scale(
    viewport: (u32, u32),
    image: (u32, u32),
    rotated90: bool,
    insets: ViewportInsets,
) -> f32 {
    let source_width = if rotated90 { image.1 } else { image.0 } as f32;
    if source_width <= 0.0 || viewport.0 == 0 {
        return 0.0;
    }
    let placement = fit_to_viewport(viewport, image, rotated90, insets);
    placement.scale[0] * viewport.0 as f32 / source_width
}

/// Apply a multiplicative zoom while keeping the NDC point under the cursor fixed.
///
/// `cursor_ndc` is the pointer position in normalized device coordinates
/// (`[-1, 1]` on each axis, y up). `offset` is the current pan in NDC.
/// Returns the pan that should be used after `zoom` becomes `zoom * factor`.
#[must_use]
pub fn pan_after_zoom_at_cursor(offset: [f32; 2], cursor_ndc: [f32; 2], factor: f32) -> [f32; 2] {
    // offset' = cursor - (cursor - offset) * factor
    [
        cursor_ndc[0] - (cursor_ndc[0] - offset[0]) * factor,
        cursor_ndc[1] - (cursor_ndc[1] - offset[1]) * factor,
    ]
}

/// Convert a physical-pixel cursor position to NDC (y up).
#[must_use]
pub fn cursor_to_ndc(cursor_px: (f64, f64), viewport: (u32, u32)) -> Option<[f32; 2]> {
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    if vw <= 0.0 || vh <= 0.0 {
        return None;
    }
    let ndc_x = (cursor_px.0 as f32 / vw) * 2.0 - 1.0;
    let ndc_y = 1.0 - (cursor_px.1 as f32 / vh) * 2.0;
    Some([ndc_x, ndc_y])
}

#[cfg(test)]
mod tests {
    use super::{
        PhysicalViewport, ViewportInsets, cursor_to_ndc, fit_pixel_scale, fit_to_physical_viewport,
        fit_to_viewport, fit_to_window, pan_after_zoom_at_cursor, physical_rect_to_logical,
        safe_viewport_rect, uv_transform,
    };

    fn is_zero(v: [f32; 2]) -> bool {
        v[0].abs() < f32::EPSILON && v[1].abs() < f32::EPSILON
    }

    fn assert_matrix_close(actual: [f32; 4], expected: [f32; 4]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn exact_fit_fills_both_axes() {
        let p = fit_to_window((800, 600), (800, 600), false);
        assert!((p.scale[0] - 1.0).abs() < 1e-6);
        assert!((p.scale[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn wide_image_letterboxes_vertically() {
        // A 2:1 image in a 1:1 viewport fills width, is half height.
        let p = fit_to_window((1000, 1000), (2000, 1000), false);
        assert!((p.scale[0] - 1.0).abs() < 1e-6);
        assert!((p.scale[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn tall_image_letterboxes_horizontally() {
        let p = fit_to_window((1000, 1000), (1000, 2000), false);
        assert!((p.scale[0] - 0.5).abs() < 1e-6);
        assert!((p.scale[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_small_image_is_not_enlarged_to_fill_the_window() {
        // This asserted the opposite until fit stopped enlarging: a 100 by 100
        // source in a 1000 by 1000 viewport filled it at 1000 percent. It now
        // rests at actual size, occupying a tenth of each axis.
        let p = fit_to_window((1000, 1000), (100, 100), false);
        assert!((p.scale[0] - 0.1).abs() < 1e-6);
        assert!((p.scale[1] - 0.1).abs() < 1e-6);
    }

    #[test]
    fn is_always_centered() {
        let p = fit_to_window((1280, 720), (4000, 3000), false);
        assert!(is_zero(p.offset));
    }

    #[test]
    fn degenerate_dimensions_are_hidden_not_panicking() {
        assert!(is_zero(fit_to_window((0, 0), (100, 100), false).scale));
        assert!(is_zero(fit_to_window((100, 100), (0, 0), false).scale));
    }

    #[test]
    fn docked_panels_center_and_fit_inside_the_remaining_viewport() {
        let p = fit_to_viewport(
            (1000, 800),
            (700, 600),
            false,
            ViewportInsets {
                left: 100.0,
                right: 200.0,
                top: 50.0,
                bottom: 150.0,
            },
        );
        assert!((p.scale[0] - 0.7).abs() < 1e-6);
        assert!((p.scale[1] - 0.75).abs() < 1e-6);
        assert!((p.offset[0] + 0.1).abs() < 1e-6);
        assert!((p.offset[1] - 0.125).abs() < 1e-6);
    }

    #[test]
    fn arbitrary_physical_viewport_centers_the_complete_image_in_its_cell() {
        let placement = fit_to_physical_viewport(
            (1000, 800),
            (800, 400),
            PhysicalViewport {
                x: 100,
                y: 200,
                width: 400,
                height: 300,
            },
            false,
        );
        assert!((placement.scale[0] - 0.4).abs() < 1e-6);
        assert!((placement.scale[1] - 0.25).abs() < 1e-6);
        assert!((placement.offset[0] + 0.4).abs() < 1e-6);
        assert!((placement.offset[1] - 0.125).abs() < 1e-6);

        let rotated = fit_to_physical_viewport(
            (1000, 800),
            (800, 400),
            PhysicalViewport {
                x: 100,
                y: 200,
                width: 400,
                height: 300,
            },
            true,
        );
        assert!((rotated.scale[0] - 0.15).abs() < 1e-6);
        assert!((rotated.scale[1] - 0.375).abs() < 1e-6);
    }

    #[test]
    fn collage_tile_enlarges_a_complete_small_image_without_changing_its_aspect() {
        let placement = fit_to_physical_viewport(
            (1000, 800),
            (30, 40),
            PhysicalViewport {
                x: 100,
                y: 200,
                width: 300,
                height: 400,
            },
            false,
        );
        assert!((placement.scale[0] - 0.3).abs() < 1e-6);
        assert!((placement.scale[1] - 0.5).abs() < 1e-6);
        assert!((placement.offset[0] + 0.5).abs() < 1e-6);
        assert!(placement.offset[1].abs() < 1e-6);
        assert!(placement.crop_rect.iter().all(|value| value.abs() < 1e-6));
    }

    #[test]
    fn a_source_smaller_than_the_viewport_rests_at_actual_size() {
        // A 64 by 64 fixture in a 1000 by 560 viewport would otherwise be
        // enlarged to 812 percent and read as a soft wall rather than a small
        // image. It now occupies exactly its own pixels, centered.
        let small = fit_to_viewport((1000, 560), (64, 64), false, ViewportInsets::default());
        assert!((small.scale[0] - 64.0 / 1000.0).abs() < 1e-6);
        assert!((small.scale[1] - 64.0 / 560.0).abs() < 1e-6);
        assert!(small.offset[0].abs() < 1e-6);
        assert!(small.offset[1].abs() < 1e-6);
        assert!(
            (fit_pixel_scale((1000, 560), (64, 64), false, ViewportInsets::default()) - 1.0).abs()
                < 1e-6
        );

        // An image that already exceeds one axis is still shrunk to fit.
        assert!(
            (fit_pixel_scale((1000, 560), (2000, 1000), false, ViewportInsets::default()) - 0.5)
                .abs()
                < 1e-6
        );

        // The cap is per fitted axis, not per source axis: a source narrower
        // than the viewport but taller than it still shrinks.
        assert!(
            (fit_pixel_scale((1000, 560), (100, 1120), false, ViewportInsets::default()) - 0.5)
                .abs()
                < 1e-6
        );

        // Rotation swaps the source axes before the cap applies.
        assert!(
            (fit_pixel_scale((1000, 560), (64, 64), true, ViewportInsets::default()) - 1.0).abs()
                < 1e-6
        );

        // Chrome reduces the space, but a small source still rests at actual
        // size inside what remains.
        let docked = fit_pixel_scale(
            (1000, 560),
            (64, 64),
            false,
            ViewportInsets {
                left: 64.0,
                right: 304.0,
                top: 40.0,
                bottom: 112.0,
            },
        );
        assert!((docked - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fit_pixel_scale_reports_actual_display_ratio() {
        assert!(
            (fit_pixel_scale((1000, 800), (2000, 1000), false, ViewportInsets::default()) - 0.5)
                .abs()
                < 1e-6
        );
        assert!(
            (fit_pixel_scale((800, 1000), (2000, 1000), true, ViewportInsets::default()) - 0.5)
                .abs()
                < 1e-6
        );
        assert!(
            fit_pixel_scale((0, 800), (2000, 1000), false, ViewportInsets::default()).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn chrome_that_consumes_an_axis_hides_the_image() {
        let p = fit_to_viewport(
            (800, 600),
            (100, 100),
            false,
            ViewportInsets {
                left: 500.0,
                right: 500.0,
                ..ViewportInsets::default()
            },
        );
        assert!(is_zero(p.scale));
    }

    #[test]
    fn safe_viewport_rounds_fractional_chrome_inward() {
        let rect = safe_viewport_rect(
            (1000, 800),
            ViewportInsets {
                left: 55.2,
                right: 100.4,
                top: 50.1,
                bottom: 75.5,
            },
        );
        assert_eq!(
            rect,
            Some(PhysicalViewport {
                x: 56,
                y: 51,
                width: 843,
                height: 673,
            })
        );
    }

    #[test]
    fn safe_viewport_rejects_fully_consumed_window() {
        assert_eq!(
            safe_viewport_rect(
                (100, 100),
                ViewportInsets {
                    left: 60.0,
                    right: 40.0,
                    ..ViewportInsets::default()
                }
            ),
            None
        );
    }

    #[test]
    fn physical_geometry_converts_to_logical_points() {
        assert_eq!(
            physical_rect_to_logical([20.0, 40.0, 220.0, 140.0], 2.0),
            Some([10.0, 20.0, 110.0, 70.0])
        );
        assert_eq!(physical_rect_to_logical([0.0; 4], 0.0), None);
        assert_eq!(
            PhysicalViewport {
                x: 20,
                y: 40,
                width: 200,
                height: 100,
            }
            .logical_bounds(2.0),
            Some([10.0, 20.0, 110.0, 70.0])
        );
    }

    #[test]
    fn physical_viewport_is_clamped_to_the_render_target() {
        let viewport = PhysicalViewport {
            x: 80,
            y: 30,
            width: 100,
            height: 90,
        };
        assert_eq!(
            viewport.intersect((120, 80)),
            Some(PhysicalViewport {
                x: 80,
                y: 30,
                width: 40,
                height: 50,
            })
        );
        assert_eq!(viewport.intersect((80, 80)), None);
    }

    #[test]
    fn uv_transform_normalizes_turns_and_applies_flips_once() {
        assert_matrix_close(uv_transform(0, false, false), [1.0, 0.0, 0.0, 1.0]);
        assert_matrix_close(uv_transform(5, false, false), [0.0, -1.0, 1.0, 0.0]);
        assert_matrix_close(uv_transform(-1, false, false), [0.0, 1.0, -1.0, 0.0]);
        assert_matrix_close(uv_transform(0, true, false), [-1.0, 0.0, -0.0, 1.0]);
        assert_matrix_close(uv_transform(0, false, true), [1.0, -0.0, 0.0, -1.0]);
    }

    #[test]
    fn zoom_at_cursor_keeps_cursor_point_fixed() {
        // Point under cursor relative to image center should stay fixed after zoom.
        let offset = [0.2_f32, -0.1];
        let cursor = [0.5_f32, 0.25];
        let factor = 1.15_f32;
        let new_off = pan_after_zoom_at_cursor(offset, cursor, factor);
        // (cursor - offset) / 1  == (cursor - new_off) / factor
        let old_rel = [cursor[0] - offset[0], cursor[1] - offset[1]];
        let new_rel = [
            (cursor[0] - new_off[0]) / factor,
            (cursor[1] - new_off[1]) / factor,
        ];
        assert!((old_rel[0] - new_rel[0]).abs() < 1e-5);
        assert!((old_rel[1] - new_rel[1]).abs() < 1e-5);
    }

    #[test]
    fn cursor_to_ndc_center_is_origin() {
        let n = cursor_to_ndc((400.0, 300.0), (800, 600)).unwrap();
        assert!(n[0].abs() < 1e-5);
        assert!(n[1].abs() < 1e-5);
    }
}
