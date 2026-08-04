//! Pure lifecycle policy for source-removing curation operations.
//!
//! The event loop owns workers, paths, playlist state, and recovery application.
//! This module owns only deterministic recovery priority, close decisions, and
//! user-facing status derived from immutable facts.

use crate::curate::{GuardedActionError, TrashRestoreError};

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
    pub(crate) fn source_removal_preflight(self) -> Option<&'static str> {
        self.highest_risk().map(curation_recovery_message)
    }

    #[must_use]
    pub(crate) fn status(self) -> Option<String> {
        self.highest_risk()
            .map(|kind| curation_recovery_message(kind).to_owned())
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
pub(crate) const fn curation_recovery_message(kind: CurationKind) -> &'static str {
    match kind {
        CurationKind::Trash => {
            "Move to Trash stopped unexpectedly. The file may have moved. Review the folder and system Trash, then close and reopen viewr before trying another destructive action."
        }
        CurationKind::PermanentDelete => {
            "Permanent delete stopped unexpectedly. The file may have been deleted. Review the folder, then close and reopen viewr before trying another destructive action."
        }
        CurationKind::Restore => {
            "Trash restore stopped unexpectedly. Some files may have restored. Undo receipts were kept; review the folder and system Trash, then retry U before moving more files to Trash."
        }
    }
}

#[must_use]
pub(crate) fn curation_status(kind: CurationKind, submitted: usize, closing: bool) -> String {
    let count = file_count(submitted);
    match (kind, closing) {
        (CurationKind::Trash, false) => format!("Moving {count} to Trash..."),
        (CurationKind::Trash, true) => {
            format!("Finishing move to Trash for {count} before closing...")
        }
        (CurationKind::PermanentDelete, false) => {
            format!("Permanently deleting {count}...")
        }
        (CurationKind::PermanentDelete, true) => {
            format!("Finishing permanent delete for {count} before closing...")
        }
        (CurationKind::Restore, false) => format!("Restoring {count} from Trash..."),
        (CurationKind::Restore, true) => {
            format!("Finishing Trash restore for {count} before closing...")
        }
    }
}

#[must_use]
pub(crate) fn file_count(count: usize) -> String {
    format!("{count} {}", if count == 1 { "file" } else { "files" })
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
    action: GuardedSourceAction,
    error: &GuardedActionError,
) -> String {
    match (action, error) {
        (GuardedSourceAction::Trash, GuardedActionError::Changed) =>
            "This file changed after it was displayed. Reload it before moving it to Trash. Nothing was moved."
                .to_owned(),
        (GuardedSourceAction::PermanentDelete, GuardedActionError::Changed) =>
            "This file changed after it was displayed. Reload it before deleting it. Nothing was deleted."
                .to_owned(),
        (GuardedSourceAction::Trash, GuardedActionError::Missing) =>
            "This file is no longer available. Nothing was moved.".to_owned(),
        (GuardedSourceAction::PermanentDelete, GuardedActionError::Missing) =>
            "This file is no longer available. Nothing was deleted.".to_owned(),
        (GuardedSourceAction::Trash, GuardedActionError::Unsupported) =>
            "This filesystem entry cannot be safely moved from the displayed source. Nothing was moved."
                .to_owned(),
        (GuardedSourceAction::PermanentDelete, GuardedActionError::Unsupported) =>
            "This filesystem entry cannot be safely deleted from the displayed source. Nothing was deleted."
                .to_owned(),
        (GuardedSourceAction::Trash, GuardedActionError::Unavailable) =>
            "Safe file identity could not be verified. Nothing was moved.".to_owned(),
        (GuardedSourceAction::PermanentDelete, GuardedActionError::Unavailable) =>
            "Safe file identity could not be verified. Nothing was deleted.".to_owned(),
        (GuardedSourceAction::Trash, GuardedActionError::OperationFailed(error)) => {
            format!("Trash failed: {error}. Nothing was moved.")
        }
        (GuardedSourceAction::PermanentDelete, GuardedActionError::OperationFailed(error)) => {
            format!("Delete failed: {error}. Nothing was deleted.")
        }
    }
}

/// Path-free success copy after a single move to Trash.
#[must_use]
pub(crate) const fn single_trash_result_message(
    has_receipt: bool,
    previous_undo_preserved: bool,
) -> &'static str {
    if has_receipt {
        "Moved to Trash. Undo with U."
    } else if previous_undo_preserved {
        "Moved to Trash, but U is unavailable for this move. Use the system Trash; U still restores the previous Trash action."
    } else {
        "Moved to Trash, but U is unavailable for this move. Use the system Trash for recovery."
    }
}

/// Exact confirmation button label for permanent delete dialogs.
pub(crate) const PERMANENT_DELETE_ACTION: &str = "Delete permanently";

/// Path-free permanent-delete confirmation body. `safe_name` must already be
/// privacy-safe and quote-sanitized by the caller.
#[must_use]
pub(crate) fn permanent_delete_description(safe_name: &str) -> String {
    format!(
        "Delete \"{safe_name}\" forever?\n\nThis skips the system Trash and cannot be undone from viewr."
    )
}

/// True only when the user chose the explicit permanent-delete action label.
#[must_use]
pub(crate) fn permanent_delete_confirmed(custom_label: Option<&str>) -> bool {
    matches!(custom_label, Some(label) if label == PERMANENT_DELETE_ACTION)
}

/// Path-free success copy after permanent delete. `safe_name` must already be
/// privacy-safe and quote-sanitized by the caller.
#[must_use]
pub(crate) fn permanent_delete_success_message(
    safe_name: &str,
    previous_trash_undo: bool,
) -> String {
    if previous_trash_undo {
        format!(
            "Permanently deleted \"{safe_name}\". This cannot be undone; U still restores the previous Trash action."
        )
    } else {
        format!("Permanently deleted \"{safe_name}\". This cannot be undone.")
    }
}

/// Path-free single-file restore failure copy.
#[must_use]
pub(crate) fn single_restore_failure_message(error: TrashRestoreError) -> String {
    match error {
        TrashRestoreError::DestinationOccupied =>
            "Restore blocked: The original folder already contains an item with that name. Move or rename it, then retry with U."
                .to_owned(),
        TrashRestoreError::AccessDenied =>
            "Restore blocked: Access was denied. Check permissions, then retry with U."
                .to_owned(),
        TrashRestoreError::OperationFailed =>
            "Restore failed: The operating system could not restore the file. Retry with U."
                .to_owned(),
        TrashRestoreError::MissingFromTrash =>
            "The exact item is no longer in the system Trash. No retry remains in viewr."
                .to_owned(),
        TrashRestoreError::AmbiguousReceipt =>
            "The exact Trash receipt is ambiguous. Use the system Trash; no retry remains in viewr."
                .to_owned(),
        TrashRestoreError::Unsupported =>
            "In-app restore is unsupported on this platform. Use the system Trash; no retry remains in viewr."
                .to_owned(),
        TrashRestoreError::InvalidReceipt =>
            "The exact Trash receipt is unavailable. Use the system Trash; no retry remains in viewr."
                .to_owned(),
    }
}

/// Path-free restore summary across mixed outcomes.
#[must_use]
pub(crate) fn restore_result_message(
    restored: usize,
    retry_now: usize,
    resolve_then_retry: usize,
    manual_review: usize,
    terminal: usize,
    first_failure: Option<TrashRestoreError>,
    active_playlist: bool,
) -> String {
    let failure_total = retry_now + resolve_then_retry + manual_review + terminal;
    if failure_total == 0 {
        let suffix = if active_playlist {
            ""
        } else {
            "; reopen the source folder to refresh its view"
        };
        return format!("Restored {}{suffix}", file_count(restored));
    }
    if restored == 0 && failure_total == 1 {
        return first_failure.map_or_else(
            || "Restore failed. No retry remains in viewr.".to_owned(),
            single_restore_failure_message,
        );
    }

    let mut clauses = if restored == 0 {
        vec!["Nothing restored".to_owned()]
    } else {
        vec![format!("Restored {}", file_count(restored))]
    };
    if retry_now > 0 {
        clauses.push(format!("{} can retry with U", file_count(retry_now)));
    }
    if resolve_then_retry > 0 {
        let verb = if resolve_then_retry == 1 {
            "needs"
        } else {
            "need"
        };
        clauses.push(format!(
            "{} {verb} the blocking condition resolved, then U can retry",
            file_count(resolve_then_retry),
        ));
    }
    if manual_review > 0 {
        let verb = if manual_review == 1 {
            "requires"
        } else {
            "require"
        };
        clauses.push(format!(
            "{} {verb} system Trash review",
            file_count(manual_review),
        ));
    }
    if terminal > 0 {
        let verb = if terminal == 1 { "is" } else { "are" };
        clauses.push(format!(
            "{} {verb} no longer available for in-app restore",
            file_count(terminal),
        ));
    }
    if !active_playlist && restored > 0 {
        clauses.push("reopen the source folder to refresh its view".to_owned());
    }
    format!("{}.", clauses.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

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
                recovery.source_removal_preflight(),
                expected.map(curation_recovery_message)
            );
            assert_eq!(
                recovery.status().as_deref(),
                expected.map(curation_recovery_message)
            );
            for (bit, kind) in KINDS.into_iter().enumerate() {
                assert_eq!(recovery.contains(kind), mask & (1 << bit) != 0);
                recovery.clear(kind);
            }
            assert_eq!(recovery, CurationRecovery::default());
        }
    }

    #[test]
    fn recovery_guidance_is_exact_and_operation_specific() {
        assert_eq!(
            curation_recovery_message(CurationKind::Trash),
            "Move to Trash stopped unexpectedly. The file may have moved. Review the folder and system Trash, then close and reopen viewr before trying another destructive action."
        );
        assert_eq!(
            curation_recovery_message(CurationKind::PermanentDelete),
            "Permanent delete stopped unexpectedly. The file may have been deleted. Review the folder, then close and reopen viewr before trying another destructive action."
        );
        assert_eq!(
            curation_recovery_message(CurationKind::Restore),
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
            assert_eq!(curation_status(kind, submitted, closing), expected);
        }
        assert_eq!(file_count(0), "0 files");
        assert_eq!(file_count(usize::MAX), format!("{} files", usize::MAX));
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
            let message = guarded_source_action_failure_message(GuardedSourceAction::Trash, &error);
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
            let message =
                guarded_source_action_failure_message(GuardedSourceAction::PermanentDelete, &error);
            assert_eq!(message, expected);
            assert!(!message.contains("private"));
            assert!(!message.contains("album"));
        }
    }

    #[test]
    fn single_trash_copy_routes_every_move_to_a_real_recovery_path() {
        assert_eq!(
            single_trash_result_message(true, false),
            "Moved to Trash. Undo with U."
        );
        assert_eq!(
            single_trash_result_message(false, true),
            "Moved to Trash, but U is unavailable for this move. Use the system Trash; U still restores the previous Trash action."
        );
        assert_eq!(
            single_trash_result_message(false, false),
            "Moved to Trash, but U is unavailable for this move. Use the system Trash for recovery."
        );
    }

    #[test]
    fn permanent_delete_success_copy_disambiguates_prior_trash_undo() {
        assert_eq!(
            permanent_delete_success_message("bad???gpj", true),
            "Permanently deleted \"bad???gpj\". This cannot be undone; U still restores the previous Trash action."
        );
        assert_eq!(
            permanent_delete_success_message("bad???gpj", false),
            "Permanently deleted \"bad???gpj\". This cannot be undone."
        );
    }

    #[test]
    fn permanent_delete_confirmation_is_bounded_and_label_exact() {
        let description = permanent_delete_description("bad???gpj");
        assert!(description.starts_with("Delete \"bad???gpj\" forever?"));
        assert_eq!(description.matches('\n').count(), 2);
        assert!(description.contains("system Trash"));
        assert!(!description.contains('\\'));
        assert!(!description.contains('/'));

        assert!(permanent_delete_confirmed(Some(PERMANENT_DELETE_ACTION)));
        assert!(!permanent_delete_confirmed(Some("Cancel")));
        assert!(!permanent_delete_confirmed(None));
        assert!(!permanent_delete_confirmed(Some("Ok")));
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
            assert_eq!(single_restore_failure_message(error), expected);
        }

        assert_eq!(
            restore_result_message(1, 0, 0, 0, 0, None, true),
            "Restored 1 file"
        );
        assert_eq!(
            restore_result_message(2, 0, 0, 0, 0, None, false),
            "Restored 2 files; reopen the source folder to refresh its view"
        );
        assert_eq!(
            restore_result_message(
                1,
                1,
                1,
                1,
                1,
                Some(TrashRestoreError::OperationFailed),
                true
            ),
            "Restored 1 file; 1 file can retry with U; 1 file needs the blocking condition resolved, then U can retry; 1 file requires system Trash review; 1 file is no longer available for in-app restore."
        );
        let manual_only = restore_result_message(
            0,
            0,
            0,
            1,
            1,
            Some(TrashRestoreError::AmbiguousReceipt),
            true,
        );
        assert_eq!(
            manual_only,
            "Nothing restored; 1 file requires system Trash review; 1 file is no longer available for in-app restore."
        );
        assert!(!manual_only.contains('U'));
    }
}
