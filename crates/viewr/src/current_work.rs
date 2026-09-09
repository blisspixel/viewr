//! Pure preflight policy for concurrent foreground work.
//!
//! The event loop owns workers and job handles. This module owns only the
//! priority order of blockers and the user-visible wait copy derived from
//! immutable facts.

use crate::curation_state::CurationKind;
use crate::locale::Language;

/// Wait sentence. `{work}` is the running work, `{action}` the attempted one.
const WAIT_TEMPLATE: &str = "Wait for {work} to finish {action}";
/// Placeholder for the running work.
const WORK_PLACEHOLDER: &str = "{work}";
/// Placeholder for the attempted action clause.
const ACTION_PLACEHOLDER: &str = "{action}";
/// Spot Heal needs a settled source, not a last good frame.
const WAIT_FOR_OPEN_BEFORE_HEAL: &str =
    "Wait for the image to finish opening before using Spot Heal";
/// Spot Heal after a failed open.
const RETRY_LOAD_BEFORE_HEAL: &str = "Retry the failed image load before using Spot Heal";
/// Undo with nothing to restore.
pub(crate) const NOTHING_TO_RESTORE: &str = "Nothing to restore from Trash";

/// What the user tried to do while other work held the foreground.
///
/// A typed action keeps the wait sentence assembled from two cataloged halves.
/// A free-form English fragment could not be translated, because each language
/// needs its own connective and verb form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BlockedAction {
    ApplyCrop,
    Browse,
    ChangeCrop,
    ChangeRating,
    ChangeRatingFilter,
    ChangeSpotHeal,
    FlipImage,
    OpenAnotherFolder,
    OpenAnotherImage,
    OpenInAnotherApp,
    PermanentlyDelete,
    RedoEdit,
    RefreshHealSource,
    ReloadFile,
    RestoreFromTrash,
    RetryImageLoad,
    RotateImage,
    SaveCopy,
    StartHealStroke,
    Trash,
    UndoEdit,
}

impl BlockedAction {
    /// Cataloged clause that completes the wait sentence, connective included.
    const fn source(self) -> &'static str {
        match self {
            Self::ApplyCrop => "before applying the crop",
            Self::Browse => "before browsing to another image",
            Self::ChangeCrop => "before changing Crop",
            Self::ChangeRating => "before changing the rating",
            Self::ChangeRatingFilter => "before changing the rating filter",
            Self::ChangeSpotHeal => "before changing Spot Heal",
            Self::FlipImage => "before flipping the image",
            Self::OpenAnotherFolder => "before opening another folder",
            Self::OpenAnotherImage => "before opening another image",
            Self::OpenInAnotherApp => "before opening the source in another app",
            Self::PermanentlyDelete => "before permanently deleting this file",
            Self::RedoEdit => "before redoing an edit",
            Self::RefreshHealSource => "before refreshing the heal source",
            Self::ReloadFile => "before reloading this file",
            Self::RestoreFromTrash => "before restoring files from Trash",
            Self::RetryImageLoad => "before retrying the image load",
            Self::RotateImage => "before rotating the image",
            Self::SaveCopy => "before saving a copy",
            Self::StartHealStroke => "before starting a spot-heal stroke",
            Self::Trash => "before moving this file to Trash",
            Self::UndoEdit => "before undoing an edit",
        }
    }
}

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
pub(crate) fn spot_heal_source_blocker(
    language: Language,
    image_open_in_progress: bool,
    image_open_failed: bool,
) -> Option<&'static str> {
    if image_open_in_progress {
        Some(language.text(WAIT_FOR_OPEN_BEFORE_HEAL))
    } else if image_open_failed {
        Some(language.text(RETRY_LOAD_BEFORE_HEAL))
    } else {
        None
    }
}

/// Wait sentence assembled from the running work and the attempted action.
///
/// Both halves are separate catalog entries and the action clause carries its
/// own connective, so a language can order and inflect the sentence its own
/// way instead of receiving English word order with translated words in it.
#[must_use]
pub(crate) fn blocked_action_message(
    language: Language,
    action: BlockedAction,
    blocker: CurrentWork,
) -> String {
    language
        .text(WAIT_TEMPLATE)
        .replace(WORK_PLACEHOLDER, language.text(work_source(blocker)))
        .replace(ACTION_PLACEHOLDER, language.text(action.source()))
}

const fn work_source(blocker: CurrentWork) -> &'static str {
    match blocker {
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
    }
}

#[must_use]
pub(crate) fn curation_action_preflight(
    language: Language,
    active: Option<CurationKind>,
    has_work: bool,
    action: BlockedAction,
    empty_message: &'static str,
) -> Option<String> {
    if let Some(kind) = active {
        Some(blocked_action_message(
            language,
            action,
            curation_work(kind),
        ))
    } else if has_work {
        None
    } else {
        Some(language.text(empty_message).to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::is_cataloged;

    /// Assertions state the English sentence, so the seam is called with it.
    const EN: Language = Language::English;
    const TRANSLATED: [Language; 3] = [Language::Spanish, Language::French, Language::German];
    const ACTIONS: [BlockedAction; 21] = [
        BlockedAction::ApplyCrop,
        BlockedAction::Browse,
        BlockedAction::ChangeCrop,
        BlockedAction::ChangeRating,
        BlockedAction::ChangeRatingFilter,
        BlockedAction::ChangeSpotHeal,
        BlockedAction::FlipImage,
        BlockedAction::OpenAnotherFolder,
        BlockedAction::OpenAnotherImage,
        BlockedAction::OpenInAnotherApp,
        BlockedAction::PermanentlyDelete,
        BlockedAction::RedoEdit,
        BlockedAction::RefreshHealSource,
        BlockedAction::ReloadFile,
        BlockedAction::RestoreFromTrash,
        BlockedAction::RetryImageLoad,
        BlockedAction::RotateImage,
        BlockedAction::SaveCopy,
        BlockedAction::StartHealStroke,
        BlockedAction::Trash,
        BlockedAction::UndoEdit,
    ];
    const WORKS: [CurrentWork; 10] = [
        CurrentWork::TrashMove,
        CurrentWork::PermanentDelete,
        CurrentWork::TrashRestore,
        CurrentWork::SourceVerification,
        CurrentWork::FolderScan,
        CurrentWork::ImagePreparation,
        CurrentWork::Crop,
        CurrentWork::Save,
        CurrentWork::SpotHeal,
        CurrentWork::RatingWrite,
    ];

    /// Every half of the wait sentence has all four languages.
    #[test]
    fn every_wait_string_is_cataloged() {
        let mut sources = vec![
            WAIT_TEMPLATE,
            WAIT_FOR_OPEN_BEFORE_HEAL,
            RETRY_LOAD_BEFORE_HEAL,
            NOTHING_TO_RESTORE,
        ];
        sources.extend(ACTIONS.map(BlockedAction::source));
        sources.extend(WORKS.map(work_source));
        let missing: Vec<_> = sources
            .into_iter()
            .filter(|source| !is_cataloged(source))
            .collect();
        assert!(missing.is_empty(), "uncataloged wait copy: {missing:?}");
    }

    /// Each of the 210 wait sentences is translated and fully substituted.
    #[test]
    fn wait_sentences_are_translated_and_fully_substituted() {
        for action in ACTIONS {
            for work in WORKS {
                let english = blocked_action_message(EN, action, work);
                for language in TRANSLATED {
                    let translated = blocked_action_message(language, action, work);
                    assert!(
                        !translated.contains('{') && !translated.contains('}'),
                        "unsubstituted placeholder in {language:?}: {translated}"
                    );
                    assert_ne!(
                        english, translated,
                        "{language:?} fell back to English: {english}"
                    );
                }
            }
        }
    }

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
            blocked_action_message(EN, BlockedAction::Trash, CurrentWork::FolderScan),
            "Wait for the folder scan to finish before moving this file to Trash"
        );
        assert_eq!(
            blocked_action_message(EN, BlockedAction::Trash, CurrentWork::SpotHeal),
            "Wait for Spot Heal to finish before moving this file to Trash"
        );
        assert_eq!(
            blocked_action_message(EN, BlockedAction::SaveCopy, CurrentWork::SourceVerification),
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
            spot_heal_source_blocker(EN, true, true),
            Some("Wait for the image to finish opening before using Spot Heal")
        );
        assert_eq!(
            spot_heal_source_blocker(EN, false, true),
            Some("Retry the failed image load before using Spot Heal")
        );
        assert_eq!(spot_heal_source_blocker(EN, false, false), None);
    }

    #[test]
    fn repeated_trash_ignores_only_the_serialized_trash_owner() {
        assert_eq!(
            trash_submission_work_blocker([Some(CurrentWork::FolderScan), None, None, None,]),
            Some(CurrentWork::FolderScan)
        );
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
                EN,
                Some(CurationKind::Restore),
                false,
                BlockedAction::RestoreFromTrash,
                NOTHING_TO_RESTORE,
            ),
            Some(
                "Wait for the Trash restore to finish before restoring files from Trash".to_owned()
            )
        );
        assert_eq!(
            curation_action_preflight(
                EN,
                None,
                false,
                BlockedAction::RestoreFromTrash,
                NOTHING_TO_RESTORE,
            ),
            Some("Nothing to restore from Trash".to_owned())
        );
        assert_eq!(
            curation_action_preflight(
                EN,
                None,
                true,
                BlockedAction::RestoreFromTrash,
                NOTHING_TO_RESTORE
            ),
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
