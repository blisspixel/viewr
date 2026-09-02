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
    SourceVerification,
    FolderScan,
    ImagePreparation,
    Crop,
    Save,
    SpotHeal,
    RatingWrite,
}

/// An action may operate inside one selected edit mode while every live
/// interaction and every other foreground owner remains exclusive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActiveModeAllowance {
    None,
    Crop,
    SpotHeal,
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
pub(crate) const fn crop_work(
    mode_active: bool,
    interaction_active: bool,
    worker_active: bool,
    allowance: ActiveModeAllowance,
) -> Option<CurrentWork> {
    if worker_active
        || interaction_active
        || (mode_active && !matches!(allowance, ActiveModeAllowance::Crop))
    {
        Some(CurrentWork::Crop)
    } else {
        None
    }
}

/// Selected modes may be allowed for their own safe commands, but a live stroke
/// or worker remains exclusive. An idle Spot Heal tool is therefore not
/// unfinished edit-history work by itself.
#[must_use]
pub(crate) const fn spot_heal_work(
    mode_active: bool,
    worker_active: bool,
    stroke_active: bool,
    allowance: ActiveModeAllowance,
) -> Option<CurrentWork> {
    if worker_active
        || stroke_active
        || (mode_active && !matches!(allowance, ActiveModeAllowance::SpotHeal))
    {
        Some(CurrentWork::SpotHeal)
    } else {
        None
    }
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

/// A running move to Trash may accept another fully presented source. The
/// platform operations remain serialized by the application-owned queue; all
/// other work retains its normal exclusivity.
#[must_use]
pub(crate) fn trash_submission_work_blocker<const N: usize>(
    work: [Option<CurrentWork>; N],
) -> Option<CurrentWork> {
    current_work_blocker(
        work.map(|entry| entry.filter(|active| !matches!(active, CurrentWork::TrashMove))),
    )
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
        assert_eq!(
            crop_work(true, false, false, ActiveModeAllowance::None),
            Some(CurrentWork::Crop)
        );
        assert_eq!(
            crop_work(false, false, true, ActiveModeAllowance::None),
            Some(CurrentWork::Crop)
        );
        assert_eq!(
            crop_work(false, false, false, ActiveModeAllowance::None),
            None
        );
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
    fn repeated_trash_ignores_only_the_serialized_trash_owner() {
        assert_eq!(
            trash_submission_work_blocker([Some(CurrentWork::TrashMove), None, None, None,]),
            None
        );
        assert_eq!(
            trash_submission_work_blocker([
                Some(CurrentWork::TrashMove),
                Some(CurrentWork::ImagePreparation),
                Some(CurrentWork::Save),
                None,
            ]),
            Some(CurrentWork::ImagePreparation)
        );
        assert_eq!(
            trash_submission_work_blocker([Some(CurrentWork::PermanentDelete), None, None, None,]),
            Some(CurrentWork::PermanentDelete)
        );
    }

    #[test]
    fn mode_allowances_never_allow_live_interactions_or_workers() {
        assert_eq!(
            crop_work(true, false, false, ActiveModeAllowance::Crop),
            None
        );
        assert_eq!(
            crop_work(true, true, false, ActiveModeAllowance::Crop),
            Some(CurrentWork::Crop)
        );
        assert_eq!(
            crop_work(true, false, true, ActiveModeAllowance::Crop),
            Some(CurrentWork::Crop)
        );
        assert_eq!(
            crop_work(true, false, false, ActiveModeAllowance::SpotHeal),
            Some(CurrentWork::Crop)
        );

        assert_eq!(
            spot_heal_work(true, false, false, ActiveModeAllowance::SpotHeal),
            None
        );
        assert_eq!(
            spot_heal_work(true, true, false, ActiveModeAllowance::SpotHeal),
            Some(CurrentWork::SpotHeal)
        );
        assert_eq!(
            spot_heal_work(true, false, true, ActiveModeAllowance::SpotHeal),
            Some(CurrentWork::SpotHeal)
        );
        assert_eq!(
            spot_heal_work(true, false, false, ActiveModeAllowance::Crop),
            Some(CurrentWork::SpotHeal)
        );
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
