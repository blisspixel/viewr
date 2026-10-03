//! Compact pan cursor bitmaps for platforms without distinct native hands.

use egui::{CursorIcon, CustomCursorImage};

const OPEN: &[u8] = include_bytes!("../assets/pan-open.svg");
const CLOSED: &[u8] = include_bytes!("../assets/pan-closed.svg");
const LOGICAL_SIZE: f64 = 24.0;

#[derive(Clone)]
struct Hands {
    size: u16,
    open: CustomCursorImage,
    closed: CustomCursorImage,
}

/// Keep the OS cursor on chrome and editing handles; replace only pan requests.
pub(crate) fn image(
    context: &egui::Context,
    display_scale: f64,
    icon: CursorIcon,
) -> Option<CustomCursorImage> {
    if !cfg!(target_os = "windows") || !matches!(icon, CursorIcon::Grab | CursorIcon::Grabbing) {
        return None;
    }
    let size = physical_size(display_scale);
    let key = egui::Id::new("viewr_pan_cursors");
    context.data_mut(|data| {
        let cached = data
            .get_temp::<Hands>(key)
            .filter(|hands| hands.size == size);
        let hands = if let Some(hands) = cached {
            hands
        } else {
            let hands = Hands {
                size,
                open: rasterize(OPEN, size)?,
                closed: rasterize(CLOSED, size)?,
            };
            data.insert_temp(key, hands.clone());
            hands
        };
        Some(if icon == CursorIcon::Grabbing {
            hands.closed
        } else {
            hands.open
        })
    })
}

#[allow(clippy::cast_sign_loss, reason = "validated size is between 24 and 96")]
fn physical_size(scale: f64) -> u16 {
    let scale = if scale.is_finite() {
        scale.clamp(1.0, 4.0)
    } else {
        1.0
    };
    (LOGICAL_SIZE * scale).round() as u16
}

fn rasterize(svg: &[u8], size: u16) -> Option<CustomCursorImage> {
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(u32::from(size), u32::from(size))?;
    let scale = f32::from(size) / LOGICAL_SIZE as f32;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // Native cursor buffers use straight alpha; the rasterizer premultiplies it.
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect::<Vec<_>>();
    Some(CustomCursorImage {
        rgba: rgba.into(),
        size: [size, size],
        hotspot: [size / 2, size / 2],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_are_distinct_small_native_bitmaps_at_each_display_scale() {
        for (scale, expected) in [
            (1.0, 24),
            (1.25, 30),
            (1.5, 36),
            (2.0, 48),
            (3.0, 72),
            (4.0, 96),
        ] {
            let size = physical_size(scale);
            let open = rasterize(OPEN, size).unwrap();
            let closed = rasterize(CLOSED, size).unwrap();
            assert_eq!(open.size, [expected; 2]);
            assert_eq!(open.hotspot, closed.hotspot);
            assert_ne!(open.rgba, closed.rgba);
            for cursor in [open, closed] {
                assert_eq!(cursor.rgba.len(), usize::from(size).pow(2) * 4);
                assert!(
                    cursor
                        .rgba
                        .chunks_exact(4)
                        .any(|pixel| pixel == [255, 255, 255, 255])
                );
                assert!(
                    cursor
                        .rgba
                        .chunks_exact(4)
                        .any(|pixel| pixel == [23, 26, 32, 255])
                );
                assert!(
                    cursor
                        .rgba
                        .chunks_exact(4)
                        .any(|pixel| pixel[3] > 0 && pixel[3] < 255 && pixel[0] > pixel[3])
                );
                assert!(
                    cursor
                        .rgba
                        .chunks_exact(4)
                        .take(usize::from(size))
                        .all(|pixel| pixel[3] == 0)
                );
            }
        }
    }

    #[test]
    fn invalid_scales_are_bounded_and_invalid_assets_fall_back() {
        for scale in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0, 0.5] {
            assert_eq!(physical_size(scale), 24);
        }
        assert_eq!(physical_size(1000.0), 96);
        assert!(rasterize(b"invalid", 24).is_none());
    }

    #[test]
    fn only_pan_uses_a_bitmap_and_reuses_its_buffer_until_dpi_changes() {
        let context = egui::Context::default();
        for icon in [
            CursorIcon::Default,
            CursorIcon::PointingHand,
            CursorIcon::Crosshair,
            CursorIcon::ResizeNwSe,
            CursorIcon::None,
        ] {
            assert!(image(&context, 1.0, icon).is_none());
        }
        let first = image(&context, 1.0, CursorIcon::Grab);
        if cfg!(target_os = "windows") {
            let first = first.unwrap();
            let again = image(&context, 1.0, CursorIcon::Grab).unwrap();
            assert!(std::sync::Arc::ptr_eq(&first.rgba, &again.rgba));
            let closed = image(&context, 1.0, CursorIcon::Grabbing).unwrap();
            assert_ne!(first.rgba, closed.rgba);
            let larger = image(&context, 2.0, CursorIcon::Grab).unwrap();
            assert_eq!(larger.size, [48, 48]);
            assert!(!std::sync::Arc::ptr_eq(&first.rgba, &larger.rgba));
        } else {
            assert!(first.is_none());
        }
    }
}
