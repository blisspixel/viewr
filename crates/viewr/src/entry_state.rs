//! Pure entry and folder-scan disposition policy.
//!
//! The event loop owns dialogs, scan workers, playlist mutation, and image loads.
//! This module owns only whether a completed scan still applies and which
//! visible playlist outcome follows from purpose plus scan facts.

use std::path::Path;

/// Outcome of a finished folder scan for one entry request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FolderScanDisposition {
    /// Scan no longer matches the selected file or was cancelled; drop it.
    Discard,
    /// Build the playlist from scan entries and select the matched index.
    InstallScanAt(usize),
    /// Keep only the selected file as a one-item playlist.
    InstallSelectedOnly,
    /// Same as selected-only after a hard folder size or path budget limit.
    InstallSelectedOnlyLimitExceeded,
    /// Same as selected-only after a non-cancel scan failure.
    InstallSelectedOnlyScanFailed,
    /// Open-folder scan found no supported images.
    OpenFolderEmpty,
    /// Open-folder scan found images; install scan and load the first entry.
    OpenFolderFirst,
    /// Open-folder hit a hard safety limit.
    OpenFolderLimitExceeded,
    /// Open-folder failed for a non-cancel reason.
    OpenFolderFailed,
}

/// Successful scan facts already resolved against the active purpose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FolderScanSuccess {
    /// Selected-file scan still current; index is `Some` when the path matched.
    Selected { matched_index: Option<usize> },
    /// Open-folder scan completed with `count` supported images.
    OpenFolder { count: usize },
}

/// Failure class for a completed scan worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FolderScanFailure {
    Cancelled,
    LimitExceeded,
    Other,
}

#[must_use]
pub(crate) fn selected_scan_is_current(current: Option<&Path>, selected: &Path) -> bool {
    current == Some(selected)
}

#[must_use]
pub(crate) fn selected_file_index_by<T>(
    files: &[T],
    selected: &Path,
    path: impl Fn(&T) -> &Path + Copy,
) -> Option<usize> {
    files
        .iter()
        .position(|entry| path(entry) == selected)
        .or_else(|| {
            let selected_name = selected.file_name()?;
            files
                .iter()
                .position(|entry| path(entry).file_name() == Some(selected_name))
        })
}

#[must_use]
pub(crate) const fn folder_scan_failure_class(
    cancelled: bool,
    limit_exceeded: bool,
) -> FolderScanFailure {
    if cancelled {
        FolderScanFailure::Cancelled
    } else if limit_exceeded {
        FolderScanFailure::LimitExceeded
    } else {
        FolderScanFailure::Other
    }
}

/// Purpose-aware disposition for a completed folder scan.
#[must_use]
pub(crate) fn folder_scan_disposition(
    open_folder: bool,
    selected_is_current: bool,
    result: Result<FolderScanSuccess, FolderScanFailure>,
) -> FolderScanDisposition {
    if !open_folder && !selected_is_current {
        return FolderScanDisposition::Discard;
    }
    match result {
        Ok(FolderScanSuccess::Selected {
            matched_index: Some(index),
        }) => FolderScanDisposition::InstallScanAt(index),
        Ok(FolderScanSuccess::Selected {
            matched_index: None,
        }) => FolderScanDisposition::InstallSelectedOnly,
        Ok(FolderScanSuccess::OpenFolder { count: 0 }) => FolderScanDisposition::OpenFolderEmpty,
        Ok(FolderScanSuccess::OpenFolder { .. }) => FolderScanDisposition::OpenFolderFirst,
        Err(FolderScanFailure::Cancelled) => FolderScanDisposition::Discard,
        Err(FolderScanFailure::LimitExceeded) if open_folder => {
            FolderScanDisposition::OpenFolderLimitExceeded
        }
        Err(FolderScanFailure::LimitExceeded) => {
            FolderScanDisposition::InstallSelectedOnlyLimitExceeded
        }
        Err(FolderScanFailure::Other) if open_folder => FolderScanDisposition::OpenFolderFailed,
        Err(FolderScanFailure::Other) => FolderScanDisposition::InstallSelectedOnlyScanFailed,
    }
}

#[must_use]
pub(crate) const fn folder_scan_user_message(
    disposition: FolderScanDisposition,
) -> Option<&'static str> {
    match disposition {
        FolderScanDisposition::InstallSelectedOnlyLimitExceeded => {
            Some("Folder is too large for safe automatic browsing. Opened only the selected image")
        }
        FolderScanDisposition::InstallSelectedOnlyScanFailed => {
            Some("Folder browsing is unavailable. Opened only the selected image")
        }
        FolderScanDisposition::OpenFolderEmpty => {
            Some("The selected folder contains no supported images")
        }
        FolderScanDisposition::OpenFolderLimitExceeded => {
            Some("The selected folder exceeds safe browsing limits")
        }
        FolderScanDisposition::OpenFolderFailed => Some("Could not read the selected folder"),
        FolderScanDisposition::Discard
        | FolderScanDisposition::InstallScanAt(_)
        | FolderScanDisposition::InstallSelectedOnly
        | FolderScanDisposition::OpenFolderFirst => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn selected_scan_currency_is_exact_path_identity() {
        let selected = Path::new("album/photo.png");
        assert!(selected_scan_is_current(Some(selected), selected));
        assert!(!selected_scan_is_current(None, selected));
        assert!(!selected_scan_is_current(
            Some(Path::new("album/other.png")),
            selected
        ));
    }

    #[test]
    fn selected_file_index_prefers_full_path_then_basename() {
        let files = [
            PathBuf::from("a/img1.png"),
            PathBuf::from("b/img2.png"),
            PathBuf::from("c/img3.png"),
        ];
        assert_eq!(
            selected_file_index_by(&files, Path::new("b/img2.png"), PathBuf::as_path),
            Some(1)
        );
        assert_eq!(
            selected_file_index_by(&files, Path::new("missing.png"), PathBuf::as_path),
            None
        );
        assert_eq!(
            selected_file_index_by(&files, Path::new("elsewhere/img2.png"), PathBuf::as_path),
            Some(1)
        );
    }

    #[test]
    fn selected_file_scan_outcomes_cover_success_and_limits() {
        assert_eq!(
            folder_scan_disposition(
                false,
                true,
                Ok(FolderScanSuccess::Selected {
                    matched_index: Some(1)
                })
            ),
            FolderScanDisposition::InstallScanAt(1)
        );
        assert_eq!(
            folder_scan_disposition(
                false,
                true,
                Ok(FolderScanSuccess::Selected {
                    matched_index: None
                })
            ),
            FolderScanDisposition::InstallSelectedOnly
        );
        assert_eq!(
            folder_scan_disposition(
                false,
                false,
                Ok(FolderScanSuccess::Selected {
                    matched_index: Some(0)
                })
            ),
            FolderScanDisposition::Discard
        );
        assert_eq!(
            folder_scan_disposition(false, true, Err(FolderScanFailure::LimitExceeded)),
            FolderScanDisposition::InstallSelectedOnlyLimitExceeded
        );
        assert_eq!(
            folder_scan_disposition(false, true, Err(FolderScanFailure::Cancelled)),
            FolderScanDisposition::Discard
        );
        assert_eq!(
            folder_scan_disposition(false, true, Err(FolderScanFailure::Other)),
            FolderScanDisposition::InstallSelectedOnlyScanFailed
        );
    }

    #[test]
    fn open_folder_scan_outcomes_cover_empty_first_limits_and_failure() {
        assert_eq!(
            folder_scan_disposition(true, true, Ok(FolderScanSuccess::OpenFolder { count: 0 })),
            FolderScanDisposition::OpenFolderEmpty
        );
        assert_eq!(
            folder_scan_disposition(true, true, Ok(FolderScanSuccess::OpenFolder { count: 3 })),
            FolderScanDisposition::OpenFolderFirst
        );
        assert_eq!(
            folder_scan_disposition(true, true, Err(FolderScanFailure::LimitExceeded)),
            FolderScanDisposition::OpenFolderLimitExceeded
        );
        assert_eq!(
            folder_scan_disposition(true, true, Err(FolderScanFailure::Cancelled)),
            FolderScanDisposition::Discard
        );
        assert_eq!(
            folder_scan_disposition(true, true, Err(FolderScanFailure::Other)),
            FolderScanDisposition::OpenFolderFailed
        );
    }

    #[test]
    fn failure_class_mapping_is_exhaustive() {
        assert_eq!(
            folder_scan_failure_class(true, false),
            FolderScanFailure::Cancelled
        );
        assert_eq!(
            folder_scan_failure_class(false, true),
            FolderScanFailure::LimitExceeded
        );
        assert_eq!(
            folder_scan_failure_class(false, false),
            FolderScanFailure::Other
        );
        assert_eq!(
            folder_scan_failure_class(true, true),
            FolderScanFailure::Cancelled
        );
    }

    #[test]
    fn user_messages_are_truthful_and_path_free() {
        assert_eq!(
            folder_scan_user_message(FolderScanDisposition::InstallSelectedOnlyLimitExceeded),
            Some("Folder is too large for safe automatic browsing. Opened only the selected image")
        );
        assert_eq!(
            folder_scan_user_message(FolderScanDisposition::OpenFolderEmpty),
            Some("The selected folder contains no supported images")
        );
        assert_eq!(
            folder_scan_user_message(FolderScanDisposition::InstallScanAt(0)),
            None
        );
        for message in [
            folder_scan_user_message(FolderScanDisposition::InstallSelectedOnlyScanFailed),
            folder_scan_user_message(FolderScanDisposition::OpenFolderFailed),
            folder_scan_user_message(FolderScanDisposition::OpenFolderLimitExceeded),
        ]
        .into_iter()
        .flatten()
        {
            assert!(!message.contains('\\'));
            assert!(!message.contains('/'));
        }
    }
}
