//! Pure descriptor policy consumed by the wgpu resource adapter.
//!
//! These functions select or validate plain GPU descriptors. They create no
//! device resource and own no renderer or event-loop state.

use crate::color::OutputColorTransform;
use crate::error::Error;
use crate::heal::ImagePatch;
use crate::theme::Palette;
use crate::view::Placement;

/// The uniform buffer size: 16 bytes each for scale/offset, UV matrix, and crop.
pub(super) const PLACEMENT_BYTES: u64 = 48;

pub(super) struct PatchUpload {
    pub(super) origin: wgpu::Origin3d,
    pub(super) extent: wgpu::Extent3d,
    pub(super) bytes_per_row: u32,
}

pub(super) fn validate_patch_upload(
    source_size: (u32, u32),
    texture_size: (u32, u32),
    patch: &ImagePatch,
) -> Option<PatchUpload> {
    if source_size != texture_size {
        return None;
    }
    let bounds = patch.bounds;
    let right = bounds.x.checked_add(bounds.width)?;
    let bottom = bounds.y.checked_add(bounds.height)?;
    let expected_bytes = viewr_protocol::checked_rgba_len(bounds.width, bounds.height).ok()?;
    if right > texture_size.0 || bottom > texture_size.1 || patch.rgba.len() != expected_bytes {
        return None;
    }
    Some(PatchUpload {
        origin: wgpu::Origin3d {
            x: bounds.x,
            y: bounds.y,
            z: 0,
        },
        extent: wgpu::Extent3d {
            width: bounds.width,
            height: bounds.height,
            depth_or_array_layers: 1,
        },
        bytes_per_row: bounds.width.checked_mul(4)?,
    })
}

pub(super) fn select_srgb_surface_format(
    formats: &[wgpu::TextureFormat],
) -> Result<(wgpu::TextureFormat, OutputColorTransform), Error> {
    formats
        .iter()
        .copied()
        .find(wgpu::TextureFormat::is_srgb)
        .map(|format| (format, OutputColorTransform::SRGB_TO_SRGB))
        .ok_or_else(|| Error::Gpu("display surface does not support sRGB presentation".into()))
}

/// Pack `Placement` into three consecutive 16-byte shader vectors.
pub(super) fn pack_placement(placement: &Placement) -> [u8; 48] {
    let mut bytes = [0; 48];
    bytes[0..4].copy_from_slice(&placement.scale[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&placement.scale[1].to_ne_bytes());
    bytes[8..12].copy_from_slice(&placement.offset[0].to_ne_bytes());
    bytes[12..16].copy_from_slice(&placement.offset[1].to_ne_bytes());
    bytes[16..20].copy_from_slice(&placement.uv_matrix[0].to_ne_bytes());
    bytes[20..24].copy_from_slice(&placement.uv_matrix[1].to_ne_bytes());
    bytes[24..28].copy_from_slice(&placement.uv_matrix[2].to_ne_bytes());
    bytes[28..32].copy_from_slice(&placement.uv_matrix[3].to_ne_bytes());
    bytes[32..36].copy_from_slice(&placement.crop_rect[0].to_ne_bytes());
    bytes[36..40].copy_from_slice(&placement.crop_rect[1].to_ne_bytes());
    bytes[40..44].copy_from_slice(&placement.crop_rect[2].to_ne_bytes());
    bytes[44..48].copy_from_slice(&placement.crop_rect[3].to_ne_bytes());
    bytes
}

/// Convert a palette background into the renderer's clear-color descriptor.
pub(super) fn palette_to_color(palette: Palette) -> wgpu::Color {
    let [r, g, b, a] = palette.background;
    wgpu::Color { r, g, b, a }
}

#[cfg(test)]
mod tests {
    use super::{
        PLACEMENT_BYTES, pack_placement, palette_to_color, select_srgb_surface_format,
        validate_patch_upload,
    };
    use crate::color::{OutputColorSpace, WorkingColorEncoding};
    use crate::edit::Rect;
    use crate::heal::ImagePatch;
    use crate::theme::{self, Mode};
    use crate::view::Placement;

    fn patch(bounds: Rect, byte_len: usize) -> ImagePatch {
        ImagePatch {
            bounds,
            rgba: vec![3; byte_len],
        }
    }

    fn assert_patch_rejected(bounds: Rect, byte_len: usize) {
        assert!(validate_patch_upload((4, 6), (4, 6), &patch(bounds, byte_len)).is_none());
    }

    #[test]
    fn placement_packing_preserves_all_shader_fields_in_order() {
        let placement = Placement {
            scale: [0.5, 0.25],
            offset: [-0.1, 0.2],
            uv_matrix: [1.0, 2.0, 3.0, 4.0],
            crop_rect: [0.125, 0.375, 0.625, 0.875],
        };
        let expected: [f32; 12] = [
            0.5, 0.25, -0.1, 0.2, 1.0, 2.0, 3.0, 4.0, 0.125, 0.375, 0.625, 0.875,
        ];
        let bytes = pack_placement(&placement);
        assert_eq!(PLACEMENT_BYTES, bytes.len() as u64);

        for (actual, expected) in bytes.chunks_exact(4).zip(expected) {
            assert_eq!(
                f32::from_ne_bytes(actual.try_into().unwrap()).to_bits(),
                expected.to_bits()
            );
        }
    }

    #[test]
    fn surface_selection_uses_the_first_explicit_srgb_format() {
        let (format, transform) = select_srgb_surface_format(&[
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ])
        .unwrap();

        assert_eq!(format, wgpu::TextureFormat::Rgba8UnormSrgb);
        assert_eq!(transform.source(), WorkingColorEncoding::SRGB_RGBA8);
        assert_eq!(transform.destination(), OutputColorSpace::Srgb);
        assert!(select_srgb_surface_format(&[wgpu::TextureFormat::Bgra8Unorm]).is_err());
        assert!(select_srgb_surface_format(&[]).is_err());
    }

    #[test]
    fn patch_upload_requires_exact_full_resolution_geometry_and_bytes() {
        let valid = patch(
            Rect {
                x: 1,
                y: 2,
                width: 2,
                height: 3,
            },
            24,
        );
        let upload = validate_patch_upload((4, 6), (4, 6), &valid).unwrap();
        assert_eq!(upload.origin, wgpu::Origin3d { x: 1, y: 2, z: 0 });
        assert_eq!(
            upload.extent,
            wgpu::Extent3d {
                width: 2,
                height: 3,
                depth_or_array_layers: 1,
            }
        );
        assert_eq!(upload.bytes_per_row, 8);

        let inclusive_edges = patch(
            Rect {
                x: 2,
                y: 3,
                width: 2,
                height: 3,
            },
            24,
        );
        assert!(validate_patch_upload((4, 6), (4, 6), &inclusive_edges).is_some());

        assert!(validate_patch_upload((4, 6), (2, 3), &valid).is_none());
        assert_patch_rejected(
            Rect {
                x: 3,
                y: 2,
                width: 2,
                height: 3,
            },
            24,
        );
        assert_patch_rejected(
            Rect {
                x: 1,
                y: 4,
                width: 2,
                height: 3,
            },
            24,
        );
        assert_patch_rejected(
            Rect {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 1,
            },
            8,
        );
        assert_patch_rejected(
            Rect {
                x: 0,
                y: u32::MAX,
                width: 1,
                height: 2,
            },
            8,
        );
        assert_patch_rejected(
            Rect {
                x: 1,
                y: 2,
                width: 2,
                height: 3,
            },
            23,
        );
        assert_patch_rejected(
            Rect {
                x: 1,
                y: 2,
                width: 2,
                height: 3,
            },
            25,
        );
        assert_patch_rejected(
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            0,
        );
    }

    #[test]
    fn palette_mapping_preserves_every_background_channel() {
        let palette = theme::palette_for(Mode::Dark);
        let expected = palette.background;
        let color = palette_to_color(palette);
        for (actual, expected) in [color.r, color.g, color.b, color.a]
            .into_iter()
            .zip(expected)
        {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }
}
