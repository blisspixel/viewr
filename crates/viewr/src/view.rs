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

/// Scale the image to fit entirely within the viewport, preserving aspect ratio
/// and centering it. The longer-relative axis touches the edges; the other is
/// letterboxed. A zero viewport or image dimension yields a hidden placement
/// rather than a division by zero.
#[must_use]
pub fn fit_to_window(viewport: (u32, u32), image: (u32, u32), rotated90: bool) -> Placement {
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    let (iw, ih) = if rotated90 {
        (image.1 as f32, image.0 as f32)
    } else {
        (image.0 as f32, image.1 as f32)
    };

    if vw <= 0.0 || vh <= 0.0 || iw <= 0.0 || ih <= 0.0 {
        return Placement {
            scale: [0.0, 0.0],
            offset: [0.0, 0.0],
            uv_matrix: [1.0, 0.0, 0.0, 1.0],
            crop_rect: [0.0, 0.0, 0.0, 0.0],
        };
    }
    let s = (vw / iw).min(vh / ih);
    Placement {
        scale: [(iw * s) / vw, (ih * s) / vh],
        offset: [0.0, 0.0],
        uv_matrix: [1.0, 0.0, 0.0, 1.0],
        crop_rect: [0.0, 0.0, 0.0, 0.0],
    }
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
    use super::{cursor_to_ndc, fit_to_window, pan_after_zoom_at_cursor};

    fn is_zero(v: [f32; 2]) -> bool {
        v[0].abs() < f32::EPSILON && v[1].abs() < f32::EPSILON
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
    fn small_image_scales_up_to_fit() {
        // A 100x100 image in a 1000x1000 viewport fits exactly (fills).
        let p = fit_to_window((1000, 1000), (100, 100), false);
        assert!((p.scale[0] - 1.0).abs() < 1e-6);
        assert!((p.scale[1] - 1.0).abs() < 1e-6);
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
