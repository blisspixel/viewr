//! Pure lifecycle policy for crop recovery and crop start readiness.
//!
//! The event loop owns the crop worker, transform mutation, and GPU preview
//! presentation. This module owns only deterministic recovery matching, blocker
//! priority, and user-visible failure copy derived from immutable facts.

use std::path::Path;
use std::sync::Arc;

use crate::decode::DecodedImage;
use crate::ui::{CROP_RECOVERY_STATUS, PREVIEW_RECOVERY_STATUS};

/// Identity facts that decide whether a crop recovery snapshot still applies.
#[derive(Clone, Copy)]
pub(crate) struct CropRecoveryIdentity<'a> {
    pub path: &'a Path,
    pub generation: u64,
    pub image: &'a Arc<DecodedImage>,
}

#[must_use]
pub(crate) fn crop_recovery_matches(
    recovery: CropRecoveryIdentity<'_>,
    current_generation: u64,
    selected_path: Option<&Path>,
    presented_path: Option<&Path>,
    current_image: Option<&Arc<DecodedImage>>,
) -> bool {
    crate::work_currency::presented_work_is_current(
        recovery.generation,
        recovery.path,
        current_generation,
        selected_path,
        presented_path,
    ) && current_image.is_some_and(|image| Arc::ptr_eq(image, recovery.image))
}

#[must_use]
pub(crate) const fn crop_failure_message(selection_restored: bool) -> &'static str {
    if selection_restored {
        "Crop was not applied. Original image unchanged; selection restored. Press Enter to try again."
    } else {
        "Crop was not applied because the image changed."
    }
}

#[must_use]
pub(crate) const fn crop_disconnect_message(selection_restored: bool) -> &'static str {
    if selection_restored {
        "Crop stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again."
    } else {
        "Crop stopped unexpectedly after the image changed. Close and reopen viewr before cropping again."
    }
}

#[must_use]
pub(crate) const fn crop_preview_disconnect_message(selection_restored: bool) -> &'static str {
    if selection_restored {
        "Crop could not finish because display preview preparation stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again."
    } else {
        "Display preview preparation stopped unexpectedly after the image changed. Close and reopen viewr before cropping again."
    }
}

#[must_use]
pub(crate) const fn crop_recovery_blocker(
    crop_recovery_unsettled: bool,
    preview_recovery_unsettled: bool,
) -> Option<&'static str> {
    if crop_recovery_unsettled {
        Some(CROP_RECOVERY_STATUS)
    } else if preview_recovery_unsettled {
        Some(PREVIEW_RECOVERY_STATUS)
    } else {
        None
    }
}

#[must_use]
pub(crate) const fn preview_retry_blocker(
    preview_load_retry_blocked: bool,
) -> Option<&'static str> {
    if preview_load_retry_blocked {
        Some(PREVIEW_RECOVERY_STATUS)
    } else {
        None
    }
}

#[must_use]
pub(crate) const fn crop_source_blocker(
    image_open_in_progress: bool,
    image_open_failed: bool,
) -> Option<&'static str> {
    if image_open_in_progress {
        Some("Wait for the image to finish opening before cropping")
    } else if image_open_failed {
        Some("Retry the failed image load before cropping")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_image() -> Arc<DecodedImage> {
        Arc::new(DecodedImage {
            rgba: vec![10, 20, 30, 255],
            width: 1,
            height: 1,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: crate::color::WorkingColorEncoding::SRGB_RGBA8,
        })
    }

    #[test]
    fn crop_recovery_requires_generation_path_and_exact_source_allocation() {
        let path = PathBuf::from("album").join("source.png");
        let source_image = sample_image();
        let same_pixels_different_allocation = Arc::new(DecodedImage {
            rgba: source_image.rgba.clone(),
            width: source_image.width,
            height: source_image.height,
            color_profile: source_image.color_profile,
            working_color: source_image.working_color,
        });
        let recovery = CropRecoveryIdentity {
            path: &path,
            generation: 42,
            image: &source_image,
        };

        assert!(crop_recovery_matches(
            recovery,
            42,
            Some(&path),
            Some(&path),
            Some(&source_image),
        ));
        assert!(!crop_recovery_matches(
            recovery,
            43,
            Some(&path),
            Some(&path),
            Some(&source_image),
        ));
        assert!(!crop_recovery_matches(
            recovery,
            42,
            Some(Path::new("album/other.png")),
            Some(&path),
            Some(&source_image),
        ));
        assert!(!crop_recovery_matches(
            recovery,
            42,
            Some(&path),
            Some(&path),
            Some(&same_pixels_different_allocation),
        ));
    }

    #[test]
    fn crop_failure_copy_distinguishes_restored_selection() {
        assert_eq!(
            crop_failure_message(true),
            "Crop was not applied. Original image unchanged; selection restored. Press Enter to try again."
        );
        assert_eq!(
            crop_failure_message(false),
            "Crop was not applied because the image changed."
        );
    }

    #[test]
    fn crop_disconnect_copy_requires_restart_without_promising_retry() {
        assert_eq!(
            crop_disconnect_message(true),
            "Crop stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again."
        );
        assert_eq!(
            crop_disconnect_message(false),
            "Crop stopped unexpectedly after the image changed. Close and reopen viewr before cropping again."
        );
    }

    #[test]
    fn crop_preview_disconnect_copy_and_recovery_priority_are_truthful() {
        assert_eq!(
            crop_preview_disconnect_message(true),
            "Crop could not finish because display preview preparation stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again."
        );
        assert_eq!(
            crop_preview_disconnect_message(false),
            "Display preview preparation stopped unexpectedly after the image changed. Close and reopen viewr before cropping again."
        );
        assert_eq!(
            crop_recovery_blocker(true, true),
            Some(CROP_RECOVERY_STATUS)
        );
        assert_eq!(
            crop_recovery_blocker(false, true),
            Some(PREVIEW_RECOVERY_STATUS)
        );
        assert_eq!(crop_recovery_blocker(false, false), None);
        assert_eq!(preview_retry_blocker(true), Some(PREVIEW_RECOVERY_STATUS));
        assert_eq!(preview_retry_blocker(false), None);
    }

    #[test]
    fn crop_source_requires_a_settled_successful_image_load() {
        assert_eq!(
            crop_source_blocker(true, false),
            Some("Wait for the image to finish opening before cropping")
        );
        assert_eq!(
            crop_source_blocker(false, true),
            Some("Retry the failed image load before cropping")
        );
        assert_eq!(
            crop_source_blocker(true, true),
            Some("Wait for the image to finish opening before cropping")
        );
        assert_eq!(crop_source_blocker(false, false), None);
    }
}
