//! Pure preflight policy for concurrent foreground work.
//!
//! The event loop owns workers and job handles. This module owns only the
//! priority order of blockers and the user-visible wait copy derived from
//! immutable facts.

use crate::curation_state::CurationKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CurrentWork {
    TrashMove,
    PermanentDelete,
    TrashRestore,
    #[cfg(target_os = "windows")]
    SourceVerification,
    FolderScan,
    ImagePreparation,
    Crop,
    Save,
    SpotHeal,
    RatingWrite,
}

#[must_use]
pub(crate) const fn curation_work(kind: CurationKind) -> CurrentWork {
    match kind {
        CurationKind::Trash => CurrentWork::TrashMove,
        CurationKind::PermanentDelete => CurrentWork::PermanentDelete,
        CurationKind::Restore => CurrentWork::TrashRestore,
    }
}

#[must_use]
pub(crate) fn image_preparation_work(
    foreground_load: bool,
    preview_preparation: bool,
) -> Option<CurrentWork> {
    (foreground_load || preview_preparation).then_some(CurrentWork::ImagePreparation)
}

#[must_use]
pub(crate) fn crop_work(selection_active: bool, worker_active: bool) -> Option<CurrentWork> {
    (selection_active || worker_active).then_some(CurrentWork::Crop)
}

#[must_use]
pub(crate) fn current_work_blocker<const N: usize>(
    work: [Option<CurrentWork>; N],
) -> Option<CurrentWork> {
    work.into_iter().flatten().next()
}

/// Folder browsing may replace an in-flight decode. The last good frame stays
/// until the newly selected image is ready.
#[must_use]
pub(crate) const fn blocks_browse(work: CurrentWork) -> bool {
    !matches!(work, CurrentWork::ImagePreparation)
}

/// Select the first browse blocker after ignoring replaceable image preparation.
#[must_use]
pub(crate) fn browse_work_blocker<const N: usize>(
    work: [Option<CurrentWork>; N],
) -> Option<CurrentWork> {
    current_work_blocker(work.map(|entry| entry.filter(|active| blocks_browse(*active))))
}

/// Spot Heal needs a settled selected source even when a last good frame remains visible.
#[must_use]
pub(crate) const fn spot_heal_source_blocker(
    image_open_in_progress: bool,
    image_open_failed: bool,
) -> Option<&'static str> {
    if image_open_in_progress {
        Some("Wait for the image to finish opening before using Spot Heal")
    } else if image_open_failed {
        Some("Retry the failed image load before using Spot Heal")
    } else {
        None
    }
}

#[must_use]
pub(crate) fn blocked_action_message(action: &str, blocker: CurrentWork) -> String {
    let work = match blocker {
        CurrentWork::TrashMove => "the move to Trash",
        CurrentWork::PermanentDelete => "the permanent delete",
        CurrentWork::TrashRestore => "the Trash restore",
        #[cfg(target_os = "windows")]
        CurrentWork::SourceVerification => "source verification",
        CurrentWork::FolderScan => "the folder scan",
        CurrentWork::ImagePreparation => "image preparation",
        CurrentWork::Crop => "the crop",
        CurrentWork::Save => "Save As",
        CurrentWork::SpotHeal => "Spot Heal",
        CurrentWork::RatingWrite => "the rating update",
    };
    format!("Wait for {work} to finish before {action}")
}

#[must_use]
pub(crate) fn curation_action_preflight(
    active: Option<CurationKind>,
    has_work: bool,
    action: &str,
    empty_message: &str,
) -> Option<String> {
    if let Some(kind) = active {
        Some(blocked_action_message(action, curation_work(kind)))
    } else if has_work {
        None
    } else {
        Some(empty_message.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_action_copy_is_specific_and_prioritized() {
        assert_eq!(crop_work(true, false), Some(CurrentWork::Crop));
        assert_eq!(crop_work(false, true), Some(CurrentWork::Crop));
        assert_eq!(crop_work(false, false), None);
        assert_eq!(
            current_work_blocker([
                None,
                None,
                image_preparation_work(true, false),
                Some(CurrentWork::Crop),
                Some(CurrentWork::Save),
                Some(CurrentWork::SpotHeal),
            ]),
            Some(CurrentWork::ImagePreparation)
        );
        assert_eq!(
            current_work_blocker([
                None,
                None,
                image_preparation_work(false, true),
                Some(CurrentWork::Crop),
                Some(CurrentWork::Save),
                Some(CurrentWork::SpotHeal),
            ]),
            Some(CurrentWork::ImagePreparation)
        );
        assert_eq!(
            current_work_blocker([
                None,
                None,
                None,
                Some(CurrentWork::Crop),
                Some(CurrentWork::Save),
                Some(CurrentWork::SpotHeal),
            ]),
            Some(CurrentWork::Crop)
        );
        assert_eq!(
            current_work_blocker([
                None,
                None,
                None,
                None,
                Some(CurrentWork::Save),
                Some(CurrentWork::SpotHeal),
            ]),
            Some(CurrentWork::Save)
        );
        assert_eq!(
            current_work_blocker([None, None, None, None, None, Some(CurrentWork::SpotHeal)]),
            Some(CurrentWork::SpotHeal)
        );
        assert_eq!(
            current_work_blocker([
                Some(CurrentWork::TrashRestore),
                Some(CurrentWork::FolderScan),
                None,
                None,
                None,
                None,
            ]),
            Some(CurrentWork::TrashRestore)
        );
        assert_eq!(
            current_work_blocker([None, Some(CurrentWork::FolderScan), None, None, None, None,]),
            Some(CurrentWork::FolderScan)
        );
        assert_eq!(
            current_work_blocker([None, None, None, None, None, None]),
            None
        );
        assert_eq!(
            blocked_action_message("moving this file to Trash", CurrentWork::SpotHeal),
            "Wait for Spot Heal to finish before moving this file to Trash"
        );
        #[cfg(target_os = "windows")]
        assert_eq!(
            blocked_action_message("saving a copy", CurrentWork::SourceVerification),
            "Wait for source verification to finish before saving a copy"
        );
    }

    #[test]
    fn browse_and_spot_heal_preflight_inspect_every_relevant_fact() {
        assert!(!blocks_browse(CurrentWork::ImagePreparation));
        assert!(blocks_browse(CurrentWork::Crop));
        assert!(blocks_browse(CurrentWork::FolderScan));
        assert!(blocks_browse(CurrentWork::SpotHeal));
        assert_eq!(
            browse_work_blocker([
                Some(CurrentWork::ImagePreparation),
                Some(CurrentWork::Crop),
                Some(CurrentWork::Save),
            ]),
            Some(CurrentWork::Crop)
        );
        assert_eq!(
            browse_work_blocker([Some(CurrentWork::ImagePreparation), None]),
            None
        );
        assert_eq!(
            spot_heal_source_blocker(true, true),
            Some("Wait for the image to finish opening before using Spot Heal")
        );
        assert_eq!(
            spot_heal_source_blocker(false, true),
            Some("Retry the failed image load before using Spot Heal")
        );
        assert_eq!(spot_heal_source_blocker(false, false), None);
    }

    #[test]
    fn curation_preflight_follows_app_ownership() {
        assert_eq!(
            curation_action_preflight(
                Some(CurationKind::Restore),
                false,
                "restoring files from Trash",
                "Nothing to restore from Trash",
            ),
            Some(
                "Wait for the Trash restore to finish before restoring files from Trash".to_owned()
            )
        );
        assert_eq!(
            curation_action_preflight(
                None,
                false,
                "restoring files from Trash",
                "Nothing to restore from Trash",
            ),
            Some("Nothing to restore from Trash".to_owned())
        );
        assert_eq!(
            curation_action_preflight(None, true, "restoring files from Trash", "unused",),
            None
        );
    }

    #[test]
    fn curation_kind_maps_to_exclusive_work() {
        assert_eq!(curation_work(CurationKind::Trash), CurrentWork::TrashMove);
        assert_eq!(
            curation_work(CurationKind::PermanentDelete),
            CurrentWork::PermanentDelete
        );
        assert_eq!(
            curation_work(CurationKind::Restore),
            CurrentWork::TrashRestore
        );
    }
}
