//! Pure lifecycle policy for source-removing curation operations.
//!
//! The event loop owns workers, paths, playlist state, and recovery application.
//! This module owns only deterministic recovery priority, close decisions, and
//! user-facing status derived from immutable facts.

use crate::curate::{GuardedActionError, TrashRestoreError};
use crate::locale::Language;

/// Catalog keys for the copy this seam composes.
///
/// Every entry is an English source string in `locale`, proven by
/// `every_curation_message_is_cataloged`. Counts, file names, and platform
/// error text are substituted after translation so each language decides where
/// they belong in the sentence.
mod key {
    pub(super) const TRASH_RECOVERY: &str = "Move to Trash stopped unexpectedly. The file may have moved. Review the folder and system Trash, then close and reopen viewr before trying another destructive action.";
    pub(super) const PERMANENT_DELETE_RECOVERY: &str = "Permanent delete stopped unexpectedly. The file may have been deleted. Review the folder, then close and reopen viewr before trying another destructive action.";
    pub(super) const RESTORE_RECOVERY: &str = "Trash restore stopped unexpectedly. Some files may have restored. Undo receipts were kept; review the folder and system Trash, then retry U before moving more files to Trash.";

    pub(super) const ONE_FILE: &str = "{count} file";
    pub(super) const MANY_FILES: &str = "{count} files";

    pub(super) const MOVING_TO_TRASH: &str = "Moving {files} to Trash...";
    pub(super) const FINISHING_TRASH: &str =
        "Finishing move to Trash for {files} before closing...";
    pub(super) const DELETING: &str = "Permanently deleting {files}...";
    pub(super) const FINISHING_DELETE: &str =
        "Finishing permanent delete for {files} before closing...";
    pub(super) const RESTORING: &str = "Restoring {files} from Trash...";
    pub(super) const FINISHING_RESTORE: &str =
        "Finishing Trash restore for {files} before closing...";

    pub(super) const TRASH_CHANGED: &str = "This file changed after it was displayed. Reload it before moving it to Trash. Nothing was moved.";
    pub(super) const DELETE_CHANGED: &str = "This file changed after it was displayed. Reload it before deleting it. Nothing was deleted.";
    pub(super) const TRASH_MISSING: &str = "This file is no longer available. Nothing was moved.";
    pub(super) const DELETE_MISSING: &str =
        "This file is no longer available. Nothing was deleted.";
    pub(super) const TRASH_UNSUPPORTED: &str = "This filesystem entry cannot be safely moved from the displayed source. Nothing was moved.";
    pub(super) const DELETE_UNSUPPORTED: &str = "This filesystem entry cannot be safely deleted from the displayed source. Nothing was deleted.";
    pub(super) const TRASH_UNAVAILABLE: &str =
        "Safe file identity could not be verified. Nothing was moved.";
    pub(super) const DELETE_UNAVAILABLE: &str =
        "Safe file identity could not be verified. Nothing was deleted.";
    pub(super) const TRASH_FAILED: &str = "Trash failed: {error}. Nothing was moved.";
    pub(super) const DELETE_FAILED: &str = "Delete failed: {error}. Nothing was deleted.";

    pub(super) const TRASHED_WITH_UNDO: &str = "Moved to Trash. Undo with U.";
    pub(super) const TRASHED_PRIOR_UNDO_KEPT: &str = "Moved to Trash, but U is unavailable for this move. Use the system Trash; U still restores the previous Trash action.";
    pub(super) const TRASHED_WITHOUT_UNDO: &str =
        "Moved to Trash, but U is unavailable for this move. Use the system Trash for recovery.";

    pub(super) const DELETE_PERMANENTLY: &str = "Delete permanently";
    pub(super) const DELETE_CONFIRMATION: &str = "Delete \"{name}\" forever?\n\nThis skips the system Trash and cannot be undone from viewr.";
    pub(super) const DELETED_PRIOR_UNDO_KEPT: &str = "Permanently deleted \"{name}\". This cannot be undone; U still restores the previous Trash action.";
    pub(super) const DELETED: &str = "Permanently deleted \"{name}\". This cannot be undone.";

    pub(super) const RESTORE_OCCUPIED: &str = "Restore blocked: The original folder already contains an item with that name. Move or rename it, then retry with U.";
    pub(super) const RESTORE_DENIED: &str =
        "Restore blocked: Access was denied. Check permissions, then retry with U.";
    pub(super) const RESTORE_FAILED: &str =
        "Restore failed: The operating system could not restore the file. Retry with U.";
    pub(super) const RESTORE_GONE: &str =
        "The exact item is no longer in the system Trash. No retry remains in viewr.";
    pub(super) const RESTORE_AMBIGUOUS: &str =
        "The exact Trash receipt is ambiguous. Use the system Trash; no retry remains in viewr.";
    pub(super) const RESTORE_UNSUPPORTED: &str = "In-app restore is unsupported on this platform. Use the system Trash; no retry remains in viewr.";
    pub(super) const RESTORE_INVALID_RECEIPT: &str =
        "The exact Trash receipt is unavailable. Use the system Trash; no retry remains in viewr.";
    pub(super) const RESTORE_FAILED_TERMINAL: &str = "Restore failed. No retry remains in viewr.";

    pub(super) const RESTORED_FILES: &str = "Restored {files}";
    pub(super) const NOTHING_RESTORED: &str = "Nothing restored";
    pub(super) const REOPEN_FOLDER: &str = "reopen the source folder to refresh its view";
    pub(super) const CAN_RETRY: &str = "{files} can retry with U";
    pub(super) const NEEDS_RESOLUTION_ONE: &str =
        "{files} needs the blocking condition resolved, then U can retry";
    pub(super) const NEEDS_RESOLUTION_MANY: &str =
        "{files} need the blocking condition resolved, then U can retry";
    pub(super) const NEEDS_REVIEW_ONE: &str = "{files} requires system Trash review";
    pub(super) const NEEDS_REVIEW_MANY: &str = "{files} require system Trash review";
    pub(super) const TERMINAL_ONE: &str = "{files} is no longer available for in-app restore";
    pub(super) const TERMINAL_MANY: &str = "{files} are no longer available for in-app restore";

    /// Complete copy surface, for the catalog coverage test.
    #[cfg(test)]
    pub(super) const ALL: &[&str] = &[
        TRASH_RECOVERY,
        PERMANENT_DELETE_RECOVERY,
        RESTORE_RECOVERY,
        ONE_FILE,
        MANY_FILES,
        MOVING_TO_TRASH,
        FINISHING_TRASH,
        DELETING,
        FINISHING_DELETE,
        RESTORING,
        FINISHING_RESTORE,
        TRASH_CHANGED,
        DELETE_CHANGED,
        TRASH_MISSING,
        DELETE_MISSING,
        TRASH_UNSUPPORTED,
        DELETE_UNSUPPORTED,
        TRASH_UNAVAILABLE,
        DELETE_UNAVAILABLE,
        TRASH_FAILED,
        DELETE_FAILED,
        TRASHED_WITH_UNDO,
        TRASHED_PRIOR_UNDO_KEPT,
        TRASHED_WITHOUT_UNDO,
        DELETE_PERMANENTLY,
        DELETE_CONFIRMATION,
        DELETED_PRIOR_UNDO_KEPT,
        DELETED,
        RESTORE_OCCUPIED,
        RESTORE_DENIED,
        RESTORE_FAILED,
        RESTORE_GONE,
        RESTORE_AMBIGUOUS,
        RESTORE_UNSUPPORTED,
        RESTORE_INVALID_RECEIPT,
        RESTORE_FAILED_TERMINAL,
        RESTORED_FILES,
        NOTHING_RESTORED,
        REOPEN_FOLDER,
        CAN_RETRY,
        NEEDS_RESOLUTION_ONE,
        NEEDS_RESOLUTION_MANY,
        NEEDS_REVIEW_ONE,
        NEEDS_REVIEW_MANY,
        TERMINAL_ONE,
        TERMINAL_MANY,
    ];
}

/// Placeholder for a count phrase such as "2 files".
const FILES_PLACEHOLDER: &str = "{files}";
/// Placeholder for a bare number.
const COUNT_PLACEHOLDER: &str = "{count}";
/// Placeholder for a privacy-safe file name.
const NAME_PLACEHOLDER: &str = "{name}";
/// Placeholder for platform error text that viewr did not write.
const ERROR_PLACEHOLDER: &str = "{error}";

const RECOVERY_PRIORITY: [CurationKind; 3] = [
    CurationKind::PermanentDelete,
    CurationKind::Trash,
    CurationKind::Restore,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CurationKind {
    Trash,
    PermanentDelete,
    Restore,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CurationRecovery {
    trash: bool,
    permanent_delete: bool,
    restore: bool,
}

impl CurationRecovery {
    pub(crate) fn record(&mut self, kind: CurationKind) {
        match kind {
            CurationKind::Trash => self.trash = true,
            CurationKind::PermanentDelete => self.permanent_delete = true,
            CurationKind::Restore => self.restore = true,
        }
    }

    pub(crate) fn clear(&mut self, kind: CurationKind) {
        match kind {
            CurationKind::Trash => self.trash = false,
            CurationKind::PermanentDelete => self.permanent_delete = false,
            CurationKind::Restore => self.restore = false,
        }
    }

    #[must_use]
    pub(crate) const fn contains(self, kind: CurationKind) -> bool {
        match kind {
            CurationKind::Trash => self.trash,
            CurationKind::PermanentDelete => self.permanent_delete,
            CurationKind::Restore => self.restore,
        }
    }

    #[must_use]
    pub(crate) fn source_removal_preflight(self, language: Language) -> Option<&'static str> {
        self.highest_risk()
            .map(|kind| curation_recovery_message(language, kind))
    }

    #[must_use]
    pub(crate) fn status(self, language: Language) -> Option<String> {
        self.highest_risk()
            .map(|kind| curation_recovery_message(language, kind).to_owned())
    }

    fn highest_risk(self) -> Option<CurationKind> {
        RECOVERY_PRIORITY
            .into_iter()
            .find(|kind| self.contains(*kind))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CurationTerminalState {
    Succeeded,
    NeedsAttention,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CurationCloseDisposition {
    StayOpen,
    Exit,
    WaitForSave,
    CancelDeferredClose,
}

#[must_use]
pub(crate) const fn curation_close_disposition(
    close_requested: bool,
    terminal: CurationTerminalState,
    save_active: bool,
) -> CurationCloseDisposition {
    if !close_requested {
        CurationCloseDisposition::StayOpen
    } else if !matches!(terminal, CurationTerminalState::Succeeded) {
        CurationCloseDisposition::CancelDeferredClose
    } else if save_active {
        CurationCloseDisposition::WaitForSave
    } else {
        CurationCloseDisposition::Exit
    }
}

#[must_use]
pub(crate) fn curation_recovery_message(language: Language, kind: CurationKind) -> &'static str {
    language.text(match kind {
        CurationKind::Trash => key::TRASH_RECOVERY,
        CurationKind::PermanentDelete => key::PERMANENT_DELETE_RECOVERY,
        CurationKind::Restore => key::RESTORE_RECOVERY,
    })
}

#[must_use]
pub(crate) fn curation_status(
    language: Language,
    kind: CurationKind,
    submitted: usize,
    closing: bool,
) -> String {
    let source = match (kind, closing) {
        (CurationKind::Trash, false) => key::MOVING_TO_TRASH,
        (CurationKind::Trash, true) => key::FINISHING_TRASH,
        (CurationKind::PermanentDelete, false) => key::DELETING,
        (CurationKind::PermanentDelete, true) => key::FINISHING_DELETE,
        (CurationKind::Restore, false) => key::RESTORING,
        (CurationKind::Restore, true) => key::FINISHING_RESTORE,
    };
    with_files(language, source, submitted)
}

#[must_use]
pub(crate) fn file_count(language: Language, count: usize) -> String {
    let source = if count == 1 {
        key::ONE_FILE
    } else {
        key::MANY_FILES
    };
    language
        .text(source)
        .replace(COUNT_PLACEHOLDER, &count.to_string())
}

/// Substitute a translated count phrase into a translated sentence.
fn with_files(language: Language, source: &'static str, count: usize) -> String {
    language
        .text(source)
        .replace(FILES_PLACEHOLDER, &file_count(language, count))
}

/// Source-bound destructive action that failed before or during mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GuardedSourceAction {
    Trash,
    PermanentDelete,
}

/// Path-free failure copy for Trash and permanent delete preflight rejections.
#[must_use]
pub(crate) fn guarded_source_action_failure_message(
    language: Language,
    action: GuardedSourceAction,
    error: &GuardedActionError,
) -> String {
    let source = match (action, error) {
        (GuardedSourceAction::Trash, GuardedActionError::Changed) => key::TRASH_CHANGED,
        (GuardedSourceAction::PermanentDelete, GuardedActionError::Changed) => key::DELETE_CHANGED,
        (GuardedSourceAction::Trash, GuardedActionError::Missing) => key::TRASH_MISSING,
        (GuardedSourceAction::PermanentDelete, GuardedActionError::Missing) => key::DELETE_MISSING,
        (GuardedSourceAction::Trash, GuardedActionError::Unsupported) => key::TRASH_UNSUPPORTED,
        (GuardedSourceAction::PermanentDelete, GuardedActionError::Unsupported) => {
            key::DELETE_UNSUPPORTED
        }
        (GuardedSourceAction::Trash, GuardedActionError::Unavailable) => key::TRASH_UNAVAILABLE,
        (GuardedSourceAction::PermanentDelete, GuardedActionError::Unavailable) => {
            key::DELETE_UNAVAILABLE
        }
        (GuardedSourceAction::Trash, GuardedActionError::OperationFailed(platform)) => {
            return language
                .text(key::TRASH_FAILED)
                .replace(ERROR_PLACEHOLDER, platform);
        }
        (GuardedSourceAction::PermanentDelete, GuardedActionError::OperationFailed(platform)) => {
            return language
                .text(key::DELETE_FAILED)
                .replace(ERROR_PLACEHOLDER, platform);
        }
    };
    language.text(source).to_owned()
}

/// Path-free success copy after a single move to Trash.
#[must_use]
pub(crate) fn single_trash_result_message(
    language: Language,
    has_receipt: bool,
    previous_undo_preserved: bool,
) -> &'static str {
    language.text(if has_receipt {
        key::TRASHED_WITH_UNDO
    } else if previous_undo_preserved {
        key::TRASHED_PRIOR_UNDO_KEPT
    } else {
        key::TRASHED_WITHOUT_UNDO
    })
}

/// Exact confirmation button label for permanent delete dialogs.
///
/// The dialog shows this label, so the confirmation check compares against the
/// same language rather than an English string the user never saw.
#[must_use]
pub(crate) fn permanent_delete_action(language: Language) -> &'static str {
    language.text(key::DELETE_PERMANENTLY)
}

/// Path-free permanent-delete confirmation body. `safe_name` must already be
/// privacy-safe and quote-sanitized by the caller.
#[must_use]
pub(crate) fn permanent_delete_description(language: Language, safe_name: &str) -> String {
    language
        .text(key::DELETE_CONFIRMATION)
        .replace(NAME_PLACEHOLDER, safe_name)
}

/// True only when the user chose the explicit permanent-delete action label.
#[must_use]
pub(crate) fn permanent_delete_confirmed(language: Language, custom_label: Option<&str>) -> bool {
    matches!(custom_label, Some(label) if label == permanent_delete_action(language))
}

/// Wait copy when Trash or permanent delete has no ready source.
///
/// A missing selection with no collage focus is a quiet no-op: there is no
/// destructive target to name.
#[must_use]
pub(crate) fn removal_unready_message(
    action: GuardedSourceAction,
    mosaic: bool,
    has_selection: bool,
    load_failed: bool,
) -> Option<&'static str> {
    if mosaic {
        return Some(match action {
            GuardedSourceAction::Trash => {
                "Wait for this photo to finish opening before moving it to Trash"
            }
            GuardedSourceAction::PermanentDelete => {
                "Wait for this photo to finish opening before permanently deleting it"
            }
        });
    }
    if !has_selection {
        return None;
    }
    Some(if load_failed {
        match action {
            GuardedSourceAction::Trash => "Reload or open another image before moving it to Trash",
            GuardedSourceAction::PermanentDelete => {
                "Reload or open another image before permanently deleting it"
            }
        }
    } else {
        match action {
            GuardedSourceAction::Trash => {
                "Wait for the selected image to finish opening before moving it to Trash"
            }
            GuardedSourceAction::PermanentDelete => {
                "Wait for the selected image to finish opening before permanently deleting it"
            }
        }
    })
}

/// Path-free success copy after permanent delete. `safe_name` must already be
/// privacy-safe and quote-sanitized by the caller.
#[must_use]
pub(crate) fn permanent_delete_success_message(
    language: Language,
    safe_name: &str,
    previous_trash_undo: bool,
) -> String {
    let source = if previous_trash_undo {
        key::DELETED_PRIOR_UNDO_KEPT
    } else {
        key::DELETED
    };
    language.text(source).replace(NAME_PLACEHOLDER, safe_name)
}

/// Path-free single-file restore failure copy.
#[must_use]
pub(crate) fn single_restore_failure_message(
    language: Language,
    error: TrashRestoreError,
) -> String {
    language
        .text(match error {
            TrashRestoreError::DestinationOccupied => key::RESTORE_OCCUPIED,
            TrashRestoreError::AccessDenied => key::RESTORE_DENIED,
            TrashRestoreError::OperationFailed => key::RESTORE_FAILED,
            TrashRestoreError::MissingFromTrash => key::RESTORE_GONE,
            TrashRestoreError::AmbiguousReceipt => key::RESTORE_AMBIGUOUS,
            TrashRestoreError::Unsupported => key::RESTORE_UNSUPPORTED,
            TrashRestoreError::InvalidReceipt => key::RESTORE_INVALID_RECEIPT,
        })
        .to_owned()
}

/// Counted result of one restore batch, grouped so the summary reads from one
/// immutable fact set rather than a long positional argument list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RestoreOutcomeCounts {
    /// Files the platform restored.
    pub restored: usize,
    /// Failures that `U` can retry immediately.
    pub retry_now: usize,
    /// Failures that need a blocking condition resolved first.
    pub resolve_then_retry: usize,
    /// Failures that need system Trash review.
    pub manual_review: usize,
    /// Failures with no in-app retry left.
    pub terminal: usize,
    /// First failure, used when a single failure can speak for itself.
    pub first_failure: Option<TrashRestoreError>,
    /// Whether the restored files belong to the folder currently in view.
    pub active_playlist: bool,
}

/// Path-free restore summary across mixed outcomes.
#[must_use]
pub(crate) fn restore_result_message(language: Language, counts: RestoreOutcomeCounts) -> String {
    let RestoreOutcomeCounts {
        restored,
        retry_now,
        resolve_then_retry,
        manual_review,
        terminal,
        first_failure,
        active_playlist,
    } = counts;
    let failure_total = retry_now + resolve_then_retry + manual_review + terminal;
    if failure_total == 0 {
        let restored_clause = with_files(language, key::RESTORED_FILES, restored);
        if active_playlist {
            return restored_clause;
        }
        return format!("{restored_clause}; {}", language.text(key::REOPEN_FOLDER));
    }
    if restored == 0 && failure_total == 1 {
        return first_failure.map_or_else(
            || language.text(key::RESTORE_FAILED_TERMINAL).to_owned(),
            |error| single_restore_failure_message(language, error),
        );
    }

    let mut clauses = if restored == 0 {
        vec![language.text(key::NOTHING_RESTORED).to_owned()]
    } else {
        vec![with_files(language, key::RESTORED_FILES, restored)]
    };
    if retry_now > 0 {
        clauses.push(with_files(language, key::CAN_RETRY, retry_now));
    }
    if resolve_then_retry > 0 {
        let source = if resolve_then_retry == 1 {
            key::NEEDS_RESOLUTION_ONE
        } else {
            key::NEEDS_RESOLUTION_MANY
        };
        clauses.push(with_files(language, source, resolve_then_retry));
    }
    if manual_review > 0 {
        let source = if manual_review == 1 {
            key::NEEDS_REVIEW_ONE
        } else {
            key::NEEDS_REVIEW_MANY
        };
        clauses.push(with_files(language, source, manual_review));
    }
    if terminal > 0 {
        let source = if terminal == 1 {
            key::TERMINAL_ONE
        } else {
            key::TERMINAL_MANY
        };
        clauses.push(with_files(language, source, terminal));
    }
    if !active_playlist && restored > 0 {
        clauses.push(language.text(key::REOPEN_FOLDER).to_owned());
    }
    format!("{}.", clauses.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::is_cataloged;

    /// Assertions below state the English copy, so the seam is called with it.
    const EN: Language = Language::English;
    const TRANSLATED: [Language; 3] = [Language::Spanish, Language::French, Language::German];

    const RESTORE_ERRORS: [TrashRestoreError; 7] = [
        TrashRestoreError::DestinationOccupied,
        TrashRestoreError::AccessDenied,
        TrashRestoreError::OperationFailed,
        TrashRestoreError::MissingFromTrash,
        TrashRestoreError::AmbiguousReceipt,
        TrashRestoreError::Unsupported,
        TrashRestoreError::InvalidReceipt,
    ];

    const KINDS: [CurationKind; 3] = [
        CurationKind::Trash,
        CurationKind::PermanentDelete,
        CurationKind::Restore,
    ];

    #[test]
    fn recovery_flags_exhaust_every_combination_in_fixed_risk_order() {
        let cases = [
            (0_u8, None),
            (1, Some(CurationKind::Trash)),
            (2, Some(CurationKind::PermanentDelete)),
            (3, Some(CurationKind::PermanentDelete)),
            (4, Some(CurationKind::Restore)),
            (5, Some(CurationKind::Trash)),
            (6, Some(CurationKind::PermanentDelete)),
            (7, Some(CurationKind::PermanentDelete)),
        ];

        for (mask, expected) in cases {
            let mut recovery = CurationRecovery::default();
            for (bit, kind) in KINDS.into_iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    recovery.record(kind);
                }
            }

            assert_eq!(recovery.highest_risk(), expected);
            assert_eq!(
                recovery.source_removal_preflight(EN),
                expected.map(|kind| curation_recovery_message(EN, kind))
            );
            assert_eq!(
                recovery.status(EN).as_deref(),
                expected.map(|kind| curation_recovery_message(EN, kind))
            );
            for (bit, kind) in KINDS.into_iter().enumerate() {
                assert_eq!(recovery.contains(kind), mask & (1 << bit) != 0);
                recovery.clear(kind);
            }
            assert_eq!(recovery, CurationRecovery::default());
        }
    }

    /// Every source string this seam composes has all four languages.
    #[test]
    fn every_curation_message_is_cataloged() {
        let missing: Vec<_> = key::ALL
            .iter()
            .copied()
            .filter(|source| !is_cataloged(source))
            .collect();
        assert!(missing.is_empty(), "uncataloged curation copy: {missing:?}");
    }

    /// Destructive-action copy is translated, and every placeholder is filled.
    ///
    /// The catalog falls back to English silently, so comparing against the
    /// English output is what proves a translation is actually present. The
    /// placeholder check catches a translation that dropped or misspelled
    /// `{count}`, `{files}`, `{name}`, or `{error}`, which would otherwise show
    /// a user a brace instead of their file name.
    /// Every message this seam can compose, in one language.
    fn every_composed_message(language: Language) -> Vec<String> {
        let mut produced = Vec::new();
        for kind in KINDS {
            produced.push(curation_recovery_message(language, kind).to_owned());
            for count in [0_usize, 1, 2] {
                for closing in [false, true] {
                    produced.push(curation_status(language, kind, count, closing));
                }
            }
        }
        for count in [0_usize, 1, 2] {
            produced.push(file_count(language, count));
        }
        for action in [
            GuardedSourceAction::Trash,
            GuardedSourceAction::PermanentDelete,
        ] {
            for error in [
                GuardedActionError::Changed,
                GuardedActionError::Missing,
                GuardedActionError::Unsupported,
                GuardedActionError::Unavailable,
                GuardedActionError::OperationFailed("access denied".to_owned()),
            ] {
                produced.push(guarded_source_action_failure_message(
                    language, action, &error,
                ));
            }
        }
        for has_receipt in [false, true] {
            for preserved in [false, true] {
                produced
                    .push(single_trash_result_message(language, has_receipt, preserved).to_owned());
            }
        }
        produced.push(permanent_delete_action(language).to_owned());
        produced.push(permanent_delete_description(language, "night.png"));
        for previous in [false, true] {
            produced.push(permanent_delete_success_message(
                language,
                "night.png",
                previous,
            ));
        }
        for error in RESTORE_ERRORS {
            produced.push(single_restore_failure_message(language, error));
        }
        for restored in [0_usize, 1, 2] {
            for failures in [0_usize, 1, 2] {
                for active in [false, true] {
                    produced.push(restore_result_message(
                        language,
                        RestoreOutcomeCounts {
                            restored,
                            retry_now: failures,
                            resolve_then_retry: failures,
                            manual_review: failures,
                            terminal: failures,
                            first_failure: Some(TrashRestoreError::AccessDenied),
                            active_playlist: active,
                        },
                    ));
                    produced.push(restore_result_message(
                        language,
                        RestoreOutcomeCounts {
                            restored,
                            retry_now: failures,
                            resolve_then_retry: 0,
                            manual_review: 0,
                            terminal: 0,
                            first_failure: None,
                            active_playlist: active,
                        },
                    ));
                }
            }
        }

        produced
    }

    #[test]
    fn composed_copy_is_translated_and_fully_substituted() {
        let english = every_composed_message(EN);
        for language in TRANSLATED {
            let produced = every_composed_message(language);
            assert_eq!(produced.len(), english.len());
            for (source, translated) in english.iter().zip(&produced) {
                assert!(
                    !translated.contains('{') && !translated.contains('}'),
                    "unsubstituted placeholder in {language:?}: {translated}"
                );
                assert_ne!(
                    source, translated,
                    "{language:?} fell back to the English source: {source}"
                );
            }
        }
        for message in &english {
            assert!(
                !message.contains('{') && !message.contains('}'),
                "unsubstituted placeholder in English: {message}"
            );
        }
    }

    #[test]
    fn recovery_guidance_is_exact_and_operation_specific() {
        assert_eq!(
            curation_recovery_message(EN, CurationKind::Trash),
            "Move to Trash stopped unexpectedly. The file may have moved. Review the folder and system Trash, then close and reopen viewr before trying another destructive action."
        );
        assert_eq!(
            curation_recovery_message(EN, CurationKind::PermanentDelete),
            "Permanent delete stopped unexpectedly. The file may have been deleted. Review the folder, then close and reopen viewr before trying another destructive action."
        );
        assert_eq!(
            curation_recovery_message(EN, CurationKind::Restore),
            "Trash restore stopped unexpectedly. Some files may have restored. Undo receipts were kept; review the folder and system Trash, then retry U before moving more files to Trash."
        );
    }

    #[test]
    fn close_disposition_exhausts_request_terminal_and_save_state() {
        let cases = [
            (
                false,
                CurationTerminalState::Succeeded,
                false,
                CurationCloseDisposition::StayOpen,
            ),
            (
                false,
                CurationTerminalState::Succeeded,
                true,
                CurationCloseDisposition::StayOpen,
            ),
            (
                false,
                CurationTerminalState::NeedsAttention,
                false,
                CurationCloseDisposition::StayOpen,
            ),
            (
                false,
                CurationTerminalState::NeedsAttention,
                true,
                CurationCloseDisposition::StayOpen,
            ),
            (
                true,
                CurationTerminalState::Succeeded,
                false,
                CurationCloseDisposition::Exit,
            ),
            (
                true,
                CurationTerminalState::Succeeded,
                true,
                CurationCloseDisposition::WaitForSave,
            ),
            (
                true,
                CurationTerminalState::NeedsAttention,
                false,
                CurationCloseDisposition::CancelDeferredClose,
            ),
            (
                true,
                CurationTerminalState::NeedsAttention,
                true,
                CurationCloseDisposition::CancelDeferredClose,
            ),
        ];

        for (close_requested, terminal, save_active, expected) in cases {
            assert_eq!(
                curation_close_disposition(close_requested, terminal, save_active),
                expected
            );
        }
    }

    #[test]
    fn status_copy_covers_every_operation_phase_and_count_grammar() {
        let cases = [
            (CurationKind::Trash, 1, false, "Moving 1 file to Trash..."),
            (CurationKind::Trash, 2, false, "Moving 2 files to Trash..."),
            (
                CurationKind::Trash,
                1,
                true,
                "Finishing move to Trash for 1 file before closing...",
            ),
            (
                CurationKind::Trash,
                2,
                true,
                "Finishing move to Trash for 2 files before closing...",
            ),
            (
                CurationKind::PermanentDelete,
                1,
                false,
                "Permanently deleting 1 file...",
            ),
            (
                CurationKind::PermanentDelete,
                2,
                false,
                "Permanently deleting 2 files...",
            ),
            (
                CurationKind::PermanentDelete,
                1,
                true,
                "Finishing permanent delete for 1 file before closing...",
            ),
            (
                CurationKind::PermanentDelete,
                2,
                true,
                "Finishing permanent delete for 2 files before closing...",
            ),
            (
                CurationKind::Restore,
                1,
                false,
                "Restoring 1 file from Trash...",
            ),
            (
                CurationKind::Restore,
                2,
                false,
                "Restoring 2 files from Trash...",
            ),
            (
                CurationKind::Restore,
                1,
                true,
                "Finishing Trash restore for 1 file before closing...",
            ),
            (
                CurationKind::Restore,
                2,
                true,
                "Finishing Trash restore for 2 files before closing...",
            ),
        ];

        for (kind, submitted, closing, expected) in cases {
            assert_eq!(curation_status(EN, kind, submitted, closing), expected);
        }
        assert_eq!(file_count(EN, 0), "0 files");
        assert_eq!(file_count(EN, usize::MAX), format!("{} files", usize::MAX));
    }

    #[test]
    fn source_bound_destructive_copy_is_exhaustive_and_path_free() {
        let trash_cases = [
            (
                GuardedActionError::Changed,
                "This file changed after it was displayed. Reload it before moving it to Trash. Nothing was moved.",
            ),
            (
                GuardedActionError::Missing,
                "This file is no longer available. Nothing was moved.",
            ),
            (
                GuardedActionError::Unsupported,
                "This filesystem entry cannot be safely moved from the displayed source. Nothing was moved.",
            ),
            (
                GuardedActionError::Unavailable,
                "Safe file identity could not be verified. Nothing was moved.",
            ),
            (
                GuardedActionError::OperationFailed("access denied".to_owned()),
                "Trash failed: access denied. Nothing was moved.",
            ),
        ];
        for (error, expected) in trash_cases {
            let message =
                guarded_source_action_failure_message(EN, GuardedSourceAction::Trash, &error);
            assert_eq!(message, expected);
            assert!(!message.contains("private"));
            assert!(!message.contains("album"));
        }

        let permanent_delete_cases = [
            (
                GuardedActionError::Changed,
                "This file changed after it was displayed. Reload it before deleting it. Nothing was deleted.",
            ),
            (
                GuardedActionError::Missing,
                "This file is no longer available. Nothing was deleted.",
            ),
            (
                GuardedActionError::Unsupported,
                "This filesystem entry cannot be safely deleted from the displayed source. Nothing was deleted.",
            ),
            (
                GuardedActionError::Unavailable,
                "Safe file identity could not be verified. Nothing was deleted.",
            ),
            (
                GuardedActionError::OperationFailed("access denied".to_owned()),
                "Delete failed: access denied. Nothing was deleted.",
            ),
        ];
        for (error, expected) in permanent_delete_cases {
            let message = guarded_source_action_failure_message(
                EN,
                GuardedSourceAction::PermanentDelete,
                &error,
            );
            assert_eq!(message, expected);
            assert!(!message.contains("private"));
            assert!(!message.contains("album"));
        }
    }

    #[test]
    fn single_trash_copy_routes_every_move_to_a_real_recovery_path() {
        assert_eq!(
            single_trash_result_message(EN, true, false),
            "Moved to Trash. Undo with U."
        );
        assert_eq!(
            single_trash_result_message(EN, false, true),
            "Moved to Trash, but U is unavailable for this move. Use the system Trash; U still restores the previous Trash action."
        );
        assert_eq!(
            single_trash_result_message(EN, false, false),
            "Moved to Trash, but U is unavailable for this move. Use the system Trash for recovery."
        );
    }

    #[test]
    fn permanent_delete_success_copy_disambiguates_prior_trash_undo() {
        assert_eq!(
            permanent_delete_success_message(EN, "bad???gpj", true),
            "Permanently deleted \"bad???gpj\". This cannot be undone; U still restores the previous Trash action."
        );
        assert_eq!(
            permanent_delete_success_message(EN, "bad???gpj", false),
            "Permanently deleted \"bad???gpj\". This cannot be undone."
        );
    }

    #[test]
    fn permanent_delete_confirmation_is_bounded_and_label_exact() {
        let description = permanent_delete_description(EN, "bad???gpj");
        assert!(description.starts_with("Delete \"bad???gpj\" forever?"));
        assert_eq!(description.matches('\n').count(), 2);
        assert!(description.contains("system Trash"));
        assert!(!description.contains('\\'));
        assert!(!description.contains('/'));

        assert!(permanent_delete_confirmed(
            EN,
            Some(permanent_delete_action(EN))
        ));
        assert!(!permanent_delete_confirmed(EN, Some("Cancel")));
        assert!(!permanent_delete_confirmed(EN, None));
        assert!(!permanent_delete_confirmed(EN, Some("Ok")));
    }

    #[test]
    fn removal_unready_copy_names_the_wait_without_a_silent_no_op() {
        assert_eq!(
            removal_unready_message(GuardedSourceAction::Trash, true, false, false),
            Some("Wait for this photo to finish opening before moving it to Trash")
        );
        assert_eq!(
            removal_unready_message(GuardedSourceAction::PermanentDelete, true, true, false),
            Some("Wait for this photo to finish opening before permanently deleting it")
        );
        assert_eq!(
            removal_unready_message(GuardedSourceAction::Trash, false, true, true),
            Some("Reload or open another image before moving it to Trash")
        );
        assert_eq!(
            removal_unready_message(GuardedSourceAction::PermanentDelete, false, true, false),
            Some("Wait for the selected image to finish opening before permanently deleting it")
        );
        assert_eq!(
            removal_unready_message(GuardedSourceAction::PermanentDelete, false, false, false),
            None
        );
    }

    #[test]
    fn restore_copy_exposes_only_valid_retry_routes() {
        let cases = [
            (
                TrashRestoreError::DestinationOccupied,
                "Restore blocked: The original folder already contains an item with that name. Move or rename it, then retry with U.",
            ),
            (
                TrashRestoreError::AccessDenied,
                "Restore blocked: Access was denied. Check permissions, then retry with U.",
            ),
            (
                TrashRestoreError::OperationFailed,
                "Restore failed: The operating system could not restore the file. Retry with U.",
            ),
            (
                TrashRestoreError::MissingFromTrash,
                "The exact item is no longer in the system Trash. No retry remains in viewr.",
            ),
            (
                TrashRestoreError::AmbiguousReceipt,
                "The exact Trash receipt is ambiguous. Use the system Trash; no retry remains in viewr.",
            ),
            (
                TrashRestoreError::Unsupported,
                "In-app restore is unsupported on this platform. Use the system Trash; no retry remains in viewr.",
            ),
            (
                TrashRestoreError::InvalidReceipt,
                "The exact Trash receipt is unavailable. Use the system Trash; no retry remains in viewr.",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(single_restore_failure_message(EN, error), expected);
        }

        assert_eq!(
            restore_result_message(
                EN,
                RestoreOutcomeCounts {
                    restored: 1,
                    retry_now: 0,
                    resolve_then_retry: 0,
                    manual_review: 0,
                    terminal: 0,
                    first_failure: None,
                    active_playlist: true,
                },
            ),
            "Restored 1 file"
        );
        assert_eq!(
            restore_result_message(
                EN,
                RestoreOutcomeCounts {
                    restored: 2,
                    retry_now: 0,
                    resolve_then_retry: 0,
                    manual_review: 0,
                    terminal: 0,
                    first_failure: None,
                    active_playlist: false,
                },
            ),
            "Restored 2 files; reopen the source folder to refresh its view"
        );
        assert_eq!(
            restore_result_message(
                EN,
                RestoreOutcomeCounts {
                    restored: 1,
                    retry_now: 1,
                    resolve_then_retry: 1,
                    manual_review: 1,
                    terminal: 1,
                    first_failure: Some(TrashRestoreError::OperationFailed),
                    active_playlist: true,
                },
            ),
            "Restored 1 file; 1 file can retry with U; 1 file needs the blocking condition resolved, then U can retry; 1 file requires system Trash review; 1 file is no longer available for in-app restore."
        );
        let manual_only = restore_result_message(
            EN,
            RestoreOutcomeCounts {
                restored: 0,
                retry_now: 0,
                resolve_then_retry: 0,
                manual_review: 1,
                terminal: 1,
                first_failure: Some(TrashRestoreError::AmbiguousReceipt),
                active_playlist: true,
            },
        );
        assert_eq!(
            manual_only,
            "Nothing restored; 1 file requires system Trash review; 1 file is no longer available for in-app restore."
        );
        assert!(!manual_only.contains('U'));
    }
}
