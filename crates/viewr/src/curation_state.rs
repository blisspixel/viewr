//! Pure lifecycle policy for source-removing curation operations.
//!
//! The event loop owns workers, paths, playlist state, and recovery application.
//! This module owns only deterministic recovery priority, close decisions, and
//! user-facing status derived from immutable facts.

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
}
