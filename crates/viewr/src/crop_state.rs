//! Pure lifecycle policy for crop recovery and crop start readiness.
//!
//! The event loop owns the crop worker, transform mutation, and GPU preview
//! presentation. This module owns only deterministic recovery matching, blocker
//! priority, and user-visible failure copy derived from immutable facts.

use std::path::Path;
use std::sync::Arc;

use crate::decode::DecodedImage;
use crate::locale::Language;

/// Persistent crop recovery status after an unexpected crop stop.
pub(crate) const CROP_RECOVERY_STATUS: &str =
    "Crop stopped unexpectedly. Close and reopen viewr before cropping again.";
/// Persistent preview-recovery status after the display-preview executor is lost.
pub(crate) const PREVIEW_RECOVERY_STATUS: &str = "Display preview preparation stopped unexpectedly. Close and reopen viewr before opening another over-limit image or cropping again.";

const CROP_FAILURE_RESTORED: &str =
    "Crop was not applied. Original image unchanged; selection restored. Press Enter to try again.";
const CROP_FAILURE_CHANGED: &str = "Crop was not applied because the image changed.";
const CROP_DISCONNECT_RESTORED: &str = "Crop stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again.";
const CROP_DISCONNECT_CHANGED: &str = "Crop stopped unexpectedly after the image changed. Close and reopen viewr before cropping again.";
const CROP_PREVIEW_DISCONNECT_RESTORED: &str = "Crop could not finish because display preview preparation stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again.";
const CROP_PREVIEW_DISCONNECT_CHANGED: &str = "Display preview preparation stopped unexpectedly after the image changed. Close and reopen viewr before cropping again.";
const WAIT_FOR_OPEN_BEFORE_CROP: &str = "Wait for the image to finish opening before cropping";
const RETRY_LOAD_BEFORE_CROP: &str = "Retry the failed image load before cropping";

#[cfg(test)]
const ALL: &[&str] = &[
    CROP_RECOVERY_STATUS,
    PREVIEW_RECOVERY_STATUS,
    CROP_FAILURE_RESTORED,
    CROP_FAILURE_CHANGED,
    CROP_DISCONNECT_RESTORED,
    CROP_DISCONNECT_CHANGED,
    CROP_PREVIEW_DISCONNECT_RESTORED,
    CROP_PREVIEW_DISCONNECT_CHANGED,
    WAIT_FOR_OPEN_BEFORE_CROP,
    RETRY_LOAD_BEFORE_CROP,
];

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
pub(crate) fn crop_failure_message(language: Language, selection_restored: bool) -> &'static str {
    language.text(if selection_restored {
        CROP_FAILURE_RESTORED
    } else {
        CROP_FAILURE_CHANGED
    })
}

#[must_use]
pub(crate) fn crop_disconnect_message(
    language: Language,
    selection_restored: bool,
) -> &'static str {
    language.text(if selection_restored {
        CROP_DISCONNECT_RESTORED
    } else {
        CROP_DISCONNECT_CHANGED
    })
}

#[must_use]
pub(crate) fn crop_preview_disconnect_message(
    language: Language,
    selection_restored: bool,
) -> &'static str {
    language.text(if selection_restored {
        CROP_PREVIEW_DISCONNECT_RESTORED
    } else {
        CROP_PREVIEW_DISCONNECT_CHANGED
    })
}

#[must_use]
pub(crate) fn crop_recovery_blocker(
    language: Language,
    crop_recovery_unsettled: bool,
    preview_recovery_unsettled: bool,
) -> Option<&'static str> {
    if crop_recovery_unsettled {
        Some(language.text(CROP_RECOVERY_STATUS))
    } else if preview_recovery_unsettled {
        Some(language.text(PREVIEW_RECOVERY_STATUS))
    } else {
        None
    }
}

#[must_use]
pub(crate) fn preview_retry_blocker(
    language: Language,
    preview_load_retry_blocked: bool,
) -> Option<&'static str> {
    if preview_load_retry_blocked {
        Some(language.text(PREVIEW_RECOVERY_STATUS))
    } else {
        None
    }
}

#[must_use]
pub(crate) fn crop_source_blocker(
    language: Language,
    image_open_in_progress: bool,
    image_open_failed: bool,
) -> Option<&'static str> {
    if image_open_in_progress {
        Some(language.text(WAIT_FOR_OPEN_BEFORE_CROP))
    } else if image_open_failed {
        Some(language.text(RETRY_LOAD_BEFORE_CROP))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::is_cataloged;
    use std::path::PathBuf;

    const EN: Language = Language::English;
    const TRANSLATED: [Language; 3] = [Language::Spanish, Language::French, Language::German];

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
    fn every_crop_message_is_cataloged() {
        let missing: Vec<_> = ALL
            .iter()
            .copied()
            .filter(|source| !is_cataloged(source))
            .collect();
        assert!(missing.is_empty(), "uncataloged crop copy: {missing:?}");
    }

    #[test]
    fn crop_copy_is_translated() {
        for language in TRANSLATED {
            for restored in [false, true] {
                assert_ne!(
                    crop_failure_message(language, restored),
                    crop_failure_message(EN, restored)
                );
                assert_ne!(
                    crop_disconnect_message(language, restored),
                    crop_disconnect_message(EN, restored)
                );
                assert_ne!(
                    crop_preview_disconnect_message(language, restored),
                    crop_preview_disconnect_message(EN, restored)
                );
            }
            assert_ne!(
                crop_recovery_blocker(language, true, false),
                crop_recovery_blocker(EN, true, false)
            );
            assert_ne!(
                crop_source_blocker(language, true, false),
                crop_source_blocker(EN, true, false)
            );
        }
    }

    #[test]
    fn crop_failure_copy_distinguishes_restored_selection() {
        assert_eq!(
            crop_failure_message(EN, true),
            "Crop was not applied. Original image unchanged; selection restored. Press Enter to try again."
        );
        assert_eq!(
            crop_failure_message(EN, false),
            "Crop was not applied because the image changed."
        );
    }

    #[test]
    fn crop_disconnect_copy_requires_restart_without_promising_retry() {
        assert_eq!(
            crop_disconnect_message(EN, true),
            "Crop stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again."
        );
        assert_eq!(
            crop_disconnect_message(EN, false),
            "Crop stopped unexpectedly after the image changed. Close and reopen viewr before cropping again."
        );
    }

    #[test]
    fn crop_preview_disconnect_copy_and_recovery_priority_are_truthful() {
        assert_eq!(
            crop_preview_disconnect_message(EN, true),
            "Crop could not finish because display preview preparation stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again."
        );
        assert_eq!(
            crop_preview_disconnect_message(EN, false),
            "Display preview preparation stopped unexpectedly after the image changed. Close and reopen viewr before cropping again."
        );
        assert_eq!(
            crop_recovery_blocker(EN, true, true),
            Some(CROP_RECOVERY_STATUS)
        );
        assert_eq!(
            crop_recovery_blocker(EN, false, true),
            Some(PREVIEW_RECOVERY_STATUS)
        );
        assert_eq!(crop_recovery_blocker(EN, false, false), None);
        assert_eq!(
            preview_retry_blocker(EN, true),
            Some(PREVIEW_RECOVERY_STATUS)
        );
        assert_eq!(preview_retry_blocker(EN, false), None);
    }

    #[test]
    fn crop_source_requires_a_settled_successful_image_load() {
        assert_eq!(
            crop_source_blocker(EN, true, false),
            Some("Wait for the image to finish opening before cropping")
        );
        assert_eq!(
            crop_source_blocker(EN, false, true),
            Some("Retry the failed image load before cropping")
        );
        assert_eq!(
            crop_source_blocker(EN, true, true),
            Some("Wait for the image to finish opening before cropping")
        );
        assert_eq!(crop_source_blocker(EN, false, false), None);
    }
}
