//! Pure session file-coherence policy.
//!
//! The event loop owns the watcher thread, playlist mutation, decode, and
//! dialogs. This module decides what a coalesced observation of the current
//! source and folder means, and whether Open With may use a native chooser.
//! It writes no history and stores no paths.

use std::path::Path;

use crate::fs::ImageSourceMatch;

/// What the retained current source looks like at its selected pathname.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceObservation {
    /// Same object and version as the presented source.
    Unchanged,
    /// The pathname now names a different object or a newer version.
    Replaced,
    /// Nothing exists at the pathname.
    Missing,
    /// The pathname is now a link, directory, or other unsupported entry.
    Unsupported,
    /// Identity evidence could not be trusted.
    Unavailable,
}

/// What the browsed folder looks like at its retained identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FolderObservation {
    /// Same directory object and version.
    Unchanged,
    /// The directory object or its version changed, so membership may have changed.
    Changed,
    /// The folder cannot be observed.
    Unavailable,
}

/// In-memory edits that a silent reload would destroy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct UnsavedEdits {
    pub cropping: bool,
    pub applied_crop: bool,
    pub heal_pending: bool,
    pub rotated_or_flipped: bool,
}

/// Work that already owns the current source or playlist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct SessionBusy {
    pub loading: bool,
    pub saving: bool,
    pub healing: bool,
    pub cropping: bool,
    pub curating: bool,
    pub rating: bool,
}

/// Disk observations gathered away from the event loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CoherenceObservation {
    pub source: SourceObservation,
    pub folder: FolderObservation,
}

/// One coalesced observation of the current source and its folder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CoherenceFacts {
    pub source: SourceObservation,
    pub folder: FolderObservation,
    pub unsaved_edits: UnsavedEdits,
    pub busy: SessionBusy,
}

impl CoherenceFacts {
    #[must_use]
    pub(crate) const fn from_observation(
        observation: CoherenceObservation,
        unsaved_edits: UnsavedEdits,
        busy: SessionBusy,
    ) -> Self {
        Self {
            source: observation.source,
            folder: observation.folder,
            unsaved_edits,
            busy,
        }
    }
}

/// Visible action for one coalesced observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoherenceAction {
    /// Nothing user-visible should change.
    Ignore,
    /// Keep the last good frame and ask for an explicit F5.
    RemindReload,
    /// Decode the current path again without blanking the last good frame.
    ReloadCurrent,
    /// Reload the current path and rescan folder membership.
    ReloadAndRescan,
    /// Keep the last good frame; the selected path no longer names that file.
    CurrentGone,
    /// Rescan the folder without changing the presented pixels.
    RescanFolder,
    /// Rescan the folder and keep the last good frame while asking for F5.
    RemindAndRescan,
    /// Rescan the folder while the selected path no longer names the file.
    GoneAndRescan,
}

/// Whether this host may offer a user-mediated Open With chooser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpenWithAvailability {
    /// Windows `SHOpenWithDialog`, macOS application picker plus `NSWorkspace`,
    /// or the Linux desktop-portal `OpenURI` chooser.
    NativeChooser,
    /// This build has no supported chooser API.
    Unavailable,
}

/// What a gone-and-rescan folder refresh found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GoneRescanResult {
    /// The selected pathname is back in the folder, so reload it.
    Reappeared,
    /// The same object now lives at a new pathname.
    Renamed,
    /// The selected source is not in the folder.
    Missing,
}

/// Map a retained-source pathname comparison onto a coherence observation.
#[must_use]
pub(crate) const fn source_observation(source_match: ImageSourceMatch) -> SourceObservation {
    match source_match {
        ImageSourceMatch::Same => SourceObservation::Unchanged,
        ImageSourceMatch::Changed => SourceObservation::Replaced,
        ImageSourceMatch::Missing => SourceObservation::Missing,
        ImageSourceMatch::Unsupported => SourceObservation::Unsupported,
        ImageSourceMatch::Unavailable => SourceObservation::Unavailable,
    }
}

#[must_use]
pub(crate) const fn has_unsaved_edits(edits: UnsavedEdits) -> bool {
    edits.cropping || edits.applied_crop || edits.heal_pending || edits.rotated_or_flipped
}

/// A watch thread may start only after pixels, source identity, and the
/// presented pathname are the same committed selection.
#[must_use]
pub(crate) const fn watch_can_start(
    has_image: bool,
    has_source: bool,
    presented_matches_selected: bool,
) -> bool {
    has_image && has_source && presented_matches_selected
}

/// A pending observation may act only while the presented file is still the
/// path that watch thread was started against.
#[must_use]
pub(crate) fn watch_applies(loaded_path: Option<&Path>, watch_path: &Path) -> bool {
    loaded_path == Some(watch_path)
}

#[must_use]
pub(crate) const fn session_is_busy(busy: SessionBusy) -> bool {
    busy.loading || busy.saving || busy.healing || busy.cropping || busy.curating || busy.rating
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReloadStartBlocker {
    SpotHeal,
    Crop,
    Save,
    RatingWrite,
    RatingDiscovery,
    ImagePreparation,
}

#[must_use]
pub(crate) fn reload_start_blocker<const N: usize>(
    blockers: [Option<ReloadStartBlocker>; N],
) -> Option<ReloadStartBlocker> {
    blockers.into_iter().flatten().next()
}

#[must_use]
pub(crate) const fn reload_start_blocker_message(blocker: ReloadStartBlocker) -> &'static str {
    match blocker {
        ReloadStartBlocker::SpotHeal => "Wait for Spot Heal to finish before reloading",
        ReloadStartBlocker::Crop => "Wait for the crop to finish before reloading",
        ReloadStartBlocker::Save => "Wait for Save As to finish before reloading",
        ReloadStartBlocker::RatingWrite => "Wait for the rating update to finish before reloading",
        ReloadStartBlocker::RatingDiscovery => {
            "Wait for folder ratings to finish loading before reloading"
        }
        ReloadStartBlocker::ImagePreparation => "An image is already loading",
    }
}

/// Decide the visible response. Busy work wins so a watcher cannot fight a
/// load, save, crop, heal, rating operation, or curation already in flight.
/// Unsaved edits block silent reload. Folder membership changes never blank
/// the last good frame.
#[must_use]
pub(crate) fn decide(facts: CoherenceFacts) -> CoherenceAction {
    if session_is_busy(facts.busy) {
        return CoherenceAction::Ignore;
    }
    let source = match facts.source {
        SourceObservation::Unchanged => CoherenceAction::Ignore,
        SourceObservation::Replaced | SourceObservation::Unavailable
            if has_unsaved_edits(facts.unsaved_edits) =>
        {
            CoherenceAction::RemindReload
        }
        SourceObservation::Replaced => CoherenceAction::ReloadCurrent,
        SourceObservation::Unavailable => CoherenceAction::RemindReload,
        SourceObservation::Missing | SourceObservation::Unsupported => CoherenceAction::CurrentGone,
    };
    let folder_changed = matches!(
        facts.folder,
        FolderObservation::Changed | FolderObservation::Unavailable
    );
    if !folder_changed {
        return source;
    }
    match source {
        CoherenceAction::Ignore => CoherenceAction::RescanFolder,
        CoherenceAction::RemindReload => CoherenceAction::RemindAndRescan,
        CoherenceAction::ReloadCurrent => CoherenceAction::ReloadAndRescan,
        CoherenceAction::CurrentGone => CoherenceAction::GoneAndRescan,
        CoherenceAction::ReloadAndRescan
        | CoherenceAction::RescanFolder
        | CoherenceAction::RemindAndRescan
        | CoherenceAction::GoneAndRescan => source,
    }
}

/// Merge two disk observations so a burst keeps the stronger facts.
#[must_use]
pub(crate) const fn merge_observation(
    previous: CoherenceObservation,
    next: CoherenceObservation,
) -> CoherenceObservation {
    CoherenceObservation {
        source: merge_source(previous.source, next.source),
        folder: merge_folder(previous.folder, next.folder),
    }
}

#[must_use]
const fn merge_source(previous: SourceObservation, next: SourceObservation) -> SourceObservation {
    use SourceObservation::{Missing, Replaced, Unavailable, Unchanged, Unsupported};
    match (previous, next) {
        (_, Unchanged) => previous,
        (Unchanged, other) => other,
        (Missing, _) | (_, Missing) => Missing,
        (Unsupported, _) | (_, Unsupported) => Unsupported,
        (Unavailable, _) | (_, Unavailable) => Unavailable,
        (Replaced, Replaced) => Replaced,
    }
}

#[must_use]
const fn merge_folder(previous: FolderObservation, next: FolderObservation) -> FolderObservation {
    use FolderObservation::{Changed, Unavailable, Unchanged};
    match (previous, next) {
        (_, Unchanged) => previous,
        (Unchanged, other) => other,
        (Changed, _) | (_, Changed) => Changed,
        (Unavailable, Unavailable) => Unavailable,
    }
}

/// Merge two pending actions so a noisy burst becomes one visible change.
#[must_use]
pub(crate) const fn coalesce(previous: CoherenceAction, next: CoherenceAction) -> CoherenceAction {
    let gone = source_is_gone(previous) || source_is_gone(next);
    let remind = needs_reminder(previous) || needs_reminder(next);
    let reload = needs_reload(previous) || needs_reload(next);
    let rescan = needs_rescan(previous) || needs_rescan(next);
    match (gone, remind, reload, rescan) {
        (true, _, _, true) => CoherenceAction::GoneAndRescan,
        (true, _, _, false) => CoherenceAction::CurrentGone,
        (false, true, _, true) => CoherenceAction::RemindAndRescan,
        (false, true, _, false) => CoherenceAction::RemindReload,
        (false, false, true, true) => CoherenceAction::ReloadAndRescan,
        (false, false, true, false) => CoherenceAction::ReloadCurrent,
        (false, false, false, true) => CoherenceAction::RescanFolder,
        (false, false, false, false) => CoherenceAction::Ignore,
    }
}

const fn source_is_gone(action: CoherenceAction) -> bool {
    matches!(
        action,
        CoherenceAction::CurrentGone | CoherenceAction::GoneAndRescan
    )
}

const fn needs_reminder(action: CoherenceAction) -> bool {
    matches!(
        action,
        CoherenceAction::RemindReload | CoherenceAction::RemindAndRescan
    )
}

const fn needs_reload(action: CoherenceAction) -> bool {
    matches!(
        action,
        CoherenceAction::ReloadCurrent | CoherenceAction::ReloadAndRescan
    )
}

const fn needs_rescan(action: CoherenceAction) -> bool {
    matches!(
        action,
        CoherenceAction::RescanFolder
            | CoherenceAction::ReloadAndRescan
            | CoherenceAction::RemindAndRescan
            | CoherenceAction::GoneAndRescan
    )
}

/// Decide the honest notice after a gone source's folder refresh completes.
#[must_use]
pub(crate) const fn gone_rescan_result(
    found_at_same_path: bool,
    found_same_object_at_new_path: bool,
) -> GoneRescanResult {
    if found_at_same_path {
        GoneRescanResult::Reappeared
    } else if found_same_object_at_new_path {
        GoneRescanResult::Renamed
    } else {
        GoneRescanResult::Missing
    }
}

/// Repeat source-gone or F5 reminders stay attached to the last good frame
/// without stacking another toast. Gone-and-rescan waits for the scan.
#[must_use]
pub(crate) const fn should_announce(
    previous: Option<CoherenceAction>,
    next: CoherenceAction,
) -> bool {
    match next {
        CoherenceAction::RemindReload | CoherenceAction::RemindAndRescan => !matches!(
            previous,
            Some(CoherenceAction::RemindReload | CoherenceAction::RemindAndRescan)
        ),
        CoherenceAction::CurrentGone => !matches!(
            previous,
            Some(CoherenceAction::CurrentGone | CoherenceAction::GoneAndRescan)
        ),
        CoherenceAction::Ignore
        | CoherenceAction::ReloadCurrent
        | CoherenceAction::ReloadAndRescan
        | CoherenceAction::RescanFolder
        | CoherenceAction::GoneAndRescan => false,
    }
}

/// Every shipping host uses a native user-mediated chooser, never a shell
/// command and never a silent default-app launch.
#[must_use]
pub(crate) const fn open_with_availability() -> OpenWithAvailability {
    if cfg!(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    )) {
        OpenWithAvailability::NativeChooser
    } else {
        OpenWithAvailability::Unavailable
    }
}

/// Status copy when a silent reload would destroy in-memory edits.
#[must_use]
pub(crate) const fn reload_reminder_copy() -> &'static str {
    "Source may have changed. Press F5 when it is safe to reload."
}

/// Status copy when the selected path no longer names the presented file.
#[must_use]
pub(crate) const fn current_gone_copy() -> &'static str {
    "This file is no longer at its selected path. The last good image remains visible."
}

/// Status copy when a folder refresh followed the presented object to a new name.
#[must_use]
pub(crate) const fn renamed_copy() -> &'static str {
    "This file was renamed. The last good image remains visible"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> SessionBusy {
        SessionBusy {
            loading: false,
            saving: false,
            healing: false,
            cropping: false,
            curating: false,
            rating: false,
        }
    }

    fn clean() -> UnsavedEdits {
        UnsavedEdits {
            cropping: false,
            applied_crop: false,
            heal_pending: false,
            rotated_or_flipped: false,
        }
    }

    fn facts(
        source: SourceObservation,
        folder: FolderObservation,
        unsaved: UnsavedEdits,
        busy: SessionBusy,
    ) -> CoherenceFacts {
        CoherenceFacts {
            source,
            folder,
            unsaved_edits: unsaved,
            busy,
        }
    }

    #[test]
    fn source_matches_map_onto_honest_observations() {
        assert_eq!(
            source_observation(ImageSourceMatch::Same),
            SourceObservation::Unchanged
        );
        assert_eq!(
            source_observation(ImageSourceMatch::Changed),
            SourceObservation::Replaced
        );
        assert_eq!(
            source_observation(ImageSourceMatch::Missing),
            SourceObservation::Missing
        );
        assert_eq!(
            source_observation(ImageSourceMatch::Unsupported),
            SourceObservation::Unsupported
        );
        assert_eq!(
            source_observation(ImageSourceMatch::Unavailable),
            SourceObservation::Unavailable
        );
    }

    #[test]
    fn busy_work_is_never_preempted_by_the_watcher() {
        let mut busy = idle();
        busy.loading = true;
        assert_eq!(
            decide(facts(
                SourceObservation::Replaced,
                FolderObservation::Changed,
                clean(),
                busy,
            )),
            CoherenceAction::Ignore
        );
        busy = idle();
        busy.saving = true;
        assert_eq!(
            decide(facts(
                SourceObservation::Missing,
                FolderObservation::Unchanged,
                clean(),
                busy,
            )),
            CoherenceAction::Ignore
        );
    }

    #[test]
    fn a_replaced_source_reloads_only_when_edits_are_safe_to_drop() {
        assert_eq!(
            decide(facts(
                SourceObservation::Replaced,
                FolderObservation::Unchanged,
                clean(),
                idle(),
            )),
            CoherenceAction::ReloadCurrent
        );
        let mut edits = clean();
        edits.heal_pending = true;
        assert_eq!(
            decide(facts(
                SourceObservation::Replaced,
                FolderObservation::Unchanged,
                edits,
                idle(),
            )),
            CoherenceAction::RemindReload
        );
        edits = clean();
        edits.rotated_or_flipped = true;
        assert_eq!(
            decide(facts(
                SourceObservation::Replaced,
                FolderObservation::Unchanged,
                edits,
                idle(),
            )),
            CoherenceAction::RemindReload
        );
        edits = clean();
        edits.cropping = true;
        assert_eq!(
            decide(facts(
                SourceObservation::Replaced,
                FolderObservation::Unchanged,
                edits,
                idle(),
            )),
            CoherenceAction::RemindReload
        );
    }

    #[test]
    fn explicit_reload_preflight_prioritizes_every_async_owner() {
        use ReloadStartBlocker::{
            Crop, ImagePreparation, RatingDiscovery, RatingWrite, Save, SpotHeal,
        };

        assert_eq!(
            reload_start_blocker([
                Some(SpotHeal),
                Some(Crop),
                Some(Save),
                Some(RatingWrite),
                Some(RatingDiscovery),
                Some(ImagePreparation),
            ]),
            Some(SpotHeal)
        );
        assert_eq!(
            reload_start_blocker([None, None, None, None, Some(RatingDiscovery), None]),
            Some(RatingDiscovery)
        );
        assert_eq!(reload_start_blocker([None; 6]), None);
        assert_eq!(
            reload_start_blocker_message(RatingDiscovery),
            "Wait for folder ratings to finish loading before reloading"
        );
        assert_eq!(
            reload_start_blocker_message(ImagePreparation),
            "An image is already loading"
        );
    }

    #[test]
    fn a_missing_source_keeps_the_last_good_frame() {
        assert_eq!(
            decide(facts(
                SourceObservation::Missing,
                FolderObservation::Unchanged,
                clean(),
                idle(),
            )),
            CoherenceAction::CurrentGone
        );
        assert_eq!(
            decide(facts(
                SourceObservation::Unsupported,
                FolderObservation::Changed,
                clean(),
                idle(),
            )),
            CoherenceAction::GoneAndRescan
        );
        assert!(current_gone_copy().contains("last good image remains visible"));
    }

    #[test]
    fn folder_membership_changes_rescan_without_blanking() {
        assert_eq!(
            decide(facts(
                SourceObservation::Unchanged,
                FolderObservation::Changed,
                clean(),
                idle(),
            )),
            CoherenceAction::RescanFolder
        );
        let mut edits = clean();
        edits.heal_pending = true;
        assert_eq!(
            decide(facts(
                SourceObservation::Replaced,
                FolderObservation::Changed,
                edits,
                idle(),
            )),
            CoherenceAction::RemindAndRescan
        );
        assert_eq!(
            decide(facts(
                SourceObservation::Replaced,
                FolderObservation::Changed,
                clean(),
                idle(),
            )),
            CoherenceAction::ReloadAndRescan
        );
    }

    #[test]
    fn an_applied_crop_blocks_silent_reload() {
        let mut edits = clean();
        edits.applied_crop = true;
        assert_eq!(
            decide(facts(
                SourceObservation::Replaced,
                FolderObservation::Unchanged,
                edits,
                idle(),
            )),
            CoherenceAction::RemindReload
        );
        assert!(has_unsaved_edits(edits));
    }

    #[test]
    fn unavailable_identity_never_silently_reloads() {
        assert_eq!(
            decide(facts(
                SourceObservation::Unavailable,
                FolderObservation::Unchanged,
                clean(),
                idle(),
            )),
            CoherenceAction::RemindReload
        );
        let mut busy = idle();
        busy.healing = true;
        assert_eq!(
            decide(facts(
                SourceObservation::Replaced,
                FolderObservation::Changed,
                clean(),
                busy,
            )),
            CoherenceAction::Ignore
        );
        busy = idle();
        busy.cropping = true;
        assert_eq!(
            decide(facts(
                SourceObservation::Missing,
                FolderObservation::Changed,
                clean(),
                busy,
            )),
            CoherenceAction::Ignore
        );
        busy = idle();
        busy.curating = true;
        assert_eq!(
            decide(facts(
                SourceObservation::Replaced,
                FolderObservation::Unchanged,
                clean(),
                busy,
            )),
            CoherenceAction::Ignore
        );
        busy = idle();
        busy.rating = true;
        assert_eq!(
            decide(facts(
                SourceObservation::Replaced,
                FolderObservation::Unchanged,
                clean(),
                busy,
            )),
            CoherenceAction::Ignore
        );
    }

    #[test]
    fn noisy_observations_collapse_to_one_visible_action() {
        assert_eq!(
            coalesce(CoherenceAction::RescanFolder, CoherenceAction::RescanFolder),
            CoherenceAction::RescanFolder
        );
        assert_eq!(
            coalesce(
                CoherenceAction::ReloadCurrent,
                CoherenceAction::RescanFolder
            ),
            CoherenceAction::ReloadAndRescan
        );
        assert_eq!(
            coalesce(CoherenceAction::RemindReload, CoherenceAction::RescanFolder),
            CoherenceAction::RemindAndRescan
        );
        assert_eq!(
            (
                source_is_gone(CoherenceAction::CurrentGone),
                needs_rescan(CoherenceAction::RescanFolder),
                coalesce(CoherenceAction::CurrentGone, CoherenceAction::RescanFolder),
            ),
            (true, true, CoherenceAction::GoneAndRescan)
        );
        assert_eq!(
            coalesce(CoherenceAction::ReloadCurrent, CoherenceAction::CurrentGone),
            CoherenceAction::CurrentGone
        );
        assert_eq!(
            coalesce(CoherenceAction::Ignore, CoherenceAction::RemindReload),
            CoherenceAction::RemindReload
        );
    }

    #[test]
    fn pending_disk_observations_keep_the_stronger_facts() {
        let replaced = CoherenceObservation {
            source: SourceObservation::Replaced,
            folder: FolderObservation::Unchanged,
        };
        let missing = CoherenceObservation {
            source: SourceObservation::Missing,
            folder: FolderObservation::Changed,
        };
        assert_eq!(
            merge_observation(replaced, missing),
            CoherenceObservation {
                source: SourceObservation::Missing,
                folder: FolderObservation::Changed,
            }
        );
    }

    #[test]
    fn open_with_is_a_native_chooser_on_every_shipping_host() {
        assert_eq!(
            open_with_availability(),
            OpenWithAvailability::NativeChooser
        );
        assert!(reload_reminder_copy().contains("Press F5"));
        assert!(renamed_copy().contains("renamed"));
    }

    #[test]
    fn a_watch_starts_only_after_the_presented_source_is_committed() {
        assert!(!watch_can_start(true, false, true));
        assert!(!watch_can_start(true, true, false));
        assert!(!watch_can_start(false, true, true));
        assert!(watch_can_start(true, true, true));
    }

    #[test]
    fn a_watch_observation_cannot_act_on_a_different_loaded_path() {
        let watched = Path::new("current.jpg");
        assert!(watch_applies(Some(watched), watched));
        assert!(!watch_applies(Some(Path::new("other.jpg")), watched));
        assert!(!watch_applies(None, watched));
    }

    #[test]
    fn a_gone_rescan_distinguishes_reappear_rename_and_delete() {
        assert_eq!(
            gone_rescan_result(true, false),
            GoneRescanResult::Reappeared
        );
        assert_eq!(gone_rescan_result(false, true), GoneRescanResult::Renamed);
        assert_eq!(gone_rescan_result(false, false), GoneRescanResult::Missing);
        assert!(
            !should_announce(None, CoherenceAction::GoneAndRescan),
            "gone-and-rescan waits for the scan before speaking"
        );
        assert!(should_announce(None, CoherenceAction::CurrentGone));
        assert!(!should_announce(
            Some(CoherenceAction::GoneAndRescan),
            CoherenceAction::CurrentGone
        ));
        assert!(!should_announce(
            Some(CoherenceAction::RemindAndRescan),
            CoherenceAction::RemindReload
        ));
        assert!(should_announce(None, CoherenceAction::RemindReload));
    }
}
