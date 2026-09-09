//! Pure lifecycle policy for Save As readiness and deferred close.
//!
//! The event loop owns destinations, workers, image buffers, and dialog timing.
//! This module owns only deterministic start blockers, terminal dispositions, and
//! close coordination derived from immutable facts.

use crate::chrome::SAVE_RECOVERY_STATUS;
use crate::locale::Language;
use crate::playlist::ScanPurpose;

const WAIT_FOLDER_OPEN: &str =
    "Wait for the selected folder to finish opening before saving a copy";
const WAIT_RATING_WRITE: &str = "Wait for the rating update to finish before saving a copy";
const WAIT_PREVIEW: &str = "Wait for the image preview to finish before saving";
const WAIT_SPOT_HEAL: &str = "Wait for spot heal to finish before saving";
const WAIT_CROP: &str = "Wait for the crop to finish before saving";
const WAIT_CROP_SELECTION: &str = "Apply or cancel the crop before saving a copy";
const SAVE_ALREADY: &str = "A copy is already being saved";

#[cfg(test)]
const ALL_COPY: &[&str] = &[
    WAIT_FOLDER_OPEN,
    WAIT_RATING_WRITE,
    WAIT_PREVIEW,
    WAIT_SPOT_HEAL,
    WAIT_CROP,
    WAIT_CROP_SELECTION,
    SAVE_ALREADY,
    SAVE_RECOVERY_STATUS,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CloseDisposition {
    Exit,
    WaitForSave,
    WaitForCuration,
    WaitForSaveAndCuration,
}

#[must_use]
pub(crate) const fn close_disposition(
    save_active: bool,
    curation_active: bool,
) -> CloseDisposition {
    match (save_active, curation_active) {
        (false, false) => CloseDisposition::Exit,
        (true, false) => CloseDisposition::WaitForSave,
        (false, true) => CloseDisposition::WaitForCuration,
        (true, true) => CloseDisposition::WaitForSaveAndCuration,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SaveTerminalState {
    Succeeded,
    Failed,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SaveCloseDisposition {
    StayOpen,
    Exit,
    WaitForCuration,
    CancelDeferredClose,
}

#[must_use]
pub(crate) const fn save_close_disposition(
    close_requested: bool,
    terminal: SaveTerminalState,
    curation_active: bool,
) -> SaveCloseDisposition {
    if !close_requested {
        SaveCloseDisposition::StayOpen
    } else if !matches!(terminal, SaveTerminalState::Succeeded) {
        SaveCloseDisposition::CancelDeferredClose
    } else if curation_active {
        SaveCloseDisposition::WaitForCuration
    } else {
        SaveCloseDisposition::Exit
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SaveStartBlocker {
    Recovery,
    FolderOpen,
    RatingWrite,
    Preview,
    SpotHeal,
    Crop,
    CropSelection,
    Save,
}

#[must_use]
pub(crate) fn save_start_blocker<const N: usize>(
    blockers: [Option<SaveStartBlocker>; N],
) -> Option<SaveStartBlocker> {
    blockers.into_iter().flatten().next()
}

#[must_use]
pub(crate) fn save_start_blocker_message(
    language: Language,
    blocker: SaveStartBlocker,
) -> &'static str {
    language.text(match blocker {
        SaveStartBlocker::Recovery => SAVE_RECOVERY_STATUS,
        SaveStartBlocker::FolderOpen => WAIT_FOLDER_OPEN,
        SaveStartBlocker::RatingWrite => WAIT_RATING_WRITE,
        SaveStartBlocker::Preview => WAIT_PREVIEW,
        SaveStartBlocker::SpotHeal => WAIT_SPOT_HEAL,
        SaveStartBlocker::Crop => WAIT_CROP,
        SaveStartBlocker::CropSelection => WAIT_CROP_SELECTION,
        SaveStartBlocker::Save => SAVE_ALREADY,
    })
}

#[must_use]
pub(crate) const fn folder_scan_blocks_save(purpose: Option<&ScanPurpose>) -> bool {
    matches!(purpose, Some(ScanPurpose::OpenFolder))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::{Language, is_cataloged};
    use crate::playlist::ScanPurpose;
    use std::path::PathBuf;

    #[test]
    fn every_save_message_is_cataloged() {
        let missing: Vec<_> = ALL_COPY
            .iter()
            .copied()
            .filter(|source| !is_cataloged(source))
            .collect();
        assert!(missing.is_empty(), "uncataloged save copy: {missing:?}");
    }

    #[test]
    fn save_blocker_copy_is_translated() {
        for language in [Language::Spanish, Language::French, Language::German] {
            assert_ne!(
                save_start_blocker_message(language, SaveStartBlocker::FolderOpen),
                save_start_blocker_message(Language::English, SaveStartBlocker::FolderOpen)
            );
        }
    }

    #[test]
    fn close_disposition_exhausts_save_and_curation_activity() {
        assert_eq!(close_disposition(false, false), CloseDisposition::Exit);
        assert_eq!(
            close_disposition(true, false),
            CloseDisposition::WaitForSave
        );
        assert_eq!(
            close_disposition(false, true),
            CloseDisposition::WaitForCuration
        );
        assert_eq!(
            close_disposition(true, true),
            CloseDisposition::WaitForSaveAndCuration
        );
    }

    #[test]
    fn deferred_close_requires_a_successful_save_terminal_state() {
        assert_eq!(
            save_close_disposition(false, SaveTerminalState::Succeeded, false),
            SaveCloseDisposition::StayOpen
        );
        assert_eq!(
            save_close_disposition(true, SaveTerminalState::Succeeded, false),
            SaveCloseDisposition::Exit
        );
        assert_eq!(
            save_close_disposition(true, SaveTerminalState::Succeeded, true),
            SaveCloseDisposition::WaitForCuration
        );
        for terminal in [SaveTerminalState::Failed, SaveTerminalState::Disconnected] {
            assert_eq!(
                save_close_disposition(true, terminal, false),
                SaveCloseDisposition::CancelDeferredClose
            );
            assert_eq!(
                save_close_disposition(true, terminal, true),
                SaveCloseDisposition::CancelDeferredClose
            );
        }
        for terminal in [
            SaveTerminalState::Succeeded,
            SaveTerminalState::Failed,
            SaveTerminalState::Disconnected,
        ] {
            assert_eq!(
                save_close_disposition(false, terminal, true),
                SaveCloseDisposition::StayOpen
            );
        }
    }

    #[test]
    fn save_start_preflight_excludes_source_changes_writes_and_unsettled_recovery() {
        use SaveStartBlocker::{
            Crop, CropSelection, FolderOpen, Preview, RatingWrite, Recovery, Save, SpotHeal,
        };

        let cases = [
            (
                [
                    Some(Recovery),
                    Some(FolderOpen),
                    Some(RatingWrite),
                    Some(Preview),
                    Some(SpotHeal),
                    Some(Crop),
                    Some(CropSelection),
                    Some(Save),
                ],
                Some(Recovery),
            ),
            (
                [None, Some(FolderOpen), None, None, None, None, None, None],
                Some(FolderOpen),
            ),
            (
                [None, None, Some(RatingWrite), None, None, None, None, None],
                Some(RatingWrite),
            ),
            (
                [
                    None,
                    None,
                    None,
                    Some(Preview),
                    Some(SpotHeal),
                    Some(Crop),
                    Some(CropSelection),
                    Some(Save),
                ],
                Some(Preview),
            ),
            (
                [
                    None,
                    None,
                    None,
                    None,
                    Some(SpotHeal),
                    Some(Crop),
                    Some(CropSelection),
                    Some(Save),
                ],
                Some(SpotHeal),
            ),
            (
                [
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(Crop),
                    Some(CropSelection),
                    Some(Save),
                ],
                Some(Crop),
            ),
            (
                [
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(CropSelection),
                    Some(Save),
                ],
                Some(CropSelection),
            ),
            (
                [None, None, None, None, None, None, None, Some(Save)],
                Some(Save),
            ),
            ([None; 8], None),
        ];
        for (blockers, expected) in cases {
            assert_eq!(save_start_blocker(blockers), expected);
        }
    }

    #[test]
    fn save_start_blocker_copy_names_the_required_recovery() {
        use SaveStartBlocker::{
            Crop, CropSelection, FolderOpen, Preview, RatingWrite, Recovery, Save, SpotHeal,
        };

        let en = crate::locale::Language::English;
        assert_eq!(
            save_start_blocker_message(en, FolderOpen),
            "Wait for the selected folder to finish opening before saving a copy"
        );
        assert_eq!(
            save_start_blocker_message(en, RatingWrite),
            "Wait for the rating update to finish before saving a copy"
        );
        assert_eq!(
            save_start_blocker_message(en, Preview),
            "Wait for the image preview to finish before saving"
        );
        assert_eq!(
            save_start_blocker_message(en, SpotHeal),
            "Wait for spot heal to finish before saving"
        );
        assert_eq!(
            save_start_blocker_message(en, Crop),
            "Wait for the crop to finish before saving"
        );
        assert_eq!(
            save_start_blocker_message(en, CropSelection),
            "Apply or cancel the crop before saving a copy"
        );
        assert_eq!(
            save_start_blocker_message(en, Save),
            "A copy is already being saved"
        );
        assert_eq!(
            save_start_blocker_message(en, Recovery),
            SAVE_RECOVERY_STATUS
        );
        assert_eq!(
            SAVE_RECOVERY_STATUS,
            "Save As stopped unexpectedly. Close and reopen viewr before saving again."
        );
    }

    #[test]
    fn only_an_explicit_open_folder_scan_blocks_save_preflight() {
        let selected = ScanPurpose::SelectedFile {
            path: PathBuf::from("image.png"),
            missing_recovery: false,
        };
        assert!(folder_scan_blocks_save(Some(&ScanPurpose::OpenFolder)));
        assert!(!folder_scan_blocks_save(Some(&selected)));
        assert!(!folder_scan_blocks_save(None));
    }
}
